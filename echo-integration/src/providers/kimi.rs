//! Kimi (Moonshot) provider — full `LlmClient` implementation.
//!
//! api.moonshot.cn differences:
//! 1. kimi-k2.7-code: thinking always on, NO request-side depth knob
//! 2. No `reasoning_effort` — depth chosen by model selection
//! 3. Base URL overridable via `KIMI_BASE_URL` env var

use echo_core::error::{LlmError, Result};
use echo_core::llm::types::ChatCompletionRequest;
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{Instrument, info_span, warn};

use super::client::{post, stream_post};

const DEFAULT_BASE_URL: &str = "https://api.moonshot.cn/v1/chat/completions";
const BASE_URL_ENV: &str = "KIMI_BASE_URL";

fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

pub struct KimiClient {
    client: Arc<Client>,
    header_map: HeaderMap,
    model: String,
    base_url: String,
}

impl KimiClient {
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
        if request.thinking.is_some() {
            warn!(model = %self.model, "Kimi: thinking depth is model-selected, not request-controlled");
        }
        ChatCompletionRequest {
            model: self.model.clone(),
            messages: request.messages,
            temperature: request.temperature,
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
            reasoning_effort: None,
            enable_thinking: None,
            thinking_budget: None,
            glm_thinking: None,
            user_id: request.user_id,
        }
    }
}

impl LlmClient for KimiClient {
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
            .instrument(info_span!("kimi_chat", model = %model)),
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
            .instrument(info_span!("kimi_chat_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}
