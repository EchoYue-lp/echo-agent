//! Task definitions

use serde::{Deserialize, Serialize};

/// Task status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Pending
    Pending,
    /// In progress
    InProgress,
    /// Completed
    Completed,
    /// Cancelled
    Cancelled,
    /// Failed
    Failed(String),
    /// Blocked
    Blocked(String),
    /// Timed out
    TimedOut { error: String },
    /// Retrying
    Retrying { attempt: u32, last_error: String },
}

impl TaskStatus {
    /// Whether this is a terminal state (will not change further)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed
                | TaskStatus::Cancelled
                | TaskStatus::Failed(_)
                | TaskStatus::TimedOut { .. }
        )
    }

    /// Whether the state transition is valid
    pub fn can_transition_to(&self, target: &TaskStatus) -> bool {
        match self {
            TaskStatus::Pending => {
                matches!(target, TaskStatus::InProgress | TaskStatus::Cancelled)
            }
            TaskStatus::InProgress => matches!(
                target,
                TaskStatus::Completed
                    | TaskStatus::Cancelled
                    | TaskStatus::Failed(_)
                    | TaskStatus::TimedOut { .. }
                    | TaskStatus::Retrying { .. }
            ),
            TaskStatus::Retrying { .. } => matches!(
                target,
                TaskStatus::Completed
                    | TaskStatus::Cancelled
                    | TaskStatus::Failed(_)
                    | TaskStatus::TimedOut { .. }
                    | TaskStatus::Retrying { .. }
            ),
            TaskStatus::Blocked(_) => matches!(target, TaskStatus::Pending | TaskStatus::Cancelled),
            _ => false,
        }
    }

    /// Execute state transition, return new state after validating legality
    ///
    /// If the transition is invalid, return `Err` with detailed error info.
    pub fn transition_to(&self, target: TaskStatus) -> Result<TaskStatus, String> {
        if !self.can_transition_to(&target) {
            return Err(format!(
                "Invalid task state transition: {:?} → {:?}",
                self, target
            ));
        }
        Ok(target)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Task ID
    pub id: String,
    /// Task description
    pub description: String,
    /// Task status
    pub status: TaskStatus,
    /// List of dependent task IDs
    pub dependencies: Vec<String>,
    /// Priority (0-10, 10 is highest)
    pub priority: u8,
    /// Task result
    pub result: Option<String>,
    /// Execution rationale or notes
    pub reasoning: Option<String>,
    /// Name of the Agent assigned to execute this task
    pub assigned_agent: Option<String>,
    /// Tags (for categorization and filtering)
    pub tags: Vec<String>,
    pub parent_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Task topic/title (for logging and events)
    pub subject: String,
    /// Timeout in seconds, 0 means no timeout
    pub timeout_secs: u64,
    /// Maximum retry count
    pub max_retries: u32,
    /// Current retry count
    pub retry_count: u32,
}

impl Task {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        let description = description.into();
        Self {
            id: id.into(),
            subject: description.clone(),
            description,
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
            priority: 5,
            result: None,
            reasoning: None,
            assigned_agent: None,
            tags: Vec::new(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            timeout_secs: 0,
            max_retries: 0,
            retry_count: 0,
        }
    }

    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    pub fn add_dependency(&mut self, dep: String) {
        self.dependencies.push(dep);
    }

    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(10);
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Specify the Agent to execute
    pub fn with_assigned_agent(mut self, agent: impl Into<String>) -> Self {
        self.assigned_agent = Some(agent.into());
        self
    }

    /// Add tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Add a single tag
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        self.tags.push(tag.into());
    }

    /// Whether already cancelled
    pub fn is_cancelled(&self) -> bool {
        self.status == TaskStatus::Cancelled
    }

    /// Cancel the task (using state machine validation)
    ///
    /// Succeeds only when the current state allows transition to `Cancelled`.
    /// Returns `true` if cancellation succeeded, `false` if current state does not allow cancellation.
    pub fn cancel(&mut self) -> bool {
        match self.status.transition_to(TaskStatus::Cancelled) {
            Ok(new_status) => {
                self.status = new_status;
                true
            }
            Err(_) => false,
        }
    }

    /// Record an execution result
    pub fn record_execution(
        &mut self,
        attempt: u32,
        error: Option<String>,
        duration_secs: Option<u64>,
        result: Option<String>,
    ) {
        self.retry_count = attempt.saturating_sub(1);
        self.updated_at = super::time::now_secs();
        if let Some(r) = result {
            self.result = Some(r);
        }
        if let Some(dur) = duration_secs {
            let _ = dur; // Record execution duration (usable for future statistics)
        }
        if let Some(err) = error {
            self.reasoning = Some(format!("Attempt {} failed: {}", attempt, err));
        }
    }
}
