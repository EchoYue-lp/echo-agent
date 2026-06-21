//! DeepSeek provider — full `LlmClient` implementation.
//!
//! api.deepseek.com is OpenAI-compatible with these differences:
//! 1. `user_id` field for KVCache partition isolation — without it, <1% cache hit
//! 2. Thinking: requires BOTH `reasoning_effort` (high/max) AND
//!    `thinking:{type:"enabled"|"disabled"}`. Only high/max are accepted;
//!    low/medium → high, xhigh → max, minimal/none → thinking.type disabled.
//! 3. Usage returns `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` as
//!    top-level fields (not `prompt_tokens_details.cached_tokens`).
//! 4. `stream_options.include_usage` supported for streaming usage reporting.

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

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/chat/completions";
const BASE_URL_ENV: &str = "DEEPSEEK_BASE_URL";

fn resolve_base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

pub struct DeepSeekClient {
    client: Arc<Client>,
    header_map: HeaderMap,
    model: String,
    base_url: String,
    cache_user_id: Option<String>,
}

impl DeepSeekClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self> {
        Self::with_base_url_and_user_id(api_key, model, resolve_base_url(), None)
    }

    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        Self::with_base_url_and_user_id(api_key, model, base_url, None)
    }

    pub fn with_user_id(
        api_key: impl Into<String>,
        model: impl Into<String>,
        cache_user_id: Option<String>,
    ) -> Result<Self> {
        Self::with_base_url_and_user_id(api_key, model, resolve_base_url(), cache_user_id)
    }

    pub fn with_base_url_and_user_id(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
        cache_user_id: Option<String>,
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
            cache_user_id,
        })
    }

    fn build_request(&self, request: ChatRequest) -> ChatCompletionRequest {
        let t = translate_thinking_openai_compat(
            &self.model,
            "deepseek",
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
            enable_thinking: None, // DeepSeek uses reasoning_effort, not enable_thinking
            thinking_budget: None,
            glm_thinking: t.glm_thinking,
            user_id: self.cache_user_id.clone().or(request.user_id),
        };
        // DeepSeek KVCache: ensure user_id is set for cache reuse
        if req.user_id.is_none() {
            if let Some(ref uid) = self.cache_user_id {
                req.user_id = Some(uid.clone());
            }
        }
        req
    }

    fn build_stream_request(&self, request: ChatRequest) -> ChatCompletionRequest {
        let mut req = self.build_request(request);
        req.stream = Some(true);
        req.stream_options = Some(serde_json::json!({"include_usage": true}));
        req
    }
}

impl LlmClient for DeepSeekClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.model.clone();
        let url = self.base_url.clone();
        Box::pin(
            async move {
                let req = self.build_request(request);
                let raw = post(self.client.clone(), &req, self.header_map.clone(), &url).await?;
                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;
                Ok(ChatResponse {
                    message: choice.message.clone(),
                    finish_reason: choice.finish_reason.clone(),
                    raw,
                })
            }
            .instrument(info_span!("deepseek_chat", model = %model)),
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
                let req = self.build_stream_request(request);
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
            .instrument(info_span!("deepseek_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
