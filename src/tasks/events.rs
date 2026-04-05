//! Task event system — lifecycle notifications for task state changes
//!
//! Uses async broadcast channel for non-blocking event distribution.
//! Listeners can subscribe and receive events in their own async tasks.

use crate::tasks::task::{Task, TaskStatus};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

/// Default channel capacity for the event bus
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

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
                let result_preview = if result.len() > 100 {
                    format!("{}...", &result[..100])
                } else {
                    result.clone()
                };
                info!(
                    task_id = %task_id,
                    result = %result_preview,
                    "task_completed"
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
    // Keep sync listeners for backward compatibility
    sync_listeners: Vec<Box<dyn TaskEventListener>>,
}

impl TaskEventBus {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            sync_listeners: Vec::new(),
        }
    }

    /// Register a sync listener (called immediately on emit)
    pub fn register(&mut self, listener: Box<dyn TaskEventListener>) {
        self.sync_listeners.push(listener);
    }

    /// Subscribe to async event stream
    ///
    /// Returns a receiver that can be used in async tasks.
    /// Note: If the receiver falls behind, old events will be dropped.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TaskEvent>> {
        self.tx.subscribe()
    }

    /// Emit an event to all listeners
    ///
    /// Sync listeners are called immediately.
    /// Async subscribers receive the event through their receiver.
    pub fn emit(&self, event: TaskEvent) {
        // Notify sync listeners
        for listener in &self.sync_listeners {
            listener.on_event(&event);
        }

        // Broadcast to async subscribers
        let event = Arc::new(event);
        let _ = self.tx.send(event);
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
        }
    }
}

impl Default for TaskEventBus {
    fn default() -> Self {
        Self::new()
    }
}
