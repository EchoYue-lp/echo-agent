//! ManagedTask persistence layer — stores task state across runs.
//!
//! Uses the existing [`Store`] trait internally, so any Store implementation
//! (in-memory, file, SQLite, etc.) works out of the box.

use super::runtime::TaskStatus;
use super::task::ManagedTask;
use echo_core::error::Result;
use echo_core::memory::store::Store;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Trait for task persistence operations
pub trait TaskStore: Send + Sync {
    /// Persist a single task
    fn save_task<'a>(&'a self, task: &'a ManagedTask) -> BoxFuture<'a, Result<()>>;

    /// Load a task by ID
    fn load_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<ManagedTask>>>;

    /// Load all tasks (with automatic pagination — no hard limit)
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ManagedTask>>>;

    /// Delete a task by ID
    fn delete_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<bool>>;

    /// Save all tasks (batch upsert)
    fn save_all<'a>(&'a self, tasks: &'a [ManagedTask]) -> BoxFuture<'a, Result<()>> {
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

/// Store-trait-backed task KV store (namespace `["tasks"]`).
///
/// Not bound to SQLite despite the historical name — any [`Store`] impl
/// (in-memory, file, sqlite) is accepted. Renaming would ripple through
/// re-exports, docs and bindings for no behavioral gain, so the name stays;
/// this doc is the correction.
pub struct SqliteTaskStore {
    store: Arc<dyn Store>,
}

impl SqliteTaskStore {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }
}

impl TaskStore for SqliteTaskStore {
    fn save_task<'a>(&'a self, task: &'a ManagedTask) -> BoxFuture<'a, Result<()>> {
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

    fn load_task<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<ManagedTask>>> {
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

    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<ManagedTask>>> {
        Box::pin(async move {
            let items = self
                .store
                .list(TASK_NAMESPACE)
                .await
                .map_err(|e| echo_core::error::ReactError::Other(format!("load_all: {}", e)))?;

            let mut tasks = Vec::with_capacity(items.len());
            for item in items {
                match serde_json::from_value::<ManagedTask>(item.value) {
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
