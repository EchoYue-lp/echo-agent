//! 记忆系统
//!
//! 分多层，职责各不相同：
//!
//! | 层次 | 实现 | 作用域 |
//! |------|------|--------|
//! | 短期上下文 | [`compression::ContextManager`] | 单次 `execute()` 内 |
//! | 短期持久化 | [`Checkpointer`] / [`FileCheckpointer`] | 跨进程恢复同一会话 |
//! | 对话历史 | [`ConversationStore`] / `SqliteConversationStore` | 对话持久化、恢复、多用户隔离 |
//! | 长期记忆 | [`Store`] / [`FileStore`] / `SqliteStore` | 跨会话、跨用户共享 |
//!
//! ## 会话持久化（Checkpointer）
//!
//! ```rust,no_run
//! use echo_agent::memory::checkpointer::{FileCheckpointer, Checkpointer};
//! use echo_agent::prelude::{ReactAgent, AgentConfig};
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let cp = Arc::new(FileCheckpointer::new("~/.echo-agent/checkpoints.json")?);
//! let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");
//! let mut agent = ReactAgent::new(config);
//! agent.set_checkpointer(cp, "alice-session-1".to_string());
//! // execute() 自动恢复上次对话，结束后自动保存快照
//! # Ok(())
//! # }
//! ```
//!
//! ## 对话持久化（ConversationStore）
//!
//! ```rust,no_run
//! use echo_agent::memory::conversation::{ConversationStore, NewConversation, StoredMessage, ConversationFilter};
//! # async fn example(store: &dyn ConversationStore) -> echo_agent::error::Result<()> {
//! let conv = store.create_conversation(NewConversation {
//!     conversation_id: "conv-001".to_string(),
//!     user_id: "default".to_string(),
//!     agent_type: None,
//!     title: Some("Rust 讨论".to_string()),
//! }).await?;
//! store.save_messages("conv-001", &[/* messages */]).await?;
//! let msgs = store.get_messages("conv-001").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## 长期 KV 存储（Store）
//!
//! ```rust,no_run
//! use echo_agent::memory::store::{FileStore, Store};
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let store = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
//! store.put(&["alice", "memories"], "pref-001", serde_json::json!({
//!     "content": "用户偏好深色主题",
//!     "importance": 8
//! })).await?;
//! let items = store.search(&["alice", "memories"], "主题", 3).await?;
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

pub use checkpointer::{Checkpoint, Checkpointer, FileCheckpointer, InMemoryCheckpointer};
pub use conversation::{
    Conversation, ConversationFilter, ConversationMeta, ConversationStore, NewConversation,
    StoredMessage,
};
pub use embedder::{Embedder, HttpEmbedder};
pub use embedding_store::EmbeddingStore;
pub use snapshot::{SnapshotManager, SnapshotPolicy, StateSnapshot};
#[cfg(feature = "sqlite")]
pub use sqlite_conversation::SqliteConversationStore;
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteStore;
pub use store::{FileStore, InMemoryStore, Store, StoreItem};
