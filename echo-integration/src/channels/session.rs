//! Session Manager — IM channel session management
//!
//! Provides framework-level session lifecycle management:
//! - Maintains independent sessions per user and conversation
//!   (isolated by channel_id + chat_id + sender_id)
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
//! let handler = SessionHandler::new(config, |_instance| -> Box<dyn MessageHandler> {
//!     Box::new(DummyHandler)
//! });
//! ```

use super::types::*;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OwnedMutexGuard};

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
        self.timeout = Duration::from_secs(minutes.saturating_mul(60));
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

/// Stable identity for one user's session within one channel conversation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SessionKey {
    channel_id: String,
    conversation_id: String,
    sender_id: String,
}

/// One concrete incarnation of a sender-scoped channel session.
///
/// The channel coordinates remain stable while `incarnation_id` changes every
/// time [`SessionHandler`] creates or replaces the inner message handler. The
/// opaque identifier lets consumers isolate ephemeral model/runtime state from
/// product conversation history without reimplementing session lifecycle.
#[derive(Clone, Debug)]
pub struct ChannelSessionInstance {
    channel_id: String,
    conversation_id: String,
    sender_id: String,
    incarnation_id: Arc<StdMutex<String>>,
    previous_incarnation_id: Option<String>,
}

/// Atomic result of rotating one channel session to a new incarnation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct ChannelSessionRotation {
    previous_incarnation_id: String,
    incarnation_id: String,
}

impl ChannelSessionRotation {
    /// Incarnation that must be settled and retired before new admission.
    pub fn previous_incarnation_id(&self) -> &str {
        &self.previous_incarnation_id
    }

    /// Newly-authoritative incarnation for subsequent runtime state.
    pub fn incarnation_id(&self) -> &str {
        &self.incarnation_id
    }
}

impl ChannelSessionInstance {
    fn from_key(key: &SessionKey, previous_incarnation_id: Option<String>) -> Self {
        Self {
            channel_id: key.channel_id.clone(),
            conversation_id: key.conversation_id.clone(),
            sender_id: key.sender_id.clone(),
            incarnation_id: Arc::new(StdMutex::new(uuid::Uuid::new_v4().to_string())),
            previous_incarnation_id,
        }
    }

    fn lock_incarnation(&self) -> StdMutexGuard<'_, String> {
        match self.incarnation_id.lock() {
            Ok(incarnation_id) => incarnation_id,
            Err(poisoned) => {
                tracing::error!("channel session incarnation mutex was poisoned");
                poisoned.into_inner()
            }
        }
    }

    /// Stable channel identifier for this session scope.
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    /// Stable channel conversation identifier for this session scope.
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Stable sender identifier for this session scope.
    pub fn sender_id(&self) -> &str {
        &self.sender_id
    }

    /// Opaque identity for this concrete handler incarnation.
    pub fn incarnation_id(&self) -> String {
        self.lock_incarnation().clone()
    }

    /// Incarnation replaced directly by this factory-created instance.
    ///
    /// A pruned session has no immediate replacement; consumers should use
    /// [`SessionEndInfo`] to retain any cleanup obligation until the sender
    /// returns.
    pub fn previous_incarnation_id(&self) -> Option<&str> {
        self.previous_incarnation_id.as_deref()
    }

    /// Atomically rotate this validated sender scope to a new incarnation.
    ///
    /// This is intended for application-owned reset commands that must settle
    /// product work before rotating model/runtime state themselves. All clones
    /// of this instance and the framework end callback observe the new value.
    pub fn rotate(&self) -> ChannelSessionRotation {
        let incarnation_id = uuid::Uuid::new_v4().to_string();
        let mut current = self.lock_incarnation();
        let previous_incarnation_id = std::mem::replace(&mut *current, incarnation_id.clone());
        ChannelSessionRotation {
            previous_incarnation_id,
            incarnation_id,
        }
    }
}

impl SessionKey {
    fn from_message(msg: &InboundMessage) -> echo_core::error::Result<Self> {
        let channel_id = require_channel_id(Some(&msg.channel_id))?;
        let conversation_id = require_conversation_id(Some(msg.conversation_id()))?;
        let sender_id = require_sender_id(Some(&msg.sender_id))?;
        Ok(Self {
            channel_id: channel_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sender_id: sender_id.to_string(),
        })
    }
}

/// Single user session
struct Session {
    handler: Arc<dyn MessageHandler>,
    instance: ChannelSessionInstance,
    generation: Arc<SessionGeneration>,
}

struct SessionGeneration {
    state: StdMutex<SessionGenerationState>,
}

struct SessionGenerationState {
    active_streams: usize,
    last_active: Instant,
}

impl SessionGeneration {
    fn new() -> Self {
        Self {
            state: StdMutex::new(SessionGenerationState {
                active_streams: 0,
                last_active: Instant::now(),
            }),
        }
    }

    fn lock_state(&self) -> StdMutexGuard<'_, SessionGenerationState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                tracing::error!("session generation lifecycle mutex was poisoned");
                poisoned.into_inner()
            }
        }
    }

    fn begin(self: &Arc<Self>) -> Option<SessionStreamReceipt> {
        let mut state = self.lock_state();
        state.active_streams = state.active_streams.checked_add(1)?;
        state.last_active = Instant::now();
        drop(state);
        Some(SessionStreamReceipt {
            generation: Arc::clone(self),
        })
    }

    fn is_idle_and_expired(&self, timeout: Duration) -> bool {
        let state = self.lock_state();
        state.active_streams == 0 && state.last_active.elapsed() >= timeout
    }

    fn touch(&self) {
        self.lock_state().last_active = Instant::now();
    }
}

/// Exact ownership for one inner stream setup/consumption lifetime.
///
/// The counter is independent of the async session mutex so dropping an
/// unconsumed or partially-consumed stream releases admission synchronously.
struct SessionStreamReceipt {
    generation: Arc<SessionGeneration>,
}

impl Drop for SessionStreamReceipt {
    fn drop(&mut self) {
        let mut state = self.generation.lock_state();
        state.last_active = Instant::now();
        match state.active_streams.checked_sub(1) {
            Some(active_streams) => state.active_streams = active_streams,
            None => tracing::error!("session stream activity receipt underflow"),
        }
    }
}

// ── SessionFactory ───────────────────────────────────────────────────────────

/// Session factory — creates new MessageHandler instances.
///
/// Called whenever a new session is needed, returning a brand new MessageHandler.
/// Users typically create an Agent inside a closure and wrap it as a MessageHandler.
pub trait SessionFactory: Send + Sync {
    /// Create a new message handler for one concrete session incarnation.
    fn create(&self, instance: &ChannelSessionInstance) -> Box<dyn MessageHandler>;
}

/// Closure-based SessionFactory implementation
impl<F> SessionFactory for F
where
    F: Fn(&ChannelSessionInstance) -> Box<dyn MessageHandler> + Send + Sync,
{
    fn create(&self, instance: &ChannelSessionInstance) -> Box<dyn MessageHandler> {
        self(instance)
    }
}

// ── SessionHandler ───────────────────────────────────────────────────────────

/// Session end callback parameters
pub struct SessionEndInfo {
    pub channel_id: String,
    pub chat_id: String,
    pub sender_id: String,
    /// Opaque identity of the handler incarnation that ended.
    pub incarnation_id: String,
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

    /// Get the number of currently retained sender-scoped sessions.
    ///
    /// Each distinct `(channel_id, conversation_id, sender_id)` identity counts
    /// separately, including different participants in the same group chat.
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }

    fn create_session(&self, key: &SessionKey, previous_incarnation_id: Option<String>) -> Session {
        let instance = ChannelSessionInstance::from_key(key, previous_incarnation_id);
        let handler = Arc::from(self.factory.create(&instance));
        Session {
            handler,
            instance,
            generation: Arc::new(SessionGeneration::new()),
        }
    }

    /// Get or create a session (atomic operation, uses DashMap entry API to prevent race conditions)
    fn get_or_create(&self, key: &SessionKey) -> Arc<Mutex<Session>> {
        self.sessions
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(self.create_session(key, None))))
            .clone()
    }

    /// Lock the authoritative map entry. A timeout prune can remove a session
    /// after `get_or_create` returns but before its async mutex is acquired, so
    /// identity must be rechecked under the same lock used by pruning.
    async fn lock_current_session(&self, key: &SessionKey) -> OwnedMutexGuard<Session> {
        loop {
            let session = self.get_or_create(key);
            let guard = Arc::clone(&session).lock_owned().await;
            let is_current = self
                .sessions
                .get(key)
                .map(|current| Arc::ptr_eq(current.value(), &session))
                .unwrap_or(false);
            if is_current {
                return guard;
            }
        }
    }

    fn notify_session_end(&self, instance: ChannelSessionInstance, reason: SessionEndReason) {
        if let Some(ref callback) = self.on_session_end {
            let incarnation_id = instance.incarnation_id();
            callback(SessionEndInfo {
                channel_id: instance.channel_id,
                chat_id: instance.conversation_id,
                sender_id: instance.sender_id,
                incarnation_id,
                reason,
            });
        }
    }

    async fn prune_expired_except(&self, requested_key: &SessionKey) {
        let sessions = self
            .sessions
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect::<Vec<_>>();
        for (key, session) in sessions {
            // The requested key must be replaced under its own session mutex so
            // the new factory call receives the exact previous incarnation.
            if &key == requested_key {
                continue;
            }
            let Some(guard) = session.try_lock().ok() else {
                continue;
            };
            if !guard.generation.is_idle_and_expired(self.config.timeout) {
                continue;
            }
            let ended_instance = guard.instance.clone();
            let removed = self
                .sessions
                .remove_if(&key, |_, current| Arc::ptr_eq(current, &session))
                .is_some();
            drop(guard);
            if removed {
                self.notify_session_end(ended_instance, SessionEndReason::TimeoutReplaced);
            }
        }
    }
}

#[async_trait]
impl MessageHandler for SessionHandler {
    async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
        let key = SessionKey::from_message(&msg)?;
        self.prune_expired_except(&key).await;
        let mut guard = self.lock_current_session(&key).await;
        if self.config.is_reset(&msg.text) {
            let ended_instance = guard.instance.clone();
            *guard = self.create_session(&key, Some(ended_instance.incarnation_id()));
            drop(guard);
            self.notify_session_end(ended_instance, SessionEndReason::CommandReset);
            return Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                &self.config.reset_reply,
            ));
        }
        if guard.generation.is_idle_and_expired(self.config.timeout) {
            let ended_instance = guard.instance.clone();
            *guard = self.create_session(&key, Some(ended_instance.incarnation_id()));
            self.notify_session_end(ended_instance, SessionEndReason::TimeoutReplaced);
        }
        guard.generation.touch();
        let result = guard.handler.handle(msg).await;
        guard.generation.touch();
        result
    }

    async fn handle_stream<'a>(
        &'a self,
        msg: InboundMessage,
    ) -> echo_core::error::Result<
        futures::stream::BoxStream<'a, echo_core::error::Result<OutboundMessage>>,
    > {
        use futures::stream::StreamExt;
        let key = SessionKey::from_message(&msg)?;
        self.prune_expired_except(&key).await;
        let mut guard = self.lock_current_session(&key).await;
        if self.config.is_reset(&msg.text) {
            let ended_instance = guard.instance.clone();
            *guard = self.create_session(&key, Some(ended_instance.incarnation_id()));
            drop(guard);
            self.notify_session_end(ended_instance, SessionEndReason::CommandReset);
            let reply = OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                &self.config.reset_reply,
            );
            return Ok(futures::stream::once(async move { Ok(reply) }).boxed());
        }

        let timeout_replaced = guard.generation.is_idle_and_expired(self.config.timeout);
        let ended_instance = timeout_replaced.then(|| guard.instance.clone());
        if timeout_replaced {
            *guard = self.create_session(
                &key,
                ended_instance
                    .as_ref()
                    .map(ChannelSessionInstance::incarnation_id),
            );
        }
        let handler = guard.handler.clone();
        let Some(stream_receipt) = guard.generation.begin() else {
            return Err(echo_core::error::ReactError::Other(
                "session active stream capacity exhausted".to_string(),
            ));
        };
        drop(guard);
        if let Some(ended_instance) = ended_instance {
            self.notify_session_end(ended_instance, SessionEndReason::TimeoutReplaced);
        }

        let stream = async_stream::stream! {
            let mut stream_receipt = Some(stream_receipt);
            let mut inner = match handler.handle_stream(msg).await {
                Ok(stream) => stream,
                Err(error) => {
                    drop(stream_receipt.take());
                    yield Err(error);
                    return;
                }
            };
            while let Some(item) = inner.next().await {
                if item.is_err() {
                    drop(inner);
                    drop(stream_receipt.take());
                    yield item;
                    return;
                }
                yield item;
            }
            drop(inner);
            drop(stream_receipt.take());
        };
        Ok(stream.boxed())
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
        // reply is handled by the channel wrapper; passthrough here
        // No additional operations needed at the SessionHandler level
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use echo_core::error::ReactError;
    use futures::stream::{BoxStream, StreamExt};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use tokio::sync::Notify;
    use tokio::time::timeout;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    /// 测试用 inner handler:override handle_stream 产 2 条分段,记录调用次数。
    struct TwoChunkHandler {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl MessageHandler for TwoChunkHandler {
        async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                "fallback",
            ))
        }
        async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
            Ok(())
        }
        async fn handle_stream<'a>(
            &'a self,
            msg: InboundMessage,
        ) -> echo_core::error::Result<BoxStream<'a, echo_core::error::Result<OutboundMessage>>>
        {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let (ch, to, ct) = (msg.channel_id, msg.chat_id, msg.chat_type);
            let s = futures::stream::iter(vec![
                Ok(OutboundMessage::new(&ch, &to, ct, "chunk1")),
                Ok(OutboundMessage::new(&ch, &to, ct, "chunk2")),
            ]);
            Ok(s.boxed())
        }
    }

    struct TwoChunkFactory {
        counter: Arc<AtomicUsize>,
    }
    impl SessionFactory for TwoChunkFactory {
        fn create(&self, _instance: &ChannelSessionInstance) -> Box<dyn MessageHandler> {
            Box::new(TwoChunkHandler {
                call_count: self.counter.clone(),
            })
        }
    }

    struct ConcurrentStreamHandler {
        parked_started: Arc<Notify>,
        release_parked: Arc<Notify>,
    }

    #[async_trait]
    impl MessageHandler for ConcurrentStreamHandler {
        async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                &msg.text,
            ))
        }

        async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
            Ok(())
        }

        async fn handle_stream<'a>(
            &'a self,
            msg: InboundMessage,
        ) -> echo_core::error::Result<BoxStream<'a, echo_core::error::Result<OutboundMessage>>>
        {
            let channel_id = msg.channel_id;
            let chat_id = msg.chat_id;
            let chat_type = msg.chat_type;
            let text = msg.text;
            if text == "park" {
                let parked_started = self.parked_started.clone();
                let release_parked = self.release_parked.clone();
                return Ok(async_stream::stream! {
                    parked_started.notify_one();
                    release_parked.notified().await;
                    yield Ok(OutboundMessage::new(
                        &channel_id,
                        &chat_id,
                        chat_type,
                        "parked-complete",
                    ));
                }
                .boxed());
            }
            if text == "fail" {
                return Err(ReactError::Other("stream setup failed".to_string()));
            }
            if text == "empty" {
                return Ok(futures::stream::empty().boxed());
            }
            Ok(futures::stream::once(async move {
                Ok(OutboundMessage::new(
                    &channel_id,
                    &chat_id,
                    chat_type,
                    &text,
                ))
            })
            .boxed())
        }
    }

    struct ConcurrentStreamFactory {
        parked_started: Arc<Notify>,
        release_parked: Arc<Notify>,
    }

    impl SessionFactory for ConcurrentStreamFactory {
        fn create(&self, _instance: &ChannelSessionInstance) -> Box<dyn MessageHandler> {
            Box::new(ConcurrentStreamHandler {
                parked_started: self.parked_started.clone(),
                release_parked: self.release_parked.clone(),
            })
        }
    }

    struct GenerationalStreamHandler {
        generation: usize,
        hitl_pending: AtomicBool,
        parked_started: Arc<Notify>,
        release_parked: Arc<Notify>,
    }

    struct PanicStream;

    impl futures::Stream for PanicStream {
        type Item = echo_core::error::Result<OutboundMessage>;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            // Deliberate test-only unwind validates receipt cleanup during panic propagation.
            std::panic::resume_unwind(Box::new("session stream panic fixture"))
        }
    }

    #[async_trait]
    impl MessageHandler for GenerationalStreamHandler {
        async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                format!("generation-{}:{}", self.generation, msg.text),
            ))
        }

        async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
            Ok(())
        }

        async fn handle_stream<'a>(
            &'a self,
            msg: InboundMessage,
        ) -> echo_core::error::Result<BoxStream<'a, echo_core::error::Result<OutboundMessage>>>
        {
            if msg.text == "fail" {
                return Err(ReactError::Other("stream setup failed".to_string()));
            }
            if msg.text == "panic" {
                return Ok(PanicStream.boxed());
            }

            let generation = self.generation;
            let channel_id = msg.channel_id;
            let chat_id = msg.chat_id;
            let chat_type = msg.chat_type;
            let text = msg.text;
            if text == "park" {
                self.hitl_pending.store(true, Ordering::Release);
                let hitl_pending = &self.hitl_pending;
                let parked_started = self.parked_started.clone();
                let release_parked = self.release_parked.clone();
                return Ok(async_stream::stream! {
                    parked_started.notify_one();
                    release_parked.notified().await;
                    hitl_pending.store(false, Ordering::Release);
                    yield Ok(OutboundMessage::new(
                        &channel_id,
                        &chat_id,
                        chat_type,
                        format!("generation-{generation}:parked-complete"),
                    ));
                }
                .boxed());
            }
            if text == "item-fail" {
                return Ok(futures::stream::once(async {
                    Err(ReactError::Other("stream item failed".to_string()))
                })
                .boxed());
            }

            let output = if text == "state" && self.hitl_pending.load(Ordering::Acquire) {
                format!("generation-{generation}:hitl-pending")
            } else {
                format!("generation-{generation}:{text}")
            };

            Ok(futures::stream::once(async move {
                Ok(OutboundMessage::new(
                    &channel_id,
                    &chat_id,
                    chat_type,
                    output,
                ))
            })
            .boxed())
        }
    }

    struct GenerationalStreamFactory {
        created: Arc<AtomicUsize>,
        parked_started: Arc<Notify>,
        release_parked: Arc<Notify>,
    }

    impl SessionFactory for GenerationalStreamFactory {
        fn create(&self, _instance: &ChannelSessionInstance) -> Box<dyn MessageHandler> {
            let generation = self
                .created
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_add(1)
                })
                .map(|previous| previous.saturating_add(1))
                .unwrap_or(usize::MAX);
            Box::new(GenerationalStreamHandler {
                generation,
                hitl_pending: AtomicBool::new(false),
                parked_started: self.parked_started.clone(),
                release_parked: self.release_parked.clone(),
            })
        }
    }

    struct SenderScopedHandler {
        instance_id: usize,
        mode: StdMutex<&'static str>,
        hitl_pending: AtomicBool,
        parked_started: Arc<Notify>,
        release_parked: Arc<Notify>,
    }

    impl SenderScopedHandler {
        fn lock_mode(&self) -> StdMutexGuard<'_, &'static str> {
            match self.mode.lock() {
                Ok(mode) => mode,
                Err(poisoned) => poisoned.into_inner(),
            }
        }

        fn state_text(&self) -> String {
            format!(
                "handler={};mode={};hitl={}",
                self.instance_id,
                *self.lock_mode(),
                self.hitl_pending.load(Ordering::Acquire)
            )
        }
    }

    #[async_trait]
    impl MessageHandler for SenderScopedHandler {
        async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
            match msg.text.as_str() {
                "park-handle" => {
                    self.parked_started.notify_one();
                    self.release_parked.notified().await;
                }
                "task-hitl" => {
                    *self.lock_mode() = "task";
                    self.hitl_pending.store(true, Ordering::Release);
                }
                "review" => {
                    *self.lock_mode() = "review";
                    self.hitl_pending.store(false, Ordering::Release);
                }
                _ => {}
            }
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                self.state_text(),
            ))
        }

        async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
            Ok(())
        }
    }

    struct SenderScopedFactory {
        created: Arc<AtomicUsize>,
        parked_started: Arc<Notify>,
        release_parked: Arc<Notify>,
    }

    impl SessionFactory for SenderScopedFactory {
        fn create(&self, _instance: &ChannelSessionInstance) -> Box<dyn MessageHandler> {
            let instance_id = self
                .created
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_add(1)
                })
                .map(|previous| previous.saturating_add(1))
                .unwrap_or(usize::MAX);
            Box::new(SenderScopedHandler {
                instance_id,
                mode: StdMutex::new("chat"),
                hitl_pending: AtomicBool::new(false),
                parked_started: self.parked_started.clone(),
                release_parked: self.release_parked.clone(),
            })
        }
    }

    fn group_message(sender_id: &str, text: &str, message_id: &str) -> InboundMessage {
        scoped_group_message("qq", sender_id, "shared-group", text, message_id)
    }

    fn scoped_group_message(
        channel_id: &str,
        sender_id: &str,
        conversation_id: &str,
        text: &str,
        message_id: &str,
    ) -> InboundMessage {
        InboundMessage::new(
            channel_id,
            sender_id,
            conversation_id,
            ChatType::Group,
            text,
            message_id,
        )
    }

    async fn group_handle_text(
        handler: &SessionHandler,
        sender_id: &str,
        text: &str,
        message_id: &str,
    ) -> Result<String, String> {
        handler
            .handle(group_message(sender_id, text, message_id))
            .await
            .map(|message| message.text)
            .map_err(|error| error.to_string())
    }

    async fn group_stream_text(
        handler: &SessionHandler,
        sender_id: &str,
        text: &str,
        message_id: &str,
    ) -> Result<String, String> {
        stream_message_text(handler, group_message(sender_id, text, message_id)).await
    }

    async fn stream_message_text(
        handler: &SessionHandler,
        message: InboundMessage,
    ) -> Result<String, String> {
        let sender_id = message.sender_id.clone();
        let mut stream = handler
            .handle_stream(message)
            .await
            .map_err(|error| error.to_string())?;
        let message = timeout(TEST_TIMEOUT, stream.next())
            .await
            .map_err(|_| format!("group stream timed out for {sender_id}"))?
            .ok_or_else(|| format!("group stream closed without output for {sender_id}"))?
            .map_err(|error| error.to_string())?;
        if timeout(TEST_TIMEOUT, stream.next())
            .await
            .map_err(|_| format!("group stream did not close for {sender_id}"))?
            .is_some()
        {
            return Err(format!(
                "group stream returned extra output for {sender_id}"
            ));
        }
        Ok(message.text)
    }

    async fn generation_for_message(
        handler: &SessionHandler,
        message: &InboundMessage,
    ) -> Result<Arc<SessionGeneration>, String> {
        let key = SessionKey::from_message(message).map_err(|error| error.to_string())?;
        let session = handler
            .sessions
            .get(&key)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| "scoped test session was not registered".to_string())?;
        let guard = session.lock().await;
        Ok(Arc::clone(&guard.generation))
    }

    fn sender_scoped_handler() -> (
        Arc<SessionHandler>,
        Arc<AtomicUsize>,
        Arc<Notify>,
        Arc<Notify>,
    ) {
        let created = Arc::new(AtomicUsize::new(0));
        let parked_started = Arc::new(Notify::new());
        let release_parked = Arc::new(Notify::new());
        let handler = Arc::new(SessionHandler::new(
            SessionConfig::default(),
            SenderScopedFactory {
                created: created.clone(),
                parked_started: parked_started.clone(),
                release_parked: release_parked.clone(),
            },
        ));
        (handler, created, parked_started, release_parked)
    }

    fn test_message(text: &str, message_id: &str) -> InboundMessage {
        InboundMessage::new("qq", "u1", "c1", ChatType::Direct, text, message_id)
    }

    async fn single_stream_text(
        handler: &SessionHandler,
        text: &str,
        message_id: &str,
    ) -> Result<String, String> {
        let mut stream = handler
            .handle_stream(test_message(text, message_id))
            .await
            .map_err(|error| error.to_string())?;
        let first = timeout(TEST_TIMEOUT, stream.next())
            .await
            .map_err(|_| format!("stream timed out for {text}"))?
            .ok_or_else(|| format!("stream closed without an item for {text}"))?
            .map_err(|error| error.to_string())?;
        let trailing = timeout(TEST_TIMEOUT, stream.next())
            .await
            .map_err(|_| format!("stream did not close for {text}"))?;
        if trailing.is_some() {
            return Err(format!("stream returned more than one item for {text}"));
        }
        Ok(first.text)
    }

    async fn parked_stream_text(handler: Arc<SessionHandler>) -> Result<String, String> {
        let mut stream = handler
            .handle_stream(test_message("park", "parked"))
            .await
            .map_err(|error| error.to_string())?;
        let first = stream
            .next()
            .await
            .ok_or_else(|| "parked stream closed without an item".to_string())?
            .map_err(|error| error.to_string())?;
        if stream.next().await.is_some() {
            return Err("parked stream returned more than one item".to_string());
        }
        Ok(first.text)
    }

    async fn current_test_generation(
        handler: &SessionHandler,
    ) -> Result<Arc<SessionGeneration>, String> {
        let key = SessionKey {
            channel_id: "qq".to_string(),
            conversation_id: "c1".to_string(),
            sender_id: "u1".to_string(),
        };
        let session = handler
            .sessions
            .get(&key)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| "test session was not registered".to_string())?;
        let guard = session.lock().await;
        Ok(Arc::clone(&guard.generation))
    }

    fn mark_generation_expired(
        generation: &SessionGeneration,
        timeout: Duration,
    ) -> Result<Instant, String> {
        let expired_at = Instant::now()
            .checked_sub(timeout.saturating_add(Duration::from_secs(1)))
            .ok_or_else(|| "test clock could not represent an expired session".to_string())?;
        generation.lock_state().last_active = expired_at;
        Ok(expired_at)
    }

    fn generation_snapshot(generation: &SessionGeneration) -> (usize, Instant) {
        let state = generation.lock_state();
        (state.active_streams, state.last_active)
    }

    async fn mark_test_session_expired(handler: &SessionHandler) -> Result<Instant, String> {
        let generation = current_test_generation(handler).await?;
        mark_generation_expired(&generation, handler.config.timeout)
    }

    #[tokio::test]
    async fn session_handler_handle_stream_forwards_inner_chunks() -> Result<(), String> {
        let counter = Arc::new(AtomicUsize::new(0));
        let sh = SessionHandler::new(
            SessionConfig::default(),
            TwoChunkFactory {
                counter: counter.clone(),
            },
        );

        // 正常消息:应透传到 inner 的 handle_stream,产 2 条
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "hi", "m1");
        let mut stream = sh
            .handle_stream(msg)
            .await
            .map_err(|error| error.to_string())?;
        let c1 = stream
            .next()
            .await
            .ok_or_else(|| "missing first chunk".to_string())?
            .map_err(|error| error.to_string())?;
        let c2 = stream
            .next()
            .await
            .ok_or_else(|| "missing second chunk".to_string())?
            .map_err(|error| error.to_string())?;
        if c1.text != "chunk1" || c2.text != "chunk2" {
            return Err("inner stream chunks were not forwarded in order".to_string());
        }
        if stream.next().await.is_some() {
            return Err("inner stream returned an unexpected third chunk".to_string());
        }
        if counter.load(Ordering::SeqCst) != 1 {
            return Err("inner handle_stream was not called exactly once".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn session_handler_handle_stream_reset_short_circuits() -> Result<(), String> {
        let counter = Arc::new(AtomicUsize::new(0));
        let sh = SessionHandler::new(
            SessionConfig::default(),
            TwoChunkFactory {
                counter: counter.clone(),
            },
        );

        // reset 命令(默认 reset_keywords 含 "reset chat"):短路返 reset_reply,不调 inner
        let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "reset chat", "m1");
        let mut stream = sh
            .handle_stream(msg)
            .await
            .map_err(|error| error.to_string())?;
        let only = stream
            .next()
            .await
            .ok_or_else(|| "reset stream closed without a reply".to_string())?
            .map_err(|error| error.to_string())?;
        if only.text != SessionConfig::default().reset_reply {
            return Err("reset stream returned the wrong reply".to_string());
        }
        if stream.next().await.is_some() {
            return Err("reset stream returned an unexpected second item".to_string());
        }
        if counter.load(Ordering::SeqCst) != 0 {
            return Err("reset command reached the inner handler".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_group_conversation_uses_one_session_per_sender() -> Result<(), String> {
        let (handler, created, _, _) = sender_scoped_handler();

        let alice = group_stream_text(&handler, "alice", "task-hitl", "m1").await?;
        let bob = group_stream_text(&handler, "bob", "state", "m2").await?;
        if alice != "handler=1;mode=task;hitl=true" {
            return Err(format!(
                "alice state was not retained by her handler: {alice}"
            ));
        }
        if bob != "handler=2;mode=chat;hitl=false" {
            return Err(format!("bob inherited alice mode or HITL state: {bob}"));
        }
        if handler.active_sessions() != 2 || created.load(Ordering::Acquire) != 2 {
            return Err("same group conversation did not create two sender sessions".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn different_group_senders_do_not_share_the_session_mutex() -> Result<(), String> {
        let (handler, _, parked_started, release_parked) = sender_scoped_handler();
        let alice_handler = handler.clone();
        let alice = tokio::spawn(async move {
            alice_handler
                .handle(group_message("alice", "park-handle", "m1"))
                .await
                .map_err(|error| error.to_string())
        });
        timeout(TEST_TIMEOUT, parked_started.notified())
            .await
            .map_err(|_| "alice did not acquire and park her session mutex".to_string())?;

        let bob = timeout(
            TEST_TIMEOUT,
            group_handle_text(&handler, "bob", "state", "m2"),
        )
        .await
        .map_err(|_| "bob was blocked by alice's session mutex".to_string())??;
        if bob != "handler=2;mode=chat;hitl=false" {
            return Err(format!("bob ran against the wrong handler: {bob}"));
        }

        release_parked.notify_one();
        let alice = timeout(TEST_TIMEOUT, alice)
            .await
            .map_err(|_| "alice did not finish after release".to_string())?
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if alice.text != "handler=1;mode=chat;hitl=false" {
            return Err(format!(
                "alice resumed with the wrong handler: {}",
                alice.text
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn sender_reset_does_not_replace_another_group_member_session() -> Result<(), String> {
        let (handler, created, _, _) = sender_scoped_handler();
        let alice_before = group_stream_text(&handler, "alice", "task-hitl", "m1").await?;
        let bob_before = group_stream_text(&handler, "bob", "review", "m2").await?;
        if alice_before != "handler=1;mode=task;hitl=true"
            || bob_before != "handler=2;mode=review;hitl=false"
        {
            return Err("test sessions were not initialized independently".to_string());
        }

        let reset = group_stream_text(&handler, "alice", "reset chat", "m3").await?;
        if reset != handler.config.reset_reply {
            return Err("alice reset did not return the configured reply".to_string());
        }
        let alice_after = group_stream_text(&handler, "alice", "state", "m4").await?;
        let bob_after = group_stream_text(&handler, "bob", "state", "m5").await?;
        if alice_after != "handler=3;mode=chat;hitl=false" {
            return Err(format!(
                "alice reset did not create a clean handler: {alice_after}"
            ));
        }
        if bob_after != "handler=2;mode=review;hitl=false" {
            return Err(format!("alice reset replaced bob's handler: {bob_after}"));
        }
        if handler.active_sessions() != 2 || created.load(Ordering::Acquire) != 3 {
            return Err("sender reset changed the group session cardinality".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_sender_reuses_mode_and_hitl_session_state() -> Result<(), String> {
        let (handler, created, _, _) = sender_scoped_handler();
        let initial = group_stream_text(&handler, "alice", "task-hitl", "m1").await?;
        let reused = group_stream_text(&handler, "alice", "state", "m2").await?;
        if initial != "handler=1;mode=task;hitl=true" || reused != initial {
            return Err(format!(
                "same sender did not reuse state: {initial} -> {reused}"
            ));
        }
        if handler.active_sessions() != 1 || created.load(Ordering::Acquire) != 1 {
            return Err("same sender created more than one session".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn application_rotation_remains_authoritative_through_timeout_replacement()
    -> Result<(), String> {
        let instances = Arc::new(StdMutex::new(Vec::<ChannelSessionInstance>::new()));
        let factory_instances = Arc::clone(&instances);
        let ended = Arc::new(StdMutex::new(Vec::<String>::new()));
        let callback_ended = Arc::clone(&ended);
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            move |instance: &ChannelSessionInstance| -> Box<dyn MessageHandler> {
                match factory_instances.lock() {
                    Ok(mut instances) => instances.push(instance.clone()),
                    Err(poisoned) => poisoned.into_inner().push(instance.clone()),
                }
                Box::new(TwoChunkHandler {
                    call_count: Arc::clone(&calls),
                })
            },
        )
        .with_on_session_end(move |info| match callback_ended.lock() {
            Ok(mut ended) => ended.push(info.incarnation_id),
            Err(poisoned) => poisoned.into_inner().push(info.incarnation_id),
        });

        handler
            .handle(test_message("first", "m1"))
            .await
            .map_err(|error| error.to_string())?;
        handler
            .handle(test_message("same", "m2"))
            .await
            .map_err(|error| error.to_string())?;
        let first_instance = match instances.lock() {
            Ok(instances) => instances
                .first()
                .cloned()
                .ok_or_else(|| "factory did not receive the first instance".to_string())?,
            Err(poisoned) => poisoned
                .into_inner()
                .first()
                .cloned()
                .ok_or_else(|| "factory did not receive the first instance".to_string())?,
        };
        let first_incarnation = first_instance.incarnation_id();
        let rotation = first_instance.rotate();
        if rotation.previous_incarnation_id() != first_incarnation
            || rotation.incarnation_id() == first_incarnation
            || first_instance.incarnation_id() != rotation.incarnation_id()
        {
            return Err("application rotation did not update the shared authority".to_string());
        }
        if match instances.lock() {
            Ok(instances) => instances.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        } != 1
        {
            return Err("same sender did not reuse its original instance".to_string());
        }

        mark_test_session_expired(&handler).await?;
        handler
            .handle(test_message("after-timeout", "m3"))
            .await
            .map_err(|error| error.to_string())?;
        let replacement_instance = match instances.lock() {
            Ok(instances) => instances.get(1).cloned(),
            Err(poisoned) => poisoned.into_inner().get(1).cloned(),
        }
        .ok_or_else(|| "timeout did not create a replacement instance".to_string())?;
        let replacement_incarnation = replacement_instance.incarnation_id();
        if replacement_incarnation == first_incarnation
            || replacement_incarnation == rotation.incarnation_id()
        {
            return Err("timeout reused an earlier incarnation".to_string());
        }
        if replacement_instance.previous_incarnation_id() != Some(rotation.incarnation_id()) {
            return Err("replacement factory lost the exact previous incarnation".to_string());
        }
        let ended_incarnations = match ended.lock() {
            Ok(ended) => ended.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if ended_incarnations.len() != 1
            || ended_incarnations.first().map(String::as_str) != Some(rotation.incarnation_id())
        {
            return Err(format!(
                "timeout callback reported stale incarnation: {ended_incarnations:?}"
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn missing_or_unknown_sender_identity_fails_closed() -> Result<(), String> {
        let (handler, created, _, _) = sender_scoped_handler();
        for sender_id in ["", "   ", "unknown", "UNKNOWN", " alice "] {
            let result = handler
                .handle_stream(group_message(sender_id, "state", "m1"))
                .await;
            if !matches!(result, Err(ReactError::Channel(ref error)) if matches!(error.as_ref(), echo_core::error::ChannelError::Other(message) if message.contains("sender_id")))
            {
                return Err(format!("invalid sender was not rejected: {sender_id:?}"));
            }
        }
        if handler.active_sessions() != 0 || created.load(Ordering::Acquire) != 0 {
            return Err("invalid sender created a session".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn empty_channel_or_conversation_identity_fails_closed() -> Result<(), String> {
        let (handler, created, _, _) = sender_scoped_handler();
        let messages = [
            scoped_group_message("", "alice", "shared-group", "state", "m1"),
            scoped_group_message("qq", "alice", "", "state", "m2"),
            scoped_group_message(" qq ", "alice", "shared-group", "state", "m3"),
            scoped_group_message("qq", "alice", " shared-group ", "state", "m4"),
        ];
        for message in messages {
            let result = handler.handle_stream(message).await;
            if !matches!(result, Err(ReactError::Channel(ref error)) if matches!(error.as_ref(), echo_core::error::ChannelError::Other(message) if message.contains("channel_id") || message.contains("conversation_id")))
            {
                return Err("empty channel coordinate did not fail closed".to_string());
            }
        }
        if handler.active_sessions() != 0 || created.load(Ordering::Acquire) != 0 {
            return Err("invalid channel coordinate created a session".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_sender_is_isolated_across_conversations_and_channels() -> Result<(), String> {
        let (handler, created, _, _) = sender_scoped_handler();
        let first = stream_message_text(
            &handler,
            scoped_group_message("qq", "alice", "group-1", "state", "m1"),
        )
        .await?;
        let conversation = stream_message_text(
            &handler,
            scoped_group_message("qq", "alice", "group-2", "state", "m2"),
        )
        .await?;
        let channel = stream_message_text(
            &handler,
            scoped_group_message("feishu", "alice", "group-1", "state", "m3"),
        )
        .await?;
        if first != "handler=1;mode=chat;hitl=false"
            || conversation != "handler=2;mode=chat;hitl=false"
            || channel != "handler=3;mode=chat;hitl=false"
        {
            return Err("channel or conversation was omitted from the session scope".to_string());
        }
        if handler.active_sessions() != 3 || created.load(Ordering::Acquire) != 3 {
            return Err("distinct channel conversations reused one sender session".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn timeout_prune_reports_each_scoped_session_original_sender() -> Result<(), String> {
        type EndRecord = (String, String, String, SessionEndReason);

        let records = Arc::new(StdMutex::new(Vec::<EndRecord>::new()));
        let callback_records = records.clone();
        let created = Arc::new(AtomicUsize::new(0));
        let handler = SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            SenderScopedFactory {
                created: created.clone(),
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        )
        .with_on_session_end(move |info| {
            let mut records = match callback_records.lock() {
                Ok(records) => records,
                Err(poisoned) => poisoned.into_inner(),
            };
            records.push((info.channel_id, info.chat_id, info.sender_id, info.reason));
        });

        let alice_message = group_message("alice", "state", "m1");
        let bob_message = group_message("bob", "state", "m2");
        stream_message_text(&handler, alice_message.clone()).await?;
        stream_message_text(&handler, bob_message.clone()).await?;
        for message in [&alice_message, &bob_message] {
            let generation = generation_for_message(&handler, message).await?;
            mark_generation_expired(&generation, handler.config.timeout)?;
        }

        group_stream_text(&handler, "carol", "state", "m4").await?;
        let records = match records.lock() {
            Ok(records) => records,
            Err(poisoned) => poisoned.into_inner(),
        };
        for sender_id in ["alice", "bob"] {
            if !records.iter().any(|record| {
                record.0 == "qq"
                    && record.1 == "shared-group"
                    && record.2 == sender_id
                    && record.3 == SessionEndReason::TimeoutReplaced
            }) {
                return Err(format!(
                    "timeout callback lost sender identity: {sender_id}"
                ));
            }
        }
        if records.len() != 2 {
            return Err(format!("timeout prune emitted {} callbacks", records.len()));
        }
        if handler.active_sessions() != 1 || created.load(Ordering::Acquire) != 3 {
            return Err(
                "timeout prune did not remove only the expired sender sessions".to_string(),
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn session_handler_allows_same_session_message_while_stream_is_parked() {
        let parked_started = Arc::new(Notify::new());
        let release_parked = Arc::new(Notify::new());
        let handler = Arc::new(SessionHandler::new(
            SessionConfig::default(),
            ConcurrentStreamFactory {
                parked_started: parked_started.clone(),
                release_parked: release_parked.clone(),
            },
        ));

        let first_handler = handler.clone();
        let first = tokio::spawn(async move {
            let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "park", "m1");
            match first_handler.handle_stream(msg).await {
                Ok(mut stream) => stream.next().await,
                Err(error) => Some(Err(error)),
            }
        });
        assert!(
            timeout(std::time::Duration::from_secs(2), parked_started.notified())
                .await
                .is_ok(),
            "first stream did not reach its parked state"
        );

        let second = timeout(std::time::Duration::from_secs(2), async {
            let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "control", "m2");
            match handler.handle_stream(msg).await {
                Ok(mut stream) => stream.next().await,
                Err(error) => Some(Err(error)),
            }
        })
        .await;
        assert!(
            matches!(second, Ok(Some(Ok(ref output))) if output.text == "control"),
            "same-session control message was blocked by the parked stream"
        );

        release_parked.notify_one();
        let first_result = timeout(std::time::Duration::from_secs(2), first).await;
        assert!(
            matches!(first_result, Ok(Ok(Some(Ok(ref output)))) if output.text == "parked-complete"),
            "parked stream did not close cleanly after release"
        );
    }

    #[tokio::test]
    async fn session_handler_remains_usable_after_stream_error_and_close() {
        let handler = SessionHandler::new(
            SessionConfig::default(),
            ConcurrentStreamFactory {
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        );

        let fail = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "fail", "m1");
        let failed = match handler.handle_stream(fail).await {
            Ok(mut stream) => stream.next().await,
            Err(error) => Some(Err(error)),
        };
        assert!(matches!(failed, Some(Err(ReactError::Other(_)))));

        let empty = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "empty", "m2");
        let closed = match handler.handle_stream(empty).await {
            Ok(mut stream) => stream.next().await,
            Err(error) => Some(Err(error)),
        };
        assert!(closed.is_none(), "empty inner stream should close normally");

        let follow_up = timeout(std::time::Duration::from_secs(2), async {
            let msg = InboundMessage::new("qq", "u1", "c1", ChatType::Direct, "follow-up", "m3");
            match handler.handle_stream(msg).await {
                Ok(mut stream) => stream.next().await,
                Err(error) => Some(Err(error)),
            }
        })
        .await;
        assert!(
            matches!(follow_up, Ok(Some(Ok(ref output))) if output.text == "follow-up"),
            "session was not reusable after stream error and close"
        );
    }

    #[tokio::test]
    async fn active_stream_blocks_timeout_replacement_until_it_settles() -> Result<(), String> {
        let created = Arc::new(AtomicUsize::new(0));
        let parked_started = Arc::new(Notify::new());
        let release_parked = Arc::new(Notify::new());
        let handler = Arc::new(SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created: created.clone(),
                parked_started: parked_started.clone(),
                release_parked: release_parked.clone(),
            },
        ));

        let first = tokio::spawn(parked_stream_text(handler.clone()));
        timeout(TEST_TIMEOUT, parked_started.notified())
            .await
            .map_err(|_| "parked stream did not start".to_string())?;
        mark_test_session_expired(&handler).await?;

        let concurrent = single_stream_text(&handler, "state", "m2").await;
        release_parked.notify_one();
        let parked = timeout(TEST_TIMEOUT, first)
            .await
            .map_err(|_| "parked stream did not finish".to_string())?
            .map_err(|error| error.to_string())??;

        if concurrent? != "generation-1:hitl-pending" {
            return Err(
                "timeout replaced the handler or lost its pending HITL state while active"
                    .to_string(),
            );
        }
        if parked != "generation-1:parked-complete" {
            return Err("parked stream changed handler generation".to_string());
        }

        mark_test_session_expired(&handler).await?;
        let after = single_stream_text(&handler, "after", "m3").await?;
        if after != "generation-2:after" {
            return Err("idle session was not replaced after timeout".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn explicit_reset_remains_immediate_while_old_stream_settles() -> Result<(), String> {
        let created = Arc::new(AtomicUsize::new(0));
        let parked_started = Arc::new(Notify::new());
        let release_parked = Arc::new(Notify::new());
        let config = SessionConfig::default()
            .with_reset_keywords(vec!["framework-reset".to_string()])
            .with_command_prefix(None)
            .with_reset_reply("configured reset reply");
        let reset_reply = config.reset_reply.clone();
        let handler = Arc::new(SessionHandler::new(
            config,
            GenerationalStreamFactory {
                created: created.clone(),
                parked_started: parked_started.clone(),
                release_parked: release_parked.clone(),
            },
        ));

        let first = tokio::spawn(parked_stream_text(handler.clone()));
        timeout(TEST_TIMEOUT, parked_started.notified())
            .await
            .map_err(|_| "parked stream did not start".to_string())?;

        let reset = single_stream_text(&handler, "framework-reset", "m2").await?;
        if reset != reset_reply {
            return Err("explicit reset did not return its configured reply".to_string());
        }

        let reset_generation = current_test_generation(&handler).await?;
        mark_generation_expired(&reset_generation, handler.config.timeout)?;
        let after_reset = single_stream_text(&handler, "after-reset", "m3").await?;
        if after_reset != "generation-3:after-reset" {
            return Err(
                "old stream activity prevented the reset generation from timing out".to_string(),
            );
        }
        let current_generation = current_test_generation(&handler).await?;
        let (_, current_last_active) = generation_snapshot(&current_generation);

        release_parked.notify_one();
        let parked = timeout(TEST_TIMEOUT, first)
            .await
            .map_err(|_| "parked stream did not finish".to_string())?
            .map_err(|error| error.to_string())??;

        if parked != "generation-1:parked-complete" {
            return Err("explicit reset interrupted the already-active stream".to_string());
        }
        let (_, current_last_active_after_old_settlement) =
            generation_snapshot(&current_generation);
        if current_last_active_after_old_settlement != current_last_active {
            return Err("old stream settlement touched the current generation".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn stream_setup_error_releases_and_touches_activity_receipt() -> Result<(), String> {
        let created = Arc::new(AtomicUsize::new(0));
        let handler = SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created,
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        );
        let mut failed = handler
            .handle_stream(test_message("fail", "m1"))
            .await
            .map_err(|error| error.to_string())?;
        let generation = current_test_generation(&handler).await?;
        let expired_at = mark_generation_expired(&generation, handler.config.timeout)?;
        let failed_item = timeout(TEST_TIMEOUT, failed.next())
            .await
            .map_err(|_| "setup failure was not returned".to_string())?;
        if !matches!(failed_item, Some(Err(ReactError::Other(_)))) {
            return Err("setup failure did not reach the outer stream".to_string());
        }
        let (active_streams, last_active) = generation_snapshot(&generation);
        if active_streams != 0 || last_active <= expired_at {
            return Err("setup failure did not settle and touch its activity receipt".to_string());
        }

        mark_test_session_expired(&handler).await?;
        let after = single_stream_text(&handler, "after-error", "m2").await?;
        if after != "generation-2:after-error" {
            return Err("setup error retained active-stream admission".to_string());
        }
        drop(failed);
        Ok(())
    }

    #[tokio::test]
    async fn inner_item_error_releases_and_touches_activity_receipt() -> Result<(), String> {
        let handler = SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created: Arc::new(AtomicUsize::new(0)),
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        );
        let mut failed = handler
            .handle_stream(test_message("item-fail", "m1"))
            .await
            .map_err(|error| error.to_string())?;
        let generation = current_test_generation(&handler).await?;
        let expired_at = mark_generation_expired(&generation, handler.config.timeout)?;
        let failed_item = timeout(TEST_TIMEOUT, failed.next())
            .await
            .map_err(|_| "inner item failure was not returned".to_string())?;
        if !matches!(failed_item, Some(Err(ReactError::Other(_)))) {
            return Err("inner item failure did not reach the outer stream".to_string());
        }
        let (active_streams, last_active) = generation_snapshot(&generation);
        if active_streams != 0 || last_active <= expired_at {
            return Err("inner item failure did not settle and touch its receipt".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn dropping_polled_stream_releases_and_touches_activity_receipt() -> Result<(), String> {
        let created = Arc::new(AtomicUsize::new(0));
        let parked_started = Arc::new(Notify::new());
        let handler = Arc::new(SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created,
                parked_started: parked_started.clone(),
                release_parked: Arc::new(Notify::new()),
            },
        ));

        let first = tokio::spawn(parked_stream_text(handler.clone()));
        timeout(TEST_TIMEOUT, parked_started.notified())
            .await
            .map_err(|_| "parked stream did not start".to_string())?;
        let generation = current_test_generation(&handler).await?;
        let expired_at = mark_generation_expired(&generation, handler.config.timeout)?;
        first.abort();
        let aborted = timeout(TEST_TIMEOUT, first)
            .await
            .map_err(|_| "aborted stream did not settle".to_string())?;
        if !matches!(aborted, Err(ref error) if error.is_cancelled()) {
            return Err("parked stream task was not cancelled".to_string());
        }
        let (active_streams, last_active) = generation_snapshot(&generation);
        if active_streams != 0 || last_active <= expired_at {
            return Err("dropped stream did not settle and touch its receipt".to_string());
        }

        let after = single_stream_text(&handler, "after-drop", "m2").await?;
        if after != "generation-1:after-drop" {
            return Err("dropped stream did not restart the idle timeout".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn unpolled_outer_stream_holds_and_releases_activity_receipt() -> Result<(), String> {
        let created = Arc::new(AtomicUsize::new(0));
        let handler = SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created,
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        );

        let unpolled = handler
            .handle_stream(test_message("park", "m1"))
            .await
            .map_err(|error| error.to_string())?;
        let generation = current_test_generation(&handler).await?;
        let expired_at = mark_generation_expired(&generation, handler.config.timeout)?;
        let (active_streams, _) = generation_snapshot(&generation);
        if active_streams != 1 {
            return Err("unpolled outer stream was not admitted eagerly".to_string());
        }
        drop(unpolled);
        let (active_streams, last_active) = generation_snapshot(&generation);
        if active_streams != 0 || last_active <= expired_at {
            return Err("unpolled stream drop did not settle and touch its receipt".to_string());
        }

        let after = single_stream_text(&handler, "after-unpolled", "m2").await?;
        if after != "generation-1:after-unpolled" {
            return Err("unpolled stream drop did not restart the idle timeout".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn unpolled_admission_prevents_timeout_prune_before_later_poll() -> Result<(), String> {
        let handler = SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created: Arc::new(AtomicUsize::new(0)),
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        );

        let mut unpolled = handler
            .handle_stream(test_message("late", "m1"))
            .await
            .map_err(|error| error.to_string())?;
        mark_test_session_expired(&handler).await?;
        let concurrent = single_stream_text(&handler, "probe", "m2").await?;
        if concurrent != "generation-1:probe" {
            return Err("timeout pruned an eagerly admitted unpolled stream".to_string());
        }

        let late = timeout(TEST_TIMEOUT, unpolled.next())
            .await
            .map_err(|_| "later-polled stream timed out".to_string())?
            .ok_or_else(|| "later-polled stream closed without an item".to_string())?
            .map_err(|error| error.to_string())?;
        if late.text != "generation-1:late" {
            return Err("later poll ran against a replacement session".to_string());
        }
        if timeout(TEST_TIMEOUT, unpolled.next())
            .await
            .map_err(|_| "later-polled stream did not close".to_string())?
            .is_some()
        {
            return Err("later-polled stream returned an extra item".to_string());
        }

        mark_test_session_expired(&handler).await?;
        let after = single_stream_text(&handler, "after-late", "m3").await?;
        if after != "generation-2:after-late" {
            return Err("settled later-polled stream still blocked timeout pruning".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn panic_unwind_releases_and_touches_activity_receipt() -> Result<(), String> {
        let admitted = Arc::new(Notify::new());
        let release_poll = Arc::new(Notify::new());
        let handler = Arc::new(SessionHandler::new(
            SessionConfig::default().with_timeout(Duration::from_secs(60)),
            GenerationalStreamFactory {
                created: Arc::new(AtomicUsize::new(0)),
                parked_started: Arc::new(Notify::new()),
                release_parked: Arc::new(Notify::new()),
            },
        ));
        let task_handler = handler.clone();
        let task_admitted = admitted.clone();
        let task_release_poll = release_poll.clone();
        let task = tokio::spawn(async move {
            let mut stream = task_handler
                .handle_stream(test_message("panic", "m1"))
                .await
                .map_err(|error| error.to_string())?;
            task_admitted.notify_one();
            task_release_poll.notified().await;
            let _ = stream.next().await;
            Ok::<(), String>(())
        });
        timeout(TEST_TIMEOUT, admitted.notified())
            .await
            .map_err(|_| "panic stream was not admitted".to_string())?;
        let generation = current_test_generation(&handler).await?;
        let expired_at = mark_generation_expired(&generation, handler.config.timeout)?;
        release_poll.notify_one();
        let joined = timeout(TEST_TIMEOUT, task)
            .await
            .map_err(|_| "panic stream task did not settle".to_string())?;
        if !matches!(joined, Err(ref error) if error.is_panic()) {
            return Err("panic stream did not unwind its owner task".to_string());
        }
        let (active_streams, last_active) = generation_snapshot(&generation);
        if active_streams != 0 || last_active <= expired_at {
            return Err("panic unwind did not settle and touch its receipt".to_string());
        }
        Ok(())
    }
}
