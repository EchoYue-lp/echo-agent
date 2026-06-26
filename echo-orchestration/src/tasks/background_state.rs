//! Background task state management
//!
//! This module provides state management for background tasks,
//! allowing them to be tracked, checkpointed, and resumed.

use serde::{Deserialize, Serialize};

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

/// Checkpoint store trait for persisting task states.
///
/// The previous `SqliteCheckpointStore` impl was removed — EKO went
/// SQLite-free and this was the only always-compiled sqlx usage in
/// echo-orchestration. Non-SQLite implementations (memory/file) should
/// implement this trait instead.
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
}
