//! LLM 客户端
//!
//! 统一的 LLM 抽象层，支持：
//! - OpenAI 兼容 API（默认实现）
//! - 自定义 LLM 实现（依赖注入）
//! - Mock 测试客户端
//!
//! # 快速上手
//!
//! ```rust,no_run
//! use echo_agent::llm::{LlmClient, OpenAiClient, ChatRequest};
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! // 使用环境变量配置
//! let client = OpenAiClient::from_env("qwen3-max")?;
//!
//! // 发送请求
//! let response = client.chat(ChatRequest {
//!     messages: vec![echo_agent::llm::Message::user("你好".to_string())],
//!     ..Default::default()
//! }).await?;
//!
//! println!("{}", response.content().unwrap_or_default());
//! # Ok(())
//! # }
//! ```

mod client;
pub mod config;
pub mod types;

use crate::error::Result;
pub use crate::llm::config::LlmConfig;
use crate::llm::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Message, ToolDefinition,
};
pub use crate::llm::types::{JsonSchemaSpec, Message as LlmMessage, ResponseFormat};
use async_trait::async_trait;
use futures::Stream;
use std::sync::Arc;

// ── 统一的 LLM 客户端 Trait ────────────────────────────────────────────────────

/// LLM 客户端统一接口
///
/// 所有 LLM 实现（OpenAI、本地模型、Mock）都应实现此 trait。
/// 通过 `ReactAgent::with_llm()` 注入到 Agent 中。
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 同步聊天请求（阻塞直到完整响应）
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// 流式聊天请求（返回 SSE chunk 流）
    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<'_, Result<ChatChunk>>>;

    /// 简单对话（无工具，返回文本）
    async fn chat_simple(&self, messages: Vec<Message>) -> Result<String> {
        let response = self
            .chat(ChatRequest {
                messages,
                temperature: Some(0.7),
                max_tokens: Some(2048),
                ..Default::default()
            })
            .await?;
        Ok(response.content().unwrap_or_default().to_string())
    }

    /// 获取模型名称
    fn model_name(&self) -> &str;
}

/// 聊天请求参数
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// 消息列表
    pub messages: Vec<Message>,
    /// 温度参数（0.0-2.0）
    pub temperature: Option<f32>,
    /// 最大生成 token 数
    pub max_tokens: Option<u32>,
    /// 工具定义列表
    pub tools: Option<Vec<ToolDefinition>>,
    /// 工具选择策略
    pub tool_choice: Option<String>,
    /// 响应格式
    pub response_format: Option<ResponseFormat>,
}

impl ChatRequest {
    /// 创建新的请求（仅消息）
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// 添加工具定义
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// 聊天响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// 消息内容
    pub message: Message,
    /// 完成原因
    pub finish_reason: Option<String>,
    /// 原始响应（保留完整信息）
    pub raw: ChatCompletionResponse,
}

impl ChatResponse {
    /// 获取文本内容
    pub fn content(&self) -> Option<&str> {
        self.message.content.as_deref()
    }

    /// 获取工具调用列表
    pub fn tool_calls(&self) -> Option<&Vec<types::ToolCall>> {
        self.message.tool_calls.as_ref()
    }

    /// 是否包含工具调用
    pub fn has_tool_calls(&self) -> bool {
        self.message
            .tool_calls
            .as_ref()
            .is_some_and(|t| !t.is_empty())
    }
}

/// 流式响应块
#[derive(Debug, Clone)]
pub struct ChatChunk {
    /// 增量消息
    pub delta: types::DeltaMessage,
    /// 完成原因
    pub finish_reason: Option<String>,
}

// ── BoxStream 别名 ─────────────────────────────────────────────────────────────

use futures::stream::BoxStream;

// ── 便捷函数（向后兼容）─────────────────────────────────────────────────────────

use crate::llm::client::{post, stream_post};
use crate::llm::config::{Config, ModelConfig};
use reqwest::Client;
use reqwest::header::HeaderMap;

/// 组装请求头
pub fn assemble_req_header(model: &ModelConfig) -> Result<HeaderMap> {
    let mut header_map = HeaderMap::new();
    header_map.insert(
        "Authorization",
        format!("Bearer {}", model.apikey).parse().map_err(|e| {
            crate::error::ReactError::Other(format!("Invalid Authorization header: {}", e))
        })?,
    );
    header_map.insert(
        "Content-Type",
        "application/json".parse().map_err(|e| {
            crate::error::ReactError::Other(format!("Invalid Content-Type header: {}", e))
        })?,
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
    pub fn new(config: config::LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client: Arc::new(Client::new()),
            config: model_config,
            header_map,
        })
    }

    /// 使用共享的 HTTP 客户端
    pub fn with_client(client: Arc<Client>, config: config::LlmConfig) -> Result<Self> {
        let model_config = config.to_model_config();
        let header_map = assemble_req_header(&model_config)?;
        Ok(Self {
            client,
            config: model_config,
            header_map,
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
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

        let choice = raw
            .choices
            .first()
            .ok_or_else(|| crate::error::LlmError::EmptyResponse)?;

        Ok(ChatResponse {
            message: choice.message.clone(),
            finish_reason: choice.finish_reason.clone(),
            raw,
        })
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<'_, Result<ChatChunk>>> {
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

        // 转换为 ChatChunk 流
        Ok(Box::pin(futures::StreamExt::map(stream, |result| {
            result.map(|chunk| {
                let choice = chunk.choices.first();
                ChatChunk {
                    delta: choice.map(|c| c.delta.clone()).unwrap_or_default(),
                    finish_reason: choice.and_then(|c| c.finish_reason.clone()),
                }
            })
        })))
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }
}

// ── 默认客户端（向后兼容）───────────────────────────────────────────────────────

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

#[async_trait]
impl LlmClient for DefaultLlmClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
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

        let choice = raw
            .choices
            .first()
            .ok_or_else(|| crate::error::LlmError::EmptyResponse)?;

        Ok(ChatResponse {
            message: choice.message.clone(),
            finish_reason: choice.finish_reason.clone(),
            raw,
        })
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<'_, Result<ChatChunk>>> {
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
        })))
    }

    async fn chat_simple(&self, messages: Vec<Message>) -> Result<String> {
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
            .ok_or_else(|| crate::error::ReactError::Other("LLM 返回空内容".to_string()))
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
