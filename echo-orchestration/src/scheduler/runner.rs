//! Scheduler runner — periodic tick loop that fires cron tasks.
//!
//! The [`SchedulerRunner`] is generic over the fire function `F`,
//! making it decoupled from any specific agent or task service.

use super::cron_task::{CronTask, CronTaskStatus, CronTaskStore};
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Type alias for the fire function: takes a CronTask, returns a future with the result.
pub type FireFn =
    Arc<dyn Fn(CronTask) -> BoxFuture<'static, echo_core::error::Result<String>> + Send + Sync>;

/// Periodic scheduler that checks and fires cron tasks.
///
/// Runs a tokio background task that polls every 30 seconds.
/// Uses `last_fired` tracking to prevent double-firing.
pub struct SchedulerRunner {
    store: CronTaskStore,
    fire_fn: FireFn,
    tasks: Arc<RwLock<Vec<CronTask>>>,
    last_fired: Arc<RwLock<HashMap<String, DateTime<Utc>>>>,
    cancel: CancellationToken,
}

impl SchedulerRunner {
    /// Create a new scheduler with the given store and fire function.
    pub fn new(store: CronTaskStore, cancel: CancellationToken, fire_fn: FireFn) -> Self {
        let tasks = store.load_all().unwrap_or_default();
        Self {
            store,
            fire_fn,
            tasks: Arc::new(RwLock::new(tasks)),
            last_fired: Arc::new(RwLock::new(HashMap::new())),
            cancel,
        }
    }

    /// Spawn the scheduler as a background tokio task.
    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            self.run_loop().await;
        });
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
        let tasks = self.tasks.read().await;
        let now = Utc::now();
        let window_start = now - chrono::Duration::seconds(30);

        let mut to_fire: Vec<CronTask> = Vec::new();
        let mut last_fired = self.last_fired.write().await;

        for task in tasks.iter() {
            if task.status != CronTaskStatus::Enabled {
                continue;
            }
            if let Some(next) = task.next_run() {
                // Check if next_run is within the window
                if next >= window_start && next <= now {
                    // Prevent double-fire
                    if let Some(last) = last_fired.get(&task.id)
                        && (now - *last).num_seconds() < 30
                    {
                        debug!(task = %task.name, "Skipping double-fire");
                        continue;
                    }
                    last_fired.insert(task.id.clone(), now);
                    to_fire.push(task.clone());
                }
            }
        }
        drop(tasks);
        drop(last_fired);

        // Fire outside locks
        for task in to_fire {
            self.fire_task(task).await;
        }
    }

    /// Fire a single task using the fire function.
    async fn fire_task(&self, task: CronTask) {
        info!(task_id = %task.id, name = %task.name, "Firing cron task");
        let fire_fn = self.fire_fn.clone();
        match fire_fn(task.clone()).await {
            Ok(result) => {
                let _ = self.store.update_last_run(&task.id, &result);
                info!(task = %task.name, "Cron task completed");
            }
            Err(e) => {
                warn!(task = %task.name, error = %e, "Cron task failed");
                let _ = self.store.update_last_run(&task.id, &format!("ERROR: {e}"));
            }
        }
    }

    // ── Management API ────────────────────────────────────────────

    /// Add a new cron task and persist it.
    pub async fn add_task(&self, task: CronTask) -> echo_core::error::Result<()> {
        self.store.add(task.clone())?;
        self.tasks.write().await.push(task);
        Ok(())
    }

    /// Remove a cron task by ID prefix.
    pub async fn remove_task(&self, id: &str) -> echo_core::error::Result<bool> {
        let removed = self.store.remove(id)?;
        if removed {
            self.tasks.write().await.retain(|t| !t.id.starts_with(id));
        }
        Ok(removed)
    }

    /// Enable or disable a task.
    pub async fn set_status(
        &self,
        id: &str,
        status: CronTaskStatus,
    ) -> echo_core::error::Result<bool> {
        let updated = self.store.set_status(id, status)?;
        if updated {
            let mut tasks = self.tasks.write().await;
            if let Some(task) = tasks.iter_mut().find(|t| t.id.starts_with(id)) {
                task.status = status;
            }
        }
        Ok(updated)
    }

    /// List all cron tasks.
    pub async fn list_tasks(&self) -> Vec<CronTask> {
        self.tasks.read().await.clone()
    }

    /// Manually fire a task immediately (bypassing schedule).
    pub async fn run_once(&self, id: &str) -> echo_core::error::Result<String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.id.starts_with(id))
            .cloned()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(format!("Cron task '{id}' not found"))
            })?;
        drop(tasks);

        let result = (self.fire_fn)(task.clone()).await?;
        let _ = self.store.update_last_run(&task.id, &result);
        Ok(result)
    }

    /// Reload tasks from the store.
    pub async fn reload(&self) -> echo_core::error::Result<usize> {
        let tasks = self.store.load_all()?;
        let count = tasks.len();
        *self.tasks.write().await = tasks;
        Ok(count)
    }
}
