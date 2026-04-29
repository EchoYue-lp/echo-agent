//! MCP Server —— 将 echo-agent 的 Tool 暴露为 MCP 服务端
//!
//! 支持将任意 [`Tool`](echo_core::tools::Tool) 实现暴露给 MCP 客户端（如 Claude Desktop、
//! Cursor、其他 MCP 客户端），实现双向 MCP 互操作。
//!
//! # 支持的传输方式
//!
//! | 传输 | 方法 | 场景 |
//! |------|------|------|
//! | Stdio | [`serve_stdio`](McpServer::serve_stdio) | CLI 工具、子进程、npx 集成 |
//!
//! # 快速开始
//!
//! ```rust,no_run
//! use echo_integration::mcp::server::McpServer;
//! use std::sync::Arc;
//! # use echo_core::tools::{Tool, ToolResult, ToolParameters};
//! # use futures::future::BoxFuture;
//! # use echo_core::error::Result;
//!
//! # struct MyTool;
//! # impl Tool for MyTool {
//! #     fn name(&self) -> &str { "my_tool" }
//! #     fn description(&self) -> &str { "A tool" }
//! #     fn parameters(&self) -> serde_json::Value { serde_json::json!({}) }
//! #     fn execute(&self, _: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
//! #         Box::pin(async { Ok(ToolResult::success("ok")) })
//! #     }
//! # }
//!
//! # #[tokio::main]
//! # async fn main() -> echo_core::error::Result<()> {
//! let server = McpServer::builder()
//!     .name("my-agent-server")
//!     .version("0.1.0")
//!     .tool(Arc::new(MyTool))
//!     .build();
//!
//! server.serve_stdio().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # 从已有工具列表快速构建
//!
//! ```rust,no_run
//! use echo_integration::mcp::server::McpServer;
//! use std::sync::Arc;
//! # use echo_core::tools::Tool;
//!
//! # fn get_tools() -> Vec<Arc<dyn Tool>> { vec![] }
//! let server = McpServer::from_tools(get_tools())
//!     .name("echo-agent")
//!     .version("0.1.0")
//!     .build();
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use super::types::{
    InitializeParams, InitializeResult, JsonRpcError, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, MCP_PROTOCOL_VERSION, McpContent, McpPrompt, McpResource, McpTool,
    McpToolCallParams, McpToolCallResult, McpToolsListResult, PromptsCapability,
    ResourcesCapability, SUPPORTED_PROTOCOL_VERSIONS, ServerCapabilities, ServerInfo,
    ToolsCapability,
};
use echo_core::error::{McpError, ReactError, Result};
use echo_core::tools::Tool;

// ── JSON-RPC 标准错误码 ──────────────────────────────────────────────────────

const ERR_PARSE: i32 = -32700;
const ERR_INVALID_REQUEST: i32 = -32600;
const ERR_METHOD_NOT_FOUND: i32 = -32601;
const ERR_INVALID_PARAMS: i32 = -32602;
const ERR_INTERNAL: i32 = -32603;

// ── McpServer ────────────────────────────────────────────────────────────────

/// MCP 服务端：将 echo-agent 的 Tool 暴露为标准 MCP 服务
///
/// 支持 MCP 2025-11-25（最新）及向下兼容 2025-03-26 / 2024-11-05，处理：
/// - `initialize` 握手与**版本协商**
/// - `tools/list` 工具发现
/// - `tools/call` 工具执行
/// - `ping` 健康检查
pub struct McpServer {
    name: String,
    version: String,
    tools: HashMap<String, Arc<dyn Tool>>,
    tool_list: Vec<McpTool>,
    instructions: Option<String>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
}

impl McpServer {
    /// 创建服务端构建器
    pub fn builder() -> McpServerBuilder {
        McpServerBuilder::new()
    }

    /// 从工具列表快速创建构建器
    ///
    /// 返回的 `McpServerBuilder` 可继续链式调用 `.name()` / `.version()` 等方法。
    pub fn from_tools(tools: Vec<Arc<dyn Tool>>) -> McpServerBuilder {
        let mut builder = McpServerBuilder::new();
        for tool in tools {
            builder = builder.tool(tool);
        }
        builder
    }

    /// 处理单个 JSON-RPC 请求字符串，返回 JSON 响应字符串
    ///
    /// 适用于自定义传输层集成（HTTP、WebSocket 等）或测试场景。
    /// 对于 stdio 传输，直接使用 [`serve_stdio`](McpServer::serve_stdio)。
    pub async fn handle_json_rpc(&self, request_json: &str) -> String {
        let json: Value = match serde_json::from_str(request_json) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: ERR_PARSE,
                        message: format!("JSON parse error: {e}"),
                        data: None,
                    }),
                };
                return serde_json::to_string(&err_resp).unwrap_or_default();
            }
        };

        if json.get("id").is_some() {
            let request: JsonRpcRequest = match serde_json::from_value(json) {
                Ok(r) => r,
                Err(e) => {
                    let err_resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: ERR_INVALID_REQUEST,
                            message: format!("Invalid JSON-RPC request: {e}"),
                            data: None,
                        }),
                    };
                    return serde_json::to_string(&err_resp).unwrap_or_default();
                }
            };
            let response = self.handle_request(request).await;
            serde_json::to_string(&response).unwrap_or_default()
        } else {
            if let Ok(notification) = serde_json::from_value::<JsonRpcNotification>(json) {
                self.handle_notification(&notification);
            }
            String::new()
        }
    }

    // ── Stdio 传输 ───────────────────────────────────────────────────────────

    /// 以 Stdio 模式运行 MCP 服务端
    ///
    /// 从 stdin 读取 JSON-RPC 请求（每行一个 JSON），
    /// 将响应写入 stdout（每行一个 JSON）。
    /// 直到 stdin 关闭（EOF）时退出。
    ///
    /// 这是最常用的 MCP 服务端模式，适用于：
    /// - Claude Desktop 集成
    /// - Cursor / VS Code 集成
    /// - `npx` / CLI 工具
    pub async fn serve_stdio(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        tracing::info!(
            "MCP Server '{}' v{} 已启动 (stdio)，等待客户端连接...",
            self.name,
            self.version
        );

        loop {
            let line = match lines.next_line().await {
                Ok(Some(l)) => l,
                Ok(None) => {
                    tracing::debug!("MCP Server: stdin EOF，服务端退出");
                    break;
                }
                Err(e) => {
                    tracing::warn!("MCP Server: 读取 stdin 失败: {}", e);
                    break;
                }
            };

            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let json: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    let err_resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: ERR_PARSE,
                            message: format!("JSON 解析失败: {e}"),
                            data: None,
                        }),
                    };
                    write_response(&stdout, &err_resp).await?;
                    continue;
                }
            };

            if json.get("id").is_some() {
                let request: JsonRpcRequest = match serde_json::from_value(json) {
                    Ok(r) => r,
                    Err(e) => {
                        let err_resp = JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: None,
                            result: None,
                            error: Some(JsonRpcError {
                                code: ERR_INVALID_REQUEST,
                                message: format!("无效的 JSON-RPC 请求: {e}"),
                                data: None,
                            }),
                        };
                        write_response(&stdout, &err_resp).await?;
                        continue;
                    }
                };

                let response = self.handle_request(request).await;
                write_response(&stdout, &response).await?;
            } else {
                // 通知消息（无 id），静默处理
                if let Ok(notification) = serde_json::from_value::<JsonRpcNotification>(json) {
                    self.handle_notification(&notification);
                }
            }
        }

        tracing::info!("MCP Server '{}' 已停止", self.name);
        Ok(())
    }

    // ── 请求分发 ─────────────────────────────────────────────────────────────

    async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone();

        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(request.params),
            "ping" => Ok(serde_json::json!({})),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tools_call(request.params).await,
            "resources/list" => self.handle_resources_list(),
            "resources/read" => self.handle_resources_read(request.params),
            "prompts/list" => self.handle_prompts_list(),
            "prompts/get" => self.handle_prompts_get(request.params),
            method => {
                tracing::debug!("MCP Server: 未知方法 '{}'", method);
                Err(JsonRpcError {
                    code: ERR_METHOD_NOT_FOUND,
                    message: format!("Method not found: {method}"),
                    data: None,
                })
            }
        };

        match result {
            Ok(value) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(value),
                error: None,
            },
            Err(err) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(err),
            },
        }
    }

    fn handle_notification(&self, notification: &JsonRpcNotification) {
        match notification.method.as_str() {
            "notifications/initialized" => {
                tracing::info!("MCP Server: 客户端初始化完成");
            }
            "notifications/cancelled" => {
                tracing::debug!("MCP Server: 客户端取消了请求");
            }
            method => {
                tracing::debug!("MCP Server: 收到通知 '{}'", method);
            }
        }
    }

    // ── 协议方法实现 ─────────────────────────────────────────────────────────

    fn handle_initialize(&self, params: Option<Value>) -> std::result::Result<Value, JsonRpcError> {
        let negotiated_version = if let Some(params) = params {
            match serde_json::from_value::<InitializeParams>(params) {
                Ok(init) => {
                    tracing::info!(
                        "MCP Server: 客户端 '{}' v{} 请求初始化 (协议版本: {})",
                        init.client_info.name,
                        init.client_info.version,
                        init.protocol_version,
                    );

                    // 版本协商：验证客户端请求的版本是否在支持列表中
                    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&init.protocol_version.as_str()) {
                        return Err(JsonRpcError {
                            code: ERR_INVALID_REQUEST,
                            message: format!(
                                "不支持的协议版本 '{}'. 支持的版本: {}",
                                init.protocol_version,
                                SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                            ),
                            data: None,
                        });
                    }

                    // 客户端请求的版本在支持列表中，回复相同版本
                    init.protocol_version.clone()
                }
                Err(e) => {
                    tracing::warn!("MCP Server: 解析 initialize 参数失败: {}", e);
                    MCP_PROTOCOL_VERSION.to_string()
                }
            }
        } else {
            MCP_PROTOCOL_VERSION.to_string()
        };

        let result = InitializeResult {
            protocol_version: negotiated_version,
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(false),
                }),
                resources: if self.resources.is_empty() {
                    None
                } else {
                    Some(ResourcesCapability {
                        subscribe: Some(false),
                        list_changed: Some(false),
                    })
                },
                prompts: if self.prompts.is_empty() {
                    None
                } else {
                    Some(PromptsCapability {
                        list_changed: Some(false),
                    })
                },
                logging: None,
                completions: None,
                experimental: None,
            },
            server_info: Some(ServerInfo {
                name: self.name.clone(),
                version: self.version.clone(),
                title: None,
                description: self.instructions.clone(),
                icons: Vec::new(),
                website_url: None,
            }),
            instructions: self.instructions.clone(),
        };

        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化 initialize 结果失败: {e}"),
            data: None,
        })
    }

    fn handle_tools_list(&self) -> std::result::Result<Value, JsonRpcError> {
        let result = McpToolsListResult {
            tools: self.tool_list.clone(),
            next_cursor: None,
        };

        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化工具列表失败: {e}"),
            data: None,
        })
    }

    async fn handle_tools_call(
        &self,
        params: Option<Value>,
    ) -> std::result::Result<Value, JsonRpcError> {
        let params: McpToolCallParams = serde_json::from_value(params.unwrap_or(Value::Null))
            .map_err(|e| JsonRpcError {
                code: ERR_INVALID_PARAMS,
                message: format!("无效的 tools/call 参数: {e}"),
                data: None,
            })?;

        // 验证 arguments 必须是 Object 类型（如果存在）
        if let Some(ref args) = params.arguments
            && !args.is_object()
        {
            return Err(JsonRpcError {
                code: ERR_INVALID_PARAMS,
                message: "tools/call arguments 必须是 JSON Object".to_string(),
                data: None,
            });
        }

        let tool = self.tools.get(&params.name).ok_or_else(|| JsonRpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("工具 '{}' 不存在", params.name),
            data: None,
        })?;

        tracing::info!("MCP Server: 执行工具 '{}'", params.name);

        let tool_params: echo_core::tools::ToolParameters =
            if let Some(Value::Object(map)) = params.arguments {
                map.into_iter().collect()
            } else {
                HashMap::new()
            };

        let call_result = match tool.execute(tool_params).await {
            Ok(result) => {
                let text = if result.success {
                    result.output
                } else {
                    result.error.unwrap_or(result.output)
                };
                McpToolCallResult {
                    content: vec![McpContent::Text { text }],
                    is_error: !result.success,
                    structured_content: None,
                    extra: serde_json::Map::new(),
                }
            }
            Err(e) => McpToolCallResult {
                content: vec![McpContent::Text {
                    text: format!("工具执行错误: {e}"),
                }],
                is_error: true,
                structured_content: None,
                extra: serde_json::Map::new(),
            },
        };

        serde_json::to_value(call_result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化工具结果失败: {e}"),
            data: None,
        })
    }

    fn handle_resources_list(&self) -> std::result::Result<Value, JsonRpcError> {
        use super::types::McpResourcesListResult;
        let result = McpResourcesListResult {
            resources: self.resources.clone(),
            next_cursor: None,
        };
        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化资源列表失败: {e}"),
            data: None,
        })
    }

    fn handle_resources_read(
        &self,
        params: Option<Value>,
    ) -> std::result::Result<Value, JsonRpcError> {
        use super::types::McpResourceReadParams;
        let params: McpResourceReadParams = serde_json::from_value(params.unwrap_or(Value::Null))
            .map_err(|e| JsonRpcError {
            code: ERR_INVALID_PARAMS,
            message: format!("无效的 resources/read 参数: {e}"),
            data: None,
        })?;

        let resource = self
            .resources
            .iter()
            .find(|r| r.uri == params.uri)
            .ok_or_else(|| JsonRpcError {
                code: ERR_INVALID_PARAMS,
                message: format!("资源 '{}' 不存在", params.uri),
                data: None,
            })?;

        let result = super::types::McpResourceReadResult {
            contents: vec![super::types::McpResourceContents::Text {
                uri: resource.uri.clone(),
                mime_type: resource.mime_type.clone(),
                text: format!("Resource: {}", resource.name),
            }],
        };
        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化资源内容失败: {e}"),
            data: None,
        })
    }

    fn handle_prompts_list(&self) -> std::result::Result<Value, JsonRpcError> {
        use super::types::McpPromptsListResult;
        let result = McpPromptsListResult {
            prompts: self.prompts.clone(),
            next_cursor: None,
        };
        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化提示词列表失败: {e}"),
            data: None,
        })
    }

    fn handle_prompts_get(
        &self,
        params: Option<Value>,
    ) -> std::result::Result<Value, JsonRpcError> {
        use super::types::McpPromptGetParams;
        let params: McpPromptGetParams = serde_json::from_value(params.unwrap_or(Value::Null))
            .map_err(|e| JsonRpcError {
                code: ERR_INVALID_PARAMS,
                message: format!("无效的 prompts/get 参数: {e}"),
                data: None,
            })?;

        let prompt = self
            .prompts
            .iter()
            .find(|p| p.name == params.name)
            .ok_or_else(|| JsonRpcError {
                code: ERR_INVALID_PARAMS,
                message: format!("提示词 '{}' 不存在", params.name),
                data: None,
            })?;

        let result = super::types::McpPromptGetResult {
            description: prompt.description.clone(),
            messages: vec![],
        };
        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: ERR_INTERNAL,
            message: format!("序列化提示词内容失败: {e}"),
            data: None,
        })
    }
}

// ── McpServerBuilder ─────────────────────────────────────────────────────────

/// MCP 服务端构建器
pub struct McpServerBuilder {
    name: String,
    version: String,
    tools: Vec<Arc<dyn Tool>>,
    instructions: Option<String>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
}

impl McpServerBuilder {
    fn new() -> Self {
        Self {
            name: "echo-agent".to_string(),
            version: "0.1.0".to_string(),
            tools: Vec::new(),
            instructions: None,
            resources: Vec::new(),
            prompts: Vec::new(),
        }
    }

    /// 设置服务端名称（显示在 MCP 客户端中）
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 设置服务端版本号
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// 注册单个工具
    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// 批量注册工具
    pub fn tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// 设置服务端指令说明（在 initialize 响应中返回给客户端）
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// 注册单个资源
    pub fn resource(mut self, resource: McpResource) -> Self {
        self.resources.push(resource);
        self
    }

    /// 批量注册资源
    pub fn resources(mut self, resources: Vec<McpResource>) -> Self {
        self.resources.extend(resources);
        self
    }

    /// 注册单个提示词
    pub fn prompt(mut self, prompt: McpPrompt) -> Self {
        self.prompts.push(prompt);
        self
    }

    /// 批量注册提示词
    pub fn prompts(mut self, prompts: Vec<McpPrompt>) -> Self {
        self.prompts.extend(prompts);
        self
    }

    /// 构建 MCP 服务端
    pub fn build(self) -> McpServer {
        let mut tool_map: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        let mut tool_list: Vec<McpTool> = Vec::new();

        for tool in self.tools {
            let mcp_tool = McpTool {
                name: tool.name().to_string(),
                title: None,
                description: Some(tool.description().to_string()),
                input_schema: tool.parameters(),
                output_schema: None,
                icons: Vec::new(),
                annotations: None,
                execution: None,
                meta: None,
            };
            tool_map.insert(tool.name().to_string(), tool);
            tool_list.push(mcp_tool);
        }

        tracing::debug!(
            "MCP Server '{}' 已构建，注册 {} 个工具: {:?}",
            self.name,
            tool_list.len(),
            tool_list.iter().map(|t| &t.name).collect::<Vec<_>>()
        );

        McpServer {
            name: self.name,
            version: self.version,
            tools: tool_map,
            tool_list,
            instructions: self.instructions,
            resources: self.resources,
            prompts: self.prompts,
        }
    }
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

async fn write_response(
    stdout: &Arc<Mutex<tokio::io::Stdout>>,
    response: &JsonRpcResponse,
) -> Result<()> {
    let line = serde_json::to_string(response)
        .map_err(|e| ReactError::Mcp(McpError::ProtocolError(format!("序列化响应失败: {e}"))))?
        + "\n";

    let mut stdout = stdout.lock().await;
    stdout
        .write_all(line.as_bytes())
        .await
        .map_err(|e| ReactError::Mcp(McpError::ProtocolError(format!("写入 stdout 失败: {e}"))))?;
    stdout
        .flush()
        .await
        .map_err(|e| ReactError::Mcp(McpError::ProtocolError(format!("flush stdout 失败: {e}"))))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::{ToolParameters, ToolResult};
    use futures::future::BoxFuture;

    struct AddTool;

    impl Tool for AddTool {
        fn name(&self) -> &str {
            "add"
        }
        fn description(&self) -> &str {
            "Add two numbers"
        }
        fn parameters(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number", "description": "First number" },
                    "b": { "type": "number", "description": "Second number" }
                },
                "required": ["a", "b"]
            })
        }
        fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
            Box::pin(async move {
                let a = params.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let b = params.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                Ok(ToolResult::success(format!("{}", a + b)))
            })
        }
    }

    #[test]
    fn test_builder_basic() {
        let server = McpServer::builder()
            .name("test-server")
            .version("1.0.0")
            .tool(Arc::new(AddTool))
            .build();

        assert_eq!(server.name, "test-server");
        assert_eq!(server.version, "1.0.0");
        assert_eq!(server.tools.len(), 1);
        assert_eq!(server.tool_list.len(), 1);
        assert_eq!(server.tool_list[0].name, "add");
    }

    #[test]
    fn test_from_tools() {
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(AddTool)];
        let server = McpServer::from_tools(tools).name("from-tools").build();

        assert_eq!(server.name, "from-tools");
        assert_eq!(server.tools.len(), 1);
    }

    fn make_init_params(version: &str) -> Value {
        serde_json::to_value(InitializeParams {
            protocol_version: version.to_string(),
            capabilities: crate::mcp::types::ClientCapabilities::default(),
            client_info: crate::mcp::types::ClientInfo {
                name: "test-client".to_string(),
                version: "1.0".to_string(),
                title: None,
                description: None,
                icons: Vec::new(),
                website_url: None,
            },
        })
        .unwrap()
    }

    #[test]
    fn test_handle_initialize() {
        let server = McpServer::builder()
            .name("test")
            .version("0.1.0")
            .instructions("Test server")
            .tool(Arc::new(AddTool))
            .build();

        let params = make_init_params(MCP_PROTOCOL_VERSION);
        let result = server.handle_initialize(Some(params)).unwrap();
        let init: InitializeResult = serde_json::from_value(result).unwrap();

        assert_eq!(init.protocol_version, MCP_PROTOCOL_VERSION);
        assert!(init.capabilities.tools.is_some());
        assert_eq!(init.server_info.as_ref().unwrap().name, "test");
        assert_eq!(init.instructions.unwrap(), "Test server");
    }

    #[test]
    fn test_version_negotiation_echo_supported() {
        let server = McpServer::builder().build();

        // 客户端请求旧版本 2025-03-26 → 服务端回复相同版本
        let params = make_init_params("2025-03-26");
        let result = server.handle_initialize(Some(params)).unwrap();
        let init: InitializeResult = serde_json::from_value(result).unwrap();
        assert_eq!(init.protocol_version, "2025-03-26");

        // 客户端请求 2024-11-05 → 同样回复该版本
        let params = make_init_params("2024-11-05");
        let result = server.handle_initialize(Some(params)).unwrap();
        let init: InitializeResult = serde_json::from_value(result).unwrap();
        assert_eq!(init.protocol_version, "2024-11-05");
    }

    #[test]
    fn test_version_negotiation_unsupported_rejects() {
        let server = McpServer::builder().build();

        // 客户端请求不支持的版本 → 服务端返回错误
        let params = make_init_params("2099-01-01");
        let err = server.handle_initialize(Some(params)).unwrap_err();
        assert_eq!(err.code, ERR_INVALID_REQUEST);
        assert!(err.message.contains("不支持的协议版本"));
    }

    #[test]
    fn test_version_negotiation_latest() {
        let server = McpServer::builder().build();

        // 客户端请求最新版本 → 回复最新版本
        let params = make_init_params("2025-11-25");
        let result = server.handle_initialize(Some(params)).unwrap();
        let init: InitializeResult = serde_json::from_value(result).unwrap();
        assert_eq!(init.protocol_version, "2025-11-25");
    }

    #[test]
    fn test_old_client_without_new_fields() {
        let server = McpServer::builder().build();

        // 模拟旧客户端（无 title/icons 字段），serde 应正常反序列化
        let params = serde_json::json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "old-client", "version": "0.1" }
        });
        let result = server.handle_initialize(Some(params)).unwrap();
        let init: InitializeResult = serde_json::from_value(result).unwrap();
        assert_eq!(init.protocol_version, "2025-03-26");
    }

    #[test]
    fn test_handle_tools_list() {
        let server = McpServer::builder().tool(Arc::new(AddTool)).build();

        let result = server.handle_tools_list().unwrap();
        let list: McpToolsListResult = serde_json::from_value(result).unwrap();

        assert_eq!(list.tools.len(), 1);
        assert_eq!(list.tools[0].name, "add");
        assert_eq!(
            list.tools[0].description.as_deref(),
            Some("Add two numbers")
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call() {
        let server = McpServer::builder().tool(Arc::new(AddTool)).build();

        let params = serde_json::to_value(McpToolCallParams {
            name: "add".to_string(),
            arguments: Some(serde_json::json!({ "a": 3, "b": 7 })),
        })
        .unwrap();

        let result = server.handle_tools_call(Some(params)).await.unwrap();
        let call_result: McpToolCallResult = serde_json::from_value(result).unwrap();

        assert!(!call_result.is_error);
        assert_eq!(call_result.content.len(), 1);
        assert_eq!(call_result.content[0].as_text(), Some("10"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_not_found() {
        let server = McpServer::builder().build();

        let params = serde_json::to_value(McpToolCallParams {
            name: "nonexistent".to_string(),
            arguments: None,
        })
        .unwrap();

        let err = server.handle_tools_call(Some(params)).await.unwrap_err();
        assert_eq!(err.code, ERR_INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_request_dispatch() {
        let server = McpServer::builder().tool(Arc::new(AddTool)).build();

        // ping
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(1.into())),
            method: "ping".to_string(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());

        // unknown method
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(2.into())),
            method: "unknown/method".to_string(),
            params: None,
        };
        let resp = server.handle_request(req).await;
        assert_eq!(resp.error.unwrap().code, ERR_METHOD_NOT_FOUND);
    }
}
