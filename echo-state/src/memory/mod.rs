//! 记忆系统
//!
//! 分多层，职责各不相同：
//!
//! | 层次 | 实现 | 作用域 |
//! |------|------|--------|
//! | 短期上下文 | [`compression::ContextManager`] | 单次 `execute()` 内 |
//! | 线程状态 | [`Checkpointer`] / [`FileCheckpointer`] | 跨进程恢复同一线程 |
//! | 对话历史 | [`ConversationStore`] / `SqliteConversationStore` | transcript 投影、历史浏览、多用户隔离 |
//! | 长期记忆 | [`Store`] / [`FileStore`] / `SqliteStore` | 跨会话、跨用户共享 |
//!
//! ## 线程状态持久化（Checkpointer）
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::checkpointer::FileCheckpointer;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! let cp = Arc::new(FileCheckpointer::new("~/.echo-agent/checkpoints.json")?);
//! // 将 `cp` 接入你自己的 agent/runtime 层，或通过 `echo_agent` façade 使用。
//! let _ = cp;
//! # Ok(())
//! # }
//! ```
//!
//! ## 对话持久化（ConversationStore）
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::conversation::{ConversationStore, NewConversation};
//! # async fn example(store: &dyn ConversationStore) -> Result<()> {
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
//! use echo_core::error::Result;
//! use echo_state::memory::store::{FileStore, Store};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
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

/// 测试用嵌入器（仅在测试时可见）
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

#[cfg(test)]
pub use test_utils::MockEmbedder;
