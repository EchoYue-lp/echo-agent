//! OpenAI Chat Completions API 类型定义

use crate::tools::Tool;
use serde::{Deserialize, Serialize};

/// 对话消息，对应 OpenAI messages 数组中的单条记录
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Message {
    /// 角色：`user` / `assistant` / `system` / `tool`
    pub role: String,
    /// 文本内容（工具调用消息可能为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 工具调用列表（`assistant` 角色发起工具调用时携带）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// 工具名称（`tool` 角色使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 工具调用 ID，关联到对应的 `tool_call`（`tool` 角色使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        }
    }

    pub fn assistant_with_tools(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(tool_calls),
            name: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: String, name: String, content: String) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content),
            tool_calls: None,
            name: Some(name),
            tool_call_id: Some(tool_call_id),
        }
    }
}

/// LLM 发起的单次工具调用
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// 工具调用的函数信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    pub name: String,
    /// JSON 序列化的参数字符串
    pub arguments: String,
}

/// 结构化输出的 JSON Schema 规格
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonSchemaSpec {
    /// Schema 名称，用于模型识别（字母数字下划线）
    pub name: String,
    /// 标准 JSON Schema 对象
    pub schema: serde_json::Value,
    /// 是否强制严格遵守 schema（默认 true，推荐开启）
    #[serde(default = "default_true")]
    pub strict: bool,
}

fn default_true() -> bool {
    true
}

/// 响应格式控制，对应 OpenAI `response_format` 字段
///
/// - `Text`：默认，自由文本
/// - `JsonObject`：强制输出合法 JSON，不校验结构
/// - `JsonSchema`：按指定 JSON Schema 严格输出（需要模型支持 Structured Outputs）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// 默认文本格式
    Text,
    /// 强制输出合法 JSON 对象（不校验 schema）
    JsonObject,
    /// 按指定 JSON Schema 严格输出
    JsonSchema { json_schema: JsonSchemaSpec },
}

impl ResponseFormat {
    /// 快速创建 `JsonSchema` 格式
    pub fn json_schema(name: impl Into<String>, schema: serde_json::Value) -> Self {
        Self::JsonSchema {
            json_schema: JsonSchemaSpec {
                name: name.into(),
                schema,
                strict: true,
            },
        }
    }

    /// 是否为 JSON 输出格式（JsonObject 或 JsonSchema）
    pub fn is_json(&self) -> bool {
        matches!(self, Self::JsonObject | Self::JsonSchema { .. })
    }
}

/// OpenAI `/chat/completions` 请求体
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// 工具选择策略：`"auto"` / `"none"` / 指定工具名
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 结构化输出格式（None = 默认文本）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// 发送给 LLM 的工具定义（对应 OpenAI tools 数组元素）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSpec,
}

/// 工具的函数声明（name、描述和 JSON Schema 参数定义）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionResponse {
    id: String,
    #[serde(default)]
    pub(crate) choices: Vec<Choice>,
    #[serde(default)]
    created: Option<u64>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(flatten)]
    extra: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Choice {
    pub message: Message,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    index: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

// ── 流式响应类型 ──────────────────────────────────────────────────────────────

/// SSE 流式响应的单个 chunk
#[derive(Debug, Deserialize, Clone)]
pub struct ChatCompletionChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChunkChoice {
    pub delta: DeltaMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub index: u32,
}

/// 流式响应中的增量消息体
#[derive(Debug, Deserialize, Clone, Default)]
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
pub struct DeltaToolCall {
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: Option<DeltaFunctionCall>,
}

/// 流式函数调用的增量片段（name 和 arguments 逐步追加）
#[derive(Debug, Deserialize, Clone, Default)]
pub struct DeltaFunctionCall {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}
