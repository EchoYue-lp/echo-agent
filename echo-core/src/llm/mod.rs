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
    /// Execute a non-streaming chat request.
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>>;

    /// Execute a streaming chat request.
    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>>;

    /// Convenience helper for simple text-only calls.
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

    /// Model identifier used by this client.
    fn model_name(&self) -> &str;
}

/// 聊天请求参数
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// Ordered chat history sent to the model.
    pub messages: Vec<Message>,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// Optional response token limit.
    pub max_tokens: Option<u32>,
    /// Optional tool definitions exposed to the model.
    pub tools: Option<Vec<ToolDefinition>>,
    /// Optional provider-specific tool choice mode.
    pub tool_choice: Option<String>,
    /// Optional structured output format hint.
    pub response_format: Option<ResponseFormat>,
}

impl ChatRequest {
    /// Create a request from a message list.
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// Attach tool definitions to the request.
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// 聊天响应
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// Primary assistant message returned by the provider.
    pub message: Message,
    /// Provider-specific finish reason.
    pub finish_reason: Option<String>,
    /// Raw provider response for callers needing extra metadata.
    pub raw: ChatCompletionResponse,
}

impl ChatResponse {
    /// Extract the assistant text content.
    pub fn content(&self) -> Option<String> {
        self.message.content.as_text()
    }

    /// Borrow tool calls if the assistant emitted any.
    pub fn tool_calls(&self) -> Option<&Vec<ToolCall>> {
        self.message.tool_calls.as_ref()
    }

    /// Whether the response includes at least one tool call.
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
    /// Incremental message delta.
    pub delta: DeltaMessage,
    /// Finish reason emitted with the chunk, if any.
    pub finish_reason: Option<String>,
}
