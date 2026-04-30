//! Memory and persistence subsystem
//!
//! Centralized management of context window, long-term memory store, state snapshots,
//! Checkpoint, and conversation history projection.

use crate::compression::ContextManager;
use crate::memory::checkpointer::Checkpointer;
use crate::memory::snapshot::SnapshotManager;
use crate::memory::store::Store;
use std::sync::Arc;

/// Memory and persistence subsystem
///
/// Aggregates conversation context, long-term memory Store, state snapshots,
/// thread Checkpoint, and conversation history projection Store.
pub(crate) struct MemorySubsystem {
    pub(crate) context: Arc<tokio::sync::Mutex<ContextManager>>,
    pub(crate) store: Option<Arc<dyn Store>>,
    pub(crate) checkpointer: Option<Arc<dyn Checkpointer>>,
    pub(crate) snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    pub(crate) conversation_store: Option<Arc<dyn crate::memory::conversation::ConversationStore>>,
}
