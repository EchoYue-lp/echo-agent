//! LLM 客户端
//!
//! 封装 OpenAI Chat Completions API（兼容任意 OpenAI 格式的服务端）。
//! 外部通过 [`chat`] 和 [`stream_chat`] 发起请求；
//! 框架内部（压缩器等）通过 [`LlmClient`] trait 调用。

mod client;
pub mod config;
pub mod types;

use crate::error::{ReactError, Result};
use crate::llm::client::{post, stream_post};
use crate::llm::config::{Config, ModelConfig};
use crate::llm::types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, Message, ToolDefinition,
};
use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::sync::Arc;

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

pub async fn chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    stream: Option<bool>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
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
    };

    let header_map = assemble_req_header(&model)?;
    post(client, &request_body, header_map, model.baseurl.as_str()).await
}

/// 流式 chat 入口，返回 SSE chunk 流
pub async fn stream_chat(
    client: Arc<Client>,
    model_name: &str,
    messages: Vec<Message>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tools: Option<Vec<ToolDefinition>>,
    tool_choice: Option<String>,
) -> Result<impl Stream<Item = Result<ChatCompletionChunk>>> {
    let model = Config::get_model(model_name)?;
    let request_body = ChatCompletionRequest {
        model: model.model.clone(),
        messages,
        temperature,
        max_tokens,
        stream: Some(true),
        tools,
        tool_choice,
    };

    let header_map = assemble_req_header(&model)?;
    let url = model.baseurl.clone();
    stream_post(client, request_body, header_map, url).await
}

/// 轻量 LLM 调用接口，供框架内部（压缩器等）使用
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 无工具的单次对话，返回模型文本
    async fn chat_simple(&self, messages: Vec<Message>) -> Result<String>;
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

#[async_trait]
impl LlmClient for DefaultLlmClient {
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
        )
        .await?;

        response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| ReactError::Other("LLM 返回空内容".to_string()))
    }
}
