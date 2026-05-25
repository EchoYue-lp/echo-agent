//! Memory system traits and data types
//!
//! Core trait definitions for the memory subsystem. Concrete implementations
//! live in `echo_state`.
//!
//! | Trait | Purpose |
//! |-------|---------|
//! | [`Store`] | Long-term KV storage with namespace isolation |
//! | [`Embedder`] | Text-to-vector embedding interface |
//! | [`ConversationStore`] | Conversation persistence (transcript read-model) |
//! | [`Checkpointer`] | Short-term thread state persistence |

pub mod checkpointer;
pub mod conversation;
pub mod core_memory;
pub mod decay;
pub mod embedder;
pub mod scope;
pub mod store;
pub mod tiered;

pub use scope::MemoryScope;

pub use checkpointer::{Checkpoint, Checkpointer, ThreadState};
pub use conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage,
};
pub use embedder::Embedder;
pub use store::{SearchMode, SearchQuery, Store, StoreItem};
