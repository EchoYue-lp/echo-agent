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
//! // 3. Build one explicit client and share it across channel sessions.
//! let llm_config = LlmConfig::for_provider(
//!     "openai",
//!     "https://api.openai.com/v1",
//!     "sk-...",
//!     "gpt-5.5",
//!     LlmApiProtocol::Responses,
//! )?;
//! let llm_client: Arc<dyn LlmClient> = Arc::from(llm_config.build_client()?);
//! let session_config = SessionConfig::default();
//! let handler_factory = move |_channel_id: &str| -> Arc<dyn MessageHandler> {
//!     let llm_client = Arc::clone(&llm_client);
//!     Arc::new(SessionHandler::new(
//!         session_config.clone(),
//!         move || -> Box<dyn MessageHandler> {
//!             Box::new(AgentChannelHandler::from_config_with_client(
//!                 AgentConfig::standard("qwen3-max", "im-assistant", "You are a friendly assistant")
//!                     .enable_tool(true)
//!                     .enable_memory(true),
//!                 Arc::clone(&llm_client),
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
use crate::error::Result;
use crate::llm::{LlmClient, LlmConfig};
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

    /// Create from an `AgentConfig` and an explicit provider configuration.
    ///
    /// The provider client is built before the handler is returned, so invalid
    /// client/header configuration fails at construction instead of on the
    /// first channel message.
    pub fn from_config(config: AgentConfig, llm_config: LlmConfig) -> Result<Self> {
        let client: Arc<dyn LlmClient> = Arc::from(llm_config.build_client()?);
        let mut agent = ReactAgent::new(config);
        agent.install_llm_config(llm_config, client);
        Ok(Self::new(agent))
    }

    /// Create from an `AgentConfig` and an already constructed shared client.
    ///
    /// Session factories use this form so each conversation owns independent
    /// agent state while all sessions reuse the same provider transport.
    pub fn from_config_with_client(config: AgentConfig, client: Arc<dyn LlmClient>) -> Self {
        Self::new(ReactAgent::new(config).with_llm_client(client))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmApiProtocol;
    use crate::testing::MockLlmClient;

    #[test]
    fn from_config_installs_the_explicit_provider_contract() -> Result<()> {
        let llm_config = LlmConfig::for_provider(
            "openai",
            "https://api.example.test/v1",
            "sk-test",
            "gpt-5.5",
            LlmApiProtocol::Responses,
        )?;
        let handler = AgentChannelHandler::from_config(
            AgentConfig::minimal("stale-model", "channel-agent"),
            llm_config,
        )?;

        assert!(handler.agent.llm_client().is_some());
        assert_eq!(handler.agent.model_name(), "gpt-5.5");
        assert_eq!(
            handler
                .agent
                .llm_config()
                .map(|config| (config.api_protocol, config.model.as_str())),
            Some((LlmApiProtocol::Responses, "gpt-5.5"))
        );
        Ok(())
    }

    #[test]
    fn from_config_rejects_an_invalid_client_header() -> Result<()> {
        let mut llm_config = LlmConfig::for_provider(
            "openai",
            "https://api.example.test/v1",
            "sk-test",
            "test-model",
            LlmApiProtocol::ChatCompletions,
        )?;
        llm_config.api_key = "invalid\nheader".to_string();

        assert!(
            AgentChannelHandler::from_config(
                AgentConfig::minimal("test-model", "channel-agent"),
                llm_config,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn from_config_with_client_installs_shared_client() {
        let handler = AgentChannelHandler::from_config_with_client(
            AgentConfig::minimal("stale-model", "channel-agent"),
            Arc::new(MockLlmClient::new().with_model_name("shared-model")),
        );

        assert!(handler.agent.llm_client().is_some());
        assert_eq!(handler.agent.model_name(), "shared-model");
    }
}
