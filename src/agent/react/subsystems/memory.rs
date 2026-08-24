//! Memory and persistence subsystem
//!
//! Centralized management of context window, long-term memory store, runtime checkpoints,
//! state snapshots, and conversation history projection.

use crate::compression::ContextManager;
use crate::memory::{SnapshotManager, Store};
use std::sync::Arc;

/// Memory and persistence subsystem
///
/// Aggregates conversation context, long-term memory Store, state snapshots,
/// runtime checkpoint store, and conversation history projection Store.
pub(crate) struct MemorySubsystem {
    pub(crate) context: Arc<tokio::sync::Mutex<ContextManager>>,
    pub(crate) store: Option<Arc<dyn Store>>,
    pub(crate) snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    pub(crate) conversation_store: Option<Arc<dyn crate::memory::ConversationStore>>,
    pub(crate) state_store: Option<Arc<dyn crate::state::RuntimeStateStore>>,
    pub(crate) transcript_projection_cursor:
        Arc<tokio::sync::Mutex<crate::agent::snapshot::TranscriptProjectionCursor>>,
}
