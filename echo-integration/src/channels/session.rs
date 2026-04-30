//! Session Manager — IM channel session management
//!
//! Provides framework-level session lifecycle management:
//! - Maintains independent sessions per user (isolated by channel_id + sender_id)
//! - Auto-reset on timeout (after idle period, next message starts a new session)
//! - Keyword/command reset (user can reset by sending a specific command)
//!
//! ## Usage
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
//!     .with_reset_keywords(vec!["reset chat".into(), "new chat".into()])
//!     .with_reset_reply("Conversation has been reset.");
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

/// Session configuration
#[derive(Clone)]
pub struct SessionConfig {
    /// Session timeout duration (default: 60 minutes)
    pub timeout: Duration,
    /// Reset keyword list (exact match, case-insensitive, Unicode-aware)
    pub reset_keywords: Vec<String>,
    /// Reset command prefix (messages starting with this are treated as commands, e.g. "/")
    pub command_prefix: Option<String>,
    /// Command name list (used with command_prefix, e.g. ["reset", "clear", "new"])
    pub reset_commands: Vec<String>,
    /// Reply text after reset
    pub reset_reply: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60 * 60), // 1 hour
            reset_keywords: vec![
                "reset chat".into(),
                "new chat".into(),
                "clear memory".into(),
                "start over".into(),
            ],
            command_prefix: Some("/".into()),
            reset_commands: vec!["reset".into(), "clear".into(), "new".into()],
            reset_reply: "Conversation has been reset. You may start a new conversation.".into(),
        }
    }
}

impl SessionConfig {
    /// Set timeout in minutes
    pub fn with_timeout_minutes(mut self, minutes: u64) -> Self {
        self.timeout = Duration::from_secs(minutes * 60);
        self
    }

    /// Set timeout duration
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set reset keywords (replaces defaults)
    pub fn with_reset_keywords(mut self, keywords: Vec<String>) -> Self {
        self.reset_keywords = keywords;
        self
    }

    /// Add a reset keyword
    pub fn add_reset_keyword(mut self, keyword: impl Into<String>) -> Self {
        self.reset_keywords.push(keyword.into());
        self
    }

    /// Set the command prefix (None disables command mode)
    pub fn with_command_prefix(mut self, prefix: Option<String>) -> Self {
        self.command_prefix = prefix;
        self
    }

    /// Set reset command list (replaces defaults)
    pub fn with_reset_commands(mut self, commands: Vec<String>) -> Self {
        self.reset_commands = commands;
        self
    }

    /// Set the reply text after reset
    pub fn with_reset_reply(mut self, reply: impl Into<String>) -> Self {
        self.reset_reply = reply.into();
        self
    }

    /// Check if the text is a reset command.
    ///
    /// Uses `to_lowercase()` for Unicode-safe case-insensitive comparison,
    /// correctly supporting non-ASCII keywords (`eq_ignore_ascii_case` does not apply to non-ASCII characters).
    pub fn is_reset(&self, text: &str) -> bool {
        let trimmed = text.trim();
        let lower = trimmed.to_lowercase();

        // Keyword match (Unicode-safe)
        if self
            .reset_keywords
            .iter()
            .any(|kw| lower == kw.to_lowercase())
        {
            return true;
        }

        // Command prefix match
        if let Some(ref prefix) = self.command_prefix
            && let Some(cmd) = trimmed.strip_prefix(prefix.as_str())
        {
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

        false
    }
}

// ── Session ──────────────────────────────────────────────────────────────────

/// Session key: (channel_id, sender_id)
type SessionKey = (String, String);

/// Single user session
struct Session {
    handler: Box<dyn MessageHandler>,
    last_active: Instant,
}

// ── SessionFactory ───────────────────────────────────────────────────────────

/// Session factory — creates new MessageHandler instances.
///
/// Called whenever a new session is needed, returning a brand new MessageHandler.
/// Users typically create an Agent inside a closure and wrap it as a MessageHandler.
pub trait SessionFactory: Send + Sync {
    /// Create a new message handler (new session)
    fn create(&self) -> Box<dyn MessageHandler>;
}

/// Closure-based SessionFactory implementation
impl<F> SessionFactory for F
where
    F: Fn() -> Box<dyn MessageHandler> + Send + Sync,
{
    fn create(&self) -> Box<dyn MessageHandler> {
        self()
    }
}

// ── SessionHandler ───────────────────────────────────────────────────────────

/// Session end callback parameters
pub struct SessionEndInfo {
    pub channel_id: String,
    pub sender_id: String,
    pub reason: SessionEndReason,
}

/// Reason for session ending
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEndReason {
    /// Reset by user command
    CommandReset,
    /// Replaced after timeout
    TimeoutReplaced,
}

/// Session management handler.
///
/// Wraps a SessionFactory, maintaining independent MessageHandler instances per user.
/// Automatically handles timeout resets and keyword-based resets.
pub struct SessionHandler {
    config: SessionConfig,
    factory: Arc<dyn SessionFactory>,
    sessions: DashMap<SessionKey, Arc<Mutex<Session>>>,
    on_session_end: Option<Arc<dyn Fn(SessionEndInfo) + Send + Sync>>,
}

impl SessionHandler {
    /// Create a SessionHandler.
    ///
    /// - `config`: Session configuration
    /// - `factory`: Factory function for creating new sessions
    pub fn new(config: SessionConfig, factory: impl SessionFactory + 'static) -> Self {
        Self {
            config,
            factory: Arc::new(factory),
            sessions: DashMap::new(),
            on_session_end: None,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(factory: impl SessionFactory + 'static) -> Self {
        Self::new(SessionConfig::default(), factory)
    }

    /// Set session end callback (for resource cleanup, etc.)
    pub fn with_on_session_end<F>(mut self, callback: F) -> Self
    where
        F: Fn(SessionEndInfo) + Send + Sync + 'static,
    {
        self.on_session_end = Some(Arc::new(callback));
        self
    }

    /// Get the current number of active sessions
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }

    /// Get or create a session (atomic operation, uses DashMap entry API to prevent race conditions)
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

        // ── Keyword/Command Reset ──────────────────────────────────────────────
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

        // ── Get or create session (atomic) ──────────────────────────────────
        let session = self.get_or_create(&key);
        let mut guard = session.lock().await;

        // ── Timeout Reset ────────────────────────────────────────────────────
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

        // ── Forward to actual handler ────────────────────────────────────────
        guard.handler.handle(msg).await
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
        // reply is handled by the channel wrapper; passthrough here
        // No additional operations needed at the SessionHandler level
        Ok(())
    }
}
