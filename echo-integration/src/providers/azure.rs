//! Azure OpenAI client.
//!
//! Azure OpenAI uses a different URL pattern and auth mechanism than OpenAI:
//! - URL: `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={version}`
//! - Auth: `api-key: {key}` header (not Bearer token)
//!
//! The request/response format is identical to OpenAI.

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

const DEFAULT_API_VERSION: &str = "2024-10-21";

/// Azure OpenAI client.
///
/// Supports Azure-hosted OpenAI models with the Azure-specific URL pattern
/// and `api-key` authentication.
pub struct AzureOpenAiClient {
    client: Arc<Client>,
    config: ModelConfig,
    header_map: HeaderMap,
    /// The full base URL including deployment path
    endpoint_url: String,
}

impl AzureOpenAiClient {
    /// Create from an LlmConfig.
    ///
    /// Expected config:
    /// - `base_url`: `https://{resource}.openai.azure.com`
    /// - `model`: deployment name (e.g., `gpt-4o`)
    /// - `api_key`: Azure API key
    pub fn new(config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let api_version = DEFAULT_API_VERSION.to_string();

        let endpoint_url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            model_config.baseurl.trim_end_matches('/'),
            model_config.model,
            api_version
        );

        let header_map = build_azure_headers(&model_config)?;

        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config: model_config,
            header_map,
            endpoint_url,
        })
    }

    /// Create with explicit parameters.
    pub fn with_base_url(
        api_key: &str,
        resource: &str,
        deployment: &str,
        api_version: Option<&str>,
    ) -> Result<Self> {
        let base_url = format!("https://{}.openai.azure.com", resource);
        let ver = api_version.unwrap_or(DEFAULT_API_VERSION);
        let endpoint_url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            base_url, deployment, ver
        );

        let model_config = ModelConfig {
            model: deployment.to_string(),
            baseurl: base_url,
            apikey: api_key.to_string(),
            provider: LlmProvider::Azure,
        };

        let header_map = build_azure_headers(&model_config)?;

        Ok(Self {
            client: Arc::new(Self::build_http_client()),
            config: model_config,
            header_map,
            endpoint_url,
        })
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    }
}

fn build_azure_headers(config: &ModelConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    // Azure uses `api-key` header, not Bearer token
    headers.insert(
        "api-key",
        config
            .apikey
            .parse()
            .map_err(|e| echo_core::error::ReactError::Other(format!("Invalid api-key header: {}", e)))?,
    );
    headers.insert(
        "Content-Type",
        "application/json"
            .parse()
            .map_err(|e| echo_core::error::ReactError::Other(format!("Invalid Content-Type: {}", e)))?,
    );
    Ok(headers)
}

impl LlmClient for AzureOpenAiClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        let model = self.config.model.clone();
        let url = self.endpoint_url.clone();
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
                    &url,
                )
                .await?;

                let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;

                Ok(ChatResponse {
                    message: choice.message.clone(),
                    finish_reason: choice.finish_reason.clone(),
                    raw,
                })
            }
            .instrument(info_span!("azure_chat", model = %model)),
        )
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        let model = self.config.model.clone();
        let url = self.endpoint_url.clone();
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
                    url,
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
            .instrument(info_span!("azure_stream", model = %model)),
        )
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::openai_compatible()
    }
}
