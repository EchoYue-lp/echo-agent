//! LLM stream creation — build the HTTP streaming request with retry + circuit breaker.

use super::super::super::ReactAgent;
use crate::error::{ReactError, Result};
use crate::llm::stream_chat;
use crate::llm::types::Message;
use futures::stream::BoxStream;
use tracing::{Instrument, info};

impl ReactAgent {
    /// Create streaming LLM request (with retries and circuit breaker).
    ///
    /// Returns a `'static` stream because all captured state is cloned
    /// before entering the retry closure.
    #[tracing::instrument(skip(self, messages), fields(agent = %self.config.agent_name, model = %self.config.model_name, msg_count = messages.len()))]
    pub(crate) async fn create_llm_stream(
        &self,
        messages: Vec<Message>,
    ) -> Result<BoxStream<'static, Result<crate::llm::types::ChatCompletionChunk>>> {
        let cancel_token = self.cancel_token.lock().await.clone();
        let agent = &self.config.agent_name;
        let tools_for_stream: Option<Vec<_>> = if self.config.enable_tool {
            let tools = self.tools.tool_manager.get_openai_tools();
            if tools.is_empty() { None } else { Some(tools) }
        } else {
            None
        };

        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;
        let client = self.client.clone();
        let model_name = self.config.model_name.clone();
        let response_format = self.config.response_format.clone();
        let temperature = self.config.temperature;
        let max_tokens = self.config.max_tokens;

        info!(agent = %agent, model = %model_name, "Creating LLM streaming request");

        let circuit_breaker = self.guard.circuit_breaker.clone();
        let stream_result = super::super::retry::retry_llm_call(
            agent,
            max_retries,
            retry_delay,
            &circuit_breaker,
            || {
                let client = client.clone();
                let model_name = model_name.clone();
                let messages = messages.clone();
                let tools_for_stream = tools_for_stream.clone();
                let response_format = response_format.clone();
                let cancel_token = cancel_token.clone();
                async move {
                    stream_chat(
                        client,
                        &model_name,
                        messages,
                        temperature,
                        max_tokens,
                        tools_for_stream,
                        None,
                        response_format,
                        cancel_token,
                    )
                    .await
                }
            },
        )
        .await;

        let stream = stream_result?;
        Ok(Box::pin(stream))
    }
}
