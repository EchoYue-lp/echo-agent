//! Background task state management
//!
//! This module provides state management for background tasks,
//! allowing them to be tracked, checkpointed, and resumed.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::tasks::{TaskState, TaskStatus};

/// Background task state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskState {
    /// Task ID
    pub task_id: String,
    /// Parent task ID (if this is a sub-task)
    pub parent_task_id: Option<String>,
    /// Task status
    pub status: TaskStatus,
    /// Shell command (if command mode)
    pub command: Option<String>,
    /// Exit code (if command completed)
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Task start timestamp
    pub started_at: u64,
    /// Task completion timestamp
    pub completed_at: Option<u64>,
    /// Task duration in milliseconds
    pub duration_ms: u64,
    /// Checkpoint timestamp
    pub checkpoint_at: u64,
}

impl BackgroundTaskState {
    /// Create a new background task state
    pub fn new(task_id: impl Into<String>) -> Self {
        let now = super::time::now_secs();
        Self {
            task_id: task_id.into(),
            parent_task_id: None,
            status: TaskStatus::Pending,
            command: None,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            started_at: now,
            completed_at: None,
            duration_ms: 0,
            checkpoint_at: now,
        }
    }

    /// Set parent task ID
    pub fn with_parent_task_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_task_id = Some(parent_id.into());
        self
    }

    /// Set command
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Mark task as started
    pub fn mark_started(&mut self) {
        self.status = TaskStatus::InProgress;
        self.started_at = super::time::now_secs();
    }

    /// Mark task as completed
    pub fn mark_completed(&mut self, exit_code: i32, stdout: String, stderr: String) {
        self.status = if exit_code == 0 {
            TaskStatus::Completed
        } else {
            TaskStatus::Failed(format!("Exit code: {}", exit_code))
        };
        self.exit_code = Some(exit_code);
        self.stdout = stdout;
        self.stderr = stderr;
        self.completed_at = Some(super::time::now_secs());
        self.duration_ms = (self.completed_at.unwrap() - self.started_at) * 1000;
        self.checkpoint_at = super::time::now_secs();
    }

    /// Mark task as failed
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.status = TaskStatus::Failed(error.into());
        self.completed_at = Some(super::time::now_secs());
        self.duration_ms = (self.completed_at.unwrap() - self.started_at) * 1000;
        self.checkpoint_at = super::time::now_secs();
    }

    /// Mark task as cancelled
    pub fn mark_cancelled(&mut self) {
        self.status = TaskStatus::Cancelled;
        self.completed_at = Some(super::time::now_secs());
        self.duration_ms = (self.completed_at.unwrap() - self.started_at) * 1000;
        self.checkpoint_at = super::time::now_secs();
    }

    /// Check if task is terminal
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// Convert to TaskState
    pub fn to_task_state(&self) -> TaskState {
        TaskState {
            task_id: self.task_id.clone(),
            status: self.status.clone(),
            evidence: Vec::new(),
            changed_files: Vec::new(),
            artifacts: Vec::new(),
            commands_run: Vec::new(),
            verification_result: None,
            remaining_risks: Vec::new(),
            next_unblocked_tasks: Vec::new(),
            context_summary: None,
            retry_count: 0,
            parent_task_id: self.parent_task_id.clone(),
            checkpoint_at: self.checkpoint_at,
        }
    }
}

/// Checkpoint store trait for persisting task states
pub trait CheckpointStore: Send + Sync {
    /// Save a task state
    fn save_task_state(
        &self,
        state: &BackgroundTaskState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>;

    /// Load a task state by task ID
    fn load_task_state(
        &self,
        task_id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<BackgroundTaskState>, String>>
                + Send
                + '_,
        >,
    >;

    /// Load all task states
    fn load_all_states(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<BackgroundTaskState>, String>> + Send + '_>,
    >;

    /// Delete a task state
    fn delete_task_state(
        &self,
        task_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>;
}

/// SQLite-based checkpoint store
pub struct SqliteCheckpointStore {
    pool: Arc<sqlx::SqlitePool>,
}

impl SqliteCheckpointStore {
    /// Create a new SQLite checkpoint store
    pub fn new(pool: Arc<sqlx::SqlitePool>) -> Self {
        Self { pool }
    }

    /// Initialize the database schema
    pub async fn initialize(&self) -> Result<(), String> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS background_task_states (
                task_id TEXT PRIMARY KEY,
                parent_task_id TEXT,
                status TEXT NOT NULL,
                command TEXT,
                exit_code INTEGER,
                stdout TEXT NOT NULL,
                stderr TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                completed_at INTEGER,
                duration_ms INTEGER NOT NULL,
                checkpoint_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&*self.pool)
        .await
        .map_err(|e| format!("Failed to create table: {}", e))?;

        Ok(())
    }
}

impl CheckpointStore for SqliteCheckpointStore {
    fn save_task_state(
        &self,
        state: &BackgroundTaskState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        let pool = self.pool.clone();
        let state = state.clone();

        Box::pin(async move {
            let status_str = serde_json::to_string(&state.status)
                .map_err(|e| format!("Failed to serialize status: {}", e))?;

            sqlx::query(
                r#"
                INSERT OR REPLACE INTO background_task_states
                (task_id, parent_task_id, status, command, exit_code, stdout, stderr,
                 started_at, completed_at, duration_ms, checkpoint_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&state.task_id)
            .bind(&state.parent_task_id)
            .bind(&status_str)
            .bind(&state.command)
            .bind(state.exit_code)
            .bind(&state.stdout)
            .bind(&state.stderr)
            .bind(state.started_at as i64)
            .bind(state.completed_at.map(|t| t as i64))
            .bind(state.duration_ms as i64)
            .bind(state.checkpoint_at as i64)
            .execute(&*pool)
            .await
            .map_err(|e| format!("Failed to save task state: {}", e))?;

            Ok(())
        })
    }

    fn load_task_state(
        &self,
        task_id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<BackgroundTaskState>, String>>
                + Send
                + '_,
        >,
    > {
        let pool = self.pool.clone();
        let task_id = task_id.to_string();

        Box::pin(async move {
            let row: Option<(
                String,
                Option<String>,
                String,
                Option<String>,
                Option<i32>,
                String,
                String,
                i64,
                Option<i64>,
                i64,
                i64,
            )> = sqlx::query_as(
                r#"
                SELECT task_id, parent_task_id, status, command, exit_code, stdout, stderr,
                       started_at, completed_at, duration_ms, checkpoint_at
                FROM background_task_states
                WHERE task_id = ?
                "#,
            )
            .bind(&task_id)
            .fetch_optional(&*pool)
            .await
            .map_err(|e| format!("Failed to load task state: {}", e))?;

            match row {
                Some((
                    task_id,
                    parent_task_id,
                    status_str,
                    command,
                    exit_code,
                    stdout,
                    stderr,
                    started_at,
                    completed_at,
                    duration_ms,
                    checkpoint_at,
                )) => {
                    let status: TaskStatus = serde_json::from_str(&status_str)
                        .map_err(|e| format!("Failed to deserialize status: {}", e))?;

                    Ok(Some(BackgroundTaskState {
                        task_id,
                        parent_task_id,
                        status,
                        command,
                        exit_code,
                        stdout,
                        stderr,
                        started_at: started_at as u64,
                        completed_at: completed_at.map(|t| t as u64),
                        duration_ms: duration_ms as u64,
                        checkpoint_at: checkpoint_at as u64,
                    }))
                }
                None => Ok(None),
            }
        })
    }

    fn load_all_states(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<BackgroundTaskState>, String>> + Send + '_>,
    > {
        let pool = self.pool.clone();

        Box::pin(async move {
            let rows: Vec<(
                String,
                Option<String>,
                String,
                Option<String>,
                Option<i32>,
                String,
                String,
                i64,
                Option<i64>,
                i64,
                i64,
            )> = sqlx::query_as(
                r#"
                SELECT task_id, parent_task_id, status, command, exit_code, stdout, stderr,
                       started_at, completed_at, duration_ms, checkpoint_at
                FROM background_task_states
                ORDER BY started_at DESC
                "#,
            )
            .fetch_all(&*pool)
            .await
            .map_err(|e| format!("Failed to load all task states: {}", e))?;

            let mut states = Vec::new();
            for (
                task_id,
                parent_task_id,
                status_str,
                command,
                exit_code,
                stdout,
                stderr,
                started_at,
                completed_at,
                duration_ms,
                checkpoint_at,
            ) in rows
            {
                let status: TaskStatus = serde_json::from_str(&status_str)
                    .map_err(|e| format!("Failed to deserialize status: {}", e))?;

                states.push(BackgroundTaskState {
                    task_id,
                    parent_task_id,
                    status,
                    command,
                    exit_code,
                    stdout,
                    stderr,
                    started_at: started_at as u64,
                    completed_at: completed_at.map(|t| t as u64),
                    duration_ms: duration_ms as u64,
                    checkpoint_at: checkpoint_at as u64,
                });
            }

            Ok(states)
        })
    }

    fn delete_task_state(
        &self,
        task_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
    {
        let pool = self.pool.clone();
        let task_id = task_id.to_string();

        Box::pin(async move {
            let result = sqlx::query("DELETE FROM background_task_states WHERE task_id = ?")
                .bind(&task_id)
                .execute(&*pool)
                .await
                .map_err(|e| format!("Failed to delete task state: {}", e))?;

            Ok(result.rows_affected() > 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_background_task_state_new() {
        let state = BackgroundTaskState::new("task1");
        assert_eq!(state.task_id, "task1");
        assert_eq!(state.status, TaskStatus::Pending);
        assert!(state.parent_task_id.is_none());
        assert!(state.command.is_none());
        assert!(state.exit_code.is_none());
        assert!(state.stdout.is_empty());
        assert!(state.stderr.is_empty());
    }

    #[test]
    fn test_background_task_state_mark_started() {
        let mut state = BackgroundTaskState::new("task1");
        state.mark_started();
        assert_eq!(state.status, TaskStatus::InProgress);
        assert!(state.started_at > 0);
    }

    #[test]
    fn test_background_task_state_mark_completed() {
        let mut state = BackgroundTaskState::new("task1");
        state.mark_started();
        state.mark_completed(0, "success".to_string(), String::new());
        assert_eq!(state.status, TaskStatus::Completed);
        assert_eq!(state.exit_code, Some(0));
        assert_eq!(state.stdout, "success");
        assert!(state.completed_at.is_some());
    }

    #[test]
    fn test_background_task_state_mark_failed() {
        let mut state = BackgroundTaskState::new("task1");
        state.mark_started();
        state.mark_failed("error occurred");
        assert!(matches!(state.status, TaskStatus::Failed(_)));
        assert!(state.completed_at.is_some());
    }

    #[test]
    fn test_background_task_state_to_task_state() {
        let mut state = BackgroundTaskState::new("task1");
        state.mark_started();
        state.mark_completed(0, "success".to_string(), String::new());

        let task_state = state.to_task_state();
        assert_eq!(task_state.task_id, "task1");
        assert_eq!(task_state.status, TaskStatus::Completed);
        assert!(task_state.parent_task_id.is_none());
    }

    #[tokio::test]
    async fn test_sqlite_checkpoint_store() {
        // Create in-memory SQLite database
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();

        let store = SqliteCheckpointStore::new(std::sync::Arc::new(pool));
        store.initialize().await.unwrap();

        // Test save and load
        let mut state = BackgroundTaskState::new("task1");
        state.mark_started();
        state.mark_completed(0, "success".to_string(), String::new());

        store.save_task_state(&state).await.unwrap();

        let loaded = store.load_task_state("task1").await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.task_id, "task1");
        assert_eq!(loaded.status, TaskStatus::Completed);

        // Test load all
        let all_states = store.load_all_states().await.unwrap();
        assert_eq!(all_states.len(), 1);

        // Test delete
        let deleted = store.delete_task_state("task1").await.unwrap();
        assert!(deleted);

        let loaded = store.load_task_state("task1").await.unwrap();
        assert!(loaded.is_none());
    }
}
