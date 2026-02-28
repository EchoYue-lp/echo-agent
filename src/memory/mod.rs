//! 记忆系统
//!
//! 分两层，职责各不相同：
//!
//! | 层次 | 实现 | 作用域 |
//! |------|------|--------|
//! | 短期上下文 | [`compression::ContextManager`] | 单次 `execute()` 内 |
//! | 短期持久化 | [`Checkpointer`] / [`FileCheckpointer`] | 跨进程恢复同一会话 |
//! | 长期记忆 | [`Store`] / [`FileStore`] | 跨会话、跨用户共享 |
//!
//! ## 会话持久化（Checkpointer）
//!
//! ```rust,no_run
//! use echo_agent::memory::checkpointer::{FileCheckpointer, Checkpointer};
//! use echo_agent::prelude::ReactAgent;
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let cp = Arc::new(FileCheckpointer::new("~/.echo-agent/checkpoints.json")?);
//! let mut agent = ReactAgent::new(config);
//! agent.set_checkpointer(cp, "alice-session-1".to_string());
//! // execute() 自动恢复上次对话，结束后自动保存快照
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
pub mod store;

pub use checkpointer::{Checkpoint, Checkpointer, FileCheckpointer, InMemoryCheckpointer};
pub use store::{FileStore, InMemoryStore, Store, StoreItem};
