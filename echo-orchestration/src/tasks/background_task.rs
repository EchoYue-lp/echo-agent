//! Background task handle and spawner for long-running task support.
//!
//! This module provides a non-blocking, process-local abstraction for
//! fire-and-forget, pollable, cancellable background futures.
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
//! - [`TaskSpawner`] — Process-local spawner that manages concurrency (via
//!   Semaphore) and tracks spawned futures. Durable task graphs are owned by
//!   [`crate::tasks::TaskRevisionService`].
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
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

// ── BackgroundTaskStatus ──────────────────────────────────────────

/// Lifecycle status of a background task.
#[derive(Debug, Clone)]
pub enum BackgroundTaskStatus {
    /// The background future is queued but not yet started.
    Pending,
    /// The background future is currently executing.
    Running {
        /// When the task started executing.
        started_at: Instant,
    },
    /// The background future completed successfully.
    Completed {
        /// When the task finished.
        finished_at: Instant,
    },
    /// The background future failed with an error.
    Failed {
        /// Human-readable error description.
        error: String,
        /// When the failure occurred.
        at: Instant,
    },
    /// The background future was cancelled via its cancellation token.
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

struct BackgroundTaskHandleState<T> {
    status: BackgroundTaskStatus,
    result: Option<Result<T>>,
    panicked: bool,
}

trait BackgroundTaskStatusSource: Send + Sync {
    fn snapshot(&self) -> BackgroundTaskStatus;
}

impl<T: Send + 'static> BackgroundTaskStatusSource for Mutex<BackgroundTaskHandleState<T>> {
    fn snapshot(&self) -> BackgroundTaskStatus {
        self.lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .clone()
    }
}

/// A handle to a background task that can be polled, awaited, or cancelled.
///
/// The task runs asynchronously on the tokio runtime. The handle is cheap to
/// clone (internally uses `Arc`), and multiple handles can exist for the same task.
pub struct BackgroundTask<T: Send + 'static> {
    /// Unique process-local task ID.
    pub id: String,
    /// Human-readable name/description.
    pub name: String,
    /// Single authority for lifecycle status and the single-consumer result.
    state: Arc<Mutex<BackgroundTaskHandleState<T>>>,
    /// Notifier fired after a terminal state is atomically published.
    result_notify: Arc<Notify>,
    /// Token to request cancellation of the running task.
    cancel: CancellationToken,
}

impl<T: Send + 'static> Clone for BackgroundTask<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            state: Arc::clone(&self.state),
            result_notify: Arc::clone(&self.result_notify),
            cancel: self.cancel.clone(),
        }
    }
}

impl<T: Send + 'static> BackgroundTask<T> {
    /// Non-blocking snapshot of the current status.
    pub async fn status(&self) -> BackgroundTaskStatus {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .clone()
    }

    /// Convenience: check if the task is still running (non-blocking).
    pub async fn is_running(&self) -> bool {
        matches!(self.status().await, BackgroundTaskStatus::Running { .. })
    }

    /// Convenience: check if the task has reached a terminal state.
    pub async fn is_completed(&self) -> bool {
        self.status().await.is_terminal()
    }

    /// Request cancellation of the running task.
    ///
    /// This signals the cancellation token but does not guarantee immediate
    /// termination. The supervisor observes it during admission or execution;
    /// during execution it aborts and awaits the child before publishing the
    /// terminal state.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Check if the task has panicked.
    ///
    /// Returns `Some(true)` for a caught child execution-task panic,
    /// `Some(false)` for another terminal, or `None` while nonterminal.
    pub async fn is_panicked(&self) -> Option<bool> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match state.status {
            BackgroundTaskStatus::Pending | BackgroundTaskStatus::Running { .. } => None,
            BackgroundTaskStatus::Completed { .. }
            | BackgroundTaskStatus::Failed { .. }
            | BackgroundTaskStatus::Cancelled => Some(state.panicked),
        }
    }

    /// Wait for the task to complete and return the result.
    ///
    /// If `timeout` is `Some`, returns an error if the task doesn't complete
    /// within the given duration. If `None`, waits indefinitely.
    ///
    /// A timeout does not consume the result. The first terminal waiter owns
    /// `T`; later waiters return immediately from the persistent status.
    pub async fn wait(&self, timeout: Option<Duration>) -> Result<T> {
        let deadline = match timeout {
            Some(duration) => Some(
                tokio::time::Instant::now()
                    .checked_add(duration)
                    .ok_or_else(|| {
                        ReactError::Other(format!(
                            "Background task '{}' wait deadline overflow",
                            self.name
                        ))
                    })?,
            ),
            None => None,
        };
        loop {
            let notified = self.result_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let status = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                if let Some(result) = state.result.take() {
                    return result;
                }
                state.status.clone()
            };
            match status {
                BackgroundTaskStatus::Completed { .. } => {
                    return Err(ReactError::Other(format!(
                        "Background task '{}' completed, but its result was already consumed by another waiter",
                        self.name
                    )));
                }
                BackgroundTaskStatus::Failed { error, .. } => {
                    return Err(ReactError::Other(format!(
                        "Background task '{}' failed: {error}",
                        self.name
                    )));
                }
                BackgroundTaskStatus::Cancelled => {
                    return Err(ReactError::Other(format!(
                        "Background task '{}' was cancelled",
                        self.name
                    )));
                }
                BackgroundTaskStatus::Pending | BackgroundTaskStatus::Running { .. } => {}
            }
            match deadline {
                Some(deadline) => {
                    if tokio::time::timeout_at(deadline, &mut notified)
                        .await
                        .is_err()
                    {
                        let duration = timeout.unwrap_or_default();
                        return Err(ReactError::Other(format!(
                            "Background task '{}' timed out after {:?} (retry-safe: call wait() again)",
                            self.name, duration
                        )));
                    }
                }
                None => (&mut notified).await,
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
    /// Returns the actual [`BackgroundTaskStatus`] variant from the same shared
    /// state used by the typed handle.
    fn status_snapshot(&self) -> BackgroundTaskStatus;
    /// Request cancellation.
    fn cancel(&self);
    /// Whether the task has reached a terminal state (synchronous check).
    fn is_terminal_sync(&self) -> bool;
    /// Monotonic registration order used for bounded history retention.
    fn sequence(&self) -> u64;
}

/// A type-erased live view of the typed handle's shared lifecycle state.
struct TypeErasedTask {
    id: String,
    name: String,
    /// Type-erased view of the same generic state held by BackgroundTask<T>.
    state: Arc<dyn BackgroundTaskStatusSource>,
    cancel: CancellationToken,
    sequence: u64,
}

impl AnyBackgroundTask for TypeErasedTask {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn status_text(&self) -> &'static str {
        self.state.snapshot().as_str()
    }
    fn status_snapshot(&self) -> BackgroundTaskStatus {
        self.state.snapshot()
    }
    fn cancel(&self) {
        self.cancel.cancel();
    }
    fn is_terminal_sync(&self) -> bool {
        self.state.snapshot().is_terminal()
    }
    fn sequence(&self) -> u64 {
        self.sequence
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
    /// Maximum terminal task summaries retained for status/list queries.
    /// Running and pending tasks are never removed by retention.
    pub max_terminal_history: usize,
}

impl Default for TaskSpawnerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 16,
            default_timeout_secs: 300, // 5 minutes
            max_terminal_history: 256,
        }
    }
}

// ── TaskSpawner ───────────────────────────────────────────────────

/// System-level spawner that manages background tasks with concurrency control.
///
/// All spawned futures are tracked in a concurrent map and can be listed,
/// cancelled, or inspected. This process-local handle registry does not own
/// durable task relationships or restart recovery.
pub struct TaskSpawner {
    /// All tracked tasks (type-erased).
    tasks: Arc<DashMap<String, Arc<dyn AnyBackgroundTask>>>,
    /// Concurrency limiter.
    semaphore: Arc<tokio::sync::Semaphore>,
    /// Configuration.
    config: TaskSpawnerConfig,
    /// Monotonic registration sequence used for deterministic terminal retention.
    next_sequence: AtomicU64,
}

fn publish_terminal<T: Send + 'static>(
    state: &Arc<Mutex<BackgroundTaskHandleState<T>>>,
    notify: &Arc<Notify>,
    result: Result<T>,
    status: BackgroundTaskStatus,
    panicked: bool,
) {
    {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        if state.status.is_terminal() {
            return;
        }
        state.result = Some(result);
        state.status = status;
        state.panicked = panicked;
    }
    notify.notify_waiters();
}

fn publish_running<T: Send + 'static>(state: &Arc<Mutex<BackgroundTaskHandleState<T>>>) {
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    if matches!(state.status, BackgroundTaskStatus::Pending) {
        state.status = BackgroundTaskStatus::Running {
            started_at: Instant::now(),
        };
    }
}

fn completion_from_join<T: Send + 'static>(
    joined: std::result::Result<Result<T>, tokio::task::JoinError>,
) -> (Result<T>, BackgroundTaskStatus, bool) {
    match joined {
        Ok(result) => {
            let status = match result.as_ref() {
                Ok(_) => BackgroundTaskStatus::Completed {
                    finished_at: Instant::now(),
                },
                Err(error) => BackgroundTaskStatus::Failed {
                    error: error.to_string(),
                    at: Instant::now(),
                },
            };
            (result, status, false)
        }
        Err(error) => {
            let panicked = error.is_panic();
            let message = if panicked {
                format!("Background execution task panicked: {error}")
            } else {
                format!("Background execution task failed to join: {error}")
            };
            (
                Err(ReactError::Other(message.clone())),
                BackgroundTaskStatus::Failed {
                    error: message,
                    at: Instant::now(),
                },
                panicked,
            )
        }
    }
}

async fn abort_execution_task<T: Send + 'static>(task: &mut JoinHandle<Result<T>>) {
    task.abort();
    let _ = task.await;
}

async fn supervise_execution<T: Send + 'static>(
    mut execution_task: JoinHandle<Result<T>>,
    cancel: CancellationToken,
    deadline: Option<tokio::time::Instant>,
    timeout: Option<Duration>,
) -> (Result<T>, BackgroundTaskStatus, bool) {
    if let Some(deadline) = deadline {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                abort_execution_task(&mut execution_task).await;
                (
                    Err(ReactError::Other("Task cancelled".to_string())),
                    BackgroundTaskStatus::Cancelled,
                    false,
                )
            }
            _ = tokio::time::sleep_until(deadline) => {
                abort_execution_task(&mut execution_task).await;
                let duration = timeout.unwrap_or_default();
                let message = format!("Task timed out after {duration:?}");
                (
                    Err(ReactError::Other(message.clone())),
                    BackgroundTaskStatus::Failed {
                        error: message,
                        at: Instant::now(),
                    },
                    false,
                )
            }
            joined = &mut execution_task => completion_from_join(joined),
        }
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                abort_execution_task(&mut execution_task).await;
                (
                    Err(ReactError::Other("Task cancelled".to_string())),
                    BackgroundTaskStatus::Cancelled,
                    false,
                )
            }
            joined = &mut execution_task => completion_from_join(joined),
        }
    }
}

impl TaskSpawner {
    /// Create a new task spawner with the given configuration.
    pub fn new(config: TaskSpawnerConfig) -> Self {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_concurrent));
        Self {
            tasks: Arc::new(DashMap::new()),
            semaphore,
            config,
            next_sequence: AtomicU64::new(0),
        }
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
        self.prune_terminal_history();
        let id = uuid::Uuid::new_v4().to_string();
        let name = name.to_string();
        let cancel = CancellationToken::new();
        let state = Arc::new(Mutex::new(BackgroundTaskHandleState {
            status: BackgroundTaskStatus::Pending,
            result: None,
            panicked: false,
        }));
        let result_notify = Arc::new(Notify::new());

        let permit = self.semaphore.clone();
        let cancel_inner = cancel.clone();
        let state_inner = Arc::clone(&state);
        let id_inner = id.clone();
        let name_inner = name.clone();
        let max_concurrent = self.config.max_concurrent;
        let timeout = if self.config.default_timeout_secs > 0 {
            Some(Duration::from_secs(self.config.default_timeout_secs))
        } else {
            None
        };
        let accepted_at = tokio::time::Instant::now();
        let deadline = timeout.and_then(|duration| accepted_at.checked_add(duration));
        let deadline_overflow = timeout.is_some() && deadline.is_none();
        let notify = Arc::clone(&result_notify);

        let supervisor = tokio::spawn(async move {
            if max_concurrent == 0 {
                let message = "TaskSpawner max_concurrent must be greater than zero".to_string();
                publish_terminal(
                    &state_inner,
                    &notify,
                    Err(ReactError::Other(message.clone())),
                    BackgroundTaskStatus::Failed {
                        error: message,
                        at: Instant::now(),
                    },
                    false,
                );
                return;
            }
            if deadline_overflow {
                let message = "Background task deadline overflow".to_string();
                publish_terminal(
                    &state_inner,
                    &notify,
                    Err(ReactError::Other(message.clone())),
                    BackgroundTaskStatus::Failed {
                        error: message,
                        at: Instant::now(),
                    },
                    false,
                );
                return;
            }

            let acquire = permit.acquire_owned();
            tokio::pin!(acquire);
            let _permit = if let Some(deadline) = deadline {
                tokio::select! {
                    biased;
                    _ = cancel_inner.cancelled() => {
                        publish_terminal(
                            &state_inner,
                            &notify,
                            Err(ReactError::Other("Task cancelled before start".to_string())),
                            BackgroundTaskStatus::Cancelled,
                            false,
                        );
                        return;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        let duration = timeout.unwrap_or_default();
                        let message = format!("Task timed out during admission after {duration:?}");
                        publish_terminal(
                            &state_inner,
                            &notify,
                            Err(ReactError::Other(message.clone())),
                            BackgroundTaskStatus::Failed {
                                error: message,
                                at: Instant::now(),
                            },
                            false,
                        );
                        return;
                    }
                    acquired = &mut acquire => acquired,
                }
            } else {
                tokio::select! {
                    biased;
                    _ = cancel_inner.cancelled() => {
                        publish_terminal(
                            &state_inner,
                            &notify,
                            Err(ReactError::Other("Task cancelled before start".to_string())),
                            BackgroundTaskStatus::Cancelled,
                            false,
                        );
                        return;
                    }
                    acquired = &mut acquire => acquired,
                }
            };
            let _permit = match _permit {
                Ok(permit) => permit,
                Err(_) => {
                    let message = "Semaphore closed - cannot acquire permit".to_string();
                    publish_terminal(
                        &state_inner,
                        &notify,
                        Err(ReactError::Other(message.clone())),
                        BackgroundTaskStatus::Failed {
                            error: message,
                            at: Instant::now(),
                        },
                        false,
                    );
                    return;
                }
            };

            if cancel_inner.is_cancelled() {
                publish_terminal(
                    &state_inner,
                    &notify,
                    Err(ReactError::Other("Task cancelled before start".to_string())),
                    BackgroundTaskStatus::Cancelled,
                    false,
                );
                return;
            }
            if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                let duration = timeout.unwrap_or_default();
                let message = format!("Task timed out during admission after {duration:?}");
                publish_terminal(
                    &state_inner,
                    &notify,
                    Err(ReactError::Other(message.clone())),
                    BackgroundTaskStatus::Failed {
                        error: message,
                        at: Instant::now(),
                    },
                    false,
                );
                return;
            }

            publish_running(&state_inner);
            debug!(task_id = %id_inner, name = %name_inner, "Background task started");
            let execution_task = tokio::spawn(fut);
            let (result, final_status, panicked) =
                supervise_execution(execution_task, cancel_inner, deadline, timeout).await;
            publish_terminal(
                &state_inner,
                &notify,
                result,
                final_status.clone(),
                panicked,
            );
            let status_text = final_status.as_str();
            info!(task_id = %id_inner, name = %name_inner, status = %status_text, "Background task finished");
        });
        std::mem::drop(supervisor);

        // Register in the type-erased task map
        let erased_state: Arc<dyn BackgroundTaskStatusSource> = state.clone();
        let erased: Arc<dyn AnyBackgroundTask> = Arc::new(TypeErasedTask {
            id: id.clone(),
            name: name.clone(),
            state: erased_state,
            cancel: cancel.clone(),
            sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
        });
        self.tasks.insert(id.clone(), erased);

        BackgroundTask {
            id,
            name,
            state,
            result_notify,
            cancel,
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

    /// Retain at most the configured number of terminal task summaries.
    ///
    /// This runs before registration so a newly spawned task is always tracked.
    /// Pending/running tasks are never selected, even if the map is above the
    /// terminal-history limit.
    fn prune_terminal_history(&self) -> usize {
        let max_terminal_history = self.config.max_terminal_history;
        let mut terminal = self
            .tasks
            .iter()
            .filter(|entry| entry.value().is_terminal_sync())
            .map(|entry| (entry.value().sequence(), entry.key().clone()))
            .collect::<Vec<_>>();
        let remove_count = terminal.len().saturating_sub(max_terminal_history);
        if remove_count == 0 {
            return 0;
        }
        terminal.sort_by_key(|(sequence, _)| *sequence);
        for (_, id) in terminal.into_iter().take(remove_count) {
            self.tasks.remove(&id);
        }
        remove_count
    }

    /// Number of currently tracked tasks.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct NonCloneResult(u32);

    struct DropProbe(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[tokio::test]
    async fn test_background_task_completes() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let handle = spawner.spawn("test-task", async { Ok(42) });

        assert_eq!(handle.name, "test-task");
        assert!(!handle.is_cancelled());

        let result = handle.wait(Some(Duration::from_secs(5))).await?;
        assert_eq!(result, 42);
        assert!(handle.is_completed().await);
        Ok(())
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
    async fn ordinary_failure_cannot_spoof_panic_provenance() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let handle = spawner.spawn("panic-prefix-failure", async {
            Err::<(), _>(ReactError::Other(
                "Background execution task panicked: forged".to_string(),
            ))
        });

        let result = handle.wait(Some(Duration::from_secs(1))).await;
        if result.is_ok()
            || !matches!(handle.status().await, BackgroundTaskStatus::Failed { .. })
            || handle.is_panicked().await != Some(false)
        {
            return Err(ReactError::Other(
                "ordinary failure was misclassified as an execution panic".to_string(),
            ));
        }
        Ok(())
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
    async fn test_background_task_timeout_then_retry_recovers_result() -> Result<()> {
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
        assert_eq!(r2?, "recovered");
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_waiters_both_reach_a_terminal_observation() -> Result<()> {
        use std::task::Poll;

        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let handle = spawner.spawn("multi-waiter", async move {
            release_rx
                .await
                .map_err(|_| ReactError::Other("release sender closed".to_string()))?;
            Ok(NonCloneResult(42))
        });
        let second_handle = handle.clone();
        let waits = async { tokio::join!(handle.wait(None), second_handle.wait(None)) };
        tokio::pin!(waits);
        if !matches!(futures::poll!(&mut waits), Poll::Pending) {
            return Err(ReactError::Other(
                "waiters completed before task release".to_string(),
            ));
        }
        release_tx
            .send(())
            .map_err(|_| ReactError::Other("release receiver closed".to_string()))?;
        let (first, second) = tokio::time::timeout(Duration::from_millis(300), &mut waits)
            .await
            .map_err(|_| ReactError::Other("a terminal waiter remained blocked".to_string()))?;
        let successes = usize::from(first.as_ref().is_ok_and(|value| value.0 == 42))
            .saturating_add(usize::from(
                second.as_ref().is_ok_and(|value| value.0 == 42),
            ));
        if successes != 1 || first.is_ok() == second.is_ok() {
            return Err(ReactError::Other(
                "single-consumer result was not paired with one terminal observer".to_string(),
            ));
        }
        let observer_error = first.err().or_else(|| second.err()).ok_or_else(|| {
            ReactError::Other("terminal observer did not receive an error".to_string())
        })?;
        if !observer_error.to_string().contains("already consumed") {
            return Err(ReactError::Other(format!(
                "terminal observer returned the wrong error: {observer_error}"
            )));
        }
        Ok(())
    }

    #[tokio::test]
    async fn queued_cancellation_settles_without_starting_the_future() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            max_concurrent: 1,
            default_timeout_secs: 0,
            ..TaskSpawnerConfig::default()
        });
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let release = Arc::new(Notify::new());
        let first_release = Arc::clone(&release);
        let first = spawner.spawn("permit-holder", async move {
            let _ = started_tx.send(());
            first_release.notified().await;
            Ok(())
        });
        started_rx
            .await
            .map_err(|_| ReactError::Other("permit holder did not start".to_string()))?;

        let execution_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let execution_started_inner = Arc::clone(&execution_started);
        let queued = spawner.spawn("queued-cancel", async move {
            execution_started_inner.store(true, Ordering::Release);
            Ok(())
        });
        queued.cancel();
        let result = tokio::time::timeout(Duration::from_millis(300), queued.wait(None))
            .await
            .map_err(|_| ReactError::Other("queued cancellation did not settle".to_string()))?;
        if result.is_ok()
            || execution_started.load(Ordering::Acquire)
            || !matches!(queued.status().await, BackgroundTaskStatus::Cancelled)
        {
            return Err(ReactError::Other(
                "queued cancellation started work or published the wrong terminal".to_string(),
            ));
        }
        release.notify_one();
        first.wait(Some(Duration::from_secs(1))).await?;
        Ok(())
    }

    #[tokio::test]
    async fn zero_concurrency_settles_as_configuration_failure() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            max_concurrent: 0,
            default_timeout_secs: 0,
            ..TaskSpawnerConfig::default()
        });
        let execution_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let execution_started_inner = Arc::clone(&execution_started);
        let handle = spawner.spawn("zero-concurrency", async move {
            execution_started_inner.store(true, Ordering::Release);
            Ok(())
        });

        let result = tokio::time::timeout(Duration::from_millis(300), handle.wait(None))
            .await
            .map_err(|_| ReactError::Other("zero concurrency remained pending".to_string()))?;
        if result.is_ok()
            || execution_started.load(Ordering::Acquire)
            || !matches!(handle.status().await, BackgroundTaskStatus::Failed { .. })
        {
            return Err(ReactError::Other(
                "zero concurrency did not publish a configuration failure".to_string(),
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn queued_deadline_settles_without_starting_the_future() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            max_concurrent: 1,
            default_timeout_secs: 1,
            ..TaskSpawnerConfig::default()
        });
        let permit = spawner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ReactError::Other("test semaphore was closed".to_string()))?;
        let execution_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let execution_started_inner = Arc::clone(&execution_started);
        let queued = spawner.spawn("queued-deadline", async move {
            execution_started_inner.store(true, Ordering::Release);
            Ok(())
        });

        let result = queued.wait(Some(Duration::from_secs(2))).await;
        if result.is_ok()
            || execution_started.load(Ordering::Acquire)
            || !matches!(queued.status().await, BackgroundTaskStatus::Failed { .. })
        {
            return Err(ReactError::Other(
                "queued deadline started work or published the wrong terminal".to_string(),
            ));
        }
        drop(permit);
        Ok(())
    }

    #[tokio::test]
    async fn execution_cancel_waits_for_child_task_drop() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            default_timeout_secs: 0,
            ..TaskSpawnerConfig::default()
        });
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped_inner = Arc::clone(&dropped);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let handle = spawner.spawn("execution-cancel", async move {
            let _probe = DropProbe(dropped_inner);
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            Ok(())
        });
        started_rx
            .await
            .map_err(|_| ReactError::Other("execution task did not start".to_string()))?;
        handle.cancel();
        let result = handle.wait(Some(Duration::from_secs(1))).await;
        if result.is_ok()
            || !dropped.load(Ordering::Acquire)
            || !matches!(handle.status().await, BackgroundTaskStatus::Cancelled)
        {
            return Err(ReactError::Other(
                "execution cancellation published before child task drop".to_string(),
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn execution_timeout_waits_for_child_task_drop() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            default_timeout_secs: 1,
            ..TaskSpawnerConfig::default()
        });
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped_inner = Arc::clone(&dropped);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let handle = spawner.spawn("execution-timeout", async move {
            let _probe = DropProbe(dropped_inner);
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            Ok(())
        });
        started_rx
            .await
            .map_err(|_| ReactError::Other("execution task did not start".to_string()))?;
        let result = handle.wait(Some(Duration::from_secs(2))).await;
        if result.is_ok()
            || !dropped.load(Ordering::Acquire)
            || !matches!(handle.status().await, BackgroundTaskStatus::Failed { .. })
        {
            return Err(ReactError::Other(
                "execution timeout published before child task drop".to_string(),
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn type_erased_registry_preserves_failure_and_cancellation() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig::default());
        let failed = spawner.spawn("listed-failure", async {
            Err::<(), _>(ReactError::Other("listed failure".to_string()))
        });
        let failed_id = failed.id.clone();
        let _ = failed.wait(Some(Duration::from_secs(1))).await;

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let cancelled = spawner.spawn("listed-cancel", async move {
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            Ok(())
        });
        let cancelled_id = cancelled.id.clone();
        started_rx
            .await
            .map_err(|_| ReactError::Other("listed cancellation did not start".to_string()))?;
        cancelled.cancel();
        let _ = cancelled.wait(Some(Duration::from_secs(1))).await;

        let summaries = spawner.list().await;
        let failed_status = summaries
            .iter()
            .find(|summary| summary.id == failed_id)
            .map(|summary| &summary.status);
        let cancelled_status = summaries
            .iter()
            .find(|summary| summary.id == cancelled_id)
            .map(|summary| &summary.status);
        if !matches!(failed_status, Some(BackgroundTaskStatus::Failed { .. }))
            || !matches!(cancelled_status, Some(BackgroundTaskStatus::Cancelled))
        {
            return Err(ReactError::Other(
                "type-erased task registry changed terminal meaning".to_string(),
            ));
        }
        Ok(())
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
    async fn terminal_history_is_bounded_without_removing_running_tasks() -> Result<()> {
        let spawner = TaskSpawner::new(TaskSpawnerConfig {
            max_concurrent: 4,
            default_timeout_secs: 0,
            max_terminal_history: 2,
        });

        for index in 0..4 {
            let handle = spawner.spawn(&format!("completed-{index}"), async { Ok(()) });
            handle.wait(Some(Duration::from_secs(1))).await?;
        }

        let running = spawner.spawn("still-running", async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        });
        let trigger = spawner.spawn("retention-trigger", async { Ok(()) });

        let summaries = spawner.list().await;
        let completed_history = summaries
            .iter()
            .filter(|summary| summary.name.starts_with("completed-"))
            .count();
        assert_eq!(completed_history, 2);
        assert!(summaries.iter().any(|summary| summary.id == running.id));
        assert!(summaries.iter().any(|summary| summary.id == trigger.id));

        running.cancel();
        let _ = running.wait(Some(Duration::from_secs(1))).await;
        trigger.wait(Some(Duration::from_secs(1))).await?;
        Ok(())
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
