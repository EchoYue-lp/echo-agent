use echo_core::error::{LlmError, ReactError, Result};
use echo_core::llm::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Message, ResponseFormat,
    ToolDefinition,
};
use echo_core::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::Stream;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;

use crate::client::{post, stream_post};
use crate::config::{Config, LlmConfig, ModelConfig};

// ── 便捷函数 ─────────────────────────────────────────────────────────────────

/// 组装请求头
pub fn assemble_req_header(model: &ModelConfig) -> Result<HeaderMap> {
    let mut header_map = HeaderMap::new();
    header_map.insert(
        "Authorization",
        format!("Bearer {}", model.apikey)
            .parse()
            .map_err(|e| ReactError::Other(format!("Invalid Authorization header: {}", e)))?,
    );
    header_map.insert(
        "Content-Type",
        "application/json"
            .parse()
            .map_err(|e| ReactError::Other(format!("Invalid Content-Type header: {}", e)))?,
    );
    Ok(header_map)
}

/// 同步聊天请求（独立函数，使用环境变量配置）
#[allow(clippy::too_many_arguments)]
pub async fn chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
    response_format: Option<ResponseFormat>,
) -> Result<ChatCompletionResponse> {
    let model = Config::get_model(model_name)?;
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages,
        temperature,
        max_tokens,
        stream,
        tools,
        tool_choice,
        response_format,
    };

    let header_map = assemble_req_header(&model)?;
    post(client, &request_body, header_map, model.baseurl.as_str()).await
}

/// 流式聊天请求（独立函数，使用环境变量配置）
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
    response_format: Option<ResponseFormat>,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>> + use<>> {
    let model = Config::get_model(model_name)?;
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages,
        temperature,
        max_tokens,
        stream: Some(true),
        tools,
        tool_choice,
        response_format,
    };

    let header_map = assemble_req_header(&model)?;
    let url = model.baseurl.clone();
    stream_post(client, request_body, header_map, url).await
}

// ── OpenAI 客户端实现 ──────────────────────────────────────────────────────────

/// OpenAI 兼容客户端
///
/// 支持任何兼容 OpenAI Chat Completions API 的服务。
pub struct OpenAiClient {
    client: Arc<Client>,
    config: ModelConfig,
    header_map: HeaderMap,
}

impl OpenAiClient {
    /// 从环境变量创建客户端
    pub fn from_env(model_name: &str) -> Result<Self> {
        let config = Config::get_model(model_name)?;
        let header_map = assemble_req_header(&config)?;
        Ok(Self {
            client: Arc::new(Client::new()),
            config,
            header_map,
        })
    }

    /// 使用自定义配置创建客户端
    pub fn new(config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client: Arc::new(Client::new()),
            config: model_config,
            header_map,
        })
    }

    /// 使用共享的 HTTP 客户端
    pub fn with_client(client: Arc<Client>, config: LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client,
            config: model_config,
            header_map,
        })
    }
}

impl LlmClient for OpenAiClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            let req = ChatCompletionRequest {
                model: self.config.model.clone(),
                messages: request.messages,
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                stream: None,
                tools: request.tools,
                tool_choice: request.tool_choice,
                response_format: request.response_format,
            };

            let raw = post(
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
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        Box::pin(async move {
            let req = ChatCompletionRequest {
                model: self.config.model.clone(),
                messages: request.messages,
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                stream: Some(true),
                tools: request.tools,
                tool_choice: request.tool_choice,
                response_format: request.response_format,
            };

            let stream = stream_post(
                self.client.clone(),
                req,
                self.header_map.clone(),
                self.config.baseurl.clone(),
            )
            .await?;

            Ok(Box::pin(futures::StreamExt::map(stream, |result| {
                result.map(|chunk| {
                    let choice = chunk.choices.first();
                    ChatChunk {
                        delta: choice.map(|c| c.delta.clone()).unwrap_or_default(),
                        finish_reason: choice.and_then(|c| c.finish_reason.clone()),
                    }
                })
            })) as BoxStream<'_, Result<ChatChunk>>)
        })
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

/// 基于 [`chat`] 函数的默认 [`LlmClient`] 实现
pub struct DefaultLlmClient {
    client: Arc<Client>,
    model_name: String,
}

impl DefaultLlmClient {
    pub fn new(client: Arc<Client>, model_name: impl Into<String>) -> Self {
        Self {
            client,
            model_name: model_name.into(),
        }
    }
}

impl LlmClient for DefaultLlmClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            let raw = chat(
                self.client.clone(),
                &self.model_name,
                request.messages,
                request.temperature,
                request.max_tokens,
                None,
                request.tools,
                request.tool_choice,
                request.response_format,
            )
            .await?;

            let choice = raw.choices.first().ok_or(LlmError::EmptyResponse)?;

            Ok(ChatResponse {
                message: choice.message.clone(),
                finish_reason: choice.finish_reason.clone(),
                raw,
            })
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        Box::pin(async move {
            let stream = stream_chat(
                self.client.clone(),
                &self.model_name,
                request.messages,
                request.temperature,
                request.max_tokens,
                request.tools,
                request.tool_choice,
                request.response_format,
            )
            .await?;

            Ok(Box::pin(futures::StreamExt::map(stream, |result| {
                result.map(|chunk| {
                    let choice = chunk.choices.first();
                    ChatChunk {
                        delta: choice.map(|c| c.delta.clone()).unwrap_or_default(),
                        finish_reason: choice.and_then(|c| c.finish_reason.clone()),
                    }
                })
            })) as BoxStream<'_, Result<ChatChunk>>)
        })
    }

    fn chat_simple(&self, messages: Vec<Message>) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            let response = chat(
                self.client.clone(),
                &self.model_name,
                messages,
                Some(0.3),
                Some(2048),
                Some(false),
                None,
                None,
                None,
            )
            .await?;

            response
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .ok_or_else(|| ReactError::Other("LLM 返回空内容".to_string()))
        })
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
