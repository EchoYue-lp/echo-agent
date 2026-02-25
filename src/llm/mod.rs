mod client;
pub mod config;
pub mod types;

use crate::error::{ReactError, Result};
use crate::llm::client::post;
use crate::llm::config::{Config, ModelConfig};
use crate::llm::types::{ChatCompletionRequest, ChatCompletionResponse, Message, ToolDefinition};
use async_trait::async_trait;
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

/// 为压缩模块等内部组件提供的轻量 LLM 调用接口
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 发起一次简单的无工具对话，返回模型的文本内容
    async fn chat_simple(&self, messages: Vec<Message>) -> Result<String>;
}

/// 基于现有 `chat` 函数的默认实现
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
