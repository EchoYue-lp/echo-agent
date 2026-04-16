//! OpenAI Chat Completions API 类型定义

use crate::tools::Tool;
use serde::{Deserialize, Serialize};

// ── 多模态内容 ───────────────────────────────────────────────────────────────

/// 消息内容的单个组成部分（多模态）
///
/// 对应 OpenAI Vision / Anthropic 多模态 API 的 content parts 格式。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum ContentPart {
    /// 纯文本
    Text { text: String },
    /// 图片（Base64 编码或 URL）
    ImageUrl { image_url: ImageUrl },
    /// 文件附件（内联 Base64）
    File { name: String, content: String },
}

/// 图片 URL 或 Base64 数据
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageUrl {
    /// `data:image/png;base64,...` 或 `https://...`
    pub url: String,
    /// 可选细节级别：`"auto"` | `"low"` | `"high"`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 消息内容：兼容纯文本和多模态 parts 两种形式。
///
/// 序列化时：
/// - `Text("hello")` → `"hello"`（与旧版 API 完全兼容）
/// - `Parts([...])` → `[{"type":"text","text":"..."},...]`
#[derive(Debug, Clone)]
pub enum MessageContent {
    /// Plain text content.
    Text(String),
    /// Multimodal content parts.
    Parts(Vec<ContentPart>),
    /// Explicitly empty content payload.
    Empty,
}

impl Default for MessageContent {
    fn default() -> Self {
        MessageContent::Empty
    }
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
    /// 提取纯文本内容（多模态时拼接所有 Text 部分）
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

    /// 兼容旧版 `Option<String>::as_deref()` 调用点。
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

// ── 对话消息 ─────────────────────────────────────────────────────────────────

/// 对话消息
///
/// `content` 字段统一使用 [`MessageContent`] 枚举，支持纯文本和多模态内容。
#[derive(Debug, Clone, Default)]
pub struct Message {
    /// Message role such as `system`, `user`, `assistant`, or `tool`.
    pub role: String,
    /// Text or multimodal payload.
    pub content: MessageContent,
    /// Optional tool calls emitted by the assistant.
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Optional participant name.
    pub name: Option<String>,
    /// Optional tool call identifier for tool messages.
    pub tool_call_id: Option<String>,
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
        }
        let raw = RawMessage::deserialize(deserializer)?;
        let content = match raw.content {
            Some(serde_json::Value::String(s)) => MessageContent::Text(s),
            Some(serde_json::Value::Array(arr)) => {
                let parts: Vec<ContentPart> = serde_json::from_value(serde_json::Value::Array(arr))
                    .map_err(serde::de::Error::custom)?;
                MessageContent::Parts(parts)
            }
            Some(other) => MessageContent::Text(other.to_string()),
            None => MessageContent::Empty,
        };
        Ok(Message {
            role: raw.role,
            content,
            tool_calls: raw.tool_calls,
            name: raw.name,
            tool_call_id: raw.tool_call_id,
        })
    }
}

#[allow(missing_docs)]
impl Message {
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    /// 创建多模态用户消息
    pub fn user_multimodal(parts: Vec<ContentPart>) -> Self {
        Self {
            role: "user".to_string(),
            content: MessageContent::Parts(parts),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    /// 创建包含图片的用户消息
    ///
    /// # 示例
    ///
    /// ```rust
    /// use echo_core::llm::types::Message;
    ///
    /// let msg = Message::user_with_image(
    ///     "请描述这张图片",
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

    /// 创建包含图片 URL 的用户消息
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

    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Text(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Empty,
            tool_calls: Some(tool_calls),
            name: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: String, name: String, content: String) -> Self {
        Self {
            role: "tool".to_string(),
            content: MessageContent::Text(content),
            tool_calls: None,
            name: Some(name),
            tool_call_id: Some(tool_call_id),
        }
    }

    /// 获取纯文本内容。
    ///
    /// 若消息包含多模态内容，则提取并拼接所有 Text 部分。
    pub fn text_content(&self) -> Option<String> {
        self.content.as_text()
    }

    /// 兼容旧版直接读取 `content_parts` 的调用点。
    pub fn content_parts(&self) -> Option<&[ContentPart]> {
        self.content.parts()
    }

    /// 检查消息是否包含多模态内容
    pub fn is_multimodal(&self) -> bool {
        matches!(&self.content, MessageContent::Parts(_))
    }
}

/// LLM 发起的单次工具调用
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// 工具调用的函数信息
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// 结构化输出的 JSON Schema 规格
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct JsonSchemaSpec {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(default = "default_true")]
    pub strict: bool,
}

fn default_true() -> bool {
    true
}

/// 响应格式控制
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: JsonSchemaSpec },
}

#[allow(missing_docs)]
impl ResponseFormat {
    pub fn json_schema(name: impl Into<String>, schema: serde_json::Value) -> Self {
        Self::JsonSchema {
            json_schema: JsonSchemaSpec {
                name: name.into(),
                schema,
                strict: true,
            },
        }
    }

    pub fn is_json(&self) -> bool {
        matches!(self, Self::JsonObject | Self::JsonSchema { .. })
    }
}

/// OpenAI `/chat/completions` 请求体
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// 发送给 LLM 的工具定义
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSpec,
}

/// 工具的函数声明
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[allow(missing_docs)]
impl ToolDefinition {
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

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[allow(missing_docs)]
pub struct ChatCompletionResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Extra fields from the response that are not explicitly modeled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct Choice {
    pub message: Message,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
}

// ── 流式响应类型 ──────────────────────────────────────────────────────────────

/// SSE 流式响应的单个 chunk
#[derive(Debug, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct ChunkChoice {
    pub delta: DeltaMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub index: u32,
}

/// 流式响应中的增量消息体
#[derive(Debug, Deserialize, Clone, Default)]
#[allow(missing_docs)]
pub struct DeltaMessage {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

/// 流式工具调用的增量片段
#[derive(Debug, Deserialize, Clone)]
#[allow(missing_docs)]
pub struct DeltaToolCall {
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: Option<DeltaFunctionCall>,
}

/// 流式函数调用的增量片段
#[derive(Debug, Deserialize, Clone, Default)]
#[allow(missing_docs)]
pub struct DeltaFunctionCall {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

// ── Feature 6: 多模态支持测试 ────────────────────────────────────────────────

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
        assert_eq!(msg.role, "user");
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
        assert_eq!(msg.role, "user");
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
}
