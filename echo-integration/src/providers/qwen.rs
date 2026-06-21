//! Qwen (DashScope) provider — full `LlmClient` implementation.
//!
//! dashscope.aliyuncs.com differences from standard OpenAI:
//! 1. Qwen3: `enable_thinking: bool` + optional `thinking_budget: int`
//!    — NOT reasoning_effort; this is Qwen's native thinking protocol
//! 2. DeepSeek models hosted on DashScope ALSO use `enable_thinking`
//!    — provider takes precedence over model name
//! 3. Base URL overridable via `DASHSCOPE_BASE_URL` env var

use echo_core::error::{LlmError, Result};
use echo_core::llm::capabilities::ProviderCapabilities;
use echo_core::llm::types::ChatCompletionRequest;
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{Instrument, info_span};

use super::client::{post, stream_post};
use super::thinking_translate::translate_thinking_openai_compat;

const DEFAULT_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions";
const BASE_URL_ENV: &str = "DASHSCOPE_BASE_URL";

fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

pub struct QwenClient {
    client: Arc<Client>,
    header_map: HeaderMap,
    model: String,
    base_url: String,
}

impl QwenClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        Self::with_base_url(api_key, model, base_url())
    }

    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let auth_val: reqwest::header::HeaderValue =
            format!("Bearer {}", api_key.into()).parse().map_err(
                |e: reqwest::header::InvalidHeaderValue| LlmError::NetworkError(e.to_string()),
            )?;
        headers.insert("Authorization", auth_val);
        headers.insert(
            "Content-Type",
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        Ok(Self {
            client: Arc::new(
                Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .unwrap_or_default(),
            ),
            header_map: headers,
            model: model.into(),
            base_url: base_url.into(),
        })
    }

    fn build_request(&self, request: ChatRequest, stream: bool) -> ChatCompletionRequest {
        // DashScope provider_name triggers EnableThinkingFlag (provider > model name)
        let t = translate_thinking_openai_compat(
            &self.model,
            "dashscope",
            &request.thinking,
            ProviderCapabilities::openai_compatible(),
        );
        ChatCompletionRequest {
            model: self.model.clone(),
            messages: request.messages,
            temperature: if t.drop_temperature {
                None
            } else {
                request.temperature
            },
            max_tokens: request.max_tokens,
            stream: if stream { Some(true) } else { None },
            stream_options: if stream {
                Some(serde_json::json!({"include_usage": true}))
            } else {
                None
            },
            tools: request.tools,
            tool_choice: request.tool_choice,
            response_format: request.response_format,
            reasoning_effort: None, // Qwen uses enable_thinking exclusively
            enable_thinking: t.enable_thinking,
            thinking_budget: t.thinking_budget,
            glm_thinking: None,
            user_id: request.user_id,
        }
    }
}

impl LlmClient for QwenClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        let url = self.base_url.clone();
        Box::pin(
            async move {
                let req = self.build_request(request, false);
                let raw = post(self.client.clone(), &req, self.header_map.clone(), &url).await?;
                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;
                Ok(ChatResponse {
                    message: choice.message.clone(),
                    finish_reason: choice.finish_reason.clone(),
                    raw,
                })
            }
            .instrument(info_span!("qwen_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        let model = self.model.clone();
        let url = self.base_url.clone();
        Box::pin(
            async move {
                let cancel_token = request.cancel_token.clone();
                let req = self.build_request(request, true);
                let stream = stream_post(
                    self.client.clone(),
                    req,
                    self.header_map.clone(),
                    url,
                    cancel_token,
                )
                .await?;
                Ok(Box::pin(futures::StreamExt::map(stream, |r| {
                    r.map(|c| {
                        let choice = c.choices.first();
                        ChatChunk {
                            delta: choice.map(|x| x.delta.clone()).unwrap_or_default(),
                            finish_reason: choice.and_then(|x| x.finish_reason.clone()),
                            usage: c.usage.clone(),
                        }
                    })
                })) as BoxStream<'_, _>)
            }
            .instrument(info_span!("qwen_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
