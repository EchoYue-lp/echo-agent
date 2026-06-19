//! LLM client core trait and request/response types

pub mod capabilities;
pub mod thinking;
pub mod types;

use crate::error::Result;
pub use thinking::{ThinkingConfig, ThinkingLevel, ThinkingProtocol};
pub use types::{
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse, DeltaMessage, FunctionCall,
    FunctionSpec, JsonSchemaSpec, Message, ResponseFormat, Role, ToolCall, ToolDefinition,
};

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use tokio_util::sync::CancellationToken;

/// Options for [`LlmClient::chat_simple_with_options`].
#[derive(Debug, Clone)]
pub struct SimpleChatOptions {
    /// Sampling temperature (0.0–2.0). Defaults to `0.7`.
    pub temperature: Option<f32>,
    /// Maximum tokens to generate. Defaults to `2048`.
    pub max_tokens: Option<u32>,
}

impl Default for SimpleChatOptions {
    fn default() -> Self {
        Self {
            temperature: Some(0.7),
            max_tokens: Some(2048),
        }
    }
}

impl SimpleChatOptions {
    /// Create options with a specific temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Create options with a specific max_tokens.
    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }
}

/// LLM client unified interface
pub trait LlmClient: Send + Sync {
    /// Execute a non-streaming chat request.
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>>;

    /// Execute a streaming chat request.
    ///
    /// The returned stream is `'static` (does not borrow `&self`): all
    /// provider implementations produce an owned, boxed stream so the caller
    /// can move it across awaits / into spawned tasks. This also lets the
    /// streaming core loop route through a trait object (`Arc<dyn LlmClient>`)
    /// and have the resulting stream outlive the trait-method borrow — which
    /// is what makes the loop testable with a mock client.
    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>>;

    /// Convenience helper for simple text-only calls.
    ///
    /// Accepts optional overrides via [`SimpleChatOptions`]; defaults are
    /// `temperature: 0.7` and `max_tokens: 2048`.
    fn chat_simple(&self, messages: Vec<Message>) -> BoxFuture<'_, Result<String>> {
        self.chat_simple_with_options(messages, SimpleChatOptions::default())
    }

    /// Convenience helper with configurable options.
    fn chat_simple_with_options(
        &self,
        messages: Vec<Message>,
        options: SimpleChatOptions,
    ) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            let response = self
                .chat(ChatRequest {
                    messages,
                    temperature: options.temperature,
                    max_tokens: options.max_tokens,
                    ..Default::default()
                })
                .await?;
            Ok(response.content().unwrap_or_default().to_string())
        })
    }

    /// Model identifier used by this client.
    fn model_name(&self) -> &str;

    /// Provider capabilities for this client.
    ///
    /// Default returns OpenAI-compatible capabilities. Override for
    /// Anthropic, Ollama, or custom providers.
    fn capabilities(&self) -> capabilities::ProviderCapabilities {
        capabilities::ProviderCapabilities::openai_compatible()
    }
}

/// Controls how the model uses tools during inference.
///
/// Maps to provider-specific APIs:
/// - OpenAI: `"auto"`, `"none"`, `"required"`, `{"type": "function", "function": {"name": "..."}}`
/// - Anthropic: `{"type": "auto"}`, `{"type": "any"}`, `{"type": "tool", "name": "..."}`
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    /// Let the model decide whether to call tools (default).
    Auto,
    /// Force the model NOT to call any tools.
    None,
    /// Require the model to call at least one tool.
    Required,
    /// Force the model to call a specific tool by name.
    Function { name: String },
}

impl Default for ToolChoice {
    fn default() -> Self {
        Self::Auto
    }
}

impl ToolChoice {
    /// Create a ToolChoice that forces a specific function call.
    pub fn function(name: impl Into<String>) -> Self {
        Self::Function { name: name.into() }
    }

    /// Convert to the OpenAI wire format (`"auto"`, `"none"`, `"required"`,
    /// or `{"type":"function","function":{"name":"..."}}`).
    pub fn to_openai_value(&self) -> serde_json::Value {
        match self {
            Self::Auto => serde_json::Value::String("auto".into()),
            Self::None => serde_json::Value::String("none".into()),
            Self::Required => serde_json::Value::String("required".into()),
            Self::Function { name } => serde_json::json!({
                "type": "function",
                "function": {"name": name}
            }),
        }
    }
}

/// Chat request parameters
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
    /// Controls how the model uses tools (OpenAI wire format).
    /// Prefer [`ToolChoice`] for type-safe construction; convert via
    /// [`ToolChoice::to_openai_value`] when building a [`ChatCompletionRequest`].
    pub tool_choice: Option<String>,
    /// Optional structured output format hint.
    pub response_format: Option<ResponseFormat>,
    /// Optional reasoning-depth / "thinking" control.
    ///
    /// When set, providers translate it to their native thinking wire field
    /// (`reasoning_effort` for OpenAI reasoning models, `thinking.budget_tokens`
    /// for Claude 3.7–4.5, `enable_thinking` for Qwen3/GLM). Models that do not
    /// support a thinking control silently drop it (with a `warn!`). `None`
    /// (the default) means "use the model's default behavior" — no field sent.
    pub thinking: Option<ThinkingConfig>,
    /// Optional cancellation token for aborting in-flight requests.
    /// When set and cancelled, streaming responses will stop at the next SSE boundary.
    pub cancel_token: Option<CancellationToken>,
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

/// Chat response
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

/// Streaming response chunk
#[derive(Debug, Clone)]
pub struct ChatChunk {
    /// Incremental message delta.
    pub delta: DeltaMessage,
    /// Finish reason emitted with the chunk, if any.
    pub finish_reason: Option<String>,
    /// Token usage (present in the final chunk when stream_options.include_usage is set).
    pub usage: Option<types::Usage>,
}
