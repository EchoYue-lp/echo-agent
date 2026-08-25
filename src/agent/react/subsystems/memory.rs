//! Memory and persistence subsystem
//!
//! Centralized management of context window, long-term memory store, runtime checkpoints,
//! state snapshots, and conversation history projection.

use crate::compression::ContextManager;
use crate::memory::{SnapshotManager, Store};
use std::sync::Arc;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum RuntimeStateHydration {
    #[default]
    Uninitialized,
    Hydrating(Option<String>),
    Hydrated(Option<String>),
}

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
    /// Runtime checkpoint identity currently represented by `context`.
    ///
    /// `Hydrating` is published before any cancellable restore mutation, so a
    /// cancelled switch can never make partially replaced context look warm for
    /// the previous identity.
    pub(crate) runtime_state_hydration: Arc<tokio::sync::Mutex<RuntimeStateHydration>>,
    /// Construction-time working directory restored when a new runtime
    /// identity has no checkpoint of its own.
    pub(crate) configured_working_dir: Option<std::path::PathBuf>,
    pub(crate) transcript_projection_cursor:
        Arc<tokio::sync::Mutex<crate::agent::snapshot::TranscriptProjectionCursor>>,
}
