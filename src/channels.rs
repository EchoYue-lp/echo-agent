//! IM channel integration module.
//!
//! Two layers:
//! - Direct façade re-exports of `echo_integration::channels`
//! - Crate-local `AgentChannelHandler` adapter for connecting `ReactAgent`
//!   to the IM channel session model
//!
//! # Capability Inheritance
//!
//! Agents created via `AgentChannelHandler` automatically inherit all framework
//! capabilities:
//! - Built-in tools (think, memory, answer)
//! - External tools (MCP, Skill, web, media, data)
//! - Long-term memory (remember/recall/forget)
//! - Context compression
//! - Guards
//! - Permission policies
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//! use echo_agent::channels::*;
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! // 1. Create ChannelManager
//! let mut manager = ChannelManager::new();
//!
//! // 2. Register channel
//! manager.register(Box::new(QqChannel::new(QqConfig {
//!     app_id: "your-app-id".into(),
//!     client_secret: "your-secret".into(),
//! })?))?;
//!
//! // 3. Use AgentChannelHandler for auto-bridging
//! let session_config = SessionConfig::default();
//! let handler_factory = |_channel_id: &str| -> Arc<dyn MessageHandler> {
//!     Arc::new(SessionHandler::new(
//!         session_config.clone(),
//!         || -> Box<dyn MessageHandler> {
//!             Box::new(AgentChannelHandler::from_config(
//!                 AgentConfig::standard("qwen3-max", "im-assistant", "You are a friendly assistant")
//!                     .enable_tool(true)
//!                     .enable_memory(true)
//!             ))
//!         },
//!     ))
//! };
//!
//! // 4. Start
//! for result in manager.start_all(handler_factory).await {
//!     result.result?;
//! }
//! # Ok(())
//! # }
//! ```

/// Direct re-exports from `echo_integration::channels`.
pub mod integration {
    pub use echo_integration::channels::*;
}

pub use echo_integration::channels::prelude::*;

use crate::agent::Agent;
use crate::agent::react::ReactAgent;
use crate::prelude::AgentConfig;
use async_trait::async_trait;
use std::sync::Arc;

/// IM message handler backed by a `ReactAgent`.
///
/// Forwards IM channel messages to the agent, automatically inheriting all
/// framework capabilities (tools, memory, MCP, Skills, compression, guards, etc.).
///
/// Each user session (managed by `SessionHandler`) owns an independent
/// `AgentChannelHandler` to ensure conversation isolation.
pub struct AgentChannelHandler {
    agent: Arc<ReactAgent>,
}

impl AgentChannelHandler {
    /// Create from an existing `ReactAgent`.
    ///
    /// Use this when you need to pre-configure the agent (custom tools,
    /// `LlmConfig`, `MemoryStore`, etc.).
    pub fn new(agent: ReactAgent) -> Self {
        Self {
            agent: Arc::new(agent),
        }
    }

    /// Create from an `AgentConfig`.
    ///
    /// Convenience constructor: creates a `ReactAgent` and applies the config.
    /// Uses the framework's default LLM configuration (environment variables).
    pub fn from_config(config: AgentConfig) -> Self {
        Self::new(ReactAgent::new(config))
    }

    // NOTE: `standard` / `standard_with_llm_config` constructors were removed.
    // They built a *bare* `ReactAgent` bypassing `AgentRuntime::bootstrap`, which
    // is the root cause of A1/C6/D7/E4 (channel agents had no state_store / store /
    // compressor / MemoryLayerManager / permission_service / cache_user_id).
    // The embedding application app layer now routes IM messages through `AgentPool::acquire`
    // (see `echo-agent-cli` `AppChannelMessageHandler`), which gets the full
    // bootstrap-equivalent capability set. Framework callers/tests that need a
    // pre-configured agent should use `new(ReactAgent::new(config))` / `from_config`.
}

#[async_trait]
impl MessageHandler for AgentChannelHandler {
    async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
        let agent = self.agent.as_ref();
        let reply = agent.chat(&msg.text).await?;

        Ok(OutboundMessage::new(
            &msg.channel_id,
            msg.reply_target(),
            msg.chat_type,
            &reply,
        ))
    }

    async fn reply(&self, _msg: OutboundMessage) -> echo_core::error::Result<()> {
        // reply is handled by the channel itself
        Ok(())
    }
}
