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
//!
//! Runtime checkpointing (resume across process restarts) is handled by
//! `RuntimeStateStore` in `echo_agent::state`, not by this module.

pub mod conversation;
pub mod embedder;
pub mod scope;
pub mod store;
pub mod types;

pub use scope::MemoryScope;

pub use conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage,
};
pub use embedder::Embedder;
pub use store::{SearchMode, SearchQuery, Store, StoreItem};
pub use types::{
    MemoryMeta, MemoryRisk, MemorySource, MemoryStatus, MemoryType, TypedMemoryValue,
};
