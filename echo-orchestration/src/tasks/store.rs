//! Task persistence layer — stores task state for recovery and checkpointing
//!
//! Uses the existing [`Store`] trait internally, so any Store implementation
//! (SQLite, in-memory, etc.) works out of the box.

use super::task::{Task, TaskStatus};
use echo_core::error::Result;
use echo_state::memory::store::Store;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Trait for task persistence operations
pub trait TaskStore: Send + Sync {
    /// Persist a single task
    fn save_task<'a>(&'a self, task: &'a Task) -> BoxFuture<'a, Result<()>>;

    /// Load a task by ID
    fn load_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Task>>>;

    /// Load all tasks (with automatic pagination — no hard limit)
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Task>>>;

    /// Delete a task by ID
    fn delete_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<bool>>;

    /// Save all tasks (batch upsert)
    fn save_all<'a>(&'a self, tasks: &'a [Task]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            for task in tasks {
                self.save_task(task).await?;
            }
            Ok(())
        })
    }

    /// Count tasks by status
    fn count_by_status<'a>(&'a self, status: &'a TaskStatus) -> BoxFuture<'a, Result<usize>> {
        Box::pin(async move {
            let all = self.load_all().await?;
            Ok(all.iter().filter(|t| t.status == *status).count())
        })
    }
}

const TASK_NAMESPACE: &[&str] = &["tasks"];

/// SQLite-backed task store using the existing [`Store`] trait
pub struct SqliteTaskStore {
    store: Arc<dyn Store>,
}

impl SqliteTaskStore {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }
}

impl TaskStore for SqliteTaskStore {
    fn save_task<'a>(&'a self, task: &'a Task) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let value = serde_json::to_value(task).map_err(|e| {
                echo_core::error::ReactError::Other(format!("save_task serialize: {}", e))
            })?;
            self.store
                .put(TASK_NAMESPACE, &task.id, value)
                .await
                .map_err(|e| echo_core::error::ReactError::Other(format!("save_task: {}", e)))
        })
    }

    fn load_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Task>>> {
        Box::pin(async move {
            let item =
                self.store.get(TASK_NAMESPACE, id).await.map_err(|e| {
                    echo_core::error::ReactError::Other(format!("load_task: {}", e))
                })?;

            match item {
                Some(item) => {
                    let task = serde_json::from_value(item.value).map_err(|e| {
                        echo_core::error::ReactError::Other(format!("load_task parse: {}", e))
                    })?;
                    Ok(Some(task))
                }
                None => Ok(None),
            }
        })
    }

    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Task>>> {
        Box::pin(async move {
            let items = self
                .store
                .list(TASK_NAMESPACE)
                .await
                .map_err(|e| echo_core::error::ReactError::Other(format!("load_all: {}", e)))?;

            let mut tasks = Vec::with_capacity(items.len());
            for item in items {
                match serde_json::from_value::<Task>(item.value) {
                    Ok(task) => tasks.push(task),
                    Err(e) => {
                        tracing::warn!(error = %e, key = %item.key, "Failed to parse stored task");
                    }
                }
            }

            Ok(tasks)
        })
    }

    fn delete_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            self.store
                .delete(TASK_NAMESPACE, id)
                .await
                .map_err(|e| echo_core::error::ReactError::Other(format!("delete_task: {}", e)))
        })
    }
}

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// Execution checkpoint — captures the full state for recovery
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionCheckpoint {
    /// Unique checkpoint ID
    pub id: String,
    /// Associated plan ID (if any)
    pub plan_id: Option<String>,
    /// All tasks at checkpoint time
    pub tasks: Vec<Task>,
    /// IDs of completed tasks
    pub completed_task_ids: Vec<String>,
    /// Timestamp (seconds since epoch)
    pub created_at: u64,
}

impl ExecutionCheckpoint {
    pub fn new(plan_id: Option<String>, tasks: Vec<Task>) -> Self {
        let completed: Vec<String> = tasks
            .iter()
            .filter(|t| t.status.is_terminal())
            .map(|t| t.id.clone())
            .collect();

        Self {
            id: format!("ckpt_{}", uuid::Uuid::new_v4().as_simple()),
            plan_id,
            tasks,
            completed_task_ids: completed,
            created_at: super::time::now_secs(),
        }
    }

    /// Create checkpoint from a TaskManager snapshot
    pub fn from_manager(plan_id: Option<String>, manager: &super::manager::TaskManager) -> Self {
        Self::new(plan_id, manager.get_all_tasks())
    }
}

/// Trait for checkpoint storage
pub trait CheckpointStore: Send + Sync {
    /// Save a checkpoint
    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a ExecutionCheckpoint,
    ) -> BoxFuture<'a, Result<()>>;

    /// Load the latest checkpoint for a plan
    fn load_latest_checkpoint<'a>(
        &'a self,
        plan_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ExecutionCheckpoint>>>;

    /// List all checkpoints
    fn list_checkpoints<'a>(
        &'a self,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ExecutionCheckpoint>>>;
}

const CHECKPOINT_NAMESPACE: &[&str] = &["checkpoints"];

/// SQLite-backed checkpoint store
pub struct SqliteCheckpointStore {
    store: Arc<dyn Store>,
}

impl SqliteCheckpointStore {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }
}

impl CheckpointStore for SqliteCheckpointStore {
    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a ExecutionCheckpoint,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let value = serde_json::to_value(checkpoint).map_err(|e| {
                echo_core::error::ReactError::Other(format!("save_checkpoint serialize: {}", e))
            })?;
            self.store
                .put(CHECKPOINT_NAMESPACE, &checkpoint.id, value)
                .await
                .map_err(|e| echo_core::error::ReactError::Other(format!("save_checkpoint: {}", e)))
        })
    }

    fn load_latest_checkpoint<'a>(
        &'a self,
        plan_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ExecutionCheckpoint>>> {
        Box::pin(async move {
            let results = self
                .store
                .search(CHECKPOINT_NAMESPACE, plan_id, 100)
                .await
                .map_err(|e| {
                    echo_core::error::ReactError::Other(format!("load_latest_checkpoint: {}", e))
                })?;

            let mut latest: Option<ExecutionCheckpoint> = None;
            for item in results {
                if let Ok(ckpt) = serde_json::from_value::<ExecutionCheckpoint>(item.value)
                    && ckpt.plan_id.as_deref() == Some(plan_id)
                {
                    match &latest {
                        None => latest = Some(ckpt),
                        Some(existing) if ckpt.created_at > existing.created_at => {
                            latest = Some(ckpt);
                        }
                        _ => {}
                    }
                }
            }
            Ok(latest)
        })
    }

    fn list_checkpoints<'a>(
        &'a self,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ExecutionCheckpoint>>> {
        Box::pin(async move {
            let results = self
                .store
                .search(CHECKPOINT_NAMESPACE, "", limit)
                .await
                .map_err(|e| {
                    echo_core::error::ReactError::Other(format!("list_checkpoints: {}", e))
                })?;

            let mut checkpoints = Vec::with_capacity(results.len());
            for item in results {
                if let Ok(ckpt) = serde_json::from_value::<ExecutionCheckpoint>(item.value) {
                    checkpoints.push(ckpt);
                }
            }
            // Sort by created_at descending
            checkpoints.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            Ok(checkpoints)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_new() {
        let tasks = vec![Task::new("t1", "First"), Task::new("t2", "Second")];
        let ckpt = ExecutionCheckpoint::new(Some("plan_123".to_string()), tasks);
        assert!(ckpt.id.starts_with("ckpt_"));
        assert_eq!(ckpt.plan_id, Some("plan_123".to_string()));
        assert_eq!(ckpt.tasks.len(), 2);
        assert!(ckpt.completed_task_ids.is_empty()); // No terminal tasks
    }

    #[test]
    fn test_checkpoint_with_completed() {
        let mut t1 = Task::new("t1", "First");
        t1.status = TaskStatus::Completed;
        let t2 = Task::new("t2", "Second");

        let ckpt = ExecutionCheckpoint::new(None, vec![t1, t2]);
        assert_eq!(ckpt.completed_task_ids, vec!["t1"]);
    }
}
