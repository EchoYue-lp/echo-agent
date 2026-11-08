//! OpenAI Chat Completions API type definitions

use crate::tools::Tool;
use serde::{Deserialize, Serialize};

// ── Multimodal Content ────────────────────────────────────────────────────────

/// A provider-neutral component of structured message content.
///
/// Provider adapters translate variants that their wire API does not support
/// natively while the framework keeps the typed value in conversation state.
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
    /// A typed link to a resource that the Agent may inspect.
    ResourceLink {
        /// Lossless provider-neutral resource metadata.
        resource: Box<LinkedResource>,
    },
}

/// Provider-neutral reasoning block required for multi-turn replay.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningBlock {
    /// Reasoning text accompanied by a provider signature.
    Signed { thinking: String, signature: String },
    /// Opaque encrypted reasoning that must be replayed unchanged.
    Redacted { data: String },
    /// Provider-owned opaque reasoning state that must be replayed unchanged.
    ///
    /// Responses requires both the item id and encrypted payload when the
    /// application manages conversation history locally with `store: false`.
    Opaque {
        provider: String,
        id: String,
        data: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        summary: Vec<String>,
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

/// Provider-neutral linked resource carried in a structured user message.
///
/// Protocol adapters keep this typed until the provider boundary. Providers
/// that do not have a native resource-link block render it as deterministic
/// text while framework Agents can inspect every field without parsing a
/// private marker embedded in user text.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LinkedResource {
    /// Optional display and audience annotations supplied by the protocol.
    pub annotations: Option<serde_json::Value>,
    /// Optional human-readable details.
    pub description: Option<String>,
    /// Optional MIME type for the linked content.
    pub mime_type: Option<String>,
    /// Human-readable resource name.
    pub name: String,
    /// Optional resource size in bytes.
    pub size: Option<i64>,
    /// Optional display title.
    pub title: Option<String>,
    /// Resource URI preserved exactly from the adapter input.
    pub uri: String,
    /// Optional protocol extension metadata.
    pub meta: Option<serde_json::Value>,
}

impl LinkedResource {
    /// Render a deterministic provider fallback without changing the typed
    /// framework message stored in Session history.
    #[doc(hidden)]
    pub fn model_text(&self) -> String {
        let encoded = serde_json::to_string(self)
            .unwrap_or_else(|_| "{\"resource_link\":\"serialization_failed\"}".to_string());
        format!("[Linked resource]\n{encoded}")
    }
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
                let texts: Vec<String> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.clone()),
                        ContentPart::ResourceLink { resource } => Some(resource.model_text()),
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

    /// Conservatively estimate provider-visible input tokens, including
    /// multimodal payloads that are intentionally omitted by [`Self::as_text`].
    pub fn estimated_tokens(&self, tokenizer: &dyn crate::tokenizer::Tokenizer) -> usize {
        match self {
            MessageContent::Text(text) => tokenizer.count_tokens(text),
            MessageContent::Empty => 0,
            MessageContent::Parts(parts) => parts.iter().fold(0usize, |total, part| {
                let part_tokens = match part {
                    ContentPart::Text { text } => tokenizer.count_tokens(text),
                    ContentPart::ImageUrl { image_url } => {
                        if image_url.url.starts_with("data:") {
                            // Base64 carries roughly three bytes per four characters;
                            // charging one token per four characters is conservative
                            // across supported vision providers.
                            image_url.url.chars().count().div_ceil(4)
                        } else if image_url.detail.as_deref() == Some("low") {
                            100
                        } else {
                            1_100
                        }
                    }
                    ContentPart::File { name, content } => tokenizer
                        .count_tokens(name)
                        .saturating_add(content.chars().count().div_ceil(4)),
                    ContentPart::ResourceLink { resource } => {
                        tokenizer.count_tokens(&resource.model_text())
                    }
                };
                total.saturating_add(part_tokens)
            }),
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

/// 同一供应商响应的原始观察；不授予模型能力、正文批准或执行终态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderResponseMetadata {
    pub protocol: crate::llm::LlmApiProtocol,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "response_present_text",
        serialize_with = "response_optional_text"
    )]
    pub model: Option<serde_json::Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "response_present_text",
        serialize_with = "response_optional_text"
    )]
    pub stop_reason: Option<serde_json::Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "response_present_text",
        serialize_with = "response_optional_text"
    )]
    pub stop_sequence: Option<serde_json::Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "response_present_text",
        serialize_with = "response_optional_text"
    )]
    pub status: Option<serde_json::Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "response_present_object",
        serialize_with = "response_optional_object"
    )]
    pub incomplete_details: Option<serde_json::Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "response_present_object",
        serialize_with = "response_optional_object"
    )]
    pub usage: Option<serde_json::Value>,
}

// key缺失由default处理；存在时直接解码Value，保留Some(Null)。普通Option解码会丢此区别。
fn response_present_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() || value.is_string() {
        Ok(Some(value))
    } else {
        Err(serde::de::Error::custom(
            "provider response field must be string or null",
        ))
    }
}

fn response_present_object<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() || value.is_object() {
        Ok(Some(value))
    } else {
        Err(serde::de::Error::custom(
            "provider response field must be object or null",
        ))
    }
}

fn response_optional_text<S: serde::Serializer>(
    value: &Option<serde_json::Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value
        .as_ref()
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(serde::ser::Error::custom(
            "provider response field must be string or null",
        ));
    }
    value.serialize(serializer)
}

fn response_optional_object<S: serde::Serializer>(
    value: &Option<serde_json::Value>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value
        .as_ref()
        .is_some_and(|value| !value.is_null() && !value.is_object())
    {
        return Err(serde::ser::Error::custom(
            "provider response field must be object or null",
        ));
    }
    value.serialize(serializer)
}

/// Conversation message.
///
/// The content field preserves plain text or structured multimodal content.
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
    /// 工具结果的错误观察；None为未知，不授予Run/TaskRun终态资格。
    pub tool_result_is_error: Option<bool>,
    /// 当前响应的完整观察快照；不得用它推断执行完成或展示未批准正文。
    pub provider_response: Option<ProviderResponseMetadata>,
    /// Reasoning content from models like Qwen3/DeepSeek (thinking process).
    pub reasoning_content: Option<String>,
    /// Signed or redacted reasoning blocks needed for provider-valid replay.
    pub reasoning_blocks: Option<Vec<ReasoningBlock>>,
    /// Ordered native assistant blocks preserved for durable replay.
    ///
    /// Provider adapters may fill this field when they have stable item/block
    /// identities. It is metadata for reconstruction and never decides run
    /// completion by itself.
    pub assistant_blocks: Option<Vec<crate::agent::AssistantMessageBlock>>,
    /// Runtime confirmation for the durable message. A provider phase or
    /// block completion alone never proves this execution fact.
    pub execution_status: Option<crate::agent::ExecutionStatus>,
    /// 由配置输出校验派生的呈现投影；Some(empty) 明确禁止回退到原始块。
    /// 原始 assistant_blocks 始终用于协议回放，不赋予呈现或运行完成资格。
    pub approved_assistant_blocks: Option<Vec<crate::agent::AssistantMessageBlock>>,
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
        if let Some(is_error) = self.tool_result_is_error {
            map.serialize_entry("tool_result_is_error", &is_error)?;
        }
        if let Some(ref observation) = self.provider_response {
            map.serialize_entry("provider_response", observation)?;
        }
        if let Some(ref rc) = self.reasoning_content {
            map.serialize_entry("reasoning_content", rc)?;
        }
        if let Some(ref blocks) = self.reasoning_blocks {
            map.serialize_entry("reasoning_blocks", blocks)?;
        }
        if let Some(ref blocks) = self.assistant_blocks {
            map.serialize_entry("assistant_blocks", blocks)?;
        }
        if let Some(ref status) = self.execution_status {
            map.serialize_entry("execution_status", status)?;
        }
        if let Some(ref blocks) = self.approved_assistant_blocks {
            map.serialize_entry("approved_assistant_blocks", blocks)?;
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
            tool_result_is_error: Option<bool>,
            #[serde(default)]
            provider_response: Option<ProviderResponseMetadata>,
            #[serde(default)]
            reasoning_content: Option<String>,
            #[serde(default)]
            reasoning_blocks: Option<Vec<ReasoningBlock>>,
            #[serde(default)]
            assistant_blocks: Option<Vec<crate::agent::AssistantMessageBlock>>,
            #[serde(default)]
            execution_status: Option<crate::agent::ExecutionStatus>,
            #[serde(default)]
            approved_assistant_blocks: Option<Vec<crate::agent::AssistantMessageBlock>>,
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
            tool_result_is_error: raw.tool_result_is_error,
            provider_response: raw.provider_response,
            reasoning_content: raw.reasoning_content,
            reasoning_blocks: raw.reasoning_blocks,
            assistant_blocks: raw.assistant_blocks,
            execution_status: raw.execution_status,
            approved_assistant_blocks: raw.approved_assistant_blocks,
        })
    }
}

impl Message {
    /// 只保存失败请求的观察，不把partial正文或未执行工具片段放回模型历史。
    pub fn provider_response_observation(observation: ProviderResponseMetadata) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Empty,
            provider_response: Some(observation),
            approved_assistant_blocks: Some(Vec::new()),
            ..Default::default()
        }
    }

    /// 协议边界排除整条观察记录；合法空正文加工具或signed reasoning不匹配。
    pub fn is_provider_response_observation_only(&self) -> bool {
        matches!(&self.role, Role::Assistant)
            && matches!(&self.content, MessageContent::Empty)
            && self.provider_response.is_some()
            && self.tool_calls.is_none()
            && self.name.is_none()
            && self.tool_call_id.is_none()
            && self.tool_result_is_error.is_none()
            && self.reasoning_content.is_none()
            && self.reasoning_blocks.is_none()
            && self.assistant_blocks.is_none()
            && self.execution_status.is_none()
            && self
                .approved_assistant_blocks
                .as_ref()
                .is_some_and(Vec::is_empty)
    }

    /// Create a system message
    pub fn system(content: String) -> Self {
        Self {
            role: Role::System,
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
            tool_result_is_error: None,
            provider_response: None,
            reasoning_content: None,
            reasoning_blocks: None,
            assistant_blocks: None,
            execution_status: None,
            approved_assistant_blocks: None,
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
            tool_result_is_error: None,
            provider_response: None,
            reasoning_content: None,
            reasoning_blocks: None,
            assistant_blocks: None,
            execution_status: None,
            approved_assistant_blocks: None,
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
            tool_result_is_error: None,
            provider_response: None,
            reasoning_content: None,
            reasoning_blocks: None,
            assistant_blocks: None,
            execution_status: None,
            approved_assistant_blocks: None,
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
            tool_result_is_error: None,
            provider_response: None,
            reasoning_content: None,
            reasoning_blocks: None,
            assistant_blocks: None,
            execution_status: None,
            approved_assistant_blocks: None,
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
            tool_result_is_error: None,
            provider_response: None,
            reasoning_content: None,
            reasoning_blocks: None,
            assistant_blocks: None,
            execution_status: None,
            approved_assistant_blocks: None,
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
            tool_result_is_error: None,
            provider_response: None,
            reasoning_content: None,
            reasoning_blocks: None,
            assistant_blocks: None,
            execution_status: None,
            approved_assistant_blocks: None,
        }
    }

    /// 将真实工具结果观察附到同一消息，不改调用身份或执行权威。
    pub fn with_tool_result_is_error(mut self, is_error: bool) -> Self {
        self.tool_result_is_error = Some(is_error);
        self
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

/// Native provider provenance carried by the existing message and tool types.
///
/// It records protocol coordinates and original wire kind/phase only. It is
/// never a runtime execution or completion authority. Provider adapters create
/// this value from native fields; similarly named vendor extension fields are
/// not trusted as Framework metadata.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ProviderOutputSource {
    /// The wire protocol that owns these native coordinates.
    pub protocol: crate::llm::LlmApiProtocol,
    /// 服务实际响应／消息 ID。仅 D2 不完整 ToolCall 的坐标允许空字符串表示
    /// 未观察到原生 ID，此时同消息 provider_response.request_id 仍为 None。
    /// 空值只保当前请求局部的原始 index，不是 runtime ID、跨请求 owner
    /// 或可成功重放的身份；适配器不得用生成 ID 冒充服务原生 ID。
    pub request_id: String,
    /// Native item identity, or the adapter's stable block fallback when the
    /// protocol does not assign a separate item id.
    pub item_id: String,
    /// Native Responses output position / Anthropic content-block position /
    /// Chat Completions tool-call or choice position, according to content_kind.
    pub output_index: u32,
    /// Position inside one native item's content/summary list, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
    /// Original wire kind, e.g. output_text, refusal, function_call, thinking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_kind: Option<String>,
    /// Original provider phase. Unknown values remain lossless here while the
    /// semantic AssistantPhase remains Unknown; no wording inference is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

/// A single tool call initiated by the LLM
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    /// Provider-native coordinates used for lossless history replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_source: Option<ProviderOutputSource>,
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
    /// 通用工具选择；具名函数按Chat wire object序列化。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<crate::llm::ToolChoice>,
    /// Sampling temperature (0.0-2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Maximum number of tokens to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Reasoning-model output limit. Mutually exclusive with `max_tokens`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
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
    /// resolved model speaks `ThinkingProtocol::OpenaiReasoningEffort`;
    /// sending it to a non-reasoning model returns a 400.
    #[serde(skip_serializing_if = "Option::is_none", rename = "reasoning_effort")]
    pub reasoning_effort: Option<String>,
    /// Qwen3/GLM `enable_thinking` toggle. Only emitted for models that speak
    /// `ThinkingProtocol::EnableThinkingFlag`.
    #[serde(skip_serializing_if = "Option::is_none", rename = "enable_thinking")]
    pub enable_thinking: Option<bool>,
    /// Qwen3 `thinking_budget` (integer token budget). Optional companion to
    /// `enable_thinking` for Qwen3 models that accept an explicit budget.
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinking_budget")]
    pub thinking_budget: Option<u32>,
    /// Generic `thinking:{type:"enabled"|"disabled"}` block.
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinking")]
    pub thinking_type: Option<ThinkingTypeBlock>,
    /// Ollama's native `think` extension: a boolean or an effort-level string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think: Option<serde_json::Value>,
    /// User identifier for KVCache isolation (DeepSeek, etc.).
    /// When set, the provider uses this to partition prompt cache entries.
    /// Without it, every request may be treated as from a different user,
    /// preventing cache reuse across requests in the same session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// `thinking:{type:"enabled"|"disabled"}` wire block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingTypeBlock {
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
    /// 函数参数严格生成；None沿协议默认，不等同最终输出schema strict。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
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
                strict: None,
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
    /// Provider-specific output-token details, including reasoning tokens.
    /// Chat Completions/DeepSeek的completion_tokens_details归入同一用量权威。
    #[serde(
        default,
        alias = "completion_tokens_details",
        skip_serializing_if = "Option::is_none"
    )]
    pub output_tokens_details: Option<TokenUsageDetails>,
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
    /// Tokens used for internal model reasoning.
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
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
    /// 同一请求的完整已观察快照，不能跨请求叠加usage或推断终态。
    #[serde(default)]
    pub provider_response: Option<ProviderResponseMetadata>,
    /// Role (present on first occurrence)
    #[serde(default)]
    pub role: Option<String>,
    /// Content delta
    #[serde(default)]
    pub content: Option<String>,
    /// Reasoning content delta (thinking process from models like Qwen3/DeepSeek)
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// Completed signed/redacted reasoning blocks for replay.
    #[serde(default)]
    pub reasoning_blocks: Option<Vec<ReasoningBlock>>,
    /// Provider-native assistant block metadata for lossless stream replay.
    #[serde(default)]
    pub assistant_blocks: Option<Vec<crate::agent::AssistantMessageBlock>>,
    /// Tool call delta
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

/// Incremental fragment of a streaming tool call
#[derive(Debug, Deserialize, Clone)]
pub struct DeltaToolCall {
    /// Provider-native coordinates; later deltas may omit them after identity
    /// has been established for this tool index.
    #[serde(default)]
    pub provider_source: Option<ProviderOutputSource>,
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
    fn assistant_blocks_round_trip_with_message_history() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut message = Message::assistant("已完成检查".to_string());
        let source = ProviderOutputSource {
            protocol: crate::llm::LlmApiProtocol::Responses,
            request_id: "resp".to_string(),
            item_id: "message-0".to_string(),
            output_index: 3,
            content_index: Some(1),
            content_kind: Some("refusal".to_string()),
            phase: Some("vendor_future_phase".to_string()),
        };
        message.assistant_blocks = Some(vec![crate::agent::AssistantMessageBlock {
            provider_source: Some(source.clone()),
            block_id: "resp:message:0".to_string(),
            request_id: "resp".to_string(),
            sequence: 0,
            phase: crate::agent::AssistantPhase::FinalAnswer,
            delta: "已完成检查".to_string(),
            complete: true,
            provider: Some("test".to_string()),
            reasoning: None,
        }]);
        message.approved_assistant_blocks = Some(Vec::new());
        message.execution_status = Some(crate::agent::ExecutionStatus {
            adapter_generated: false,
            execution_id: "execution-original".to_string(),
            request_id: Some("resp".to_string()),
            sequence: 4,
            phase: crate::agent::AgentPhase::Thinking,
            status: crate::agent::ExecutionStatusKind::Candidate,
            reason: None,
        });

        let encoded = serde_json::to_value(&message)?;
        let decoded: Message = serde_json::from_value(encoded)?;
        let block = decoded
            .assistant_blocks
            .as_deref()
            .and_then(|blocks| blocks.first())
            .ok_or("assistant block was lost during Message round-trip")?;
        assert_eq!(block.block_id, "resp:message:0");
        assert_eq!(block.phase, crate::agent::AssistantPhase::FinalAnswer);
        assert!(block.complete);
        assert_eq!(block.provider_source.as_ref(), Some(&source));
        assert_eq!(decoded.approved_assistant_blocks, Some(Vec::new()));
        assert_eq!(decoded.execution_status, message.execution_status);
        Ok(())
    }

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

    #[test]
    fn multimodal_token_estimate_charges_non_text_parts() {
        let tokenizer = crate::tokenizer::HeuristicTokenizer;
        let content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "hello".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/image.png".to_string(),
                    detail: None,
                },
            },
            ContentPart::File {
                name: "note.txt".to_string(),
                content: "YQ==".repeat(100),
            },
        ]);

        assert!(content.estimated_tokens(&tokenizer) >= 1_200);
    }

    #[test]
    fn linked_resource_stays_typed_and_has_deterministic_model_text() {
        let resource = LinkedResource {
            annotations: Some(serde_json::json!({"audience": ["assistant"]})),
            description: Some("source context".to_string()),
            mime_type: Some("text/rust".to_string()),
            name: "lib.rs".to_string(),
            size: Some(42),
            title: Some("Library".to_string()),
            uri: "file:///workspace/src/lib.rs".to_string(),
            meta: Some(serde_json::json!({"trace": "abc"})),
        };
        let content = MessageContent::Parts(vec![ContentPart::ResourceLink {
            resource: Box::new(resource.clone()),
        }]);
        let text = content.as_text().unwrap_or_default();
        assert!(text.starts_with("[Linked resource]\n"));
        assert!(text.contains("file:///workspace/src/lib.rs"));
        assert!(text.contains("source context"));

        let encoded = serde_json::to_string(&content).unwrap_or_default();
        let decoded = serde_json::from_str::<MessageContent>(&encoded).ok();
        let decoded_resource = decoded
            .and_then(MessageContent::into_parts)
            .and_then(|parts| parts.into_iter().next())
            .and_then(|part| match part {
                ContentPart::ResourceLink { resource } => Some(*resource),
                _ => None,
            });
        assert_eq!(decoded_resource, Some(resource));
    }

    #[test]
    fn message_round_trips_signed_and_redacted_reasoning_blocks() {
        let mut message = Message::assistant("answer".to_string());
        message.reasoning_blocks = Some(vec![
            ReasoningBlock::Signed {
                thinking: "reason".to_string(),
                signature: "signature".to_string(),
            },
            ReasoningBlock::Redacted {
                data: "opaque".to_string(),
            },
        ]);

        let encoded = serde_json::to_string(&message).unwrap_or_default();
        let decoded = serde_json::from_str::<Message>(&encoded).ok();
        assert_eq!(
            decoded.and_then(|value| value.reasoning_blocks),
            message.reasoning_blocks
        );
    }
}

#[cfg(test)]
mod public_transport_regression_tests {
    use super::*;

    #[test]
    fn explicit_function_strict_survives_declaration_serde()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for strict in [false, true] {
            let wire = serde_json::json!({"name":"inspect", "description":"检查🙂", "parameters":{"type":"object"}, "strict":strict});
            let function: FunctionSpec = serde_json::from_value(wire.clone())?;
            assert_eq!(serde_json::to_value(function)?, wire);
        }
        let legacy = serde_json::json!({"name":"inspect", "description":"检查🙂", "parameters":{}});
        let function: FunctionSpec = serde_json::from_value(legacy.clone())?;
        assert_eq!(serde_json::to_value(function)?, legacy);
        Ok(())
    }

    #[test]
    fn tool_error_observation_survives_manual_message_serde()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for is_error in [false, true] {
            let wire = serde_json::json!({"role":"tool", "content":[{"type":"text","text":"失败仍是正文🙂"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}], "name":"inspect", "tool_call_id":"call-1", "tool_result_is_error":is_error});
            let message: Message = serde_json::from_value(wire.clone())?;
            assert_eq!(serde_json::to_value(message)?, wire);
        }
        let unknown = Message::tool_result(
            "call-legacy".into(),
            "inspect".into(),
            "[Error] is ordinary legacy text".into(),
        );
        assert!(
            serde_json::to_value(unknown)?
                .get("tool_result_is_error")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn named_choice_is_a_request_object_not_a_stringified_object()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let named = serde_json::json!({"type":"function","function":{"name":"inspect"}});
        let request: ChatCompletionRequest = serde_json::from_value(
            serde_json::json!({"model":"test", "messages":[], "tool_choice":named}),
        )?;
        assert_eq!(
            serde_json::to_value(request)?.get("tool_choice"),
            Some(&named)
        );
        for invalid in [
            serde_json::json!("inspect"),
            serde_json::json!({"type":"function","name":"inspect"}),
            serde_json::json!({"type":"function","function":{"name":""}}),
        ] {
            assert!(
                serde_json::from_value::<ChatCompletionRequest>(
                    serde_json::json!({"model":"test","messages":[],"tool_choice":invalid})
                )
                .is_err()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod provider_response_observation_tests {
    use super::*;

    #[test]
    fn message_roundtrip_keeps_stop_presence_and_nested_usage()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for sequence in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!("结束🙂")),
        ] {
            let mut observation = serde_json::json!({
                "protocol":"anthropic", "provider":"anthropic", "request_id":"request-1",
                "model":"fixture", "stop_reason":"future_stop_reason", "status":null,
                "incomplete_details":null,
                "usage":{"output_tokens":7,"vendor":{"unknown":null,"label":"中文🙂"}}
            });
            if let Some(sequence) = sequence {
                observation
                    .as_object_mut()
                    .ok_or("observation must be an object")?
                    .insert("stop_sequence".into(), sequence);
            }
            let wire = serde_json::json!({
                "role":"assistant", "content":"部分正文🙂", "provider_response":observation
            });
            let message: Message = serde_json::from_value(wire.clone())?;
            assert_eq!(serde_json::to_value(message)?, wire);
        }
        Ok(())
    }

    #[test]
    fn observation_rejects_invalid_native_field_types()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for (field, invalid) in [
            ("model", serde_json::json!(123)),
            ("stop_reason", serde_json::json!(true)),
            ("stop_sequence", serde_json::json!([])),
            ("status", serde_json::json!({})),
            ("incomplete_details", serde_json::json!("not an object")),
            ("usage", serde_json::json!(false)),
        ] {
            let mut observation = serde_json::json!({
                "protocol":"responses", "provider":"fixture", "request_id":"request-2"
            });
            observation
                .as_object_mut()
                .ok_or("observation must be an object")?
                .insert(field.into(), invalid);
            assert!(
                serde_json::from_value::<Message>(serde_json::json!({
                    "role":"assistant", "content":"partial", "provider_response":observation
                }))
                .is_err()
            );
        }
        Ok(())
    }
}

// NEW_API_ONLY：必须先有Frame声明的DeltaMessage.provider_response与其Serialize合同。
// 不要求DeltaMessage新增Serialize，不声明第二metadata类型。
#[cfg(test)]
mod provider_response_delta_contract_tests {
    use super::DeltaMessage;
    use serde_json::json;

    #[test]
    fn delta_transports_unknown_status_and_explicit_null_without_runtime_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected = json!({"protocol": "responses", "provider": "configured-responses",
            "request_id": "resp-observed", "model": null, "status": "future_native_status",
            "incomplete_details": null, "usage": {"output_tokens": 10,
                "output_tokens_details": {"reasoning_tokens": 7}}});
        let delta: DeltaMessage = serde_json::from_value(json!({"provider_response": expected}))?;
        let observation = delta
            .provider_response
            .as_ref()
            .ok_or("response observation missing")?;
        assert_eq!(serde_json::to_value(observation)?, expected);
        Ok(())
    }
}

// POST_API：只调用immutable-r01已声明的公开类型/Message helper，不复制历史过滤逻辑。
#[cfg(test)]
mod response_observation_helper_contract_tests {
    use super::{Message, MessageContent, ProviderResponseMetadata, ReasoningBlock, Role};
    use serde_json::json;

    fn observation() -> Result<ProviderResponseMetadata, serde_json::Error> {
        serde_json::from_value(
            json!({"protocol":"anthropic","provider":"configured-provider",
            "request_id":"msg-observed","stop_reason":"max_tokens","stop_sequence":null,
            "usage":{"output_tokens":10,"native_extension":{"text":"原件🙂"}}}),
        )
    }

    #[test]
    fn observation_helper_marks_only_the_durable_failure_observation_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let recorded = Message::provider_response_observation(observation()?);
        assert!(recorded.is_provider_response_observation_only());
        assert_eq!(recorded.role, Role::Assistant);
        assert!(matches!(&recorded.content, MessageContent::Empty));
        assert!(recorded.tool_calls.is_none());
        assert!(recorded.reasoning_content.is_none());
        assert!(recorded.reasoning_blocks.is_none());
        assert!(recorded.assistant_blocks.is_none());
        assert!(recorded.execution_status.is_none());
        assert!(
            recorded
                .approved_assistant_blocks
                .as_ref()
                .is_some_and(Vec::is_empty)
        );
        let decoded: Message = serde_json::from_value(serde_json::to_value(&recorded)?)?;
        assert!(decoded.is_provider_response_observation_only());

        let mut with_tools = recorded.clone();
        with_tools.tool_calls = Some(serde_json::from_value(json!([{
            "id":"call-real","type":"function","function":{"name":"inspect","arguments":"{}"}
        }]))?);
        assert!(
            !with_tools.is_provider_response_observation_only(),
            "empty tool message must survive history filtering"
        );
        for reasoning in [
            ReasoningBlock::Signed {
                thinking: "private reasoning".into(),
                signature: "sig-original".into(),
            },
            ReasoningBlock::Redacted {
                data: "redacted-original".into(),
            },
            ReasoningBlock::Opaque {
                provider: "responses".into(),
                id: "opaque-id".into(),
                data: "opaque-original".into(),
                summary: vec!["摘要🙂".into()],
            },
        ] {
            let mut with_reasoning = recorded.clone();
            with_reasoning.reasoning_blocks = Some(vec![reasoning]);
            assert!(
                !with_reasoning.is_provider_response_observation_only(),
                "signed/redacted/opaque reasoning must survive filtering"
            );
        }
        let mut with_blocks = recorded.clone();
        with_blocks.assistant_blocks = Some(Vec::new());
        assert!(!with_blocks.is_provider_response_observation_only());
        let mut with_text = recorded.clone();
        with_text.content = MessageContent::Text(String::new());
        assert!(
            !with_text.is_provider_response_observation_only(),
            "ordinary text representation is not a synthetic observation"
        );
        let mut without_marker = recorded.clone();
        without_marker.approved_assistant_blocks = None;
        assert!(!without_marker.is_provider_response_observation_only());
        let mut user_record = recorded;
        user_record.role = Role::User;
        assert!(!user_record.is_provider_response_observation_only());
        Ok(())
    }

    #[test]
    fn native_observation_serialization_rejects_invalid_direct_field_assignments()
    -> Result<(), Box<dyn std::error::Error>> {
        for field in [
            "model",
            "stop_reason",
            "stop_sequence",
            "status",
            "incomplete_details",
            "usage",
        ] {
            let mut value = observation()?;
            match field {
                "model" => value.model = Some(json!(17)),
                "stop_reason" => value.stop_reason = Some(json!(false)),
                "stop_sequence" => value.stop_sequence = Some(json!([])),
                "status" => value.status = Some(json!({"fake":"settled"})),
                "incomplete_details" => value.incomplete_details = Some(json!(8)),
                "usage" => value.usage = Some(json!("not-an-object")),
                _ => return Err("unknown test field".into()),
            }
            assert!(
                serde_json::to_value(&value).is_err(),
                "invalid native field must fail on write: {field}"
            );
            let message = Message::provider_response_observation(value);
            assert!(
                serde_json::to_value(message).is_err(),
                "Message manual serializer must propagate the same error"
            );
        }
        Ok(())
    }
}
