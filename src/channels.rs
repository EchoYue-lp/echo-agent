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
//!         move |instance: &ChannelSessionInstance| -> Box<dyn MessageHandler> {
//!             let _runtime_incarnation = instance.incarnation_id();
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
//! manager.stop_all().await?;
//! # Ok(())
//! # }
//! ```

/// Direct re-exports from `echo_integration::channels`.
pub mod integration {
    pub use echo_integration::channels::*;
}

pub use echo_integration::channels::prelude::*;

use crate::agent::react::ReactAgent;
use crate::agent::{Agent, CancellationToken, EventEnvelope, EventIdentity};
use crate::error::{AgentError, Result};
use crate::llm::{LlmClient, LlmConfig};
use crate::prelude::AgentConfig;
use crate::runtime::{
    AgentTurnDriver, EventSink, SinkControl, TurnDeliveryOutcome, TurnMode, TurnOutcome,
    TurnReceipt, TurnRequest,
};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::Arc;

/// IM message handler backed by a `ReactAgent`.
///
/// Forwards IM channel messages to the agent, automatically inheriting all
/// framework capabilities (tools, memory, MCP, Skills, compression, guards, etc.).
///
/// Each sender in each channel conversation (managed by `SessionHandler`) owns
/// an independent `AgentChannelHandler` to ensure conversation isolation.
pub struct AgentChannelHandler {
    agent: Arc<ReactAgent>,
}

struct ChannelEventSink;

#[async_trait]
impl EventSink for ChannelEventSink {
    async fn on_event(&self, _envelope: EventEnvelope) -> Result<SinkControl> {
        Ok(SinkControl::Continue)
    }
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
    /// Session factories use this form so each sender in each conversation owns
    /// independent agent state while all sessions reuse the same provider
    /// transport.
    pub fn from_config_with_client(config: AgentConfig, client: Arc<dyn LlmClient>) -> Self {
        Self::new(ReactAgent::new(config).with_llm_client(client))
    }

    /// Drive one channel message through the framework Turn authority.
    ///
    /// Callers that need cancellation, terminal classification, or usage can
    /// retain this receipt; the standard `MessageHandler` path projects its
    /// completed final answer into an outbound message.
    pub async fn drive_turn(
        &self,
        msg: &InboundMessage,
        cancel: CancellationToken,
    ) -> Result<TurnReceipt> {
        self.drive_turn_with_sink(msg, cancel, &ChannelEventSink)
            .await
    }

    /// Drive a Channel Turn while delivering every envelope to the caller's sink.
    ///
    /// The receipt's delivery status records this sink's acceptance. QQ/Feishu
    /// network delivery happens later at their transport boundary.
    pub async fn drive_turn_with_sink(
        &self,
        msg: &InboundMessage,
        cancel: CancellationToken,
        sink: &dyn EventSink,
    ) -> Result<TurnReceipt> {
        let turn_id = format!("channel-turn-{}", uuid::Uuid::new_v4());
        let conversation_id =
            serde_json::to_string(&(&msg.channel_id, &msg.chat_id, &msg.sender_id))?;
        let message_id = if msg.message_id.trim().is_empty() {
            turn_id.clone()
        } else {
            msg.message_id.clone()
        };
        let identity = EventIdentity::for_chat(Some(conversation_id), turn_id, message_id, None)?;
        let request = TurnRequest::new(identity, &msg.text)
            .mode(TurnMode::Chat)
            .cancel(cancel);
        Ok(AgentTurnDriver
            .drive(self.agent.as_ref(), request, sink)
            .await)
    }
}

fn channel_reply_from_receipt(receipt: TurnReceipt) -> Result<String> {
    match (receipt.outcome, receipt.delivery, receipt.final_answer) {
        (TurnOutcome::Completed, TurnDeliveryOutcome::Delivered, Some(reply)) => Ok(reply),
        (TurnOutcome::Cancelled, _, _) => {
            Err(AgentError::Cancelled(format!("channel turn {}", receipt.turn_id)).into())
        }
        (TurnOutcome::Failed(failure), _, _) => Err(ReactError::Other(format!(
            "channel turn {} failed ({}): {}",
            receipt.turn_id, failure.code, failure.message
        ))),
        (TurnOutcome::Completed, TurnDeliveryOutcome::Failed(failure), _) => {
            Err(ReactError::Other(format!(
                "channel turn {} delivery failed ({}): {}",
                receipt.turn_id, failure.code, failure.message
            )))
        }
        (TurnOutcome::Completed, delivery, _) => Err(ReactError::Other(format!(
            "channel turn {} cannot be replied to after delivery {delivery:?} or without a final answer",
            receipt.turn_id
        ))),
    }
}

#[async_trait]
impl MessageHandler for AgentChannelHandler {
    async fn handle(&self, msg: InboundMessage) -> echo_core::error::Result<OutboundMessage> {
        let receipt = self.drive_turn(&msg, CancellationToken::new()).await?;
        let reply = channel_reply_from_receipt(receipt)?;

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

    async fn close(&self) -> echo_core::error::Result<()> {
        self.agent.close().await
    }

    fn settles_on_cancel(&self) -> bool {
        true
    }

    async fn handle_stream_with_cancel<'a>(
        &'a self,
        msg: InboundMessage,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'a, Result<OutboundMessage>>> {
        let receipt = self.drive_turn(&msg, cancel).await?;
        let reply = channel_reply_from_receipt(receipt)?;
        Ok(futures::stream::once(async move {
            Ok(OutboundMessage::new(
                &msg.channel_id,
                msg.reply_target(),
                msg.chat_type,
                reply,
            ))
        })
        .boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentEvent};
    use crate::llm::LlmApiProtocol;
    use crate::llm::types::Usage;
    use crate::testing::MockLlmClient;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::time::timeout;

    fn test_message(message_id: &str) -> InboundMessage {
        InboundMessage::new(
            "qq",
            "sender",
            "conversation",
            ChatType::Direct,
            "hello",
            message_id,
        )
    }

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

    #[tokio::test]
    async fn channel_turn_exposes_one_driven_receipt_with_identity_and_usage() -> Result<()> {
        let client = Arc::new(
            MockLlmClient::new()
                .with_response_usage(
                    "first reply",
                    Usage {
                        prompt_tokens: Some(12),
                        completion_tokens: Some(4),
                        ..Usage::default()
                    },
                )
                .with_response("second reply"),
        );
        let handler = AgentChannelHandler::from_config_with_client(
            AgentConfig::minimal("mock-model", "channel-agent"),
            client.clone(),
        );
        let receipt = handler
            .drive_turn(&test_message("incoming-1"), CancellationToken::new())
            .await?;

        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.final_answer.as_deref(), Some("first reply"));
        assert_eq!(receipt.prompt_tokens, 12);
        assert_eq!(receipt.completion_tokens, 4);
        assert_eq!(receipt.llm_calls, 1);
        assert!(receipt.turn_id.as_str().starts_with("channel-turn-"));
        assert!(receipt.last_event_sequence > 0);
        let next = handler
            .drive_turn(&test_message("incoming-2"), CancellationToken::new())
            .await?;
        assert_eq!(next.outcome, TurnOutcome::Completed);
        assert_ne!(receipt.turn_id, next.turn_id);
        assert_eq!(client.call_count(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn channel_handler_replies_only_after_driven_turn_completion() -> Result<()> {
        let handler = AgentChannelHandler::from_config_with_client(
            AgentConfig::minimal("mock-model", "channel-agent"),
            Arc::new(MockLlmClient::new().with_response("reply")),
        );
        let outbound = handler.handle(test_message("incoming-2")).await?;
        assert_eq!(outbound.text, "reply");
        assert_eq!(outbound.to, "conversation");
        Ok(())
    }

    #[tokio::test]
    async fn channel_turn_keeps_failure_and_cancellation_out_of_success_projection() -> Result<()> {
        let failing = AgentChannelHandler::from_config_with_client(
            AgentConfig::minimal("mock-model", "channel-agent"),
            Arc::new(MockLlmClient::new().with_network_error("provider offline")),
        );
        let failed = failing
            .drive_turn(&test_message("incoming-3"), CancellationToken::new())
            .await?;
        assert!(matches!(failed.outcome, TurnOutcome::Failed(_)));
        assert!(channel_reply_from_receipt(failed).is_err());

        let cancel = CancellationToken::new();
        cancel.cancel();
        let cancelled = AgentChannelHandler::from_config_with_client(
            AgentConfig::minimal("mock-model", "channel-agent"),
            Arc::new(MockLlmClient::new().with_response("must not be delivered")),
        )
        .drive_turn(&test_message("incoming-4"), cancel)
        .await?;
        assert_eq!(cancelled.outcome, TurnOutcome::Cancelled);
        assert!(matches!(
            channel_reply_from_receipt(cancelled),
            Err(ReactError::Agent(_))
        ));

        let mut undelivered = TurnReceipt::failed(
            "undelivered-turn",
            echo_core::error::AgentFailure::from(&ReactError::Other("delivery".to_string())),
        )?;
        undelivered.outcome = TurnOutcome::Completed;
        undelivered.delivery = TurnDeliveryOutcome::Closed;
        undelivered.final_answer = Some("must not be replied to".to_string());
        assert!(channel_reply_from_receipt(undelivered).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn channel_sink_failure_is_separate_from_the_producer_terminal() -> Result<()> {
        struct FailFinalSink;
        #[async_trait]
        impl EventSink for FailFinalSink {
            async fn on_event(&self, envelope: EventEnvelope) -> Result<SinkControl> {
                if matches!(envelope.payload, AgentEvent::FinalAnswer(_)) {
                    Err(ReactError::Other("channel projection failed".to_string()))
                } else {
                    Ok(SinkControl::Continue)
                }
            }
        }

        let handler = AgentChannelHandler::from_config_with_client(
            AgentConfig::minimal("mock-model", "channel-agent"),
            Arc::new(MockLlmClient::new().with_response("producer answer")),
        );
        let receipt = handler
            .drive_turn_with_sink(
                &test_message("projection"),
                CancellationToken::new(),
                &FailFinalSink,
            )
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert!(matches!(receipt.delivery, TurnDeliveryOutcome::Failed(_)));
        assert_eq!(receipt.final_answer.as_deref(), Some("producer answer"));
        assert!(channel_reply_from_receipt(receipt).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn session_reset_cancels_a_real_channel_turn_before_cleanup() -> Result<()> {
        let client = Arc::new(
            MockLlmClient::new()
                .with_delay(Duration::from_secs(30))
                .with_response("stale answer"),
        );
        let ended = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(
            SessionHandler::new(
                SessionConfig::default()
                    .with_reset_keywords(vec!["reset-now".to_string()])
                    .with_command_prefix(None),
                {
                    let client = Arc::clone(&client);
                    move |_instance: &ChannelSessionInstance| -> Box<dyn MessageHandler> {
                        Box::new(AgentChannelHandler::from_config_with_client(
                            AgentConfig::minimal("mock-model", "channel-agent"),
                            client.clone(),
                        ))
                    }
                },
            )
            .with_on_session_end({
                let ended = Arc::clone(&ended);
                move |_info| {
                    ended.fetch_add(1, Ordering::AcqRel);
                }
            }),
        );
        let old_handler = Arc::clone(&handler);
        let old = tokio::spawn(async move {
            let mut stream = old_handler.handle_stream(test_message("old-turn")).await?;
            Ok::<_, ReactError>(stream.next().await)
        });
        timeout(Duration::from_secs(2), async {
            while client.call_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("channel model call did not start".to_string()))?;

        let mut reset = timeout(
            Duration::from_secs(2),
            handler.handle_stream(InboundMessage::new(
                "qq",
                "sender",
                "conversation",
                ChatType::Direct,
                "reset-now",
                "reset",
            )),
        )
        .await
        .map_err(|_| {
            ReactError::Other("session reset did not settle channel Turn".to_string())
        })??;
        let reply = reset
            .next()
            .await
            .ok_or_else(|| ReactError::Other("reset reply missing".to_string()))??;
        assert!(!reply.text.is_empty());
        let old_result = timeout(Duration::from_secs(2), old)
            .await
            .map_err(|_| ReactError::Other("old channel stream remained active".to_string()))?
            .map_err(|error| ReactError::Other(error.to_string()))??;
        assert!(!matches!(old_result, Some(Ok(_))));
        assert_eq!(ended.load(Ordering::Acquire), 1);
        Ok(())
    }
}
