use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── JSON-RPC 2.0 核心类型 ─────────────────────────────────────────────────────

/// JSON-RPC 2.0 请求（Client → Server）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    /// 请求 ID（由传输层自动填充，调用方可留空）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 响应（Server → Client）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 通知（单向，无需响应）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 错误对象
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ── MCP 工具相关类型 ──────────────────────────────────────────────────────────

/// MCP 工具定义（来自 tools/list）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 工具参数的 JSON Schema
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// tools/list 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpToolsListResult {
    pub tools: Vec<McpTool>,
    /// 分页游标（如有下一页）
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// tools/call 请求参数
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpToolCallParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

/// tools/call 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpToolCallResult {
    pub content: Vec<McpContent>,
    /// 为 true 时表示工具执行出错（但协议层成功）
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// MCP 内容块（文本 / 图片 / 资源引用）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        resource: Value,
    },
}

impl McpContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            McpContent::Text { text } => Some(text),
            _ => None,
        }
    }
}

// ── MCP 握手类型 ──────────────────────────────────────────────────────────────

/// initialize 请求参数
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

/// 客户端能力声明
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ClientCapabilities {}

/// 客户端身份信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// initialize 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo", skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
}

/// 服务端能力声明
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
}

/// 工具能力配置
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolsCapability {
    /// 服务端是否支持 tools/list_changed 通知
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// 服务端身份信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}
