//! OpenAI-compatible client that delegates vendor-specific behaviour to a
//! [`ProviderAdapter`]. This avoids duplicating the HTTP/SSE layer across
//! DeepSeek, GLM, Kimi, Qwen while still letting each provider own its
//! thinking protocol, cache policy, and request customisation.

use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::ProviderCapabilities;
use echo_core::llm::types::ChatCompletionRequest;
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::Instrument;
use tracing::info_span;

use super::client::{post, stream_post};
use super::thinking_translate::translate_thinking_openai_compat;
use super::traits::ProviderAdapter;

/// An OpenAI-compatible LLM client whose per-vendor differences are captured
/// in a [`ProviderAdapter`]. The HTTP layer is shared; the adapter controls
/// request customisation, thinking fields, and cache policy.
pub struct AdapterClient<A: ProviderAdapter> {
    client: Arc<Client>,
    header_map: HeaderMap,
    base_url: String,
    model: String,
    adapter: A,
    /// Resolved cache policy (cached so we don't recompute on every call).
    cache_policy: echo_core::llm::capabilities::CachePolicy,
}

impl<A: ProviderAdapter> AdapterClient<A> {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        adapter: A,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", api_key.into())
                .parse()
                .map_err(|e| LlmError::NetworkError(format!("Invalid auth header: {e}")))?,
        );
        headers.insert("Content-Type", "application/json".parse().unwrap());
        let cache_policy = adapter.cache_policy();
        Ok(Self {
            client: Arc::new(
                Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .unwrap_or_default(),
            ),
            header_map: headers,
            base_url: base_url.into(),
            model: model.into(),
            adapter,
            cache_policy,
        })
    }

    pub fn cache_policy(&self) -> &echo_core::llm::capabilities::CachePolicy {
        &self.cache_policy
    }
}

impl<A: ProviderAdapter + 'static> LlmClient for AdapterClient<A> {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                let provider_str = self.adapter.provider_name();
                let t = translate_thinking_openai_compat(
                    &self.model,
                    provider_str,
                    &request.thinking,
                    ProviderCapabilities::openai_compatible(),
                );
                let mut req = ChatCompletionRequest {
                    model: self.model.clone(),
                    messages: request.messages,
                    temperature: if t.drop_temperature {
                        None
                    } else {
                        request.temperature
                    },
                    max_tokens: request.max_tokens,
                    stream: None,
                    stream_options: None,
                    tools: request.tools,
                    tool_choice: request.tool_choice,
                    response_format: request.response_format,
                    reasoning_effort: t.reasoning_effort,
                    enable_thinking: t.enable_thinking,
                    thinking_budget: t.thinking_budget,
                    glm_thinking: t.glm_thinking,
                    user_id: request.user_id.clone(),
                };
                // Let the adapter customise the request
                self.adapter.prepare_request(&mut req);

                let raw = post(
                    self.client.clone(),
                    &req,
                    self.header_map.clone(),
                    &self.base_url,
                )
                .await?;
                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;
                Ok(ChatResponse {
                    message: choice.message.clone(),
                    finish_reason: choice.finish_reason.clone(),
                    raw,
                })
            }
            .instrument(info_span!("adapter_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        let model = self.model.clone();
        Box::pin(
            async move {
                let provider_str = self.adapter.provider_name();
                let t = translate_thinking_openai_compat(
                    &self.model,
                    provider_str,
                    &request.thinking,
                    ProviderCapabilities::openai_compatible(),
                );
                let mut req = ChatCompletionRequest {
                    model: self.model.clone(),
                    messages: request.messages,
                    temperature: if t.drop_temperature {
                        None
                    } else {
                        request.temperature
                    },
                    max_tokens: request.max_tokens,
                    stream: Some(true),
                    stream_options: Some(serde_json::json!({"include_usage": true})),
                    tools: request.tools,
                    tool_choice: request.tool_choice,
                    response_format: request.response_format,
                    reasoning_effort: t.reasoning_effort,
                    enable_thinking: t.enable_thinking,
                    thinking_budget: t.thinking_budget,
                    glm_thinking: t.glm_thinking,
                    user_id: request.user_id.clone(),
                };
                self.adapter.prepare_request(&mut req);

                let stream = stream_post(
                    self.client.clone(),
                    req,
                    self.header_map.clone(),
                    self.base_url.clone(),
                    request.cancel_token,
                )
                .await?;
                Ok(Box::pin(futures::StreamExt::map(stream, |result| {
                    result.map(|chunk| {
                        let choice = chunk.choices.first();
                        ChatChunk {
                            delta: choice.map(|c| c.delta.clone()).unwrap_or_default(),
                            finish_reason: choice.and_then(|c| c.finish_reason.clone()),
                            usage: chunk.usage.clone(),
                        }
                    })
                })) as BoxStream<'_, Result<ChatChunk>>)
            }
            .instrument(info_span!("adapter_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
