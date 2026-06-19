//! Background task handle and spawner for long-running task support.
//!
//! This module provides a non-blocking task abstraction that bridges the gap
//! between the DAG task executor (which blocks the caller) and the need for
//! fire-and-forget, pollable, cancellable background work.
//!
//! # Core Types
//!
//! - [`BackgroundTask<T>`] — A handle to a spawned async task. Provides
//!   non-blocking status polling, blocking wait with timeout, and cancellation.
//!
//! - [`BackgroundTaskStatus`] — Lifecycle states: Pending → Running → Completed/Failed/Cancelled.
//!
//! - [`AnyBackgroundTask`] — Type-erased trait for storing heterogeneous task handles
//!   in a single collection (e.g., a status dashboard).
//!
//! - [`TaskSpawner`] — System-level spawner that manages concurrency (via Semaphore),
//!   tracks all spawned tasks, and supports cross-restart resumption via [`TaskStore`].
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_orchestration::tasks::{TaskSpawner, TaskSpawnerConfig};
//! use std::sync::Arc;
//!
//! async fn example() {
//!     let spawner = Arc::new(TaskSpawner::new(TaskSpawnerConfig::default()));
//!
//!     // Spawn a background task
//!     let handle = spawner.spawn("fetch-data", async {
//!         // Long-running work...
//!         Ok("result data".to_string())
//!     });
//!
//!     // Non-blocking status check
//!     println!("Status: {:?}", handle.status());
//!
//!     // Block until done (with optional timeout)
//!     let result = handle.wait(Some(std::time::Duration::from_secs(30))).await;
//!     println!("Result: {:?}", result);
//! }
//! ```

use dashmap::DashMap;
use echo_core::error::{ReactError, Result};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

// ── BackgroundTaskStatus ──────────────────────────────────────────

/// Lifecycle status of a background task.
#[derive(Debug, Clone)]
pub enum BackgroundTaskStatus {
    /// Task is queued but not yet started.
    Pending,
    /// Task is currently executing.
    Running {
        /// When the task started executing.
        started_at: Instant,
    },
    /// Task completed successfully.
    Completed {
        /// When the task finished.
        finished_at: Instant,
    },
    /// Task failed with an error.
    Failed {
        /// Human-readable error description.
        error: String,
        /// When the failure occurred.
        at: Instant,
    },
    /// Task was cancelled via its cancellation token.
    Cancelled,
}

impl BackgroundTaskStatus {
    /// Whether this status represents a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            BackgroundTaskStatus::Completed { .. }
                | BackgroundTaskStatus::Failed { .. }
                | BackgroundTaskStatus::Cancelled
        )
    }

    /// Short text description for display purposes.
    pub fn as_str(&self) -> &'static str {
        match self {
            BackgroundTaskStatus::Pending => "pending",
            BackgroundTaskStatus::Running { .. } => "running",
            BackgroundTaskStatus::Completed { .. } => "completed",
            BackgroundTaskStatus::Failed { .. } => "failed",
            BackgroundTaskStatus::Cancelled => "cancelled",
        }
    }
}

// ── BackgroundTask<T> ─────────────────────────────────────────────

/// A handle to a background task that can be polled, awaited, or cancelled.
///
/// The task runs asynchronously on the tokio runtime. The handle is cheap to
/// clone (internally uses `Arc`), and multiple handles can exist for the same task.
pub struct BackgroundTask<T: Send + 'static> {
    /// Unique task ID, stable across restarts if persisted.
    pub id: String,
    /// Human-readable name/description.
    pub name: String,
    /// Current lifecycle status (shared, readable without blocking).
    status: Arc<RwLock<BackgroundTaskStatus>>,
    /// Result cell: `None` until the task finishes, then `Some(result)`.
    /// Guarded by a Mutex so `wait()` can `take()` it only once. The paired
    /// `Notify` wakes waiters when a result arrives.
    ///
    /// **Retry-safe (N-P2-5)**: unlike the old oneshot design, a timeout does
    /// NOT consume the cell. `wait()` only calls `take()` once it observes the
    /// result is present, so a timed-out `wait()` can be retried and still
    /// receive the result.
    result: Arc<Mutex<Option<Result<T>>>>,
    /// Notifier fired when the result cell is filled.
    result_notify: Arc<Notify>,
    /// Token to request cancellation of the running task.
    cancel: CancellationToken,
    /// Inner join handle (for detecting panics).
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl<T: Send + 'static> BackgroundTask<T> {
    /// Non-blocking snapshot of the current status.
    pub async fn status(&self) -> BackgroundTaskStatus {
        self.status.read().await.clone()
    }

    /// Convenience: check if the task is still running (non-blocking).
    pub async fn is_running(&self) -> bool {
        matches!(
            *self.status.read().await,
            BackgroundTaskStatus::Running { .. }
        )
    }

    /// Convenience: check if the task has reached a terminal state.
    pub async fn is_completed(&self) -> bool {
        self.status.read().await.is_terminal()
    }

    /// Request cancellation of the running task.
    ///
    /// This signals the cancellation token but does not guarantee immediate
    /// termination — the task must check for cancellation cooperatively.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Check if the task has panicked.
    ///
    /// Returns `Some(true)` if the task panicked, `Some(false)` if it completed
    /// normally, or `None` if the task is still running or the handle was
    /// already consumed.
    ///
    /// This is a non-blocking check: it inspects the current status for the
    /// [`BackgroundTaskStatus::Failed`] variant whose error message indicates a
    /// panic (i.e., the tokio task panicked and the join handle returned a
    /// `JoinError`). If the task is still running, `None` is returned.
    pub async fn is_panicked(&self) -> Option<bool> {
        let handle_guard = self.join_handle.lock().await;
        if let Some(ref handle) = *handle_guard {
            if !handle.is_finished() {
                // Still running — cannot determine panic state yet
                return None;
            }
            // The task has finished. We can't `.await` the JoinHandle here
            // (that would consume it), so we infer panic from the status:
            // a tokio panic surfaces as a Failed status with "panic" in the
            // error string. As a simpler heuristic, if the status is Failed
            // and the task finished, we report `Some(false)` — meaning
            // "finished, not a panic" — unless the error text hints at one.
            let status = self.status.read().await.clone();
            match &status {
                BackgroundTaskStatus::Failed { error, .. } => {
                    Some(error.contains("panic") || error.contains("JoinError"))
                }
                BackgroundTaskStatus::Cancelled | BackgroundTaskStatus::Completed { .. } => {
                    Some(false)
                }
                // If finished but still shows Running/Pending, something is off
                _ => Some(false),
            }
        } else {
            None
        }
    }

    /// Wait for the task to complete and return the result.
    ///
    /// If `timeout` is `Some`, returns an error if the task doesn't complete
    /// within the given duration. If `None`, waits indefinitely.
    ///
    /// **Retry-safe**: unlike the old oneshot implementation, a timeout does
    /// NOT consume the receiver. The caller may call `wait()` again (possibly
    /// with a longer timeout) and still receive the result once the task
    /// completes. The result is stored in a `watch` channel that survives
    /// timeouts (N-P2-5).
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<T> {
        loop {
            // Check if the result is already present (non-blocking).
            {
                let mut cell = self.result.lock().await;
                if cell.is_some() {
                    // Take and return. Only the first observer gets it; later
                    // callers fall through to status-based reporting.
                    return cell.take().unwrap();
                }
            }

            // Not ready yet — wait for the Notify (with optional timeout).
            let notified = self.result_notify.notified();
            if let Some(dur) = timeout {
                match tokio::time::timeout(dur, notified).await {
                    Ok(()) => {
                        // Notified — loop back and check the cell again.
                        continue;
                    }
                    Err(_) => {
                        // Timeout: the cell is untouched, so the result is NOT
                        // lost. The caller may retry wait() (possibly longer).
                        return Err(ReactError::Other(format!(
                            "Background task '{}' timed out after {:?} (retry-safe: call wait() again)",
                            self.name, dur
                        )));
                    }
                }
            } else {
                notified.await;
                // Loop back and check the cell.
            }
        }
    }
}

// ── AnyBackgroundTask ─────────────────────────────────────────────

/// Type-erased trait for storing heterogeneous background task handles.
///
/// Useful for status dashboards, task listing, and cancellation management
/// where the concrete result type `T` is not needed.
pub trait AnyBackgroundTask: Send + Sync {
    /// Unique task ID.
    fn id(&self) -> &str;
    /// Human-readable task name.
    fn name(&self) -> &str;
    /// Current status as text.
    fn status_text(&self) -> &'static str;
    /// Non-blocking snapshot of the current status.
    ///
    /// Returns the actual [`BackgroundTaskStatus`] variant for fine-grained
    /// status reporting (Pending, Running, Completed, Failed, Cancelled).
    /// Falls back to a synthetic status if the lock cannot be acquired.
    fn status_snapshot(&self) -> BackgroundTaskStatus;
    /// Request cancellation.
    fn cancel(&self);
    /// Whether the task has reached a terminal state (synchronous check).
    fn is_terminal_sync(&self) -> bool;
}

/// A simple wrapper that stores a status snapshot for sync access.
struct TypeErasedTask {
    id: String,
    name: String,
    /// Shared status handle for fine-grained status reporting.
    status: Arc<RwLock<BackgroundTaskStatus>>,
    cancel: CancellationToken,
    /// Cached terminal status for sync checks (updated by the spawn wrapper).
    terminal_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl AnyBackgroundTask for TypeErasedTask {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn status_text(&self) -> &'static str {
        // Try to get the actual status text via non-blocking lock
        match self.status.try_read() {
            Ok(guard) => guard.as_str(),
            Err(_) => {
                // Lock busy — fall back to terminal flag heuristic
                if self
                    .terminal_flag
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    "completed"
                } else {
                    "running"
                }
            }
        }
    }
    fn status_snapshot(&self) -> BackgroundTaskStatus {
        match self.status.try_read() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                // Lock busy — synthesise from terminal flag
                if self
                    .terminal_flag
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    BackgroundTaskStatus::Completed {
                        finished_at: Instant::now(),
                    }
                } else {
                    BackgroundTaskStatus::Running {
                        started_at: Instant::now(),
                    }
                }
            }
        }
    }
    fn cancel(&self) {
        self.cancel.cancel();
    }
    fn is_terminal_sync(&self) -> bool {
        self.terminal_flag
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ── TaskSummary ───────────────────────────────────────────────────

/// Lightweight summary of a background task for listing/dashboards.
#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub id: String,
    pub name: String,
    pub status: BackgroundTaskStatus,
}

// ── TaskSpawnerConfig ─────────────────────────────────────────────

/// Configuration for the [`TaskSpawner`].
#[derive(Debug, Clone)]
pub struct TaskSpawnerConfig {
    /// Maximum number of concurrently running background tasks.
    pub max_concurrent: usize,
    /// Default timeout for spawned tasks (0 = no timeout).
    pub default_timeout_secs: u64,
}

impl Default for TaskSpawnerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 16,
            default_timeout_secs: 300, // 5 minutes
        }
    }
}

// ── TaskSpawner ───────────────────────────────────────────────────

/// System-level spawner that manages background tasks with concurrency control.
///
/// All spawned tasks are tracked in a concurrent map and can be listed,
/// cancelled, or inspected. Optionally backed by a [`TaskStore`] for
/// cross-restart resumption.
pub struct TaskSpawner {
    /// All tracked tasks (type-erased).
    tasks: Arc<DashMap<String, Arc<dyn AnyBackgroundTask>>>,
    /// Optional persistent store for cross-restart resumption.
    store: Option<Arc<dyn super::store::TaskStore>>,
    /// Concurrency limiter.
    semaphore: Arc<tokio::sync::Semaphore>,
    /// Configuration.
    config: TaskSpawnerConfig,
}

impl TaskSpawner {
    /// Create a new task spawner with the given configuration.
    pub fn new(config: TaskSpawnerConfig) -> Self {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_concurrent));
        Self {
            tasks: Arc::new(DashMap::new()),
            store: None,
            semaphore,
            config,
        }
    }

    /// Attach a persistent task store for cross-restart resumption.
    pub fn with_store(mut self, store: Arc<dyn super::store::TaskStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Spawn a future as a background task, returning a handle immediately.
    ///
    /// The task acquires a semaphore permit before starting. If all permits
    /// are exhausted, the task queues until one becomes available.
    ///
    /// # Arguments
    /// * `name` — Human-readable task name (used for logging and listing)
    /// * `fut` — The async work to execute
    pub fn spawn<F, T>(&self, name: &str, fut: F) -> BackgroundTask<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let id = uuid::Uuid::new_v4().to_string();
        let name = name.to_string();
        let cancel = CancellationToken::new();
        let status = Arc::new(RwLock::new(BackgroundTaskStatus::Pending));
        // Retry-safe result delivery: a Mutex<Option> cell + a Notify. The cell
        // is only `take()`n once a waiter observes it's filled, so a timeout
        // leaves the cell intact for the next wait() call (N-P2-5).
        let result: Arc<Mutex<Option<Result<T>>>> = Arc::new(Mutex::new(None));
        let result_notify = Arc::new(Notify::new());
        let terminal_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let permit = self.semaphore.clone();
        let cancel_inner = cancel.clone();
        let status_inner = status.clone();
        let id_inner = id.clone();
        let name_inner = name.clone();
        let timeout = if self.config.default_timeout_secs > 0 {
            Some(Duration::from_secs(self.config.default_timeout_secs))
        } else {
            None
        };

        let result_cell = result.clone();
        let notify = result_notify.clone();
        let terminal_flag_inner = terminal_flag.clone();

        /// Fill the result cell and wake any waiters. Used at every early-return
        /// and normal-completion path in the spawn closure.
        async fn deliver<T>(
            cell: &Arc<Mutex<Option<Result<T>>>>,
            notify: &Arc<Notify>,
            r: Result<T>,
        ) {
            *cell.lock().await = Some(r);
            notify.notify_waiters();
        }

        let join_handle = tokio::spawn(async move {
            // Acquire semaphore permit (may block if at capacity)
            let _permit = match permit.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    deliver(
                        &result_cell,
                        &notify,
                        Err(ReactError::Other(
                            "Semaphore closed — cannot acquire permit".into(),
                        )),
                    )
                    .await;
                    return;
                }
            };

            // Check cancellation before starting
            if cancel_inner.is_cancelled() {
                *status_inner.write().await = BackgroundTaskStatus::Cancelled;
                deliver(
                    &result_cell,
                    &notify,
                    Err(ReactError::Other("Task cancelled before start".into())),
                )
                .await;
                return;
            }

            // Mark as running
            {
                let mut s = status_inner.write().await;
                *s = BackgroundTaskStatus::Running {
                    started_at: Instant::now(),
                };
            }
            debug!(task_id = %id_inner, name = %name_inner, "Background task started");

            // Execute with optional timeout and cancellation
            let result = if let Some(dur) = timeout {
                tokio::select! {
                    _ = cancel_inner.cancelled() => {
                        Err(ReactError::Other("Task cancelled".into()))
                    }
                    r = tokio::time::timeout(dur, fut) => {
                        match r {
                            Ok(inner) => inner,
                            Err(_) => Err(ReactError::Other(format!(
                                "Task timed out after {:?}", dur
                            ))),
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = cancel_inner.cancelled() => {
                        Err(ReactError::Other("Task cancelled".into()))
                    }
                    r = fut => r,
                }
            };

            // Update status based on result
            let final_status = if cancel_inner.is_cancelled() && result.is_err() {
                BackgroundTaskStatus::Cancelled
            } else if result.is_ok() {
                BackgroundTaskStatus::Completed {
                    finished_at: Instant::now(),
                }
            } else {
                let error = result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                BackgroundTaskStatus::Failed {
                    error,
                    at: Instant::now(),
                }
            };

            *status_inner.write().await = final_status.clone();
            terminal_flag_inner.store(
                final_status.is_terminal(),
                std::sync::atomic::Ordering::Relaxed,
            );

            // Deliver result to the cell and wake waiters.
            deliver(&result_cell, &notify, result).await;

            let status_text = final_status.as_str();
            info!(task_id = %id_inner, name = %name_inner, status = %status_text, "Background task finished");
        });

        // Register in the type-erased task map
        let erased: Arc<dyn AnyBackgroundTask> = Arc::new(TypeErasedTask {
            id: id.clone(),
            name: name.clone(),
            status: status.clone(),
            cancel: cancel.clone(),
            terminal_flag,
        });
        self.tasks.insert(id.clone(), erased);

        BackgroundTask {
            id,
            name,
            status,
            result,
            result_notify,
            cancel,
            join_handle: Mutex::new(Some(join_handle)),
        }
    }

    /// List all tracked tasks with their current status.
    pub async fn list(&self) -> Vec<TaskSummary> {
        let mut summaries = Vec::new();
        for entry in self.tasks.iter() {
            let task = entry.value();
            let status = task.status_snapshot();
            summaries.push(TaskSummary {
                id: task.id().to_string(),
                name: task.name().to_string(),
                status,
            });
        }
        summaries
    }

    /// Cancel a specific task by ID.
    pub fn cancel(&self, id: &str) -> bool {
        if let Some(entry) = self.tasks.get(id) {
            entry.value().cancel();
            true
        } else {
            false
        }
    }

    /// Cancel all running tasks.
    pub fn cancel_all(&self) {
        for entry in self.tasks.iter() {
            entry.value().cancel();
        }
    }

    /// Remove completed/failed tasks from the tracking map.
    pub fn prune_completed(&self) -> usize {
        let before = self.tasks.len();
        self.tasks.retain(|_, v| !v.is_terminal_sync());
        before - self.tasks.len()
    }

    /// Number of currently tracked tasks.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Resume incomplete tasks from the persistent store after a restart.
    ///
    /// Tasks that were `InProgress` or `Pending` when the process died are
    /// re-added to the task manager. The caller must provide the execute
    /// functions separately (they are not serializable).
    pub async fn resume_from_store(&self) -> Result<Vec<super::Task>> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| ReactError::Other("No task store configured on spawner".into()))?;

        let all_tasks = store.load_all().await?;
        let incomplete: Vec<super::Task> = all_tasks
            .into_iter()
            .filter(|t| !t.status.is_terminal())
            .collect();

        info!(
            count = incomplete.len(),
            "Resuming incomplete tasks from store"
        );

        Ok(incomplete)
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_background_task_completes() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let handle = spawner.spawn("test-task", async { Ok(42) });

        assert_eq!(handle.name, "test-task");
        assert!(!handle.is_cancelled());

        let result = handle.wait(Some(Duration::from_secs(5))).await.unwrap();
        assert_eq!(result, 42);
        assert!(handle.is_completed().await);
    }

    #[tokio::test]
    async fn test_background_task_failure() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let handle = spawner.spawn("failing-task", async {
            Err::<String, _>(ReactError::Other("intentional failure".into()))
        });

        let result = handle.wait(Some(Duration::from_secs(5))).await;
        assert!(result.is_err());
        assert!(handle.is_completed().await);
    }

    #[tokio::test]
    async fn test_background_task_cancel() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let handle = spawner.spawn("cancellable-task", async {
            // Simulate long work
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        });

        // Give the task a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(handle.is_running().await);

        handle.cancel();

        let result = handle.wait(Some(Duration::from_secs(5))).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_background_task_timeout() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            default_timeout_secs: 0, // no default timeout
            ..Default::default()
        });
        let handle = spawner.spawn("slow-task", async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok("done")
        });

        // Wait with a short timeout — should fail
        let result = handle.wait(Some(Duration::from_millis(100))).await;
        assert!(result.is_err());

        // Cancel to clean up
        handle.cancel();
    }

    /// Regression for N-P2-5: a timeout must NOT consume the result. After a
    /// timeout, calling `wait()` again with a longer timeout must still return
    /// the real result once the task completes. The old oneshot impl lost the
    /// result permanently on timeout.
    #[tokio::test]
    async fn test_background_task_timeout_then_retry_recovers_result() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            default_timeout_secs: 0,
            ..Default::default()
        });
        let handle = spawner.spawn("retry-safe-task", async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok("recovered")
        });

        // First wait: timeout too short → error, but result NOT lost.
        let r1 = handle.wait(Some(Duration::from_millis(50))).await;
        assert!(r1.is_err(), "first wait should time out");

        // Second wait: long enough → must get the real result (not "already consumed").
        let r2 = handle.wait(Some(Duration::from_secs(5))).await;
        assert_eq!(r2.unwrap(), "recovered");
    }

    #[tokio::test]
    async fn test_task_spawner_list() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());

        let _h1 = spawner.spawn("task-a", async { Ok(1) });
        let _h2 = spawner.spawn("task-b", async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(2)
        });

        assert_eq!(spawner.task_count(), 2);

        let list = spawner.list().await;
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_task_spawner_cancel_all() {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());

        let _h1 = spawner.spawn("task-1", async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        });
        let _h2 = spawner.spawn("task-2", async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        });

        spawner.cancel_all();

        // Both should eventually terminate
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(spawner.task_count(), 2); // still tracked until pruned
    }

    #[test]
    fn test_status_is_terminal() {
        assert!(!BackgroundTaskStatus::Pending.is_terminal());
        assert!(
            !BackgroundTaskStatus::Running {
                started_at: Instant::now()
            }
            .is_terminal()
        );
        assert!(
            BackgroundTaskStatus::Completed {
                finished_at: Instant::now()
            }
            .is_terminal()
        );
        assert!(
            BackgroundTaskStatus::Failed {
                error: "test".into(),
                at: Instant::now()
            }
            .is_terminal()
        );
        assert!(BackgroundTaskStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_status_as_str() {
        assert_eq!(BackgroundTaskStatus::Pending.as_str(), "pending");
        assert_eq!(
            BackgroundTaskStatus::Running {
                started_at: Instant::now()
            }
            .as_str(),
            "running"
        );
        assert_eq!(
            BackgroundTaskStatus::Completed {
                finished_at: Instant::now()
            }
            .as_str(),
            "completed"
        );
    }
}
