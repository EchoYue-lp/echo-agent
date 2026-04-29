//! 记忆与持久化子系统
//!
//! 集中管理上下文窗口、长期记忆存储、状态快照、Checkpoint、对话历史投影。

use crate::compression::ContextManager;
use crate::memory::checkpointer::Checkpointer;
use crate::memory::snapshot::SnapshotManager;
use crate::memory::store::Store;
use std::sync::Arc;

/// 记忆与持久化子系统
///
/// 聚合对话上下文、长期记忆 Store、状态快照、线程 Checkpoint、
/// 对话历史投影 Store。
pub(crate) struct MemorySubsystem {
    pub(crate) context: Arc<tokio::sync::Mutex<ContextManager>>,
    pub(crate) store: Option<Arc<dyn Store>>,
    pub(crate) checkpointer: Option<Arc<dyn Checkpointer>>,
    pub(crate) snapshot_manager: Arc<std::sync::RwLock<Option<SnapshotManager>>>,
    pub(crate) conversation_store: Option<Arc<dyn crate::memory::conversation::ConversationStore>>,
}
