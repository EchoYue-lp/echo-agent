//! Runtime State Store — DAG node state machine for long-running tasks.
//!
//! Provides a [`RuntimeStateStore`] trait and [`TaskNode`] state machine
//! for checkpointing agent execution state across turns.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::state::{RuntimeStateStore, TaskNode, TaskNodeStatus};
//!
//! # async fn example(store: &dyn RuntimeStateStore) -> echo_agent::error::Result<()> {
//! let node = TaskNode::new("plan", "Planning phase")
//!     .with_status(TaskNodeStatus::Pending);
//! store.save_node("conv-123", &node).await?;
//! # Ok(())
//! # }
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── TaskNodeStatus ─────────────────────────────────────────────────────

/// Status of a task node in the execution DAG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskNodeStatus {
    /// Waiting to start execution.
    Pending,
    /// Currently executing.
    Running,
    /// Execution completed successfully.
    Success,
    /// Execution failed.
    Failed,
    /// Blocked waiting for human approval or external input.
    Blocked { reason: String },
    /// Recovered from Blocked state and ready to resume.
    Hydrated,
}

impl TaskNodeStatus {
    /// Returns true if the node is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskNodeStatus::Success | TaskNodeStatus::Failed)
    }

    /// Returns true if the node is in a blocked state.
    pub fn is_blocked(&self) -> bool {
        matches!(self, TaskNodeStatus::Blocked { .. })
    }
}

// ── TaskNode ───────────────────────────────────────────────────────────

/// A single node in the execution DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    /// Unique node identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Current execution status.
    pub status: TaskNodeStatus,
    /// IDs of dependent nodes that must complete before this one.
    pub dependencies: Vec<String>,
    /// Node outputs (arbitrary JSON).
    pub outputs: serde_json::Value,
    /// Timestamp when the node was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp when the node was last updated.
    pub updated_at: DateTime<Utc>,
}

impl TaskNode {
    /// Create a new task node with the given id and name.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            name: name.into(),
            status: TaskNodeStatus::Pending,
            dependencies: Vec::new(),
            outputs: serde_json::Value::Null,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a new task node with a specific status.
    pub fn with_status(mut self, status: TaskNodeStatus) -> Self {
        self.status = status;
        self.updated_at = Utc::now();
        self
    }

    /// Create a new task node with dependencies.
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    /// Create a new task node with outputs.
    pub fn with_outputs(mut self, outputs: serde_json::Value) -> Self {
        self.outputs = outputs;
        self.updated_at = Utc::now();
        self
    }
}

// ── AgentCheckpoint ────────────────────────────────────────────────────

/// A full checkpoint of agent runtime state, suitable for serialization
/// and later restoration (hydration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpoint {
    /// Conversation / session identifier.
    pub conversation_id: String,
    /// Serialized message history.
    pub messages_json: String,
    /// Current plan text (optional).
    pub current_plan: Option<String>,
    /// Names of currently active skills.
    pub active_skills: Vec<String>,
    /// If the agent was blocked, the reason.
    pub blocked_reason: Option<String>,
    /// Session-bound working directory (worktree path). Restored on hydration
    /// so a worktree-bound session resumes in the same isolated checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<std::path::PathBuf>,
    /// Timestamp when the checkpoint was captured.
    pub timestamp: DateTime<Utc>,
}

impl AgentCheckpoint {
    /// Create a new checkpoint.
    pub fn new(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            messages_json: String::new(),
            current_plan: None,
            active_skills: Vec::new(),
            blocked_reason: None,
            working_dir: None,
            timestamp: Utc::now(),
        }
    }
}

// ── RuntimeStateStore trait ────────────────────────────────────────────

/// Trait for persistent runtime state storage.
///
/// Implementations may use SQLite, JSON files, or in-memory storage.
pub trait RuntimeStateStore: Send + Sync {
    /// Save or update a task node.
    fn save_node<'a>(
        &'a self,
        conversation_id: &'a str,
        node: &'a TaskNode,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;

    /// Load all nodes for a conversation.
    fn load_nodes<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Vec<TaskNode>>>;

    /// Update the status of a specific node.
    fn update_status<'a>(
        &'a self,
        conversation_id: &'a str,
        node_id: &'a str,
        status: TaskNodeStatus,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;

    /// Get the most recent checkpoint for a conversation, if any.
    fn get_checkpoint<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>>;

    /// Save a checkpoint.
    fn save_checkpoint<'a>(
        &'a self,
        checkpoint: &'a AgentCheckpoint,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;

    /// Delete all state for a conversation.
    fn clear_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> futures::future::BoxFuture<'a, crate::error::Result<()>>;
}

// ── Re-export SQLite implementation ────────────────────────────────────

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteRuntimeStateStore;
