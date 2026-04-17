//! Session Manager —— IM 通道会话管理
//!
//! 提供框架级的会话生命周期管理：
//! - 为每个用户维护独立会话（按 channel_id + sender_id 隔离）
//! - 超时自动重置（空闲超过指定时间后，下条消息开始新会话）
//! - 关键词/命令重置（用户发送特定指令即可重置）
//!
//! ## 使用方式
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use echo_integration::channels::prelude::*;
//! use echo_integration::channels::session::{SessionConfig, SessionHandler};
//! use echo_core::error::Result;
//!
//! struct DummyHandler;
//!
//! #[async_trait]
//! impl MessageHandler for DummyHandler {
//!     async fn handle(&self, msg: InboundMessage) -> Result<OutboundMessage> {
//!         Ok(OutboundMessage::new(
//!             msg.channel_id,
//!             msg.chat_id,
//!             msg.chat_type,
//!             "ok",
//!         ))
//!     }
//!
//!     async fn reply(&self, _msg: OutboundMessage) -> Result<()> {
//!         Ok(())
//!     }
//! }
//!
//! let config = SessionConfig::default()
//!     .with_timeout_minutes(30)
//!     .with_reset_keywords(vec!["重置对话".into(), "新对话".into()])
//!     .with_reset_reply("✅ 对话已重置");
//!
//! let handler = SessionHandler::new(config, || -> Box<dyn MessageHandler> {
//!     Box::new(DummyHandler)
//! });
//! ```

use super::types::*;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::info;

// ── SessionConfig ────────────────────────────────────────────────────────────

/// 会话配置
#[derive(Clone)]
pub struct SessionConfig {
    /// 会话超时时间（默认 60 分钟）
    pub timeout: Duration,
    /// 重置关键词列表（精确匹配，忽略大小写，支持 Unicode）
    pub reset_keywords: Vec<String>,
    /// 重置命令前缀（以此开头的消息视为命令，如 "/"）
    pub command_prefix: Option<String>,
    /// 命令名列表（与 command_prefix 配合，如 ["reset", "clear", "new"]）
    pub reset_commands: Vec<String>,
    /// 重置后的回复文本
    pub reset_reply: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60 * 60), // 1 小时
            reset_keywords: vec![
                "重置对话".into(),
                "新对话".into(),
                "清除记忆".into(),
                "重新开始".into(),
            ],
            command_prefix: Some("/".into()),
            reset_commands: vec!["reset".into(), "clear".into(), "new".into()],
            reset_reply: "✅ 对话已重置，请开始新的对话。".into(),
        }
    }
}

impl SessionConfig {
    /// 设置超时时间（分钟）
    pub fn with_timeout_minutes(mut self, minutes: u64) -> Self {
        self.timeout = Duration::from_secs(minutes * 60);
        self
    }

    /// 设置超时时间
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 设置重置关键词（替换默认值）
    pub fn with_reset_keywords(mut self, keywords: Vec<String>) -> Self {
        self.reset_keywords = keywords;
        self
    }

    /// 追加重置关键词
    pub fn add_reset_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.reset_keywords.push(keyword.into());
        self
    }

    /// 设置命令前缀（None 表示禁用命令模式）
    pub fn with_command_prefix(mut self, prefix: Option<String>) -> Self {
        self.command_prefix = prefix;
        self
    }

    /// 设置重置命令列表（替换默认值）
    pub fn with_reset_commands(mut self, commands: Vec<String>) -> Self {
        self.reset_commands = commands;
        self
    }

    /// 设置重置后的回复文本
    pub fn with_reset_reply(mut self, reply: impl Into<String>) -> Self {
        self.reset_reply = reply.into();
        self
    }

    /// 检查文本是否为重置指令。
    ///
    /// 使用 `to_lowercase()` 进行 Unicode 安全的大小写忽略比较，
    /// 正确支持中文关键词（`eq_ignore_ascii_case` 不适用于非 ASCII 字符）。
    pub fn is_reset(&self, text: &str) -> bool {
        let trimmed = text.trim();
        let lower = trimmed.to_lowercase();

        // 关键词匹配（Unicode 安全）
        if self
            .reset_keywords
            .iter()
            .any(|kw| lower == kw.to_lowercase())
        {
            return true;
        }

        // 命令前缀匹配
        if let Some(ref prefix) = self.command_prefix {
            if let Some(cmd) = trimmed.strip_prefix(prefix.as_str()) {
                let cmd = cmd.trim();
                let cmd_lower = cmd.to_lowercase();
                if self
                    .reset_commands
                    .iter()
                    .any(|c| cmd_lower == c.to_lowercase())
                {
                    return true;
                }
            }
        }

        false
    }
}

// ── Session ──────────────────────────────────────────────────────────────────

/// 会话 key：(channel_id, sender_id)
type SessionKey = (String, String);

/// 单个用户会话
struct Session {
    handler: Box<dyn MessageHandler>,
    last_active: Instant,
}

// ── SessionFactory ───────────────────────────────────────────────────────────

/// 会话工厂 —— 创建新的 MessageHandler 实例
///
/// 每次需要新会话时调用，返回一个全新的 MessageHandler。
/// 使用者通常在闭包中创建 Agent 并包装为 MessageHandler。
pub trait SessionFactory: Send + Sync {
    /// 创建新的消息处理器（新会话）
    fn create(&self) -> Box<dyn MessageHandler>;
}

/// 闭包实现的 SessionFactory
impl<F> SessionFactory for F
where
    F: Fn() -> Box<dyn MessageHandler> + Send + Sync,
{
    fn create(&self) -> Box<dyn MessageHandler> {
        self()
    }
}

// ── SessionHandler ───────────────────────────────────────────────────────────

/// 会话结束回调参数
pub struct SessionEndInfo {
    pub channel_id: String,
    pub sender_id: String,
    pub reason: SessionEndReason,
}

/// 会话结束原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEndReason {
    /// 被用户命令重置
    CommandReset,
    /// 超时后被替换
    TimeoutReplaced,
}

/// 会话管理 Handler
///
/// 包装 SessionFactory，为每个用户维护独立的 MessageHandler 实例。
/// 自动处理超时重置和关键词重置。
pub struct SessionHandler {
    config: SessionConfig,
    factory: Arc<dyn SessionFactory>,
    sessions: DashMap<SessionKey, Arc<Mutex<Session>>>,
    on_session_end: Option<Arc<dyn Fn(SessionEndInfo) + Send + Sync>>,
}

impl SessionHandler {
    /// 创建 SessionHandler
    ///
    /// - `config`: 会话配置
    /// - `factory`: 创建新会话的工厂函数
    pub fn new(config: SessionConfig, factory: impl SessionFactory + 'static) -> Self {
        Self {
            config,
            factory: Arc::new(factory),
            sessions: DashMap::new(),
            on_session_end: None,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults(factory: impl SessionFactory + 'static) -> Self {
        Self::new(SessionConfig::default(), factory)
    }

    /// 设置会话结束回调（用于资源清理等）
    pub fn with_on_session_end<F>(mut self, callback: F) -> Self
    where
        F: Fn(SessionEndInfo) + Send + Sync + 'static,
    {
        self.on_session_end = Some(Arc::new(callback));
        self
    }

    /// 获取当前活跃会话数
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }

    /// 获取或创建会话（原子操作，使用 DashMap entry API 防止竞态条件）
    fn get_or_create(&self, key: &SessionKey) -> Arc<Mutex<Session>> {
        let handler = self.factory.clone();
        self.sessions
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(Session {
                    handler: handler.create(),
                    last_active: Instant::now(),
                }))
            })
            .clone()
    }

    fn notify_session_end(&self, channel_id: String, sender_id: String, reason: SessionEndReason) {
        if let Some(ref callback) = self.on_session_end {
            callback(SessionEndInfo {
                channel_id,
                sender_id,
                reason,
            });
        }
    }
}

#[async_trait]
impl MessageHandler for SessionHandler {
    async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
        let key = (msg.channel_id.clone(), msg.sender_id.clone());

        // ── 关键词/命令重置 ────────────────────────────────────────────
        if self.config.is_reset(&msg.text) {
            if let Some((old_key, _old_session)) = self.sessions.remove(&key) {
                self.notify_session_end(old_key.0, old_key.1, SessionEndReason::CommandReset);
            }
            info!(
                "Session reset by command: ({}, {})",
                msg.channel_id, msg.sender_id
            );
            return Ok(OutboundMessage::new(
                &msg.channel_id,
                &msg.sender_id,
                msg.chat_type,
                &self.config.reset_reply,
            ));
        }

        // ── 获取或创建 session（原子操作） ─────────────────────────────
        let session = self.get_or_create(&key);
        let mut guard = session.lock().await;

        // ── 超时重置 ──────────────────────────────────────────────────
        if guard.last_active.elapsed() >= self.config.timeout {
            info!(
                "Session timeout for ({}, {}), elapsed {:?}",
                msg.channel_id,
                msg.sender_id,
                guard.last_active.elapsed()
            );
            self.notify_session_end(
                msg.channel_id.clone(),
                msg.sender_id.clone(),
                SessionEndReason::TimeoutReplaced,
            );
            guard.handler = self.factory.create();
        }

        guard.last_active = Instant::now();

        // ── 转发给实际 handler ────────────────────────────────────────
        guard.handler.handle(msg).await
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
        // reply 由 channel wrapper 处理，这里直接透传
        // SessionHandler 层面不需要额外操作
        Ok(())
    }
}
