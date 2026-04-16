//! Team coordinator — task distribution, failure handling, and result collection

use std::collections::HashMap;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::error::Result;

use super::super::types::SubagentResult;
use super::mailbox::{Mailbox, MailboxMessage, MessageKind};

// ── Task State ────────────────────────────────────────────────────────────────

/// State of a task within the team workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Waiting to be assigned.
    Pending,
    /// Currently being executed by a teammate.
    Assigned(String),
    /// Successfully completed.
    Completed,
    /// Failed (with error message).
    Failed(String),
}

// ── Task Assignment ───────────────────────────────────────────────────────────

/// Tracks the state of a single task within a team.
#[derive(Debug, Clone)]
pub struct TaskAssignment {
    pub task: String,
    pub assigned_to: Option<String>,
    pub state: TaskState,
    /// How many times this task has been (re)assigned.
    pub attempts: u32,
}

// ── Team Coordinator ──────────────────────────────────────────────────────────

/// Coordinates task distribution and result collection for a team.
///
/// The coordinator is used by the leader agent to:
/// - Assign tasks to specific teammates
/// - Track task progress
/// - Handle failures with reassignment
/// - Collect results from all teammates
///
/// It does **not** run its own event loop — it's a data structure
/// that the leader manipulates through method calls.
pub struct TeamCoordinator {
    /// Name of the leader agent.
    leader: String,
    /// Maximum reassignment attempts before giving up.
    max_retries: u32,
    /// All tasks with their current state.
    tasks: Mutex<HashMap<String, TaskAssignment>>,
    /// Stored results for completed tasks — allows leader to query outputs.
    results: Mutex<HashMap<String, SubagentResult>>,
}

impl TeamCoordinator {
    /// Create a new coordinator with the given leader and default max retries (3).
    pub fn new(leader: impl Into<String>) -> Self {
        Self {
            leader: leader.into(),
            max_retries: 3,
            tasks: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
        }
    }

    /// Create with a custom max retry count.
    pub fn with_max_retries(leader: impl Into<String>, max_retries: u32) -> Self {
        Self {
            leader: leader.into(),
            max_retries,
            tasks: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
        }
    }

    /// Add tasks to the pending queue.
    pub async fn add_tasks(&self, tasks: Vec<String>) {
        let mut map = self.tasks.lock().await;
        for task in tasks {
            debug!(task = %task, "Task added to pending queue");
            map.entry(task.clone()).or_insert(TaskAssignment {
                task,
                assigned_to: None,
                state: TaskState::Pending,
                attempts: 0,
            });
        }
    }

    /// Assign a task to a specific teammate, sending via mailbox.
    pub async fn assign(&self, task: &str, agent_name: &str, mailbox: &Mailbox) -> Result<()> {
        info!(task = %task, agent = %agent_name, "Assigning task to teammate");

        {
            let mut map = self.tasks.lock().await;
            if let Some(assignment) = map.get_mut(task) {
                assignment.assigned_to = Some(agent_name.to_string());
                assignment.state = TaskState::Assigned(agent_name.to_string());
                assignment.attempts += 1;
            }
        }

        // Send via mailbox
        let msg = MailboxMessage::new(
            &self.leader,
            agent_name,
            MessageKind::TaskAssigned {
                task: task.to_string(),
                context: HashMap::new(),
            },
        );
        mailbox.send(msg).await?;

        Ok(())
    }

    /// Pick the next pending task and assign it to the given agent.
    pub async fn assign_next(&self, agent_name: &str, mailbox: &Mailbox) -> Result<Option<String>> {
        let task = {
            let map = self.tasks.lock().await;
            map.iter()
                .find(|(_, a)| a.state == TaskState::Pending)
                .map(|(t, _)| t.clone())
        };

        if let Some(ref t) = task {
            self.assign(t, agent_name, mailbox).await?;
        }

        Ok(task)
    }

    /// Record a completed task result, storing the output for later retrieval.
    pub async fn record_result(&self, task: &str, result: SubagentResult) {
        debug!(task = %task, agent = %result.agent_name, "Recording task result");

        let mut map = self.tasks.lock().await;
        if let Some(assignment) = map.get_mut(task) {
            assignment.state = TaskState::Completed;
        }
        drop(map);

        // Store the actual result for later retrieval
        let mut results = self.results.lock().await;
        results.insert(task.to_string(), result);
    }

    /// Record a task failure. Returns `true` if the task can be reassigned.
    ///
    /// If the task hasn't exceeded `max_retries`, its state is reset to Pending
    /// so it can be picked up by `assign_next()` again.
    pub async fn record_failure(&self, task: &str, error: &str) -> bool {
        warn!(task = %task, error = %error, "Task failed");

        let mut map = self.tasks.lock().await;
        if let Some(assignment) = map.get_mut(task) {
            if assignment.attempts < self.max_retries {
                assignment.state = TaskState::Pending;
                assignment.assigned_to = None;
                info!(task = %task, attempt = assignment.attempts, "Task returned to pending for reassignment");
                true
            } else {
                assignment.state = TaskState::Failed(error.to_string());
                warn!(task = %task, attempts = assignment.attempts, "Task permanently failed after max retries");
                false
            }
        } else {
            false
        }
    }

    /// Collect all completed results with their task names.
    pub async fn collect_results(&self) -> Vec<(String, SubagentResult)> {
        let results = self.results.lock().await;
        results
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get the result for a specific task, if available.
    pub async fn get_result(&self, task: &str) -> Option<SubagentResult> {
        let results = self.results.lock().await;
        results.get(task).cloned()
    }

    /// Check if all tasks are in a terminal state (Completed or Failed).
    pub async fn is_complete(&self) -> bool {
        let map = self.tasks.lock().await;
        map.values()
            .all(|a| matches!(a.state, TaskState::Completed | TaskState::Failed(_)))
    }

    /// Check if any task has permanently failed.
    pub async fn has_failures(&self) -> bool {
        let map = self.tasks.lock().await;
        map.values()
            .any(|a| matches!(a.state, TaskState::Failed(_)))
    }

    /// Get progress as (completed_or_failed, total).
    pub async fn progress(&self) -> (usize, usize) {
        let map = self.tasks.lock().await;
        let total = map.len();
        let done = map
            .values()
            .filter(|a| matches!(a.state, TaskState::Completed | TaskState::Failed(_)))
            .count();
        (done, total)
    }

    /// Get the state of a specific task.
    pub async fn task_state(&self, task: &str) -> Option<TaskState> {
        let map = self.tasks.lock().await;
        map.get(task).map(|a| a.state.clone())
    }

    /// Get all task assignments.
    pub async fn all_tasks(&self) -> Vec<TaskAssignment> {
        let map = self.tasks.lock().await;
        map.values().cloned().collect()
    }

    /// Get the leader name.
    pub fn leader(&self) -> &str {
        &self.leader
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_and_assign() {
        let coord = TeamCoordinator::new("leader");
        let mailbox = Mailbox::new();

        coord.add_tasks(vec!["task1".into(), "task2".into()]).await;

        coord.assign("task1", "worker", &mailbox).await.unwrap();

        let msg = mailbox.recv().await.unwrap();
        assert_eq!(msg.to, "worker");
        match msg.kind {
            MessageKind::TaskAssigned { task, .. } => assert_eq!(task, "task1"),
            _ => panic!("Wrong message kind"),
        }

        // Verify state
        let state = coord.task_state("task1").await.unwrap();
        assert!(matches!(state, TaskState::Assigned(ref a) if a == "worker"));
    }

    #[tokio::test]
    async fn test_assign_next() {
        let coord = TeamCoordinator::new("leader");
        let mailbox = Mailbox::new();

        coord.add_tasks(vec!["t1".into()]).await;

        let task = coord.assign_next("w", &mailbox).await.unwrap();
        assert_eq!(task, Some("t1".into()));

        let next = coord.assign_next("w", &mailbox).await.unwrap();
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn test_record_and_complete() {
        let coord = TeamCoordinator::new("leader");
        let mailbox = Mailbox::new();

        coord.add_tasks(vec!["t1".into()]).await;
        coord.assign("t1", "w", &mailbox).await.unwrap();

        let result = SubagentResult {
            agent_name: "w".into(),
            output: "done".into(),
            duration: std::time::Duration::from_millis(100),
            iterations: 1,
            tokens_used: None,
            was_truncated: false,
            mode: crate::agent::subagent::types::ExecutionMode::Teammate,
        };

        coord.record_result("t1", result).await;
        assert!(coord.is_complete().await);

        let (done, total) = coord.progress().await;
        assert_eq!(done, 1);
        assert_eq!(total, 1);
    }

    #[tokio::test]
    async fn test_failure_and_retry() {
        let coord = TeamCoordinator::with_max_retries("leader", 2);
        let mailbox = Mailbox::new();

        coord.add_tasks(vec!["t1".into()]).await;
        coord.assign("t1", "w", &mailbox).await.unwrap();

        // First failure: should be retriable
        let can_retry = coord.record_failure("t1", "timeout").await;
        assert!(can_retry);

        let state = coord.task_state("t1").await.unwrap();
        assert!(matches!(state, TaskState::Pending));

        // Reassign and fail again (attempt 2)
        coord.assign("t1", "w2", &mailbox).await.unwrap();
        let can_retry = coord.record_failure("t1", "timeout again").await;
        assert!(!can_retry); // max retries reached

        assert!(coord.is_complete().await);
        assert!(coord.has_failures().await);
    }

    #[tokio::test]
    async fn test_progress() {
        let coord = TeamCoordinator::new("leader");

        coord
            .add_tasks(vec!["a".into(), "b".into(), "c".into()])
            .await;

        let (done, total) = coord.progress().await;
        assert_eq!(done, 0);
        assert_eq!(total, 3);
    }
}
