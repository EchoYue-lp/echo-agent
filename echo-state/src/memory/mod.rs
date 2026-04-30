//! Memory system
//!
//! Layered architecture, each with distinct responsibilities:
//!
//! | Layer | Implementation | Scope |
//! |------|------|--------|
//! | Short-term context | [`compression::ContextManager`] | Within a single `execute()` call |
//! | Thread state | [`Checkpointer`] / [`FileCheckpointer`] | Cross-process recovery of the same thread |
//! | Conversation history | [`ConversationStore`] / `SqliteConversationStore` | Transcript projection, history browsing, multi-user isolation |
//! | Long-term memory | [`Store`] / [`FileStore`] / `SqliteStore` | Cross-session, cross-user sharing |
//!
//! ## Thread state persistence (Checkpointer)
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::checkpointer::FileCheckpointer;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! let cp = Arc::new(FileCheckpointer::new("~/.echo-agent/checkpoints.json")?);
//! // Wire `cp` into your own agent/runtime layer, or use it through the `echo_agent` façade.
//! let _ = cp;
//! # Ok(())
//! # }
//! ```
//!
//! ## Conversation persistence (ConversationStore)
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::conversation::{ConversationStore, NewConversation};
//! # async fn example(store: &dyn ConversationStore) -> Result<()> {
//! let conv = store.create_conversation(NewConversation {
//!     conversation_id: "conv-001".to_string(),
//!     user_id: "default".to_string(),
//!     agent_type: None,
//!     title: Some("Rust discussion".to_string()),
//! }).await?;
//! store.save_messages("conv-001", &[/* messages */]).await?;
//! let msgs = store.get_messages("conv-001").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Long-term KV storage (Store)
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::store::{FileStore, Store};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! let store = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
//! store.put(&["alice", "memories"], "pref-001", serde_json::json!({
//!     "content": "User prefers dark theme",
//!     "importance": 8
//! })).await?;
//! let items = store.search(&["alice", "memories"], "theme", 3).await?;
//! # Ok(())
//! # }
//! ```

pub mod checkpointer;
pub mod conversation;
pub mod embedder;
pub mod embedding_store;
pub mod snapshot;
#[cfg(feature = "sqlite")]
pub mod sqlite_conversation;
#[cfg(feature = "sqlite")]
pub mod sqlite_store;
pub mod store;

pub use checkpointer::Checkpointer as ThreadStore;
pub use checkpointer::{
    Checkpoint, Checkpointer, FileCheckpointer, InMemoryCheckpointer, ThreadState,
};
pub use conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage, project_message, project_messages,
};
pub use embedder::{Embedder, HttpEmbedder};
pub use embedding_store::EmbeddingStore;
pub use snapshot::{SnapshotManager, SnapshotPolicy, StateSnapshot};
#[cfg(feature = "sqlite")]
pub use sqlite_conversation::SqliteConversationStore;
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteStore;
pub use store::{FileStore, InMemoryStore, SearchMode, SearchQuery, Store, StoreItem};
#[cfg(test)]
pub use test_utils::MockEmbedder;

/// Test embedder (visible only in tests)
#[cfg(test)]
mod test_utils {
    use crate::memory::embedder::Embedder;
    use echo_core::error::Result;
    use futures::future::BoxFuture;

    pub struct MockEmbedder {
        dimension: usize,
    }

    impl MockEmbedder {
        pub fn new(dimension: usize) -> Self {
            assert!(dimension > 0);
            Self { dimension }
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
