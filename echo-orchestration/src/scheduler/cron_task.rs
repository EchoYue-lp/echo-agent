//! Cron task definition and persistence.

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tracing::debug;

// ── CronTask ───────────────────────────────────────────────────────

/// A scheduled task that fires according to a cron expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTask {
    /// Unique identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// 5-field cron expression: `min hour dom month dow`.
    pub cron_expr: String,
    /// The prompt or command to execute when fired.
    pub prompt: String,
    /// Whether the task is active.
    pub status: CronTaskStatus,
    /// ISO 8601 timestamp of the last execution.
    pub last_run_at: Option<String>,
    /// Truncated result from the last execution (first 500 chars).
    pub last_result: Option<String>,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
}

/// Whether a cron task is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CronTaskStatus {
    /// Task fires on schedule.
    Enabled,
    /// Task is paused.
    Disabled,
}

impl CronTask {
    /// Create a new enabled cron task.
    pub fn new(name: &str, cron_expr: &str, prompt: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            cron_expr: cron_expr.to_string(),
            prompt: prompt.to_string(),
            status: CronTaskStatus::Enabled,
            last_run_at: None,
            last_result: None,
            created_at: echo_core::utils::time::now_local().to_rfc3339(),
        }
    }

    /// Calculate the next fire time after now.
    ///
    /// Returns `None` if the cron expression is invalid or has no future occurrences.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        self.next_run_after(&Utc::now())
    }

    /// Calculate the first fire time strictly after a caller-supplied boundary.
    pub fn next_run_after(&self, after: &DateTime<Utc>) -> Option<DateTime<Utc>> {
        // The cron crate expects 7-field expressions; pad with seconds and year
        let expr = if self.cron_expr.split_whitespace().count() == 5 {
            format!("0 {} *", self.cron_expr)
        } else {
            self.cron_expr.clone()
        };
        let schedule = Schedule::from_str(&expr).ok()?;
        schedule.after(after).next()
    }

    /// Validate the cron expression.
    pub fn validate_cron(&self) -> bool {
        let expr = if self.cron_expr.split_whitespace().count() == 5 {
            format!("0 {} *", self.cron_expr)
        } else {
            self.cron_expr.clone()
        };
        Schedule::from_str(&expr).is_ok()
    }
}

// ── CronTaskStore ──────────────────────────────────────────────────

/// Persistent store for cron task definitions.
///
/// Supports two backends:
/// 1. **Store trait** (SQLite/InMemory) — recommended
/// 2. **File-based** — legacy JSON file fallback
#[derive(Clone)]
pub struct CronTaskStore {
    backend: Option<Arc<dyn echo_core::memory::Store>>,
    path: PathBuf,
    mutation_lock: Arc<Mutex<()>>,
}

const STORE_NAMESPACE: &[&str] = &["scheduler", "cron_tasks"];
const STORE_KEY: &str = "all_cron_tasks";

fn cron_store_mutation_lock() -> Arc<Mutex<()>> {
    static LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    Arc::clone(LOCK.get_or_init(|| Arc::new(Mutex::new(()))))
}

impl CronTaskStore {
    /// Create a file-based store (default: `~/.echo-agent/scheduler/tasks.json`).
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            backend: None,
            path: PathBuf::from(home).join(".echo-agent/scheduler/tasks.json"),
            mutation_lock: cron_store_mutation_lock(),
        }
    }

    /// Create a Store-backed store with automatic migration from file.
    pub async fn with_store(
        store: Arc<dyn echo_core::memory::Store>,
    ) -> echo_core::error::Result<Self> {
        let s = Self {
            backend: Some(store),
            path: PathBuf::new(),
            mutation_lock: cron_store_mutation_lock(),
        };
        s.migrate_from_file().await?;
        Ok(s)
    }

    /// Set a custom file path (for testing).
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    /// Load all cron tasks.
    pub async fn load_all(&self) -> echo_core::error::Result<Vec<CronTask>> {
        if let Some(ref backend) = self.backend {
            let item = backend.get(STORE_NAMESPACE, STORE_KEY).await?;
            match item {
                Some(store_item) => {
                    // Value is stored as serde_json::Value — extract the string
                    let json_str = match &store_item.value {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    serde_json::from_str(&json_str).map_err(|e| {
                        echo_core::error::ReactError::Other(format!(
                            "Failed to deserialize cron tasks: {e}"
                        ))
                    })
                }
                None => Ok(vec![]),
            }
        } else {
            self.load_from_file()
        }
    }

    /// Save all cron tasks.
    async fn save_all_unlocked(&self, tasks: &[CronTask]) -> echo_core::error::Result<()> {
        let json = serde_json::to_string_pretty(tasks).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to serialize cron tasks: {e}"))
        })?;

        if let Some(ref backend) = self.backend {
            backend
                .put(STORE_NAMESPACE, STORE_KEY, serde_json::Value::String(json))
                .await?;
        } else {
            self.save_to_file(&json)?;
        }
        Ok(())
    }

    /// Add a task and persist.
    pub async fn add(&self, task: CronTask) -> echo_core::error::Result<()> {
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        tasks.push(task);
        self.save_all_unlocked(&tasks).await
    }

    /// Remove a task by ID and persist. Returns true if found.
    pub async fn remove(&self, id: &str) -> echo_core::error::Result<bool> {
        let id = unique_id(id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        let before = tasks.len();
        tasks.retain(|task| task.id != id);
        let removed = tasks.len() < before;
        if removed {
            self.save_all_unlocked(&tasks).await?;
        }
        Ok(removed)
    }

    /// Remove exactly one task by its complete ID and persist.
    pub async fn remove_exact(&self, id: &str) -> echo_core::error::Result<bool> {
        self.remove(id).await
    }

    /// Update the status of a task by ID and persist.
    pub async fn set_status(
        &self,
        id: &str,
        status: CronTaskStatus,
    ) -> echo_core::error::Result<bool> {
        let id = unique_id(id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        let mut found = false;
        for task in &mut tasks {
            if task.id == id {
                task.status = status;
                found = true;
                break;
            }
        }
        if found {
            self.save_all_unlocked(&tasks).await?;
        }
        Ok(found)
    }

    /// Update last_run info after a task fires.
    pub async fn update_last_run(&self, id: &str, result: &str) -> echo_core::error::Result<()> {
        let id = unique_id(id)?;
        let _guard = self.mutation_lock.lock().await;
        let mut tasks = self.load_all().await?;
        let mut found = false;
        for task in &mut tasks {
            if task.id == id {
                task.last_run_at = Some(echo_core::utils::time::now_local().to_rfc3339());
                task.last_result = Some(result.chars().take(500).collect());
                found = true;
                break;
            }
        }
        if !found {
            return Err(echo_core::error::ReactError::Other(format!(
                "Cron task '{id}' not found"
            )));
        }
        self.save_all_unlocked(&tasks).await
    }

    /// Get a task by ID prefix.
    pub async fn get(&self, id: &str) -> echo_core::error::Result<Option<CronTask>> {
        let id = unique_id(id)?;
        let tasks = self.load_all().await?;
        Ok(tasks.into_iter().find(|task| task.id == id))
    }

    // ── Private helpers ────────────────────────────────────────────

    fn load_from_file(&self) -> echo_core::error::Result<Vec<CronTask>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&self.path).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to read cron tasks file: {e}"))
        })?;
        if content.trim().is_empty() {
            return Ok(vec![]);
        }
        serde_json::from_str(&content).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to parse cron tasks: {e}"))
        })
    }

    fn save_to_file(&self, json: &str) -> echo_core::error::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                echo_core::error::ReactError::Other(format!(
                    "Failed to create scheduler directory: {e}"
                ))
            })?;
        }
        echo_core::utils::fs::atomic_write(&self.path, json.as_bytes()).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to write cron tasks file: {e}"))
        })?;
        Ok(())
    }

    async fn migrate_from_file(&self) -> echo_core::error::Result<()> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let legacy_path = PathBuf::from(home).join(".echo-agent/scheduler/tasks.json");
        if !legacy_path.exists() {
            return Ok(());
        }
        debug!("Migrating cron tasks from file to Store backend");
        let content = std::fs::read_to_string(&legacy_path).map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Failed to read legacy cron tasks: {error}"
            ))
        })?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let tasks: Vec<CronTask> = serde_json::from_str(&content).map_err(|error| {
            echo_core::error::ReactError::Other(format!(
                "Failed to parse legacy cron tasks: {error}"
            ))
        })?;
        self.save_all_unlocked(&tasks).await?;
        // Remove legacy file after successful migration
        let _ = std::fs::remove_file(&legacy_path);
        debug!(
            "Migrated {} cron tasks and removed legacy file",
            tasks.len()
        );
        Ok(())
    }
}

fn unique_id(id: &str) -> echo_core::error::Result<&str> {
    if id.trim().is_empty() {
        return Err(echo_core::error::ReactError::Other(
            "Cron task ID cannot be empty".into(),
        ));
    }
    Ok(id)
}

impl Default for CronTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_task_creation() {
        let task = CronTask::new("test", "*/5 * * * *", "Hello");
        assert_eq!(task.name, "test");
        assert_eq!(task.status, CronTaskStatus::Enabled);
        assert!(task.validate_cron());
    }

    #[test]
    fn test_cron_task_next_run() {
        let task = CronTask::new("test", "0 12 * * *", "Noon task");
        let next = task.next_run();
        assert!(next.is_some());
    }

    #[test]
    fn test_cron_task_invalid_expr() {
        let task = CronTask::new("bad", "not a cron", "test");
        assert!(!task.validate_cron());
        assert!(task.next_run().is_none());
    }

    #[tokio::test]
    async fn remove_exact_does_not_remove_tasks_with_the_same_prefix() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-remove-exact-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let store = CronTaskStore::new().with_path(temp.join("cron-tasks.json"));
        let mut exact = CronTask::new("exact", "*/5 * * * *", "exact");
        exact.id = "plugin-monitor".to_string();
        let mut prefixed = CronTask::new("prefixed", "*/5 * * * *", "prefixed");
        prefixed.id = "plugin-monitor-longer".to_string();
        store.add(exact).await.map_err(|error| error.to_string())?;
        store
            .add(prefixed)
            .await
            .map_err(|error| error.to_string())?;

        assert!(
            store
                .remove_exact("plugin-monitor")
                .await
                .map_err(|error| error.to_string())?
        );
        let remaining = store.load_all().await.map_err(|error| error.to_string())?;
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            remaining.first().map(|task| task.id.as_str()),
            Some("plugin-monitor-longer")
        );
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn independent_file_store_instances_do_not_lose_concurrent_adds() -> Result<(), String> {
        let temp = std::env::temp_dir().join(format!(
            "echo-scheduler-concurrent-add-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
        let path = temp.join("cron-tasks.json");
        let first = CronTaskStore::new().with_path(path.clone());
        let second = CronTaskStore::new().with_path(path);

        let (first_result, second_result) = tokio::join!(
            first.add(CronTask::new("first", "*/5 * * * *", "first")),
            second.add(CronTask::new("second", "*/5 * * * *", "second")),
        );
        first_result.map_err(|error| error.to_string())?;
        second_result.map_err(|error| error.to_string())?;

        let tasks = first.load_all().await.map_err(|error| error.to_string())?;
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|task| task.name == "first"));
        assert!(tasks.iter().any(|task| task.name == "second"));
        std::fs::remove_dir_all(&temp).map_err(|error| error.to_string())?;
        Ok(())
    }
}
