//! Streaming execution entry points — delegates to `run_stream_channel`.
//!
//! Returns `BoxStream<'static>` — callers do not need to hold the agent lock.

use super::super::super::ReactAgent;
use super::super::types::{StreamInit, StreamMode};
use crate::agent::AgentEvent;
use crate::error::Result;
use tracing::Instrument;

impl ReactAgent {
    #[tracing::instrument(skip(self), fields(agent = %self.config.agent_name, model = %self.config.model_name, mode = %mode))]
    pub(crate) async fn run_stream(
        &self, input: &str, mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        self.run_stream_channel(
            StreamInit { text: input.to_string(), message: None, label: String::new() }, mode
        ).await
    }

    #[tracing::instrument(skip(self), fields(agent = %self.config.agent_name, model = %self.config.model_name, mode = %mode))]
    pub(crate) async fn run_stream_with_message(
        &self, message: crate::llm::types::Message, mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let text = message.content.as_text().unwrap_or_default();
        self.run_stream_channel(
            StreamInit { text, message: Some(message), label: "(multimodal)".to_string() }, mode
        ).await
    }
}
