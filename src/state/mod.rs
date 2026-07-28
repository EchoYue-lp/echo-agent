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
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
    /// Timestamp when the node was last updated.
    #[serde(with = "crate::utils::time::local_rfc3339")]
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
    #[serde(with = "crate::utils::time::local_rfc3339")]
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

    /// Deserialize and validate the checkpoint message history.
    ///
    /// A checkpoint is resumable only when every assistant tool call has one
    /// matching tool result in order. Rejecting malformed history here avoids
    /// sending provider-invalid context or replaying an already completed
    /// side effect after restart.
    pub fn restore_messages(&self) -> crate::error::Result<Vec<crate::llm::types::Message>> {
        let messages: Vec<crate::llm::types::Message> = serde_json::from_str(&self.messages_json)
            .map_err(|error| {
            crate::error::ReactError::RuntimeState(Box::new(
                echo_core::error::RuntimeStateError::SerializationError(format!(
                    "Failed to deserialize checkpoint messages: {error}"
                )),
            ))
        })?;
        validate_tool_message_pairing(&messages)?;
        Ok(messages)
    }

    /// Completed tool call IDs present in this checkpoint, in message order.
    pub fn completed_tool_call_ids(&self) -> crate::error::Result<Vec<String>> {
        let messages = self.restore_messages()?;
        Ok(messages
            .into_iter()
            .filter_map(|message| {
                if message.role == crate::llm::types::Role::Tool {
                    message.tool_call_id
                } else {
                    None
                }
            })
            .collect())
    }
}

fn validate_tool_message_pairing(
    messages: &[crate::llm::types::Message],
) -> crate::error::Result<()> {
    let mut pending = std::collections::HashMap::<String, String>::new();
    for message in messages {
        if message.role == crate::llm::types::Role::Assistant
            && let Some(tool_calls) = message.tool_calls.as_ref()
        {
            for call in tool_calls {
                if pending
                    .insert(call.id.clone(), call.function.name.clone())
                    .is_some()
                {
                    return Err(invalid_checkpoint(format!(
                        "duplicate in-flight tool call id {}",
                        call.id
                    )));
                }
            }
        }
        if message.role == crate::llm::types::Role::Tool {
            let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                invalid_checkpoint("tool result is missing tool_call_id".to_string())
            })?;
            let expected_name = pending.remove(call_id).ok_or_else(|| {
                invalid_checkpoint(format!("orphan or duplicate tool result for {call_id}"))
            })?;
            if message.name.as_deref() != Some(expected_name.as_str()) {
                return Err(invalid_checkpoint(format!(
                    "tool result name mismatch for {call_id}: expected {expected_name}, got {}",
                    message.name.as_deref().unwrap_or("<missing>")
                )));
            }
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        let mut ids = pending.into_keys().collect::<Vec<_>>();
        ids.sort();
        Err(invalid_checkpoint(format!(
            "checkpoint has tool calls without results: {}",
            ids.join(", ")
        )))
    }
}

fn invalid_checkpoint(message: String) -> crate::error::ReactError {
    crate::error::ReactError::RuntimeState(Box::new(
        echo_core::error::RuntimeStateError::SerializationError(message),
    ))
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

// ── Re-export implementations ─────────────────────────────────────────

/// File-backed runtime state store (default, no SQLite dependency).
pub mod file;

/// SQLite-backed runtime state store (`sqlite` feature).
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use file::FileRuntimeStateStore;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteRuntimeStateStore;

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    use crate::llm::types::{FunctionCall, Message, ToolCall};

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
        }
    }

    fn checkpoint(messages: Vec<Message>) -> crate::error::Result<AgentCheckpoint> {
        let mut checkpoint = AgentCheckpoint::new("conversation-1");
        checkpoint.messages_json = serde_json::to_string(&messages)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        Ok(checkpoint)
    }

    #[test]
    fn checkpoint_restores_paired_tool_history() -> crate::error::Result<()> {
        let checkpoint = checkpoint(vec![
            Message::assistant_with_tools(vec![tool_call("call-1", "write_file")]),
            Message::tool_result(
                "call-1".to_string(),
                "write_file".to_string(),
                "written".to_string(),
            ),
        ])?;
        assert_eq!(checkpoint.restore_messages()?.len(), 2);
        assert_eq!(checkpoint.completed_tool_call_ids()?, vec!["call-1"]);
        Ok(())
    }

    #[test]
    fn checkpoint_rejects_unpaired_or_duplicate_tool_results() -> crate::error::Result<()> {
        let unpaired = checkpoint(vec![Message::assistant_with_tools(vec![tool_call(
            "call-1",
            "write_file",
        )])])?;
        assert!(unpaired.restore_messages().is_err());

        let duplicate = checkpoint(vec![
            Message::assistant_with_tools(vec![tool_call("call-1", "write_file")]),
            Message::tool_result(
                "call-1".to_string(),
                "write_file".to_string(),
                "written".to_string(),
            ),
            Message::tool_result(
                "call-1".to_string(),
                "write_file".to_string(),
                "written twice".to_string(),
            ),
        ])?;
        assert!(duplicate.restore_messages().is_err());
        Ok(())
    }
}
