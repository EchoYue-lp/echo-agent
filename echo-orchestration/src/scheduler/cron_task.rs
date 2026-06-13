//! Cron task definition and persistence.

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
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
            created_at: Utc::now().to_rfc3339(),
        }
    }

    /// Calculate the next fire time after now.
    ///
    /// Returns `None` if the cron expression is invalid or has no future occurrences.
    pub fn next_run(&self) -> Option<DateTime<Utc>> {
        // The cron crate expects 7-field expressions; pad with seconds and year
        let expr = if self.cron_expr.split_whitespace().count() == 5 {
            format!("0 {} *", self.cron_expr)
        } else {
            self.cron_expr.clone()
        };
        let schedule = Schedule::from_str(&expr).ok()?;
        schedule.upcoming(Utc).next()
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
}

const STORE_NAMESPACE: &[&str] = &["scheduler", "cron_tasks"];
const STORE_KEY: &str = "all_cron_tasks";

impl CronTaskStore {
    /// Create a file-based store (default: `~/.echo-agent/scheduler/tasks.json`).
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            backend: None,
            path: PathBuf::from(home).join(".echo-agent/scheduler/tasks.json"),
        }
    }

    /// Create a Store-backed store with automatic migration from file.
    pub fn with_store(store: Arc<dyn echo_core::memory::Store>) -> Self {
        let mut s = Self {
            backend: Some(store),
            path: PathBuf::new(),
        };
        // Auto-migrate from file if it exists
        let _ = s.migrate_from_file();
        s
    }

    /// Set a custom file path (for testing).
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    /// Load all cron tasks.
    pub fn load_all(&self) -> echo_core::error::Result<Vec<CronTask>> {
        if let Some(ref backend) = self.backend {
            let rt = tokio::runtime::Handle::try_current()
                .map_err(|_| echo_core::error::ReactError::Other("No tokio runtime".into()))?;
            let item = tokio::task::block_in_place(|| {
                rt.block_on(backend.get(STORE_NAMESPACE, STORE_KEY))
            })?;
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
    pub fn save_all(&self, tasks: &[CronTask]) -> echo_core::error::Result<()> {
        let json = serde_json::to_string_pretty(tasks).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to serialize cron tasks: {e}"))
        })?;

        if let Some(ref backend) = self.backend {
            let rt = tokio::runtime::Handle::try_current()
                .map_err(|_| echo_core::error::ReactError::Other("No tokio runtime".into()))?;
            tokio::task::block_in_place(|| {
                rt.block_on(backend.put(
                    STORE_NAMESPACE,
                    STORE_KEY,
                    serde_json::Value::String(json),
                ))
            })?;
        } else {
            self.save_to_file(&json)?;
        }
        Ok(())
    }

    /// Add a task and persist.
    pub fn add(&self, task: CronTask) -> echo_core::error::Result<()> {
        let mut tasks = self.load_all()?;
        tasks.push(task);
        self.save_all(&tasks)
    }

    /// Remove a task by ID and persist. Returns true if found.
    pub fn remove(&self, id: &str) -> echo_core::error::Result<bool> {
        let mut tasks = self.load_all()?;
        let before = tasks.len();
        tasks.retain(|t| !t.id.starts_with(id));
        let removed = tasks.len() < before;
        if removed {
            self.save_all(&tasks)?;
        }
        Ok(removed)
    }

    /// Update the status of a task by ID and persist.
    pub fn set_status(&self, id: &str, status: CronTaskStatus) -> echo_core::error::Result<bool> {
        let mut tasks = self.load_all()?;
        let mut found = false;
        for task in &mut tasks {
            if task.id.starts_with(id) {
                task.status = status;
                found = true;
                break;
            }
        }
        if found {
            self.save_all(&tasks)?;
        }
        Ok(found)
    }

    /// Update last_run info after a task fires.
    pub fn update_last_run(&self, id: &str, result: &str) -> echo_core::error::Result<()> {
        let mut tasks = self.load_all()?;
        for task in &mut tasks {
            if task.id.starts_with(id) {
                task.last_run_at = Some(Utc::now().to_rfc3339());
                task.last_result = Some(result.chars().take(500).collect());
                break;
            }
        }
        self.save_all(&tasks)
    }

    /// Get a task by ID prefix.
    pub fn get(&self, id: &str) -> echo_core::error::Result<Option<CronTask>> {
        let tasks = self.load_all()?;
        Ok(tasks.into_iter().find(|t| t.id.starts_with(id)))
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
        std::fs::write(&self.path, json).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to write cron tasks file: {e}"))
        })?;
        Ok(())
    }

    fn migrate_from_file(&mut self) -> echo_core::error::Result<()> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let legacy_path = PathBuf::from(home).join(".echo-agent/scheduler/tasks.json");
        if !legacy_path.exists() {
            return Ok(());
        }
        debug!("Migrating cron tasks from file to Store backend");
        let content = match std::fs::read_to_string(&legacy_path) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };
        if content.trim().is_empty() {
            return Ok(());
        }
        let tasks: Vec<CronTask> = match serde_json::from_str(&content) {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };
        self.save_all(&tasks)?;
        // Remove legacy file after successful migration
        let _ = std::fs::remove_file(&legacy_path);
        debug!(
            "Migrated {} cron tasks and removed legacy file",
            tasks.len()
        );
        Ok(())
    }
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
}
