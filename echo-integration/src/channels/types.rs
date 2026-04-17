//! IM 通道统一消息类型和 trait 定义

use async_trait::async_trait;
pub use echo_core::error::ChannelError;
pub use echo_core::error::ReactError;
use echo_core::error::Result;

// ── 聊天类型 ─────────────────────────────────────────────────────────────────

/// 聊天类型：私聊 / 群聊
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    /// 私聊（Direct Message）
    Direct,
    /// 群聊（Group）
    Group,
}

impl std::fmt::Display for ChatType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatType::Direct => write!(f, "direct"),
            ChatType::Group => write!(f, "group"),
        }
    }
}

// ── 多模态附件 ───────────────────────────────────────────────────────────────

/// 附件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// 图片（PNG, JPEG, GIF, WebP 等）
    Image,
    /// 文件（PDF, DOC, TXT 等）
    File,
    /// 音频（MP3, WAV, OGG 等）
    Audio,
    /// 视频（MP4, AVI 等）
    Video,
}

/// 消息附件
#[derive(Debug, Clone)]
pub struct MessageAttachment {
    /// 附件类型
    pub kind: AttachmentKind,
    /// 附件二进制数据
    pub data: Vec<u8>,
    /// 文件名（可选）
    pub filename: Option<String>,
}

impl MessageAttachment {
    pub fn new(kind: AttachmentKind, data: Vec<u8>) -> Self {
        Self {
            kind,
            data,
            filename: None,
        }
    }

    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }
}

// ── 入站消息 ─────────────────────────────────────────────────────────────────

/// 从 IM 平台接收到的消息
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// 通道 ID："qqbot" | "feishu"
    pub channel_id: String,
    /// 发送者标识（QQ: openid / 飞书: open_id）
    pub sender_id: String,
    /// 会话标识（群聊时为 group_id，私聊时为 sender_id）
    pub chat_id: String,
    /// 聊天类型
    pub chat_type: ChatType,
    /// 消息文本
    pub text: String,
    /// 平台原始消息 ID（用于回复）
    pub message_id: String,
    /// 时间戳（Unix 秒）
    pub timestamp: u64,
    /// 消息附件（图片、文件、音频、视频等）
    pub attachments: Vec<MessageAttachment>,
}

impl InboundMessage {
    pub fn new(
        channel_id: impl Into<String>,
        sender_id: impl Into<String>,
        chat_id: impl Into<String>,
        chat_type: ChatType,
        text: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            sender_id: sender_id.into(),
            chat_id: chat_id.into(),
            chat_type,
            text: text.into(),
            message_id: message_id.into(),
            timestamp: chrono::Utc::now().timestamp() as u64,
            attachments: Vec::new(),
        }
    }

    /// 添加附件
    pub fn with_attachments(mut self, attachments: Vec<MessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }
}

// ── 出站消息 ─────────────────────────────────────────────────────────────────

/// 发送给 IM 平台的消息
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    /// 目标通道
    pub channel_id: String,
    /// 目标标识（用户 openid / 群 id）
    pub to: String,
    /// 聊天类型
    pub chat_type: ChatType,
    /// 回复文本
    pub text: String,
    /// 被回复的消息 ID（at_reply）
    pub reply_to: Option<String>,
    /// 消息附件
    pub attachments: Vec<MessageAttachment>,
}

impl OutboundMessage {
    pub fn new(
        channel_id: impl Into<String>,
        to: impl Into<String>,
        chat_type: ChatType,
        text: impl Into<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            to: to.into(),
            chat_type,
            text: text.into(),
            reply_to: None,
            attachments: Vec::new(),
        }
    }

    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }

    /// 添加附件
    pub fn with_attachments(mut self, attachments: Vec<MessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }
}

// ── ChannelPlugin Trait ─────────────────────────────────────────────────────

use std::sync::Arc;

/// 消息处理器 —— 将 IM 消息转给 Agent 处理
#[async_trait]
pub trait MessageHandler: Send + Sync {
    /// 处理入站消息，返回出站消息
    async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage>;

    /// 将出站消息发送回 IM 平台（由 Channel 自身实现）
    async fn reply(&self, msg: OutboundMessage) -> Result<()>;
}

/// Channel 能力描述
#[derive(Debug)]
pub struct ChannelCapabilities {
    /// 支持的聊天类型
    pub chat_types: &'static [ChatType],
    /// 是否支持媒体（图片/文件）
    pub supports_media: bool,
    /// 是否支持话题/线程
    pub supports_threads: bool,
}

/// IM 通道插件的统一接口
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// 通道唯一 ID："qqbot" | "feishu"
    fn id(&self) -> &str;

    /// 用户可见的名称
    fn label(&self) -> &str {
        self.id()
    }

    /// 能力描述
    fn capabilities(&self) -> &ChannelCapabilities;

    /// 启动通道（建立连接 / 启动 server）
    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()>;

    /// 停止通道
    async fn stop(&mut self) -> Result<()>;

    /// 发送消息
    async fn send(&self, msg: OutboundMessage) -> Result<()>;

    /// 健康检查 —— 验证通道是否处于正常工作状态
    ///
    /// 默认返回 Ok(())，子类可以重写以执行更复杂的检查
    /// （如 token 有效性、连接状态等）。
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

/// 通道上下文
pub struct ChannelContext {
    pub handler: Arc<dyn MessageHandler>,
}

impl ChannelContext {
    pub fn new(handler: Arc<dyn MessageHandler>) -> Self {
        Self { handler }
    }
}
