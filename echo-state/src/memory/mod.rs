//! Memory system
//!
//! Layered architecture, each with distinct responsibilities:
//!
//! | Layer | Implementation | Scope |
//! |------|------|--------|
//! | Short-term context | [`crate::compression::ContextManager`] | Within a single `execute()` call |
//! | Conversation history | [`ConversationStore`] / `SqliteConversationStore` | Transcript projection, history browsing, multi-user isolation |
//! | Long-term memory | [`Store`] / [`FileStore`] / `SqliteStore` | Cross-session, cross-user sharing |
//!
//! Runtime checkpoints (for resuming an in-flight conversation across
//! process restarts) live in `echo_agent::state::RuntimeStateStore` — not
//! in this module.
//!
//! **Note**: Trait definitions and data types now live in `echo_core::memory`.
//! They are re-exported here for backward compatibility.

pub mod conversation;
pub mod embedder;
pub mod embedding_store;
pub mod file_conversation;
pub mod snapshot;
#[cfg(feature = "sqlite")]
pub mod sqlite_conversation;
#[cfg(feature = "sqlite")]
pub mod sqlite_store;
pub mod store;
pub mod typed_store;

// Re-export traits and data types from echo-core (backward compatibility)
pub use echo_core::memory::conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage,
};
pub use echo_core::memory::embedder::Embedder;
pub use echo_core::memory::store::{SearchMode, SearchQuery, Store, StoreItem};
pub use echo_core::memory::types::{
    MemoryMeta, MemoryRisk, MemorySource, MemoryStatus, MemoryType, TypedMemoryValue,
};

// Re-export concrete implementations from sub-modules
pub use conversation::{project_message, project_messages, restore_message, restore_messages};
pub use embedder::HttpEmbedder;
pub use embedding_store::EmbeddingStore;
pub use file_conversation::FileConversationStore;
pub use snapshot::{SnapshotManager, SnapshotPolicy, StateSnapshot};
#[cfg(feature = "sqlite")]
pub use sqlite_conversation::SqliteConversationStore;
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteStore;
pub use store::{FileStore, InMemoryStore};
pub use typed_store::{MemoryFilter, TypedMemoryEntry, TypedMemoryStore};

#[cfg(test)]
pub use test_utils::MockEmbedder;

/// Test embedder (visible only in tests)
#[cfg(test)]
mod test_utils {
    use echo_core::error::Result;
    use echo_core::memory::embedder::Embedder;
    use futures::future::BoxFuture;

    pub struct MockEmbedder {
        dimension: usize,
    }

    impl MockEmbedder {
        pub fn new(dimension: usize) -> Result<Self> {
            if dimension == 0 {
                return Err(echo_core::error::ReactError::Other(
                    "mock embedder dimension must be greater than zero".to_string(),
                ));
            }
            Ok(Self { dimension })
        }
    }

    impl Embedder for MockEmbedder {
        fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
            Box::pin(async move {
                let mut vec = vec![0.0f32; self.dimension];
                for (i, b) in text.bytes().enumerate() {
                    vec[i % self.dimension] += b as f32;
                }
                let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for v in &mut vec {
                        *v /= norm;
                    }
                }
                Ok(vec)
            })
        }
    }
}
