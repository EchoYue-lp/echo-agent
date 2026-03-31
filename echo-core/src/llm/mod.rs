//! LLM 客户端核心 trait 和请求/响应类型

pub mod types;

use crate::error::Result;
pub use types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, DeltaMessage, FunctionCall,
    FunctionSpec, JsonSchemaSpec, Message, ResponseFormat, ToolCall, ToolDefinition,
};

use futures::future::BoxFuture;
use futures::stream::BoxStream;

/// LLM 客户端统一接口
pub trait LlmClient: Send + Sync {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>>;

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>>;

    fn chat_simple(&self, messages: Vec<Message>) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            let response = self
                .chat(ChatRequest {
                    messages,
                    temperature: Some(0.7),
                    max_tokens: Some(2048),
                    ..Default::default()
                })
                .await?;
            Ok(response.content().unwrap_or_default().to_string())
        })
    }

    fn model_name(&self) -> &str;
}

/// 聊天请求参数
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub tool_choice: Option<String>,
    pub response_format: Option<ResponseFormat>,
}

impl ChatRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// 聊天响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub message: Message,
    pub finish_reason: Option<String>,
    pub raw: ChatCompletionResponse,
}

impl ChatResponse {
    pub fn content(&self) -> Option<&str> {
        self.message.content.as_deref()
    }

    pub fn tool_calls(&self) -> Option<&Vec<ToolCall>> {
        self.message.tool_calls.as_ref()
    }

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
    pub delta: DeltaMessage,
    pub finish_reason: Option<String>,
}
