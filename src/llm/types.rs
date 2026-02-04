use crate::tools::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    /// 角色：user, assistant, system, tool
    pub role: String,

    /// 消息内容（可选，因为调用工具时可能没有文本）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// 工具调用列表（assistant 角色使用）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    /// 工具名称（tool 角色使用，标识是哪个工具的返回值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// 工具调用ID（tool 角色使用，关联到对应的 tool_call）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolCall {
    /// 工具调用的唯一ID
    pub id: String,

    /// 类型，通常是 "function"
    #[serde(rename = "type")]
    pub call_type: String,

    /// 函数调用详情
    pub function: FunctionCall,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    /// 函数名称
    pub name: String,

    /// JSON 格式的参数
    pub arguments: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,

    /// 可用的工具列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    /// 工具选择策略："auto", "none", 或指定工具
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String, // "function"
    pub function: FunctionSpec,
}

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
    finish_reason: Option<String>,
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
