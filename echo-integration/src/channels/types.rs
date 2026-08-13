//! IM channel unified message types and trait definitions

use async_trait::async_trait;
pub use echo_core::error::ChannelError;
pub use echo_core::error::ReactError;
use echo_core::error::Result;
use futures::stream::{BoxStream, StreamExt};

// ── Chat Type ────────────────────────────────────────────────────────────────

/// Chat type: direct message / group
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    /// Direct message
    Direct,
    /// Group
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

// ── Multimedia Attachments ───────────────────────────────────────────────────

/// Attachment type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    /// Image (PNG, JPEG, GIF, WebP, etc.)
    Image,
    /// File (PDF, DOC, TXT, etc.)
    File,
    /// Audio (MP3, WAV, OGG, etc.)
    Audio,
    /// Video (MP4, AVI, etc.)
    Video,
}

/// Message attachment
#[derive(Debug, Clone)]
pub struct MessageAttachment {
    /// Attachment type
    pub kind: AttachmentKind,
    /// Attachment binary data
    pub data: Vec<u8>,
    /// Filename (optional)
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

// ── Inbound Message ──────────────────────────────────────────────────────────

/// Message received from an IM platform
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Channel ID: "qqbot" | "feishu"
    pub channel_id: String,
    /// Sender identifier (QQ: openid / Feishu: open_id)
    pub sender_id: String,
    /// Chat identifier (group_id for group chats, sender_id for direct messages)
    pub chat_id: String,
    /// Chat type
    pub chat_type: ChatType,
    /// Message text
    pub text: String,
    /// Platform original message ID (used for replies)
    pub message_id: String,
    /// Timestamp (Unix seconds)
    pub timestamp: u64,
    /// Message attachments (images, files, audio, video, etc.)
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

    /// Add attachments
    pub fn with_attachments(mut self, attachments: Vec<MessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Stable conversation identity used for session/cache isolation.
    pub fn conversation_id(&self) -> &str {
        &self.chat_id
    }

    /// Transport target for replies to this conversation.
    pub fn reply_target(&self) -> &str {
        &self.chat_id
    }
}

// ── Outbound Message ─────────────────────────────────────────────────────────

/// Message to be sent to an IM platform
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    /// Target channel
    pub channel_id: String,
    /// Target identifier (user openid / group id)
    pub to: String,
    /// Chat type
    pub chat_type: ChatType,
    /// Reply text
    pub text: String,
    /// Message ID being replied to (at_reply)
    pub reply_to: Option<String>,
    /// Message attachments
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

    /// Add attachments
    pub fn with_attachments(mut self, attachments: Vec<MessageAttachment>) -> Self {
        self.attachments = attachments;
        self
    }
}

// ── ChannelPlugin Trait ─────────────────────────────────────────────────────

use std::sync::Arc;

/// Message handler — forwards IM messages to the Agent for processing
#[async_trait]
pub trait MessageHandler: Send + Sync {
    /// Handle an inbound message, returning an outbound message
    async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage>;

    /// Send an outbound message back to the IM platform (implemented by the Channel itself)
    async fn reply(&self, msg: OutboundMessage) -> Result<()>;

    /// 流式 handle:产出逐段 `OutboundMessage`。
    ///
    /// 默认实现:只 yield 一次 `handle` 的返回值(向后兼容,所有现有 impl 无需 override)。
    /// 需要真流式(分段投递)的 handler override 此方法。
    async fn handle_stream<'a>(
        &'a self,
        msg: InboundMessage,
    ) -> Result<BoxStream<'a, Result<OutboundMessage>>> {
        let out = self.handle(msg).await?;
        Ok(futures::stream::once(async move { Ok(out) }).boxed())
    }
}

/// Channel capability description
#[derive(Debug)]
pub struct ChannelCapabilities {
    /// Supported chat types
    pub chat_types: &'static [ChatType],
    /// Whether media is supported (images/files)
    pub supports_media: bool,
    /// Whether topics/threads are supported
    pub supports_threads: bool,
}

/// Unified interface for IM channel plugins
#[async_trait]
pub trait ChannelPlugin: Send + Sync {
    /// Unique channel ID: "qqbot" | "feishu"
    fn id(&self) -> &str;

    /// Human-readable name
    fn label(&self) -> &str {
        self.id()
    }

    /// Capability description
    fn capabilities(&self) -> &ChannelCapabilities;

    /// Start the channel (establish connection / start server)
    async fn start(&mut self, handler: Arc<dyn MessageHandler>) -> Result<()>;

    /// Stop the channel
    async fn stop(&mut self) -> Result<()>;

    /// Send a message
    async fn send(&self, msg: OutboundMessage) -> Result<()>;

    /// Health check — verifies whether the channel is in normal working state.
    ///
    /// Returns Ok(()) by default; subclasses may override to perform more
    /// complex checks (e.g. token validity, connection status, etc.).
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

/// Channel context
pub struct ChannelContext {
    pub handler: Arc<dyn MessageHandler>,
}

impl ChannelContext {
    pub fn new(handler: Arc<dyn MessageHandler>) -> Self {
        Self { handler }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    /// 只 impl `handle` 的 dummy handler —— 验证默认 `handle_stream` 行为。
    struct OnceHandler {
        reply_text: String,
    }

    #[async_trait]
    impl MessageHandler for OnceHandler {
        async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                &self.reply_text,
            ))
        }
        async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_handle_stream_yields_exactly_one_outbound() {
        let handler = OnceHandler {
            reply_text: "hello".into(),
        };
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "hi", "m1");
        let mut stream = handler.handle_stream(msg).await.expect("stream ok");

        // 第一条 = handle 的返回值
        let first = stream
            .next()
            .await
            .expect("at least one item")
            .expect("item is ok");
        assert_eq!(first.text, "hello");
        assert_eq!(first.channel_id, "qq");
        assert_eq!(first.to, "c1");

        // 之后不再有(恰好 1 条)
        assert!(stream.next().await.is_none(), "default yields exactly one");
    }
}
