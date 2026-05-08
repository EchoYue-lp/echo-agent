//! Short-term thread state persistence (Checkpointer trait and data types)
//!
//! Concrete implementations ([`InMemoryCheckpointer`], [`FileCheckpointer`])
//! live in `echo_state`.

use crate::error::Result;
use crate::llm::types::Message;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Checkpoint ────────────────────────────────────────────────────────────────

/// Snapshot of a single conversation state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Owning session identifier
    pub session_id: String,
    /// Unique snapshot ID (UUID v4)
    pub checkpoint_id: String,
    /// Complete message history at this point in time
    pub messages: Vec<Message>,
    /// Parent snapshot ID, representing checkpoint lineage.
    #[serde(default)]
    pub parent_checkpoint_id: Option<String>,
    /// Summary information persisted together with this thread state.
    #[serde(default)]
    pub summary: Option<String>,
    /// Custom metadata (e.g., execution phase, source, tags).
    #[serde(default)]
    pub metadata: Option<Value>,
    /// Creation time (Unix seconds)
    pub created_at: u64,
}

/// Thread-level runtime state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadState {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

impl ThreadState {
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            summary: None,
            metadata: None,
        }
    }
}

impl Checkpoint {
    pub fn thread_state(&self) -> ThreadState {
        ThreadState {
            messages: self.messages.clone(),
            summary: self.summary.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

// ── Checkpointer trait ────────────────────────────────────────────────────────

/// Persistence interface for short-term conversation memory
///
/// Implementations may be swapped with any storage backend (in-memory, file, database, etc.).
pub trait Checkpointer: Send + Sync {
    /// Save the current session's message history, returning the new snapshot ID
    fn put<'a>(
        &'a self,
        session_id: &'a str,
        messages: Vec<Message>,
    ) -> BoxFuture<'a, Result<String>>;

    /// Get the latest snapshot for the given session (returns `None` if not found)
    fn get<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Option<Checkpoint>>>;

    /// Get all historical snapshots for the given session (reverse chronological order)
    fn list<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Vec<Checkpoint>>>;

    /// Delete all snapshots for the given session
    fn delete_session<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<()>>;

    /// List all existing session IDs
    fn list_sessions(&self) -> BoxFuture<'_, Result<Vec<String>>>;

    /// Save complete thread state, defaulting to saving only the message list.
    fn put_state<'a>(
        &'a self,
        session_id: &'a str,
        state: ThreadState,
    ) -> BoxFuture<'a, Result<String>> {
        self.put(session_id, state.messages)
    }

    /// Get the latest thread state, defaulting to recovering from the latest checkpoint.
    fn get_state<'a>(&'a self, session_id: &'a str) -> BoxFuture<'a, Result<Option<ThreadState>>> {
        Box::pin(async move { Ok(self.get(session_id).await?.map(|cp| cp.thread_state())) })
    }
}
