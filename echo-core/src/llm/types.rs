//! OpenAI Chat Completions API type definitions

use crate::tools::Tool;
use serde::{Deserialize, Serialize};

// ── Multimodal Content ────────────────────────────────────────────────────────

/// A single component of message content (multimodal)
///
/// Corresponds to OpenAI Vision / Anthropic multimodal API content parts format.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text
    Text {
        /// Text content
        text: String,
    },
    /// Image (Base64 encoded or URL)
    ImageUrl {
        /// Image URL or Base64 data
        image_url: ImageUrl,
    },
    /// File attachment (inline Base64)
    File {
        /// File name
        name: String,
        /// File content (Base64 encoded)
        content: String,
    },
}

/// Image URL or Base64 data
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageUrl {
    /// `data:image/png;base64,...` or `https://...`
    pub url: String,
    /// Optional detail level: `"auto"` | `"low"` | `"high"`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Message content: compatible with both plain text and multimodal parts forms.
///
/// Serialization behavior:
/// - `Text("hello")` → `"hello"` (fully backward-compatible with legacy API)
/// - `Parts([...])` → `[{"type":"text","text":"..."},...]`
#[derive(Debug, Clone, Default)]
pub enum MessageContent {
    /// Plain text content.
    Text(String),
    /// Multimodal content parts.
    Parts(Vec<ContentPart>),
    /// Explicitly empty content payload.
    #[default]
    Empty,
}

impl Serialize for MessageContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Parts(parts) => parts.serialize(serializer),
            MessageContent::Empty => serializer.serialize_str(""),
        }
    }
}

impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(s) => {
                if s.is_empty() {
                    Ok(MessageContent::Empty)
                } else {
                    Ok(MessageContent::Text(s))
                }
            }
            serde_json::Value::Array(_) => {
                let parts: Vec<ContentPart> =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(MessageContent::Parts(parts))
            }
            _ => Ok(MessageContent::Empty),
        }
    }
}

impl MessageContent {
    /// Extract plain text content (joins all Text parts when multimodal)
    pub fn as_text(&self) -> Option<String> {
        match self {
            MessageContent::Text(s) => {
                if s.is_empty() {
                    None
                } else {
                    Some(s.clone())
                }
            }
            MessageContent::Parts(parts) => {
                let texts: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join(""))
                }
            }
            MessageContent::Empty => None,
        }
    }

    /// Borrow the text content as a reference (only works for Text variant).
    pub fn as_text_ref(&self) -> Option<&str> {
        match self {
            MessageContent::Text(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    /// Backward-compatible with legacy `Option<String>::as_deref()` call sites.
    pub fn as_deref(&self) -> Option<&str> {
        self.as_text_ref()
    }

    /// Borrow multimodal parts when the content is stored in `Parts`.
    pub fn parts(&self) -> Option<&[ContentPart]> {
        match self {
            MessageContent::Parts(parts) => Some(parts.as_slice()),
            _ => None,
        }
    }

    /// Consume the content and return multimodal parts if present.
    pub fn into_parts(self) -> Option<Vec<ContentPart>> {
        match self {
            MessageContent::Parts(parts) => Some(parts),
            _ => None,
        }
    }
}

// ── Message Role ────────────────────────────────────────────────────────────

/// Typed message role with backward-compatible serialization.
///
/// Standard roles (`system`, `user`, `assistant`, `tool`) are type-safe variants.
/// Provider-specific or unknown roles fall into `Custom(String)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Role {
    /// System instruction message
    System,
    /// User input message
    #[default]
    User,
    /// Assistant response message
    Assistant,
    /// Tool result message
    Tool,
    /// Provider-specific or unknown role
    Custom(String),
}

impl Role {
    /// Returns the role string for API serialization.
    pub fn as_str(&self) -> &str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::Custom(s) => s.as_str(),
        }
    }
}

impl Serialize for Role {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "system" => Ok(Role::System),
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            "tool" => Ok(Role::Tool),
            _ => Ok(Role::Custom(s)),
        }
    }
}

impl From<Role> for String {
    fn from(role: Role) -> String {
        role.as_str().to_string()
    }
}

impl From<String> for Role {
    fn from(s: String) -> Self {
        match s.as_str() {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => Role::Custom(s),
        }
    }
}

impl From<&str> for Role {
    fn from(s: &str) -> Self {
        match s {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            _ => Role::Custom(s.to_string()),
        }
    }
}

// ── Conversation Messages ─────────────────────────────────────────────────────

/// Conversation message
///
/// The `content` field uniformly uses the [`MessageContent`] enum, supporting
/// plain text and multimodal content.
#[derive(Debug, Clone, Default)]
pub struct Message {
    /// Message role: `system`, `user`, `assistant`, `tool`, or `Custom`.
    pub role: Role,
    /// Text or multimodal payload.
    pub content: MessageContent,
    /// Optional tool calls emitted by the assistant.
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Optional participant name.
    pub name: Option<String>,
    /// Optional tool call identifier for tool messages.
    pub tool_call_id: Option<String>,
    /// Reasoning content from models like Qwen3/DeepSeek (thinking process).
    pub reasoning_content: Option<String>,
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("role", &self.role)?;
        // Serialize content using MessageContent's own serialization
        if !matches!(&self.content, MessageContent::Empty) {
            map.serialize_entry("content", &self.content)?;
        } else if self.tool_calls.is_some() {
            // assistant with tool_calls but no text content still needs content field
            map.serialize_entry("content", &serde_json::Value::Null)?;
        }
        if let Some(ref tc) = self.tool_calls {
            map.serialize_entry("tool_calls", tc)?;
        }
        if let Some(ref name) = self.name {
            map.serialize_entry("name", name)?;
        }
        if let Some(ref id) = self.tool_call_id {
            map.serialize_entry("tool_call_id", id)?;
        }
        if let Some(ref rc) = self.reasoning_content {
            map.serialize_entry("reasoning_content", rc)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct RawMessage {
            #[serde(default)]
            role: String,
            #[serde(default)]
            content: Option<serde_json::Value>,
            #[serde(default)]
            tool_calls: Option<Vec<ToolCall>>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            tool_call_id: Option<String>,
            #[serde(default)]
            reasoning_content: Option<String>,
        }
        let raw = RawMessage::deserialize(deserializer)?;
        let role: Role = serde_json::from_value(serde_json::Value::String(raw.role))
            .map_err(serde::de::Error::custom)?;
        let content = match raw.content {
            Some(value) => MessageContent::deserialize(value).map_err(serde::de::Error::custom)?,
            None => MessageContent::Empty,
        };
        Ok(Message {
            role,
            content,
            tool_calls: raw.tool_calls,
            name: raw.name,
            tool_call_id: raw.tool_call_id,
            reasoning_content: raw.reasoning_content,
        })
    }
}

impl Message {
    /// Create a system message
    pub fn system(content: String) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Create a user message
    pub fn user(content: String) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Create a multimodal user message
    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Parts(parts),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Create a user message that includes an image
    ///
    /// # Example
    ///
    /// ```rust
    /// use echo_core::llm::types::Message;
    ///
    /// let msg = Message::user_with_image(
    ///     "Please describe this image",
    ///     "image/png",
    ///     "iVBORw0KGgo...", // base64 data
    /// );
    /// ```
    pub fn user_with_image(text: &str, media_type: &str, base64_data: &str) -> Self {
        Self::user_multimodal(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: format!("data:{media_type};base64,{base64_data}"),
                    detail: None,
                },
            },
        ])
    }

    /// Create a user message with an image URL
    pub fn user_with_image_url(text: &str, image_url: &str) -> Self {
        Self::user_multimodal(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: image_url.to_string(),
                    detail: None,
                },
            },
        ])
    }

    /// Create an assistant message
    pub fn assistant(content: String) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Create an assistant message with tool calls
    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Empty,
            tool_calls: Some(tool_calls),
            name: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_call_id: String, name: String, content: String) -> Self {
        Self {
            role: Role::Tool,
            content: MessageContent::Text(content),
            tool_calls: None,
            name: Some(name),
            tool_call_id: Some(tool_call_id),
            reasoning_content: None,
        }
    }

    /// Get plain text content.
    ///
    /// If the message contains multimodal content, all Text parts are extracted and joined.
    pub fn text_content(&self) -> Option<String> {
        self.content.as_text()
    }

    /// Backward-compatible with legacy call sites that read `content_parts` directly.
    pub fn content_parts(&self) -> Option<&[ContentPart]> {
        self.content.parts()
    }

    /// 检查消息是否包含多模态内容
    pub fn is_multimodal(&self) -> bool {
        matches!(&self.content, MessageContent::Parts(_))
    }
}

/// A single tool call initiated by the LLM
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    /// Unique identifier for the tool call
    pub id: String,
    /// Tool call type, typically "function"
    #[serde(rename = "type")]
    pub call_type: String,
    /// Function call details
    pub function: FunctionCall,
}

/// Function information for a tool call
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    /// Function name
    pub name: String,
    /// Function arguments (JSON string)
    pub arguments: String,
}

/// JSON Schema spec for structured output
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonSchemaSpec {
    /// JSON Schema name
    pub name: String,
    /// JSON Schema definition
    pub schema: serde_json::Value,
    /// Whether to enforce strict validation (default true)
    #[serde(default = "default_true")]
    pub strict: bool,
}

fn default_true() -> bool {
    true
}

/// Response format control
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Plain text response
    Text,
    /// JSON object response
    JsonObject,
    /// JSON Schema constrained response
    JsonSchema {
        /// JSON Schema spec
        json_schema: JsonSchemaSpec,
    },
}

impl ResponseFormat {
    /// Create a JSON Schema response format
    pub fn json_schema(name: impl Into<String>, schema: serde_json::Value) -> Self {
        Self::JsonSchema {
            json_schema: JsonSchemaSpec {
                name: name.into(),
                schema,
                strict: true,
            },
        }
    }

    /// Check whether this is a JSON response format
    pub fn is_json(&self) -> bool {
        matches!(self, Self::JsonObject | Self::JsonSchema { .. })
    }
}

/// OpenAI `/chat/completions` request body
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    /// Model name
    pub model: String,
    /// List of conversation messages
    pub messages: Vec<Message>,
    /// Optional tool definition list
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// Tool call strategy (e.g. "auto", "none", or a specific tool name)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// Sampling temperature (0.0-2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Maximum number of tokens to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Whether to enable streaming response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Streaming response options (e.g. include_usage)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<serde_json::Value>,
    /// Response format control
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// OpenAI reasoning-effort for reasoning models (GPT-5 family, o-series).
    ///
    /// One of `"minimal"`, `"low"`, `"medium"`, `"high"`. Only emitted when the
    /// resolved model speaks [`ThinkingProtocol::OpenaiReasoningEffort`];
    /// sending it to a non-reasoning model returns a 400.
    #[serde(skip_serializing_if = "Option::is_none", rename = "reasoning_effort")]
    pub reasoning_effort: Option<String>,
    /// Qwen3/GLM `enable_thinking` toggle. Only emitted for models that speak
    /// [`ThinkingProtocol::EnableThinkingFlag`].
    #[serde(skip_serializing_if = "Option::is_none", rename = "enable_thinking")]
    pub enable_thinking: Option<bool>,
    /// Qwen3 `thinking_budget` (integer token budget). Optional companion to
    /// `enable_thinking` for Qwen3 models that accept an explicit budget.
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinking_budget")]
    pub thinking_budget: Option<u32>,
    /// GLM-4.5/4.6 `thinking:{type:"enabled"|"disabled"}` block. Only emitted
    /// for models that speak [`ThinkingProtocol::GlmThinkingType`].
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinking")]
    pub glm_thinking: Option<GlmThinkingBlock>,
    /// User identifier for KVCache isolation (DeepSeek, etc.).
    /// When set, the provider uses this to partition prompt cache entries.
    /// Without it, every request may be treated as from a different user,
    /// preventing cache reuse across requests in the same session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// GLM `thinking:{type:"enabled"|"disabled"}` wire block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlmThinkingBlock {
    #[serde(rename = "type")]
    pub block_type: String,
}

/// Tool definition sent to the LLM
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    /// Tool type, typically "function"
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function spec
    pub function: FunctionSpec,
}

/// Function declaration for a tool
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionSpec {
    /// Function name
    pub name: String,
    /// Function description
    pub description: String,
    /// Function parameter JSON Schema
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a tool definition from a Tool trait object
    pub fn from_tool(tool: &dyn Tool) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionSpec {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters(),
            },
        }
    }
}

/// OpenAI chat completion response
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatCompletionResponse {
    /// Response ID
    #[serde(default)]
    pub id: String,
    /// List of candidate responses
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// Creation timestamp (seconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
    /// Model name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Token usage statistics
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Extra fields in the response (not explicitly modeled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// Candidate response
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Choice {
    /// Message content
    pub message: Message,
    /// Finish reason (e.g. "stop", "length", "tool_calls", etc.)
    #[serde(default)]
    pub finish_reason: Option<String>,
    /// Candidate index
    #[serde(default)]
    pub index: Option<u32>,
}

/// Token usage statistics
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Usage {
    /// Prompt token count
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    /// Completion token count
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    /// Total token count
    #[serde(default)]
    pub total_tokens: Option<u32>,
    /// OpenAI-compatible prompt token details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<TokenUsageDetails>,
    /// Some compatible providers report input-token details instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<TokenUsageDetails>,
    /// Anthropic cache writes for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    /// Anthropic cache reads for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
    /// DeepSeek: prompt tokens served from context cache (KV cache hit).
    /// `prompt_tokens = prompt_cache_hit_tokens + prompt_cache_miss_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_hit_tokens: Option<u32>,
    /// DeepSeek: prompt tokens NOT served from cache (cache miss).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_miss_tokens: Option<u32>,
}

impl Usage {
    /// Provider-normalized prompt tokens processed by the model, including
    /// tokens served from prompt cache.
    ///
    /// OpenAI-compatible and DeepSeek responses already include cached tokens
    /// in `prompt_tokens`. Anthropic reports cache reads and cache writes
    /// separately, so both are added back for context-window and cache metrics.
    pub fn effective_prompt_tokens(&self) -> u32 {
        let prompt = self.prompt_tokens.unwrap_or(0);
        if self.cache_read_input_tokens.is_some() || self.cache_creation_input_tokens.is_some() {
            prompt
                .saturating_add(self.cached_prompt_tokens())
                .saturating_add(self.cache_creation_prompt_tokens())
        } else {
            prompt
        }
    }

    /// Provider-normalized total tokens processed for this response.
    pub fn effective_total_tokens(&self) -> u32 {
        if self.cache_read_input_tokens.is_some() || self.cache_creation_input_tokens.is_some() {
            self.effective_prompt_tokens()
                .saturating_add(self.completion_tokens.unwrap_or(0))
        } else {
            self.total_tokens.unwrap_or_else(|| {
                self.effective_prompt_tokens()
                    .saturating_add(self.completion_tokens.unwrap_or(0))
            })
        }
    }

    /// Provider-normalized prompt tokens read from cache.
    ///
    /// Checks in priority order:
    /// 1. OpenAI `prompt_tokens_details.cached_tokens`
    /// 2. Compatible providers `input_tokens_details.cached_tokens`
    /// 3. Anthropic `cache_read_input_tokens`
    /// 4. DeepSeek `prompt_cache_hit_tokens`
    pub fn cached_prompt_tokens(&self) -> u32 {
        self.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
            .or_else(|| {
                self.input_tokens_details
                    .as_ref()
                    .and_then(|details| details.cached_tokens)
            })
            .or(self.cache_read_input_tokens)
            .or(self.prompt_cache_hit_tokens)
            .unwrap_or(0)
    }

    /// Provider-normalized prompt tokens written into cache.
    pub fn cache_creation_prompt_tokens(&self) -> u32 {
        self.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens)
            .or_else(|| {
                self.input_tokens_details
                    .as_ref()
                    .and_then(|details| details.cache_write_tokens)
            })
            .or(self.cache_creation_input_tokens)
            .unwrap_or(0)
    }

    /// Prompt cache hit rate in [0.0, 1.0].
    ///
    /// Provider semantics differ on whether `prompt_tokens` includes cached tokens:
    /// - OpenAI / DeepSeek / compatible: `prompt_tokens` is the **total** (includes cached).
    ///   Per DeepSeek docs: `prompt_tokens = prompt_cache_hit_tokens + prompt_cache_miss_tokens`.
    ///   → rate = cached / prompt_tokens
    /// - Anthropic: `input_tokens` (mapped to `prompt_tokens`) excludes both
    ///   cache reads and cache writes.
    ///   → rate = cached / (prompt_tokens + cache_read + cache_creation)
    ///
    /// We detect the Anthropic path by presence of either Anthropic cache field;
    /// all other providers use the inclusive-total formula.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        let cached = self.cached_prompt_tokens();
        let total = self.effective_prompt_tokens();
        if total == 0 {
            return None;
        }
        Some(cached as f64 / total as f64)
    }
}

/// Provider-specific token details nested under usage.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TokenUsageDetails {
    /// Tokens served from provider-side prompt cache.
    #[serde(default)]
    pub cached_tokens: Option<u32>,
    /// Tokens written into provider-side prompt cache.
    #[serde(default)]
    pub cache_write_tokens: Option<u32>,
}

// ── Streaming Response Types ───────────────────────────────────────────────────

/// A single chunk from an SSE streaming response
#[derive(Debug, Deserialize, Clone)]
pub struct ChatCompletionChunk {
    /// Response ID
    #[serde(default)]
    pub id: String,
    /// List of candidate responses
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// Token usage stats (only present in final chunk when stream_options.include_usage is set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Deserialize `null` as `T::default()` for use with `#[serde(deserialize_with)]`.
fn deserialize_null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(|opt| opt.unwrap_or_default())
}

/// Streaming candidate response
#[derive(Debug, Deserialize, Clone)]
pub struct ChunkChoice {
    /// Incremental message content.
    ///
    /// `deserialize_with = "deserialize_null_as_default"` is critical: when
    /// `stream_options.include_usage` is set and the provider returns the
    /// final chunk with usage, the `choices` array may contain an item where
    /// `delta` is `null` (e.g. `{"delta":null,"finish_reason":"stop"}`).
    /// Without this deserializer, null delta fails to parse, the entire chunk
    /// is dropped, and the `usage` field is silently lost — making every LLM
    /// call show `usage_reported: false` in observability.
    #[serde(default, deserialize_with = "deserialize_null_as_default")]
    pub delta: DeltaMessage,
    /// Finish reason
    #[serde(default)]
    pub finish_reason: Option<String>,
    /// Candidate index
    #[serde(default)]
    pub index: u32,
}

/// Incremental message body in a streaming response
#[derive(Debug, Deserialize, Clone, Default)]
pub struct DeltaMessage {
    /// Role (present on first occurrence)
    #[serde(default)]
    pub role: Option<String>,
    /// Content delta
    #[serde(default)]
    pub content: Option<String>,
    /// Reasoning content delta (thinking process from models like Qwen3/DeepSeek)
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Tool call delta
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

/// Incremental fragment of a streaming tool call
#[derive(Debug, Deserialize, Clone)]
pub struct DeltaToolCall {
    /// Tool call index
    pub index: u32,
    /// Tool call ID (appears incrementally)
    #[serde(default)]
    pub id: Option<String>,
    /// Tool call type (appears incrementally)
    #[serde(rename = "type", default)]
    pub call_type: Option<String>,
    /// Function call delta
    #[serde(default)]
    pub function: Option<DeltaFunctionCall>,
}

/// Incremental fragment of a streaming function call
#[derive(Debug, Deserialize, Clone, Default)]
pub struct DeltaFunctionCall {
    /// Function name (appears incrementally)
    #[serde(default)]
    pub name: Option<String>,
    /// Function arguments (appear incrementally)
    #[serde(default)]
    pub arguments: Option<String>,
}

// ── Feature 6: Multimodal Support Tests ────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_part_text_serde_roundtrip() {
        let part = ContentPart::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("should be Text"),
        }
    }

    #[test]
    fn content_part_image_serde_roundtrip() {
        let part = ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "data:image/png;base64,abc123".to_string(),
                detail: Some("high".to_string()),
            },
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"image_url\""));
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::ImageUrl { image_url } => {
                assert!(image_url.url.starts_with("data:image/png;base64,"));
                assert_eq!(image_url.detail, Some("high".to_string()));
            }
            _ => panic!("should be ImageUrl"),
        }
    }

    #[test]
    fn content_part_file_serde_roundtrip() {
        let part = ContentPart::File {
            name: "readme.txt".to_string(),
            content: "SGVsbG8=".to_string(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("\"type\":\"file\""));
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::File { name, content } => {
                assert_eq!(name, "readme.txt");
                assert_eq!(content, "SGVsbG8=");
            }
            _ => panic!("should be File"),
        }
    }

    #[test]
    fn message_content_text_serializes_as_string() {
        let mc = MessageContent::Text("hello".to_string());
        let json = serde_json::to_value(&mc).unwrap();
        assert_eq!(json, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn message_content_parts_serializes_as_array() {
        let mc = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "hi".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: None,
                },
            },
        ]);
        let json = serde_json::to_value(&mc).unwrap();
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[test]
    fn message_content_deserialize_string() {
        let json = serde_json::json!("plain text");
        let mc: MessageContent = serde_json::from_value(json).unwrap();
        assert_eq!(mc.as_text(), Some("plain text".to_string()));
    }

    #[test]
    fn message_content_deserialize_array() {
        let json = serde_json::json!([
            { "type": "text", "text": "describe this" },
            { "type": "image_url", "image_url": { "url": "https://img.png" } }
        ]);
        let mc: MessageContent = serde_json::from_value(json).unwrap();
        match mc {
            MessageContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
            }
            _ => panic!("should be Parts"),
        }
    }

    #[test]
    fn message_text_serde_backward_compatible() {
        let msg = Message::user("hello".to_string());
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["content"], "hello");
        assert_eq!(json["role"], "user");
        assert!(json.get("content_parts").is_none());
    }

    #[test]
    fn message_multimodal_serde() {
        let msg = Message::user_with_image("describe", "image/png", "base64data");
        let json = serde_json::to_value(&msg).unwrap();

        assert_eq!(json["role"], "user");
        let content = &json["content"];
        assert!(
            content.is_array(),
            "multimodal content should serialize as array"
        );
        assert_eq!(content.as_array().unwrap().len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
    }

    #[test]
    fn message_multimodal_deserialize() {
        let json = serde_json::json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "what is this?" },
                { "type": "image_url", "image_url": { "url": "https://example.com/cat.jpg" } }
            ]
        });
        let msg: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg.role, Role::User);
        assert!(
            matches!(msg.content, MessageContent::Parts(_)),
            "content should be Parts for multimodal"
        );
        if let MessageContent::Parts(ref parts) = msg.content {
            assert_eq!(parts.len(), 2);
        }
    }

    #[test]
    fn message_text_content_extracts_text_from_parts() {
        let msg = Message::user_multimodal(vec![
            ContentPart::Text {
                text: "part1".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://img.png".to_string(),
                    detail: None,
                },
            },
            ContentPart::Text {
                text: "part2".to_string(),
            },
        ]);
        assert_eq!(msg.text_content(), Some("part1part2".to_string()));
    }

    #[test]
    fn message_is_multimodal() {
        let text_msg = Message::user("hello".to_string());
        assert!(!text_msg.is_multimodal());

        let mm_msg = Message::user_with_image_url("describe", "https://img.png");
        assert!(mm_msg.is_multimodal());
    }

    #[test]
    fn message_user_with_image_url_helper() {
        let msg = Message::user_with_image_url("look at this", "https://example.com/photo.jpg");
        assert_eq!(msg.role, Role::User);
        assert!(msg.is_multimodal());
        let MessageContent::Parts(parts) = &msg.content else {
            panic!("expected Parts content");
        };
        assert_eq!(parts.len(), 2);
        match &parts[0] {
            ContentPart::Text { text } => assert_eq!(text, "look at this"),
            _ => panic!("first part should be text"),
        }
        match &parts[1] {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(image_url.url, "https://example.com/photo.jpg");
            }
            _ => panic!("second part should be image_url"),
        }
    }

    #[test]
    fn message_deserialize_text_string_content() {
        let json = serde_json::json!({
            "role": "assistant",
            "content": "I am a response"
        });
        let msg: Message = serde_json::from_value(json).unwrap();
        assert_eq!(msg.content.as_text(), Some("I am a response".to_string()));
    }

    #[test]
    fn usage_reads_openai_compatible_cached_tokens() {
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1000,
            "completion_tokens": 20,
            "total_tokens": 1020,
            "prompt_tokens_details": {
                "cached_tokens": 980
            }
        }))
        .unwrap();

        assert_eq!(usage.cached_prompt_tokens(), 980);
        assert_eq!(usage.cache_creation_prompt_tokens(), 0);
    }

    #[test]
    fn usage_reads_openai_cache_write_tokens() -> Result<(), serde_json::Error> {
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1200,
            "completion_tokens": 40,
            "prompt_tokens_details": {
                "cached_tokens": 800,
                "cache_write_tokens": 200
            }
        }))?;

        assert_eq!(usage.cached_prompt_tokens(), 800);
        assert_eq!(usage.cache_creation_prompt_tokens(), 200);
        Ok(())
    }

    #[test]
    fn usage_reads_anthropic_cache_tokens() {
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1000,
            "completion_tokens": 20,
            "total_tokens": 1020,
            "cache_creation_input_tokens": 64,
            "cache_read_input_tokens": 900
        }))
        .unwrap();

        assert_eq!(usage.cached_prompt_tokens(), 900);
        assert_eq!(usage.cache_creation_prompt_tokens(), 64);
        assert_eq!(usage.effective_prompt_tokens(), 1964);
        assert_eq!(usage.effective_total_tokens(), 1984);
    }

    #[test]
    fn usage_reads_deepseek_cache_hit_tokens() {
        // DeepSeek uses top-level `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`
        // instead of the OpenAI `prompt_tokens_details.cached_tokens` pattern.
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1000,
            "completion_tokens": 20,
            "total_tokens": 1020,
            "prompt_cache_hit_tokens": 800,
            "prompt_cache_miss_tokens": 200
        }))
        .unwrap();

        assert_eq!(usage.prompt_tokens, Some(1000));
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(1020));
        assert_eq!(usage.cached_prompt_tokens(), 800);
        assert_eq!(
            usage.prompt_cache_hit_tokens,
            Some(800),
            "should preserve the raw field for direct access"
        );
        assert_eq!(
            usage.prompt_cache_miss_tokens,
            Some(200),
            "should preserve the raw field for direct access"
        );
    }

    #[test]
    fn cache_hit_rate_deepseek_prompt_tokens_includes_cached() {
        // DeepSeek: prompt_tokens = cache_hit + cache_miss (inclusive total).
        // hit_rate = 800 / 1000 = 0.8 (NOT 800/1800).
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1000,
            "prompt_cache_hit_tokens": 800,
            "prompt_cache_miss_tokens": 200
        }))
        .unwrap();
        assert_eq!(usage.cache_hit_rate(), Some(0.8));
    }

    #[test]
    fn cache_hit_rate_openai_prompt_tokens_includes_cached() {
        // OpenAI: prompt_tokens is the total (includes cached_tokens).
        // hit_rate = 900 / 1000 = 0.9 (NOT 900/1900).
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1000,
            "prompt_tokens_details": { "cached_tokens": 900 }
        }))
        .unwrap();
        assert_eq!(usage.cache_hit_rate(), Some(0.9));
    }

    #[test]
    fn cache_hit_rate_anthropic_input_tokens_excludes_cached() {
        // Anthropic: input_tokens (mapped to prompt_tokens) EXCLUDES cache_read.
        // hit_rate = 900 / (100 + 900) = 0.9.
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 100,
            "cache_read_input_tokens": 900,
            "cache_creation_input_tokens": 0
        }))
        .unwrap();
        assert_eq!(usage.cache_hit_rate(), Some(0.9));
        assert_eq!(usage.effective_prompt_tokens(), 1000);
    }

    #[test]
    fn anthropic_cache_creation_counts_toward_effective_context() {
        let usage = Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(20),
            total_tokens: Some(120),
            cache_creation_input_tokens: Some(900),
            ..Default::default()
        };

        assert_eq!(usage.effective_prompt_tokens(), 1000);
        assert_eq!(usage.effective_total_tokens(), 1020);
        assert_eq!(usage.cache_hit_rate(), Some(0.0));
    }

    #[test]
    fn cache_hit_rate_none_when_no_prompt_tokens() {
        let usage = Usage::default();
        assert_eq!(usage.cache_hit_rate(), None);
    }

    #[test]
    fn cache_hit_rate_zero_when_no_cache() {
        let usage: Usage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1000
        }))
        .unwrap();
        assert_eq!(usage.cache_hit_rate(), Some(0.0));
    }

    #[test]
    fn chunk_choice_handles_null_delta_in_usage_chunk() {
        // When stream_options.include_usage is set, some providers (DeepSeek,
        // OpenAI, etc.) send a final chunk where `delta` is null because the
        // model has stopped producing content. The chunk still carries `usage`.
        let json = serde_json::json!({
            "id": "chatcmpl-xxx",
            "choices": [{
                "delta": null,
                "finish_reason": "stop",
                "index": 0
            }],
            "usage": {
                "prompt_tokens": 500,
                "completion_tokens": 80,
                "total_tokens": 580,
                "prompt_cache_hit_tokens": 400
            }
        });
        let chunk: ChatCompletionChunk = serde_json::from_value(json).unwrap();
        assert!(
            chunk.usage.is_some(),
            "usage must be parsed even when delta is null"
        );
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(500));
        assert_eq!(usage.cached_prompt_tokens(), 400);
    }

    #[test]
    fn chunk_choice_handles_missing_delta_in_usage_chunk() {
        // Some providers send a usage-only chunk with choices omitted or empty.
        let json = serde_json::json!({
            "usage": {
                "prompt_tokens": 500,
                "completion_tokens": 80,
                "total_tokens": 580,
                "prompt_cache_hit_tokens": 50
            }
        });
        let chunk: ChatCompletionChunk = serde_json::from_value(json).unwrap();
        assert!(
            chunk.usage.is_some(),
            "usage-only chunk without choices must parse"
        );
        assert!(chunk.choices.is_empty());
        assert_eq!(chunk.usage.unwrap().cached_prompt_tokens(), 50);
    }
}
