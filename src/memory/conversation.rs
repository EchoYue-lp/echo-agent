//! 对话持久化 Store
//!
//! 提供对话历史的结构化存储，支持多用户、多 Agent 隔离。
//! 内置 SQLite 实现（`SqliteConversationStore`），可替换为其他后端。
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_agent::memory::conversation::{ConversationStore, NewConversation, StoredMessage, ConversationFilter};
//! # async fn example(store: &dyn ConversationStore) -> echo_agent::error::Result<()> {
//! // 创建对话
//! let conv = store.create_conversation(NewConversation {
//!     conversation_id: "conv-001".to_string(),
//!     user_id: "alice".to_string(),
//!     agent_type: Some("react".to_string()),
//!     title: Some("关于 Rust 的讨论".to_string()),
//! }).await?;
//!
//! // 保存消息
//! store.save_messages("conv-001", &[
//!     StoredMessage {
//!         id: None,
//!         conversation_id: "conv-001".to_string(),
//!         role: "user".to_string(),
//!         content: Some("什么是所有权？".to_string()),
//!         attachments_json: None,
//!         tool_calls_json: None,
//!         tool_result_json: None,
//!         created_at: "2025-01-01T00:00:00Z".to_string(),
//!     },
//! ]).await?;
//!
//! // 恢复对话
//! let messages = store.get_messages("conv-001").await?;
//! # Ok(())
//! # }
//! ```

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

// ── 数据类型 ──────────────────────────────────────────────────────────────

/// 创建新对话的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewConversation {
    /// 外部唯一 ID（前端生成，如 `conv-1709000000-abc123`）
    pub conversation_id: String,
    /// 用户 ID（多用户隔离）
    #[serde(default = "default_user_id")]
    pub user_id: String,
    /// 对话归属 Agent 类型
    pub agent_type: Option<String>,
    /// 对话标题（首轮问题摘要）
    pub title: Option<String>,
}

fn default_user_id() -> String {
    "default".to_string()
}

/// 对话完整记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    /// 数据库自增 ID
    pub id: i64,
    /// 外部唯一 ID
    pub conversation_id: String,
    /// 用户 ID
    pub user_id: String,
    /// Agent 类型
    pub agent_type: Option<String>,
    /// 对话标题
    pub title: Option<String>,
    /// 压缩后的上下文摘要（9 段式）
    pub summary: Option<String>,
    /// 压缩边界：summary 已覆盖到此 message id（含）
    pub compressed_before_id: Option<i64>,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
}

/// 对话列表条目（轻量，不含 messages 和 summary）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: i64,
    pub conversation_id: String,
    pub user_id: String,
    pub title: Option<String>,
    pub message_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

/// 持久化消息（独立于 LLM Message，增加持久化字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// 数据库自增 ID（新建时为 None）
    pub id: Option<i64>,
    /// 所属对话外部 ID
    pub conversation_id: String,
    /// 角色：user / assistant / tool / system
    pub role: String,
    /// 消息内容
    pub content: Option<String>,
    /// 附件元数据 (JSON)
    pub attachments_json: Option<String>,
    /// 工具调用记录 (JSON)
    pub tool_calls_json: Option<String>,
    /// 工具执行结果 (JSON)
    pub tool_result_json: Option<String>,
    /// 创建时间
    pub created_at: String,
}

/// 列表过滤条件
#[derive(Debug, Clone, Default)]
pub struct ConversationFilter {
    pub user_id: Option<String>,
    pub agent_type: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

// ── Trait ─────────────────────────────────────────────────────────────────

/// 对话持久化存储 trait
///
/// 提供对话和消息的 CRUD 操作，支持替换为不同存储后端。
pub trait ConversationStore: Send + Sync {
    /// 创建新对话
    fn create_conversation<'a>(
        &'a self,
        conv: NewConversation,
    ) -> BoxFuture<'a, Result<Conversation>>;

    /// 获取对话详情
    fn get_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<Conversation>>>;

    /// 列表（支持过滤，按 updated_at DESC 排序）
    fn list_conversations<'a>(
        &'a self,
        filter: ConversationFilter,
    ) -> BoxFuture<'a, Result<Vec<ConversationMeta>>>;

    /// 更新对话（标题 / 摘要 / 压缩边界）
    fn update_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
        title: Option<&'a str>,
        summary: Option<&'a str>,
        compressed_before_id: Option<i64>,
    ) -> BoxFuture<'a, Result<()>>;

    /// 删除对话（级联删除消息）
    fn delete_conversation<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<()>>;

    /// 保存消息（upsert 模式：先删除旧消息，再插入新消息）
    fn save_messages<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: &'a [StoredMessage],
    ) -> BoxFuture<'a, Result<()>>;

    /// 获取对话的所有消息（按 id ASC 排序）
    fn get_messages<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<StoredMessage>>>;

    /// 获取消息数量
    fn count_messages<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> BoxFuture<'a, Result<usize>>;
}
