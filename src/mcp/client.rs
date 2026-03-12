use std::sync::Arc;

use serde_json::Value;

use crate::error::{McpError, ReactError, Result};
use crate::mcp::server_config::{McpServerConfig, TransportConfig};
use crate::mcp::transport::McpTransport;
use crate::mcp::transport::http::HttpTransport;
use crate::mcp::transport::sse::SseTransport;
use crate::mcp::transport::stdio::StdioTransport;
use crate::mcp::types::{
    ClientCapabilities, ClientInfo, ElicitationCapability, InitializeParams, InitializeResult,
    JsonRpcNotification, JsonRpcRequest, MCP_PROTOCOL_VERSION, McpContent, McpPrompt,
    McpPromptGetParams, McpPromptGetResult, McpPromptsListResult, McpResource,
    McpResourceReadParams, McpResourceReadResult, McpResourcesListResult, McpTool,
    McpToolCallParams, McpToolCallResult, McpToolsListResult, RootsCapability, SamplingCapability,
    ServerCapabilities,
};

/// MCP 客户端
///
/// 管理与单个 MCP 服务端的完整生命周期：
/// 1. 连接 → 2. 握手（initialize） → 3. 能力发现 → 4. 功能调用
///
/// 支持的功能：
/// - **Tools**: 工具发现与调用
/// - **Resources**: 资源列表与读取
/// - **Prompts**: 提示词列表与获取
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    server_name: String,
    /// 协商后的协议版本
    negotiated_version: String,
    /// 服务端能力
    server_capabilities: ServerCapabilities,
    /// 已发现的工具（缓存）
    tools: Vec<McpTool>,
    /// 已发现的资源（缓存）
    resources: Vec<McpResource>,
    /// 已发现的提示词（缓存）
    prompts: Vec<McpPrompt>,
}

impl McpClient {
    /// 连接到 MCP 服务端，完成握手和能力发现后返回 Arc<McpClient>
    pub async fn new(config: McpServerConfig) -> Result<Arc<Self>> {
        let transport: Arc<dyn McpTransport> = match config.transport {
            TransportConfig::Stdio { command, args, env } => {
                Arc::new(StdioTransport::new(&command, &args, &env).await?)
            }
            TransportConfig::Http { base_url, headers } => {
                Arc::new(HttpTransport::new(base_url, headers))
            }
            TransportConfig::Sse { base_url, headers } => {
                Arc::new(SseTransport::new(base_url, headers).await?)
            }
        };

        tracing::info!("MCP: 正在连接服务端 '{}'", config.name);

        // ── Step 1: initialize 握手 ───────────────────────────────────────────
        let init_params = InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: Self::build_client_capabilities(),
            client_info: ClientInfo {
                name: "echo-agent".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let init_req = JsonRpcRequest::new("initialize", Some(serde_json::to_value(init_params)?));
        let init_resp = transport.send(init_req).await?;

        if let Some(err) = init_resp.error {
            return Err(ReactError::Mcp(McpError::InitializationFailed(err.message)));
        }

        let init_result: InitializeResult =
            serde_json::from_value(init_resp.result.ok_or_else(|| {
                ReactError::Mcp(McpError::InitializationFailed(
                    "initialize 响应为空".to_string(),
                ))
            })?)?;

        let negotiated_version = init_result.protocol_version.clone();
        tracing::info!(
            "MCP: 已连接 '{}' (协议版本: {}, 请求版本: {})",
            config.name,
            negotiated_version,
            MCP_PROTOCOL_VERSION
        );
        if let Some(info) = &init_result.server_info {
            tracing::info!("MCP: 服务端信息: {} v{}", info.name, info.version);
        }
        if let Some(instructions) = &init_result.instructions {
            tracing::info!(
                "MCP: 服务端指令: {}",
                instructions.chars().take(100).collect::<String>()
            );
        }

        // ── Step 2: 发送 initialized 通知 ────────────────────────────────────
        transport
            .notify(JsonRpcNotification::new("notifications/initialized", None))
            .await?;

        // ── Step 3: 能力发现 ─────────────────────────────────────────────────
        let server_capabilities = init_result.capabilities;
        let mut tools = Vec::new();
        let mut resources = Vec::new();
        let mut prompts = Vec::new();

        // 发现工具
        if server_capabilities.tools.is_some() {
            tools = Self::fetch_tools(&transport, &config.name).await?;
            tracing::info!("MCP: 从 '{}' 发现 {} 个工具", config.name, tools.len());
        }

        // 发现资源
        if server_capabilities.resources.is_some() {
            resources = Self::fetch_resources(&transport, &config.name).await?;
            tracing::info!("MCP: 从 '{}' 发现 {} 个资源", config.name, resources.len());
        }

        // 发现提示词
        if server_capabilities.prompts.is_some() {
            prompts = Self::fetch_prompts(&transport, &config.name).await?;
            tracing::info!("MCP: 从 '{}' 发现 {} 个提示词", config.name, prompts.len());
        }

        Ok(Arc::new(McpClient {
            transport,
            server_name: config.name,
            negotiated_version,
            server_capabilities,
            tools,
            resources,
            prompts,
        }))
    }

    /// 构建客户端能力声明
    fn build_client_capabilities() -> ClientCapabilities {
        ClientCapabilities {
            roots: Some(RootsCapability {
                list_changed: Some(true),
            }),
            sampling: Some(SamplingCapability::default()),
            elicitation: Some(ElicitationCapability::default()),
            experimental: None,
        }
    }

    // ── 工具相关方法 ──────────────────────────────────────────────────────────

    /// 从传输层获取工具列表（支持分页）
    async fn fetch_tools(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpTool>> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let req = JsonRpcRequest::new("tools/list", params);
            let resp = transport.send(req).await?;

            if let Some(err) = resp.error {
                tracing::warn!(
                    "MCP: '{}' tools/list 返回错误: {}",
                    server_name,
                    err.message
                );
                break;
            }

            let result: McpToolsListResult =
                serde_json::from_value(resp.result.unwrap_or(Value::Null))?;

            all_tools.extend(result.tools);
            cursor = result.next_cursor;

            if cursor.is_none() {
                break;
            }
        }

        Ok(all_tools)
    }

    /// 刷新工具列表（重新从服务端获取）
    pub async fn refresh_tools(&mut self) -> Result<()> {
        self.tools = Self::fetch_tools(&self.transport, &self.server_name).await?;
        tracing::info!(
            "MCP: '{}' 工具列表已刷新，共 {} 个",
            self.server_name,
            self.tools.len()
        );
        Ok(())
    }

    /// 调用 MCP 工具
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolCallResult> {
        let params = McpToolCallParams {
            name: name.to_string(),
            arguments: Some(arguments),
        };

        let req = JsonRpcRequest::new("tools/call", Some(serde_json::to_value(params)?));
        let resp = self.transport.send(req).await?;

        if let Some(err) = resp.error {
            return Err(ReactError::Mcp(McpError::ToolCallFailed(format!(
                "工具 '{}' 调用失败: {}",
                name, err.message
            ))));
        }

        let result: McpToolCallResult = serde_json::from_value(resp.result.unwrap_or(Value::Null))?;
        Ok(result)
    }

    /// 获取此服务端提供的工具列表
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    // ── 资源相关方法 ──────────────────────────────────────────────────────────

    /// 从传输层获取资源列表（支持分页）
    async fn fetch_resources(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpResource>> {
        let mut all_resources = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let req = JsonRpcRequest::new("resources/list", params);
            let resp = transport.send(req).await?;

            if let Some(err) = resp.error {
                tracing::warn!(
                    "MCP: '{}' resources/list 返回错误: {}",
                    server_name,
                    err.message
                );
                break;
            }

            let result: McpResourcesListResult =
                serde_json::from_value(resp.result.unwrap_or(Value::Null))?;

            all_resources.extend(result.resources);
            cursor = result.next_cursor;

            if cursor.is_none() {
                break;
            }
        }

        Ok(all_resources)
    }

    /// 刷新资源列表（重新从服务端获取）
    pub async fn refresh_resources(&mut self) -> Result<()> {
        self.resources = Self::fetch_resources(&self.transport, &self.server_name).await?;
        tracing::info!(
            "MCP: '{}' 资源列表已刷新，共 {} 个",
            self.server_name,
            self.resources.len()
        );
        Ok(())
    }

    /// 读取资源内容
    pub async fn read_resource(&self, uri: &str) -> Result<McpResourceReadResult> {
        let params = McpResourceReadParams {
            uri: uri.to_string(),
        };

        let req = JsonRpcRequest::new("resources/read", Some(serde_json::to_value(params)?));
        let resp = self.transport.send(req).await?;

        if let Some(err) = resp.error {
            return Err(ReactError::Mcp(McpError::ProtocolError(format!(
                "读取资源 '{}' 失败: {}",
                uri, err.message
            ))));
        }

        let result: McpResourceReadResult =
            serde_json::from_value(resp.result.unwrap_or(Value::Null))?;
        Ok(result)
    }

    /// 获取此服务端提供的资源列表
    pub fn resources(&self) -> &[McpResource] {
        &self.resources
    }

    /// 检查服务端是否支持资源功能
    pub fn supports_resources(&self) -> bool {
        self.server_capabilities.resources.is_some()
    }

    // ── 提示词相关方法 ────────────────────────────────────────────────────────

    /// 从传输层获取提示词列表（支持分页）
    async fn fetch_prompts(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpPrompt>> {
        let mut all_prompts = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let req = JsonRpcRequest::new("prompts/list", params);
            let resp = transport.send(req).await?;

            if let Some(err) = resp.error {
                tracing::warn!(
                    "MCP: '{}' prompts/list 返回错误: {}",
                    server_name,
                    err.message
                );
                break;
            }

            let result: McpPromptsListResult =
                serde_json::from_value(resp.result.unwrap_or(Value::Null))?;

            all_prompts.extend(result.prompts);
            cursor = result.next_cursor;

            if cursor.is_none() {
                break;
            }
        }

        Ok(all_prompts)
    }

    /// 刷新提示词列表（重新从服务端获取）
    pub async fn refresh_prompts(&mut self) -> Result<()> {
        self.prompts = Self::fetch_prompts(&self.transport, &self.server_name).await?;
        tracing::info!(
            "MCP: '{}' 提示词列表已刷新，共 {} 个",
            self.server_name,
            self.prompts.len()
        );
        Ok(())
    }

    /// 获取提示词内容
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<std::collections::HashMap<String, String>>,
    ) -> Result<McpPromptGetResult> {
        let params = McpPromptGetParams {
            name: name.to_string(),
            arguments,
        };

        let req = JsonRpcRequest::new("prompts/get", Some(serde_json::to_value(params)?));
        let resp = self.transport.send(req).await?;

        if let Some(err) = resp.error {
            return Err(ReactError::Mcp(McpError::ProtocolError(format!(
                "获取提示词 '{}' 失败: {}",
                name, err.message
            ))));
        }

        let result: McpPromptGetResult =
            serde_json::from_value(resp.result.unwrap_or(Value::Null))?;
        Ok(result)
    }

    /// 获取此服务端提供的提示词列表
    pub fn prompts(&self) -> &[McpPrompt] {
        &self.prompts
    }

    /// 检查服务端是否支持提示词功能
    pub fn supports_prompts(&self) -> bool {
        self.server_capabilities.prompts.is_some()
    }

    // ── 其他方法 ──────────────────────────────────────────────────────────────

    /// 发送 ping 请求（健康检查）
    pub async fn ping(&self) -> Result<()> {
        let req = JsonRpcRequest::new("ping", None);
        let resp = self.transport.send(req).await?;

        if let Some(err) = resp.error {
            return Err(ReactError::Mcp(McpError::ProtocolError(format!(
                "ping 失败: {}",
                err.message
            ))));
        }

        Ok(())
    }

    /// 服务端标识名称
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// 协商后的协议版本
    pub fn protocol_version(&self) -> &str {
        &self.negotiated_version
    }

    /// 服务端能力
    pub fn server_capabilities(&self) -> &ServerCapabilities {
        &self.server_capabilities
    }

    /// 关闭连接（stdio 传输会终止子进程）
    pub async fn close(&self) {
        self.transport.close().await;
    }

    /// 将 McpContent 列表转换为可读文本
    pub fn content_to_text(content: &[McpContent]) -> String {
        content
            .iter()
            .map(|c| match c {
                McpContent::Text { text } => text.clone(),
                McpContent::Image { mime_type, .. } => format!("[图片: {}]", mime_type),
                McpContent::Resource { resource } => format!("[资源: {}]", resource.uri),
                McpContent::Audio { mime_type, .. } => format!("[音频: {}]", mime_type),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
