//! Google Gemini API client.
//!
//! Uses the OpenAI-compatible endpoint provided by Google's Generative AI API.
//! Base URL: `https://generativelanguage.googleapis.com/v1beta/openai/`
//!
//! Auth: `x-goog-api-key: {api_key}` header.

use echo_core::error::{LlmError, Result};
use echo_core::llm::types::{
    ChatCompletionRequest, ChatCompletionResponse,
};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use echo_core::llm::capabilities::ProviderCapabilities;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;
use tracing::{Instrument, info_span};

use super::client::{post, stream_post};
use super::config::{LlmConfig, LlmProvider, ModelConfig};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";

/// Google Gemini client (OpenAI-compatible endpoint).
pub struct GeminiClient {
    client: Arc<Client>,
    config: ModelConfig,
    header_map: HeaderMap,
}

impl GeminiClient {
    /// Create from an LlmConfig.
    pub fn new(config: LlmConfig) -> Result<Self> {
        let mut model_config = config.to_model_config();
        // Default base URL for Gemini
        if model_config.baseurl.is_empty() {
            model_config.baseurl = DEFAULT_BASE_URL.to_string();
        }
        let header_map = build_headers(&model_config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config: model_config,
            header_map,
        })
    }

    /// Create with explicit parameters.
    pub fn with_base_url(api_key: &str, model: &str, base_url: &str) -> Result<Self> {
        let model_config = ModelConfig {
            model: model.to_string(),
            baseurl: base_url.to_string(),
            apikey: api_key.to_string(),
            provider: LlmProvider::Gemini,
        };
        let header_map = build_headers(&model_config)?;
        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config: model_config,
            header_map,
        })
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    }
}

fn build_headers(config: &ModelConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-goog-api-key",
        config
            .apikey
            .parse()
            .map_err(|e| echo_core::error::ReactError::Other(format!("Invalid API key header: {}", e)))?,
    );
    headers.insert(
        "Content-Type",
        "application/json"
            .parse()
            .map_err(|e| echo_core::error::ReactError::Other(format!("Invalid Content-Type: {}", e)))?,
    );
    Ok(headers)
}

impl LlmClient for GeminiClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                let req = ChatCompletionRequest {
                    model: self.config.model.clone(),
                    messages: request.messages,
                    temperature: request.temperature,
                    max_tokens: request.max_tokens,
                    stream: None,
                    stream_options: None,
                    tools: request.tools,
                    tool_choice: request.tool_choice,
                    response_format: request.response_format,
                };

                let raw: ChatCompletionResponse = post(
                    self.client.clone(),
                    &req,
                    self.header_map.clone(),
                    &self.config.baseurl,
                )
                .await?;

                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;

                Ok(ChatResponse {
                    message: choice.message.clone(),
                    finish_reason: choice.finish_reason.clone(),
                    raw,
                })
            }
            .instrument(info_span!("gemini_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        let model = self.config.model.clone();
        Box::pin(
            async move {
                let req = ChatCompletionRequest {
                    model: self.config.model.clone(),
                    messages: request.messages,
                    temperature: request.temperature,
                    max_tokens: request.max_tokens,
                    stream: Some(true),
                    stream_options: Some(serde_json::json!({"include_usage": true})),
                    tools: request.tools,
                    tool_choice: request.tool_choice,
                    response_format: request.response_format,
                };

                let stream = stream_post(
                    self.client.clone(),
                    req,
                    self.header_map.clone(),
                    self.config.baseurl.clone(),
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
            .instrument(info_span!("gemini_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::openai_compatible()
    }
}
