//! Scheduler runner — periodic tick loop that fires cron tasks.
//!
//! The [`SchedulerRunner`] is generic over the fire function `F`,
//! making it decoupled from any specific agent or task service.

use super::cron_task::{CronTask, CronTaskStatus, CronTaskStore};
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Type alias for the fire function: takes a CronTask, returns a future with the result.
pub type FireFn =
    Arc<dyn Fn(CronTask) -> BoxFuture<'static, echo_core::error::Result<String>> + Send + Sync>;

/// A scheduled occurrence retains both the public task ID and the definition
/// identity captured from the store.  The scheduled timestamp makes repeated
/// cron ticks address the same occurrence instead of treating an entire task
/// as the deduplication unit.
#[derive(Debug, Clone)]
struct ScheduledOccurrence {
    task: CronTask,
    scheduled_at: DateTime<Utc>,
}

type LastFiredByTask = HashMap<String, (String, DateTime<Utc>)>;

/// Owned handle for a running scheduler loop.
pub struct SchedulerHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl SchedulerHandle {
    /// Request shutdown without waiting for the loop to exit.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Return whether the scheduler loop has exited.
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Wait for the scheduler loop to exit naturally.
    pub async fn join(self) -> echo_core::error::Result<()> {
        self.join.await.map_err(|error| {
            echo_core::error::ReactError::Other(format!("Scheduler task failed: {error}"))
        })
    }

    /// Cancel the scheduler and wait until all scheduler-owned work has stopped.
    pub async fn shutdown(self) -> echo_core::error::Result<()> {
        self.cancel();
        self.join().await
    }
}

/// Periodic scheduler that checks and fires cron tasks.
///
/// Runs a tokio background task that polls every 30 seconds.
/// Uses `last_fired` tracking to prevent double-firing.
pub struct SchedulerRunner {
    store: CronTaskStore,
    fire_fn: FireFn,
    tasks: Arc<RwLock<Vec<CronTask>>>,
    last_fired: Arc<RwLock<LastFiredByTask>>,
    last_tick_at: Arc<RwLock<DateTime<Utc>>>,
    control_lock: Arc<Mutex<()>>,
    cancel: CancellationToken,
}

impl SchedulerRunner {
    /// Create a new scheduler with the given store and fire function.
    pub async fn new(
        store: CronTaskStore,
        cancel: CancellationToken,
        fire_fn: FireFn,
    ) -> echo_core::error::Result<Self> {
        let tasks = store.load_all().await?;
        Ok(Self {
            store,
            fire_fn,
            tasks: Arc::new(RwLock::new(tasks)),
            last_fired: Arc::new(RwLock::new(HashMap::new())),
            last_tick_at: Arc::new(RwLock::new(Utc::now())),
            control_lock: Arc::new(Mutex::new(())),
            cancel,
        })
    }

    /// Spawn the scheduler as a background tokio task.
    pub fn spawn(self: Arc<Self>) -> SchedulerHandle {
        let cancel = self.cancel.clone();
        let join = tokio::spawn(async move {
            self.run_loop().await;
        });
        SchedulerHandle { cancel, join }
    }

    /// Main loop: tick every 30 seconds, checking and firing due tasks.
    async fn run_loop(&self) {
        info!("Scheduler runner started (30s tick interval)");
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Scheduler runner cancelled");
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    self.tick().await;
                }
            }
        }
    }

    /// Single tick: check all enabled tasks and fire those that are due.
    async fn tick(&self) {
        let now = Utc::now();
        self.tick_at(now).await;
    }

    async fn tick_at(&self, now: DateTime<Utc>) {
        let window_start = {
            let mut last_tick_at = self.last_tick_at.write().await;
            let previous = *last_tick_at;
            *last_tick_at = now;
            previous
        };
        let tasks = self.tasks.read().await;

        let mut to_fire: Vec<ScheduledOccurrence> = Vec::new();
        let mut last_fired = self.last_fired.write().await;

        for task in tasks.iter() {
            if task.status != CronTaskStatus::Enabled {
                continue;
            }
            if let Some(next) = task.next_run_after(&window_start) {
                // Check if next_run is within the window
                if next <= now {
                    // Reserve one occurrence by definition identity and
                    // scheduled timestamp.  The reservation is retained when
                    // a later admission gate suppresses the callback, so a
                    // disable/remove followed by re-enable cannot replay an
                    // already observed occurrence.
                    if let Some((definition_id, last)) = last_fired.get(&task.id)
                        && definition_id == &task.created_at
                        && *last == next
                    {
                        debug!(task = %task.name, "Skipping double-fire");
                        continue;
                    }
                    last_fired.insert(task.id.clone(), (task.created_at.clone(), next));
                    to_fire.push(ScheduledOccurrence {
                        task: task.clone(),
                        scheduled_at: next,
                    });
                }
            }
        }
        drop(tasks);
        drop(last_fired);

        // Fire outside locks
        for occurrence in to_fire {
            self.fire_task(occurrence).await;
        }
    }

    /// Fire a single occurrence using the fire function.
    async fn fire_task(&self, occurrence: ScheduledOccurrence) {
        // Admission is the linearization point relative to disable/remove.
        // Controls that commit before this lock is acquired suppress the
        // occurrence; once the callback future is created under the lock, it
        // is admitted and may settle after a later control operation.
        let (task, callback) = {
            let _control = self.control_lock.lock().await;
            let tasks = self.tasks.write().await;
            let Some(task) = tasks.iter().find(|candidate| {
                candidate.id == occurrence.task.id
                    && candidate.created_at == occurrence.task.created_at
                    && candidate.status == CronTaskStatus::Enabled
            }) else {
                debug!(
                    task_id = %occurrence.task.id,
                    scheduled_at = %occurrence.scheduled_at,
                    "Skipping occurrence disabled or removed before callback admission"
                );
                return;
            };
            let task = task.clone();
            let callback = (self.fire_fn)(task.clone());
            (task, callback)
        };

        info!(task_id = %task.id, name = %task.name, scheduled_at = %occurrence.scheduled_at, "Firing cron task");
        let result = tokio::select! {
            _ = self.cancel.cancelled() => {
                debug!(task = %task.name, "Cron task cancelled during scheduler shutdown");
                return;
            }
            result = callback => result,
        };
        match result {
            Ok(result) => {
                let _control = self.control_lock.lock().await;
                match self.store.update_last_run_for_task(&task, &result).await {
                    Ok(_) => {
                        if let Err(error) = self.refresh_cache().await {
                            warn!(task = %task.name, %error, "Failed to refresh scheduler cache");
                        }
                    }
                    Err(error) => {
                        warn!(task = %task.name, %error, "Failed to persist cron task success");
                    }
                }
                info!(task = %task.name, "Cron task completed");
            }
            Err(e) => {
                warn!(task = %task.name, error = %e, "Cron task failed");
                let _control = self.control_lock.lock().await;
                match self
                    .store
                    .update_last_run_for_task(&task, &format!("ERROR: {e}"))
                    .await
                {
                    Ok(_) => {
                        if let Err(error) = self.refresh_cache().await {
                            warn!(task = %task.name, %error, "Failed to refresh scheduler cache");
                        }
                    }
                    Err(error) => {
                        warn!(task = %task.name, %error, "Failed to persist cron task failure");
                    }
                }
            }
        }
    }

    // ── Management API ────────────────────────────────────────────

    /// Add a new cron task and persist it.
    pub async fn add_task(&self, task: CronTask) -> echo_core::error::Result<()> {
        let _control = self.control_lock.lock().await;
        self.store.add(task.clone()).await?;
        self.refresh_cache().await.map(|_| ())
    }

    /// Remove a cron task by ID prefix.
    pub async fn remove_task(&self, id: &str) -> echo_core::error::Result<bool> {
        let _control = self.control_lock.lock().await;
        let removed = self.store.remove(id).await?;
        if removed {
            self.last_fired.write().await.remove(id);
            self.refresh_cache().await?;
        }
        Ok(removed)
    }

    /// Remove exactly one cron task by its complete ID.
    pub async fn remove_task_exact(&self, id: &str) -> echo_core::error::Result<bool> {
        let _control = self.control_lock.lock().await;
        let removed = self.store.remove_exact(id).await?;
        if removed {
            self.last_fired.write().await.remove(id);
            self.refresh_cache().await?;
        }
        Ok(removed)
    }

    /// Enable or disable a task.
    pub async fn set_status(
        &self,
        id: &str,
        status: CronTaskStatus,
    ) -> echo_core::error::Result<bool> {
        let _control = self.control_lock.lock().await;
        let updated = self.store.set_status(id, status).await?;
        if updated {
            self.refresh_cache().await?;
        }
        Ok(updated)
    }

    /// List all cron tasks.
    pub async fn list_tasks(&self) -> Vec<CronTask> {
        self.tasks.read().await.clone()
    }

    /// Manually fire a task immediately (bypassing schedule).
    pub async fn run_once(&self, id: &str) -> echo_core::error::Result<String> {
        let (task, callback) = {
            let _control = self.control_lock.lock().await;
            let tasks = self.tasks.write().await;
            let task = tasks
                .iter()
                .find(|task| task.id == id && task.status == CronTaskStatus::Enabled)
                .cloned()
                .ok_or_else(|| {
                    echo_core::error::ReactError::Other(format!(
                        "Enabled cron task '{id}' not found"
                    ))
                })?;
            let callback = (self.fire_fn)(task.clone());
            (task, callback)
        };

        let result = callback.await?;
        let _control = self.control_lock.lock().await;
        self.store.update_last_run_for_task(&task, &result).await?;
        self.refresh_cache().await?;
        Ok(result)
    }

    /// Reload tasks from the store.
    pub async fn reload(&self) -> echo_core::error::Result<usize> {
        let _control = self.control_lock.lock().await;
        self.refresh_cache().await
    }

    async fn refresh_cache(&self) -> echo_core::error::Result<usize> {
        let tasks = self.store.load_all().await?;
        let count = tasks.len();
        *self.tasks.write().await = tasks;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn tick_fires_occurrence_in_previous_window() -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-scheduler-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("done".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        runner
            .tasks
            .write()
            .await
            .push(CronTask::new("every minute", "* * * * *", "run"));
        let previous = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 0, 30)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        *runner.last_tick_at.write().await = previous;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 5)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        runner.tick_at(now).await;
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_waits_for_scheduler_and_cancels_in_flight_fire()
    -> echo_core::error::Result<()> {
        let root = std::env::temp_dir().join(format!("echo-scheduler-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_started = Arc::new(tokio::sync::Notify::new());
        let fire_dropped = Arc::new(AtomicUsize::new(0));
        let started_for_fn = Arc::clone(&fire_started);
        let dropped_for_fn = Arc::clone(&fire_dropped);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let started = Arc::clone(&started_for_fn);
            let dropped = Arc::clone(&dropped_for_fn);
            Box::pin(async move {
                struct DropMarker(Arc<AtomicUsize>);
                impl Drop for DropMarker {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::SeqCst);
                    }
                }

                let _marker = DropMarker(dropped);
                started.notify_one();
                std::future::pending::<()>().await;
                Ok("unreachable".to_string())
            })
        });
        let runner =
            Arc::new(SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?);
        runner
            .tasks
            .write()
            .await
            .push(CronTask::new("every minute", "* * * * *", "run"));
        let previous = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 0, 30)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        *runner.last_tick_at.write().await = previous;
        let now = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 5)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;

        let handle = Arc::clone(&runner).spawn();
        let tick = tokio::spawn(async move { runner.tick_at(now).await });
        fire_started.notified().await;
        handle.shutdown().await?;
        tokio::time::timeout(std::time::Duration::from_secs(1), tick)
            .await
            .map_err(|_| {
                echo_core::error::ReactError::Other(
                    "in-flight scheduler task did not observe shutdown".into(),
                )
            })?
            .map_err(|error| {
                echo_core::error::ReactError::Other(format!("scheduler tick task failed: {error}"))
            })?;

        assert_eq!(fire_dropped.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn callback_success_refreshes_runner_cache_after_store_commit()
    -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-cache-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_task| Box::pin(async { Ok("fresh result".to_string()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("cache", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;

        assert_eq!(runner.run_once(&task_id).await?, "fresh result");
        let cached = runner
            .list_tasks()
            .await
            .into_iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| echo_core::error::ReactError::Other("cached task missing".into()))?;
        assert_eq!(cached.last_result.as_deref(), Some("fresh result"));
        assert!(cached.last_run_at.is_some());
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn control_committed_before_admission_suppresses_captured_occurrence()
    -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-control-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("should not fire".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("controlled", "*/5 * * * *", "run");
        let scheduled_at = Utc
            .with_ymd_and_hms(2026, 8, 13, 0, 1, 0)
            .single()
            .ok_or_else(|| echo_core::error::ReactError::Other("invalid test time".into()))?;
        let occurrence = ScheduledOccurrence {
            task: task.clone(),
            scheduled_at,
        };
        runner.add_task(task).await?;
        runner
            .set_status(&occurrence.task.id, CronTaskStatus::Disabled)
            .await?;
        runner.fire_task(occurrence).await;
        assert_eq!(fired.load(Ordering::SeqCst), 0);

        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn stale_callback_cannot_update_recreated_definition() -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-recreate-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fire_fn: FireFn = Arc::new(|_task| Box::pin(async { Ok("unused".to_string()) }));
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let old = CronTask::new("old", "*/5 * * * *", "old");
        let old_id = old.id.clone();
        runner.add_task(old.clone()).await?;
        runner.remove_task_exact(&old_id).await?;

        let mut current = CronTask::new("new", "*/5 * * * *", "new");
        current.id = old_id.clone();
        runner.add_task(current.clone()).await?;
        assert!(
            runner
                .store
                .update_last_run_for_task(&old, "stale")
                .await
                .is_err()
        );
        let cached = runner
            .list_tasks()
            .await
            .into_iter()
            .find(|task| task.id == old_id)
            .ok_or_else(|| echo_core::error::ReactError::Other("recreated task missing".into()))?;
        assert_eq!(cached.last_result, None);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn run_once_does_not_bypass_disabled_status() -> echo_core::error::Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-scheduler-run-once-{}", uuid::Uuid::new_v4()));
        let store = CronTaskStore::new().with_path(root.join("tasks.json"));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_for_fn = Arc::clone(&fired);
        let fire_fn: FireFn = Arc::new(move |_task| {
            let fired = Arc::clone(&fired_for_fn);
            Box::pin(async move {
                fired.fetch_add(1, Ordering::SeqCst);
                Ok("should not fire".to_string())
            })
        });
        let runner = SchedulerRunner::new(store, CancellationToken::new(), fire_fn).await?;
        let task = CronTask::new("disabled", "*/5 * * * *", "run");
        let task_id = task.id.clone();
        runner.add_task(task).await?;
        runner
            .set_status(&task_id, CronTaskStatus::Disabled)
            .await?;
        assert!(runner.run_once(&task_id).await.is_err());
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }
}
