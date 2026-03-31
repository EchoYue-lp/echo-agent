use serde::{Deserialize, Serialize};
use serde_json::Value;

/// MCP 协议版本（当前支持的最新稳定版本：2025-03-26）
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

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
pub struct ClientCapabilities {
    /// Roots 能力：客户端可提供文件系统根目录列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    /// Sampling 能力：客户端可代表服务端调用 LLM
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
    /// Elicitation 能力：客户端可向用户请求信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<ElicitationCapability>,
    /// 实验性能力
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Value>,
}

/// Roots 能力配置
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct RootsCapability {
    /// 是否支持 roots/list_changed 通知
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Sampling 能力配置（空对象表示支持）
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SamplingCapability {}

/// Elicitation 能力配置（空对象表示支持）
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ElicitationCapability {}

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
    /// 指令信息（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// 服务端能力声明
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ServerCapabilities {
    /// 工具能力
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    /// 资源能力
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    /// 提示词能力
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    /// 日志能力
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
    /// 实验性能力
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Value>,
}

/// 工具能力配置
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ToolsCapability {
    /// 服务端是否支持 tools/list_changed 通知
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// 资源能力配置
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ResourcesCapability {
    /// 是否支持 resources/subscribe
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    /// 服务端是否支持 resources/list_changed 通知
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// 提示词能力配置
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PromptsCapability {
    /// 服务端是否支持 prompts/list_changed 通知
    #[serde(rename = "listChanged", skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// 日志能力配置（空对象表示支持）
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct LoggingCapability {}

/// 服务端身份信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
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
    /// 工具的输出模式（可选）
    #[serde(rename = "outputSchema", skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// 是否为只读工具（可选）
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
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

// ── MCP 资源相关类型 ──────────────────────────────────────────────────────────

/// MCP 资源定义（来自 resources/list）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResource {
    /// 资源 URI
    pub uri: String,
    /// 资源名称
    pub name: String,
    /// 资源描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME 类型
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// 资源大小（字节）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// 资源元数据
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// resources/list 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResourcesListResult {
    pub resources: Vec<McpResource>,
    /// 分页游标（如有下一页）
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// resources/read 请求参数
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResourceReadParams {
    pub uri: String,
}

/// resources/read 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResourceReadResult {
    pub contents: Vec<McpResourceContents>,
}

/// 资源内容
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpResourceContents {
    Text {
        uri: String,
        #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
    },
    Blob {
        uri: String,
        #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        blob: String, // Base64 编码
    },
}

/// 资源模板定义（来自 resources/templates/list）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResourceTemplate {
    /// 模板 URI（包含参数占位符）
    #[serde(rename = "uriTemplate")]
    pub uri_template: String,
    /// 模板名称
    pub name: String,
    /// 模板描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME 类型
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// 模板元数据
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// resources/templates/list 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResourceTemplatesListResult {
    #[serde(rename = "resourceTemplates")]
    pub resource_templates: Vec<McpResourceTemplate>,
    /// 分页游标
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

// ── MCP 提示词相关类型 ────────────────────────────────────────────────────────

/// MCP 提示词定义（来自 prompts/list）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpPrompt {
    /// 提示词名称
    pub name: String,
    /// 提示词描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 提示词参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<McpPromptArgument>>,
}

/// 提示词参数定义
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpPromptArgument {
    /// 参数名称
    pub name: String,
    /// 参数描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 是否必填
    #[serde(default)]
    pub required: bool,
}

/// prompts/list 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpPromptsListResult {
    pub prompts: Vec<McpPrompt>,
    /// 分页游标
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// prompts/get 请求参数
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpPromptGetParams {
    pub name: String,
    /// 参数值映射
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<std::collections::HashMap<String, String>>,
}

/// prompts/get 响应结果
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpPromptGetResult {
    /// 提示词描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 消息列表
    pub messages: Vec<McpPromptMessage>,
}

/// 提示词消息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpPromptMessage {
    /// 角色：user 或 assistant
    pub role: McpPromptMessageRole,
    /// 消息内容
    pub content: McpContent,
}

/// 提示词消息角色
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpPromptMessageRole {
    User,
    Assistant,
}

// ── MCP 内容块类型 ─────────────────────────────────────────────────────────────

/// MCP 内容块（文本 / 图片 / 资源引用 / 音频）
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
        resource: McpResourceLink,
    },
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// 资源链接（嵌入在内容中的资源引用）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpResourceLink {
    pub uri: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl McpContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            McpContent::Text { text } => Some(text),
            _ => None,
        }
    }
}

// ── 通知接收器 ────────────────────────────────────────────────────────────────

use std::sync::Mutex;
use tokio::sync::broadcast;

/// 通知接收者 trait，用于接收服务端推送的通知
pub trait JsonRpcNotificationReceiver: Send + Sync {
    fn try_recv(&self) -> Option<JsonRpcNotification>;
}

/// broadcast::Receiver 的包装类型，实现 JsonRpcNotificationReceiver trait
pub struct NotificationReceiver(Mutex<broadcast::Receiver<JsonRpcNotification>>);

impl NotificationReceiver {
    pub fn new(rx: broadcast::Receiver<JsonRpcNotification>) -> Self {
        Self(Mutex::new(rx))
    }
}

impl JsonRpcNotificationReceiver for NotificationReceiver {
    fn try_recv(&self) -> Option<JsonRpcNotification> {
        self.0.lock().unwrap().try_recv().ok()
    }
}
