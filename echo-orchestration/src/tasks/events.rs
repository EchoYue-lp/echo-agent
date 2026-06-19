//! Task event system — lifecycle notifications for task state changes
//!
//! Uses async broadcast channel for non-blocking event distribution.
//! Listeners can subscribe and receive events in their own async tasks.

use super::progress::TaskProgress;
use super::task::{Task, TaskStatus};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

/// Default channel capacity for the event bus
///
/// Increased to 512 to reduce event loss under high throughput.
/// If your workload generates many events, consider using a larger capacity.
const DEFAULT_CHANNEL_CAPACITY: usize = 512;

/// Lifecycle event emitted by the TaskManager
#[derive(Debug, Clone, Serialize)]
pub enum TaskEvent {
    Created {
        task: Task,
    },
    Updated {
        task_id: String,
        old_status: TaskStatus,
        new_status: TaskStatus,
    },
    Deleted {
        task_id: String,
    },
    Assigned {
        task_id: String,
        agent: String,
    },
    Failed {
        task_id: String,
        error: String,
        attempt: u32,
    },
    Completed {
        task_id: String,
        result: String,
    },
    /// Intra-task progress update (percentage, current phase, ETA).
    ///
    /// Emitted by [`ProgressReporter`](super::progress::ProgressReporter) for
    /// long-running tasks with a [`PhasePlan`](super::progress::PhasePlan).
    Progress {
        task_id: String,
        progress: TaskProgress,
    },
}

impl TaskEvent {
    pub fn task_id(&self) -> &str {
        match self {
            TaskEvent::Created { task } => &task.id,
            TaskEvent::Updated { task_id, .. } => task_id,
            TaskEvent::Deleted { task_id } => task_id,
            TaskEvent::Assigned { task_id, .. } => task_id,
            TaskEvent::Failed { task_id, .. } => task_id,
            TaskEvent::Completed { task_id, .. } => task_id,
            TaskEvent::Progress { task_id, .. } => task_id,
        }
    }
}

/// Listener trait for task events (sync version for simple cases)
pub trait TaskEventListener: Send + Sync {
    fn on_event(&self, event: &TaskEvent);
}

/// Async listener trait for task events
#[async_trait::async_trait]
pub trait AsyncTaskEventListener: Send + Sync {
    async fn on_event(&self, event: Arc<TaskEvent>);
}

/// Logging listener — records events via tracing
pub struct LoggingListener;

impl TaskEventListener for LoggingListener {
    fn on_event(&self, event: &TaskEvent) {
        match event {
            TaskEvent::Created { task } => {
                info!(task_id = %task.id, subject = %task.subject, "task_created");
            }
            TaskEvent::Updated {
                task_id,
                old_status,
                new_status,
            } => {
                info!(
                    task_id = %task_id,
                    old_status = ?old_status,
                    new_status = ?new_status,
                    "task_updated"
                );
            }
            TaskEvent::Deleted { task_id } => {
                info!(task_id = %task_id, "task_deleted");
            }
            TaskEvent::Assigned { task_id, agent } => {
                info!(task_id = %task_id, agent = %agent, "task_assigned");
            }
            TaskEvent::Failed {
                task_id,
                error,
                attempt,
            } => {
                info!(
                    task_id = %task_id,
                    error = %error,
                    attempt = attempt,
                    "task_failed"
                );
            }
            TaskEvent::Completed { task_id, result } => {
                let result_preview = if result.chars().count() > 100 {
                    let truncated: String = result.chars().take(100).collect();
                    format!("{truncated}...")
                } else {
                    result.clone()
                };
                info!(
                    task_id = %task_id,
                    result = %result_preview,
                    "task_completed"
                );
            }
            TaskEvent::Progress {
                task_id, progress, ..
            } => {
                tracing::debug!(
                    task_id = %task_id,
                    pct = progress.percentage,
                    phase = %progress.current_phase,
                    "task_progress"
                );
            }
        }
    }
}

/// Event bus — async broadcast events to all registered listeners
///
/// Uses `tokio::sync::broadcast` for efficient fan-out to multiple subscribers.
/// Events are wrapped in `Arc` for cheap cloning.
pub struct TaskEventBus {
    tx: broadcast::Sender<Arc<TaskEvent>>,
    // Keep sync listeners for backward compatibility (wrapped in Arc for safe sharing)
    sync_listeners: Vec<Arc<dyn TaskEventListener>>,
    // Bounds the number of concurrently executing sync listeners to prevent
    // unbounded tokio::spawn when tasks pile up (P1-2.4).
    sync_semaphore: Arc<tokio::sync::Semaphore>,
}

impl TaskEventBus {
    /// Maximum concurrent sync listener invocations.
    const MAX_SYNC_CONCURRENCY: usize = 32;

    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            sync_listeners: Vec::new(),
            sync_semaphore: Arc::new(tokio::sync::Semaphore::new(Self::MAX_SYNC_CONCURRENCY)),
        }
    }

    /// Register a sync listener (offloaded to background task on emit)
    pub fn register(&mut self, listener: Arc<dyn TaskEventListener>) {
        self.sync_listeners.push(listener);
    }

    /// Subscribe to async event stream
    ///
    /// Returns a receiver that can be used in async tasks.
    ///
    /// **Warning: Slow consumers may lag.** `tokio::sync::broadcast` drops
    /// events when the channel is full and the receiver falls behind.
    /// If you need guaranteed delivery, process events promptly or use
    /// a separate persistent queue.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TaskEvent>> {
        self.tx.subscribe()
    }

    /// Emit an event to all listeners
    ///
    /// Sync listeners are offloaded to background tasks via `tokio::spawn`
    /// to prevent blocking the caller, but bounded by a semaphore (P1-2.4).
    /// Async subscribers receive the event through their receiver.
    pub fn emit(&self, event: TaskEvent) {
        // Offload sync listeners to background tasks (bounded)
        let event_for_async = Arc::new(event.clone());
        for listener in &self.sync_listeners {
            let listener = listener.clone();
            let event = event.clone();
            let sem = self.sync_semaphore.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire().await;
                listener.on_event(&event);
            });
        }

        // Broadcast to async subscribers
        let _ = self.tx.send(event_for_async);
    }

    /// Get the number of async subscribers
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Clone for TaskEventBus {
    fn clone(&self) -> Self {
        // Cloning the bus creates a new sender pointing to the same channel
        Self {
            tx: self.tx.clone(),
            sync_listeners: Vec::new(), // Don't clone sync listeners
            sync_semaphore: self.sync_semaphore.clone(),
        }
    }
}

impl Default for TaskEventBus {
    fn default() -> Self {
        Self::new()
    }
}
