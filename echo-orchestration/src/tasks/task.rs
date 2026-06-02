//! Task definitions

use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

#[derive(Clone, Serialize, Deserialize)]
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
    /// Optional per-task execution function.
    ///
    /// When set, this overrides the executor's global `execute_fn` for this task.
    /// Not serialized — callers must re-register after deserialization.
    #[serde(skip)]
    pub execute_fn: Option<super::executor::TaskExecuteFn>,

    /// Serializable typed metadata (survives persistence/serialization).
    ///
    /// Application layers can store domain-specific data (e.g., task kind,
    /// pipeline parameters, UI hints) as JSON. Use [`with_metadata`](Self::with_metadata)
    /// to set both this field and the typed [`metadata`](Self::metadata) simultaneously.
    pub metadata_json: Option<serde_json::Value>,

    /// Typed metadata (not serialized — re-register after deserialization).
    ///
    /// Provides zero-cost downcast access to the original typed value.
    /// Paired with [`metadata_json`](Self::metadata_json) for round-tripping.
    #[serde(skip)]
    pub metadata: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("description", &self.description)
            .field("status", &self.status)
            .field("dependencies", &self.dependencies)
            .field("priority", &self.priority)
            .field("result", &self.result)
            .field("reasoning", &self.reasoning)
            .field("assigned_agent", &self.assigned_agent)
            .field("tags", &self.tags)
            .field("parent_id", &self.parent_id)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("subject", &self.subject)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_retries", &self.max_retries)
            .field("retry_count", &self.retry_count)
            .field("execute_fn", &self.execute_fn.as_ref().map(|_| "Some(<fn>)"))
            .field("metadata_json", &self.metadata_json)
            .finish()
    }
}

impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.description == other.description
            && self.status == other.status
            && self.dependencies == other.dependencies
            && self.priority == other.priority
            && self.result == other.result
            && self.reasoning == other.reasoning
            && self.assigned_agent == other.assigned_agent
            && self.tags == other.tags
            && self.parent_id == other.parent_id
            && self.created_at == other.created_at
            && self.updated_at == other.updated_at
            && self.subject == other.subject
            && self.timeout_secs == other.timeout_secs
            && self.max_retries == other.max_retries
            && self.retry_count == other.retry_count
        // execute_fn, metadata_json, metadata intentionally excluded — not comparable
    }
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
            execute_fn: None,
            metadata_json: None,
            metadata: None,
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

    /// Set a per-task execution function that overrides the executor's global execute_fn.
    pub fn with_execute_fn(mut self, f: super::executor::TaskExecuteFn) -> Self {
        self.execute_fn = Some(f);
        self
    }

    /// Set typed metadata.
    ///
    /// Stores both the typed value (for zero-cost downcast access) and its
    /// JSON serialization (for persistence). After deserialization from a
    /// store, only `metadata_json` survives — call [`get_metadata`] to
    /// attempt a typed read, or re-register with `with_metadata`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// #[derive(Serialize)]
    /// struct ResearchParams { topic: String, max_papers: u32 }
    ///
    /// let task = Task::new("r1", "Research task")
    ///     .with_metadata(ResearchParams { topic: "AI".into(), max_papers: 20 });
    ///
    /// // Later, retrieve typed access:
    /// let params = task.get_metadata::<ResearchParams>().unwrap();
    /// ```
    pub fn with_metadata<T: serde::Serialize + Send + Sync + 'static>(mut self, meta: T) -> Self {
        self.metadata_json = serde_json::to_value(&meta).ok();
        self.metadata = Some(std::sync::Arc::new(meta));
        self
    }

    /// Set raw JSON metadata (without a typed value).
    pub fn with_metadata_json(mut self, json: serde_json::Value) -> Self {
        self.metadata_json = Some(json);
        self
    }

    /// Attempt to downcast the typed metadata to a concrete type.
    ///
    /// Returns `None` if no metadata was set or the type doesn't match.
    pub fn get_metadata<T: 'static>(&self) -> Option<&T> {
        self.metadata.as_ref()?.downcast_ref::<T>()
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
