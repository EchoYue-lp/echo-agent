use std::sync::Arc;

use serde_json::Value;

use super::server_config::{McpServerConfig, TransportConfig};
use super::transport::McpTransport;
use super::transport::http::HttpTransport;
use super::transport::sse::SseTransport;
use super::transport::stdio::StdioTransport;
use super::types::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcNotification,
    JsonRpcRequest, MCP_PROTOCOL_VERSION, McpContent, McpPrompt, McpPromptGetParams,
    McpPromptGetResult, McpPromptsListResult, McpResource, McpResourceReadParams,
    McpResourceReadResult, McpResourceTemplate, McpResourceTemplatesListResult,
    McpResourcesListResult, McpTool, McpToolCallParams, McpToolCallResult, McpToolsListResult,
    SUPPORTED_PROTOCOL_VERSIONS, ServerCapabilities,
};
use crate::redaction::{text as redact_text, url as redact_url};
use echo_core::error::{McpError, ReactError, Result};

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

/// Retry owner returned when MCP preparation fails and transport cleanup also
/// fails. The owner exposes cleanup only; it is never a usable initialized
/// client and cannot publish tools, resources, or prompts.
pub struct McpClientCleanupOwner {
    client: Arc<McpClient>,
}

impl McpClientCleanupOwner {
    /// Server identity associated with the failed preparation.
    pub fn server_name(&self) -> &str {
        self.client.server_name()
    }

    /// Retry settlement of the transport retained by this receipt.
    pub async fn retry_cleanup(&self) -> Result<()> {
        self.client.close().await
    }

    pub(crate) fn into_client(self) -> Arc<McpClient> {
        self.client
    }
}

impl std::fmt::Debug for McpClientCleanupOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpClientCleanupOwner")
            .field("server_name", &self.server_name())
            .finish_non_exhaustive()
    }
}

/// Typed preparation failure that preserves retry ownership when cleanup did
/// not settle. This follows Rust owner-returning error conventions: callers
/// can inspect the initialization cause and retry the retained cleanup owner.
#[derive(Debug)]
pub struct McpClientPreparationError {
    initialization_error: ReactError,
    cleanup_error: Option<String>,
    cleanup_owner: Option<McpClientCleanupOwner>,
}

impl McpClientPreparationError {
    fn settled(initialization_error: ReactError) -> Self {
        Self {
            initialization_error,
            cleanup_error: None,
            cleanup_owner: None,
        }
    }

    fn pending(
        initialization_error: ReactError,
        cleanup_error: ReactError,
        cleanup_owner: McpClientCleanupOwner,
    ) -> Self {
        Self {
            initialization_error,
            cleanup_error: Some(cleanup_error.to_string()),
            cleanup_owner: Some(cleanup_owner),
        }
    }

    /// Original initialize/notification/discovery failure.
    pub fn initialization_error(&self) -> &ReactError {
        &self.initialization_error
    }

    /// Last cleanup failure, when retry ownership remains outstanding.
    pub fn cleanup_error(&self) -> Option<&str> {
        self.cleanup_error.as_deref()
    }

    /// Borrow the retryable cleanup owner, when cleanup remains unsettled.
    pub fn cleanup_owner(&self) -> Option<&McpClientCleanupOwner> {
        self.cleanup_owner.as_ref()
    }

    /// Consume this error into the initialization cause and optional pending
    /// cleanup receipt. Dropping a returned receipt explicitly abandons retry.
    pub fn into_parts(self) -> (ReactError, Option<(String, McpClientCleanupOwner)>) {
        (
            self.initialization_error,
            self.cleanup_error.zip(self.cleanup_owner),
        )
    }
}

impl std::fmt::Display for McpClientPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "MCP initialization failed: {}",
            self.initialization_error
        )?;
        if let Some(cleanup_error) = &self.cleanup_error {
            write!(
                formatter,
                "; transport cleanup remains retryable after: {cleanup_error}"
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for McpClientPreparationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.initialization_error)
    }
}

/// Result of preparing one MCP client.
pub type McpClientPreparationResult<T> = std::result::Result<T, McpClientPreparationError>;

struct McpPreparationOwner {
    server_name: String,
    transport: Arc<dyn McpTransport>,
    runtime: tokio::runtime::Handle,
    armed: bool,
}

impl McpPreparationOwner {
    fn new(
        server_name: String,
        transport: Arc<dyn McpTransport>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            server_name,
            transport,
            runtime,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn transfer_cleanup(&mut self) -> McpClientCleanupOwner {
        self.disarm();
        McpClientCleanupOwner {
            client: McpClient::cleanup_only(self.server_name.clone(), Arc::clone(&self.transport)),
        }
    }
}

impl Drop for McpPreparationOwner {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let server_name = self.server_name.clone();
        let transport = Arc::clone(&self.transport);
        let _cleanup_task = self
            .runtime
            .spawn(settle_cancelled_preparation(server_name, transport));
    }
}

async fn settle_cancelled_preparation(server_name: String, transport: Arc<dyn McpTransport>) {
    let mut retry_delay_ms = 50_u64;
    loop {
        match transport.close().await {
            Ok(()) => {
                tracing::debug!(
                    server = %server_name,
                    "Cancelled MCP preparation transport cleanup settled"
                );
                return;
            }
            Err(error) => {
                tracing::warn!(
                    server = %server_name,
                    %error,
                    retry_delay_ms,
                    "Cancelled MCP preparation cleanup remains pending"
                );
                tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
                retry_delay_ms = retry_delay_ms.saturating_mul(2).min(5_000);
            }
        }
    }
}

impl McpClient {
    /// 连接到 MCP 服务端，完成握手和能力发现后返回 `Arc<McpClient>`
    pub async fn new(config: McpServerConfig) -> McpClientPreparationResult<Arc<Self>> {
        let transport: Arc<dyn McpTransport> = match config.transport {
            TransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => Arc::new(
                StdioTransport::new(&command, &args, &env, cwd.as_deref())
                    .await
                    .map_err(McpClientPreparationError::settled)?,
            ),
            TransportConfig::Http { base_url, headers } => {
                Arc::new(HttpTransport::new(base_url, headers))
            }
            TransportConfig::Sse { base_url, headers } => Arc::new(
                SseTransport::new(base_url, headers)
                    .await
                    .map_err(McpClientPreparationError::settled)?,
            ),
        };

        Self::from_transport(config.name, transport)?.await
    }

    /// Connect through an application-supplied transport while preserving the
    /// same initialize, notification and capability-discovery lifecycle as
    /// [`Self::new`]. This is the public consumer boundary used by SDK
    /// transport bridges; it does not let adapters bypass MCP negotiation.
    pub fn from_transport(
        server_name: impl Into<String>,
        transport: Arc<dyn McpTransport>,
    ) -> McpClientPreparationResult<
        impl std::future::Future<Output = McpClientPreparationResult<Arc<Self>>> + Send,
    > {
        let server_name = server_name.into();
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            McpClientPreparationError::pending(
                ReactError::Other(format!(
                    "MCP target '{server_name}' preparation requires an active Tokio runtime"
                )),
                ReactError::Other(format!(
                    "transport cleanup was not started because no Tokio runtime is active: {error}"
                )),
                McpClientCleanupOwner {
                    client: Self::cleanup_only(server_name.clone(), Arc::clone(&transport)),
                },
            )
        })?;
        let mut owner = McpPreparationOwner::new(server_name.clone(), transport, runtime);
        Ok(async move {
            let result =
                Self::initialize_from_transport(server_name, Arc::clone(&owner.transport)).await;
            match result {
                Ok(client) => {
                    owner.disarm();
                    Ok(client)
                }
                Err(initialization_error) => match owner.transport.close().await {
                    Ok(()) => {
                        owner.disarm();
                        Err(McpClientPreparationError::settled(initialization_error))
                    }
                    Err(cleanup_error) => {
                        let cleanup_owner = owner.transfer_cleanup();
                        Err(McpClientPreparationError::pending(
                            initialization_error,
                            cleanup_error,
                            cleanup_owner,
                        ))
                    }
                },
            }
        })
    }

    fn cleanup_only(server_name: String, transport: Arc<dyn McpTransport>) -> Arc<Self> {
        Arc::new(Self {
            transport,
            server_name,
            negotiated_version: String::new(),
            server_capabilities: ServerCapabilities::default(),
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        })
    }

    async fn initialize_from_transport(
        server_name: String,
        transport: Arc<dyn McpTransport>,
    ) -> Result<Arc<Self>> {
        tracing::info!("MCP: 正在连接服务端 '{}'", server_name);

        // ── Step 1: initialize 握手 ───────────────────────────────────────────
        let init_params = InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: Self::build_client_capabilities(),
            client_info: ClientInfo {
                name: "echo-agent".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                title: Some("Echo Agent MCP Client".to_string()),
                description: None,
                icons: Vec::new(),
                website_url: None,
            },
        };

        let init_req = JsonRpcRequest::new("initialize", Some(serde_json::to_value(init_params)?));
        let init_resp = transport.send(init_req).await?;

        if let Some(err) = init_resp.error {
            return Err(ReactError::Mcp(Box::new(McpError::InitializationFailed(
                redact_text(&err.message),
            ))));
        }

        let init_result: InitializeResult =
            serde_json::from_value(init_resp.result.ok_or_else(|| {
                ReactError::Mcp(Box::new(McpError::InitializationFailed(
                    "initialize 响应为空".to_string(),
                )))
            })?)?;

        let negotiated_version = init_result.protocol_version.clone();
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&negotiated_version.as_str()) {
            return Err(ReactError::Mcp(Box::new(McpError::InitializationFailed(
                format!(
                    "server selected unsupported MCP protocol version '{negotiated_version}'; supported versions: {}",
                    SUPPORTED_PROTOCOL_VERSIONS.join(", ")
                ),
            ))));
        }
        tracing::info!(
            "MCP: 已连接 '{}' (协议版本: {}, 请求版本: {})",
            server_name,
            negotiated_version,
            MCP_PROTOCOL_VERSION
        );
        if let Some(info) = &init_result.server_info {
            tracing::info!(
                "MCP: 服务端信息已接收 (name chars={}, version chars={})",
                info.name.chars().count(),
                info.version.chars().count()
            );
        }
        if let Some(instructions) = &init_result.instructions {
            tracing::info!(
                "MCP: 服务端指令已接收 (chars={})",
                instructions.chars().count()
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
            tools = Self::fetch_tools(&transport, &server_name).await?;
            tracing::info!("MCP: 从 '{}' 发现 {} 个工具", server_name, tools.len());
        }

        // 发现资源
        if server_capabilities.resources.is_some() {
            resources = Self::fetch_resources(&transport, &server_name).await?;
            tracing::info!("MCP: 从 '{}' 发现 {} 个资源", server_name, resources.len());
        }

        // 发现提示词
        if server_capabilities.prompts.is_some() {
            prompts = Self::fetch_prompts(&transport, &server_name).await?;
            tracing::info!("MCP: 从 '{}' 发现 {} 个提示词", server_name, prompts.len());
        }

        Ok(Arc::new(McpClient {
            transport,
            server_name,
            negotiated_version,
            server_capabilities,
            tools,
            resources,
            prompts,
        }))
    }

    /// 构建客户端能力声明
    fn build_client_capabilities() -> ClientCapabilities {
        // Do not advertise server-to-client callbacks until the transport has
        // a real request/notification dispatcher and typed handlers for them.
        // An empty capability object is truthful: this client currently only
        // sends requests and notifications to the MCP server.
        ClientCapabilities::default()
    }

    // ── 工具相关方法 ──────────────────────────────────────────────────────────

    /// 从传输层获取工具列表（支持分页，最大 100 页）
    async fn fetch_tools(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpTool>> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut iterations = 0_usize;
        const MAX_PAGINATION: usize = 100;

        loop {
            iterations = iterations.saturating_add(1);
            if iterations > MAX_PAGINATION {
                tracing::warn!(
                    "MCP: '{}' tools/list 达到最大分页限制 ({})，停止获取",
                    server_name,
                    MAX_PAGINATION
                );
                break;
            }

            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let req = JsonRpcRequest::new("tools/list", params);

            let resp =
                tokio::time::timeout(std::time::Duration::from_secs(30), transport.send(req))
                    .await
                    .map_err(|_| {
                        ReactError::Mcp(Box::new(McpError::ProtocolError(
                            "获取工具列表超时".to_string(),
                        )))
                    })??;

            if let Some(err) = resp.error {
                tracing::warn!(
                    "MCP: '{}' tools/list 返回错误: {}",
                    server_name,
                    redact_text(&err.message)
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
            return Err(ReactError::Mcp(Box::new(McpError::ToolCallFailed {
                code: err.code,
                message: format!("工具 '{}' 调用失败: {}", name, redact_text(&err.message)),
            })));
        }

        let result: McpToolCallResult = serde_json::from_value(resp.result.unwrap_or(Value::Null))?;
        Ok(result)
    }

    /// 获取此服务端提供的工具列表
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    // ── 资源相关方法 ──────────────────────────────────────────────────────────

    /// 从传输层获取资源列表（支持分页，最大 100 页）
    async fn fetch_resources(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpResource>> {
        let mut all_resources = Vec::new();
        let mut cursor: Option<String> = None;
        let mut iterations = 0_usize;
        const MAX_PAGINATION: usize = 100;

        loop {
            iterations = iterations.saturating_add(1);
            if iterations > MAX_PAGINATION {
                tracing::warn!(
                    "MCP: '{}' resources/list 达到最大分页限制 ({})，停止获取",
                    server_name,
                    MAX_PAGINATION
                );
                break;
            }

            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let req = JsonRpcRequest::new("resources/list", params);

            let resp =
                tokio::time::timeout(std::time::Duration::from_secs(30), transport.send(req))
                    .await
                    .map_err(|_| {
                        ReactError::Mcp(Box::new(McpError::ProtocolError(
                            "获取资源列表超时".to_string(),
                        )))
                    })??;

            if let Some(err) = resp.error {
                return Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "MCP 服务端 '{server_name}' 获取资源列表失败: {}",
                    redact_text(&err.message)
                )))));
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

    /// 获取最新资源列表，不修改握手阶段的缓存。
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        Self::fetch_resources(&self.transport, &self.server_name).await
    }

    /// 获取资源模板列表（支持分页，最大 100 页）。
    pub async fn list_resource_templates(&self) -> Result<Vec<McpResourceTemplate>> {
        let mut all_templates = Vec::new();
        let mut cursor: Option<String> = None;
        let mut iterations = 0_usize;
        const MAX_PAGINATION: usize = 100;

        loop {
            iterations = iterations.saturating_add(1);
            if iterations > MAX_PAGINATION {
                tracing::warn!(
                    "MCP: '{}' resources/templates/list 达到最大分页限制 ({})，停止获取",
                    self.server_name,
                    MAX_PAGINATION
                );
                break;
            }

            let params = cursor
                .as_ref()
                .map(|value| serde_json::json!({ "cursor": value }));
            let request = JsonRpcRequest::new("resources/templates/list", params);
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.transport.send(request),
            )
            .await
            .map_err(|_| {
                ReactError::Mcp(Box::new(McpError::ProtocolError(
                    "获取资源模板列表超时".to_string(),
                )))
            })??;

            if let Some(error) = response.error {
                return Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "MCP 服务端 '{}' 获取资源模板失败: {}",
                    self.server_name,
                    redact_text(&error.message)
                )))));
            }

            let result: McpResourceTemplatesListResult =
                serde_json::from_value(response.result.unwrap_or(Value::Null))?;
            all_templates.extend(result.resource_templates);
            cursor = result.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        Ok(all_templates)
    }

    /// 读取资源内容
    pub async fn read_resource(&self, uri: &str) -> Result<McpResourceReadResult> {
        let params = McpResourceReadParams {
            uri: uri.to_string(),
        };

        let req = JsonRpcRequest::new("resources/read", Some(serde_json::to_value(params)?));
        let resp = self.transport.send(req).await?;

        if let Some(err) = resp.error {
            return Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                "读取资源 '{}' 失败: {}",
                redact_url(uri),
                redact_text(&err.message)
            )))));
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

    /// 从传输层获取提示词列表（支持分页，最大 100 页）
    async fn fetch_prompts(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpPrompt>> {
        let mut all_prompts = Vec::new();
        let mut cursor: Option<String> = None;
        let mut iterations = 0;
        const MAX_PAGINATION: usize = 100;

        loop {
            iterations += 1;
            if iterations > MAX_PAGINATION {
                tracing::warn!(
                    "MCP: '{}' prompts/list 达到最大分页限制 ({})，停止获取",
                    server_name,
                    MAX_PAGINATION
                );
                break;
            }

            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let req = JsonRpcRequest::new("prompts/list", params);

            let resp =
                tokio::time::timeout(std::time::Duration::from_secs(30), transport.send(req))
                    .await
                    .map_err(|_| {
                        ReactError::Mcp(Box::new(McpError::ProtocolError(
                            "获取提示词列表超时".to_string(),
                        )))
                    })??;

            if let Some(err) = resp.error {
                tracing::warn!(
                    "MCP: '{}' prompts/list 返回错误: {}",
                    server_name,
                    redact_text(&err.message)
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
            return Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                "获取提示词 '{}' 失败: {}",
                name,
                redact_text(&err.message)
            )))));
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
            return Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                "ping 失败: {}",
                redact_text(&err.message)
            )))));
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
    pub async fn close(&self) -> Result<()> {
        self.transport.close().await
    }

    /// 将 McpContent 列表转换为可读文本
    pub fn content_to_text(content: &[McpContent]) -> String {
        content
            .iter()
            .map(|c| match c {
                McpContent::Text { text } => text.clone(),
                McpContent::Image { mime_type, .. } => format!("[图片: {}]", mime_type),
                McpContent::Resource { resource } => {
                    let name = resource.name.as_deref().unwrap_or("unnamed");
                    format!("[资源: {} ({})]", name, resource.uri)
                }
                McpContent::Audio { mime_type, .. } => format!("[音频: {}]", mime_type),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[cfg(test)]
    pub(crate) fn with_test_transport(
        server_name: impl Into<String>,
        transport: Arc<dyn McpTransport>,
    ) -> Arc<Self> {
        Arc::new(Self {
            transport,
            server_name: server_name.into(),
            negotiated_version: MCP_PROTOCOL_VERSION.to_string(),
            server_capabilities: ServerCapabilities {
                resources: Some(super::types::ResourcesCapability::default()),
                ..ServerCapabilities::default()
            },
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Mutex;

    struct FailingInitializeTransport {
        closed: Arc<AtomicBool>,
    }

    struct RetryCleanupInitializeTransport {
        close_count: Arc<std::sync::atomic::AtomicUsize>,
        failures_remaining: std::sync::atomic::AtomicUsize,
    }

    struct BlockingInitializeTransport {
        send_started: Arc<tokio::sync::Notify>,
        close_count: Arc<std::sync::atomic::AtomicUsize>,
        failures_remaining: std::sync::atomic::AtomicUsize,
    }

    async fn prepare_test_client(
        server_name: &str,
        transport: Arc<dyn McpTransport>,
    ) -> McpClientPreparationResult<Arc<McpClient>> {
        match McpClient::from_transport(server_name, transport) {
            Ok(preparation) => preparation.await,
            Err(error) => Err(error),
        }
    }

    impl McpTransport for FailingInitializeTransport {
        fn send(
            &self,
            request: JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            Box::pin(async move {
                Ok(super::super::types::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: None,
                    error: Some(super::super::types::JsonRpcError {
                        code: -32000,
                        message: "initialization rejected".to_string(),
                        data: None,
                    }),
                })
            })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            self.closed.store(true, Ordering::Release);
            Box::pin(async { Ok(()) })
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    impl McpTransport for RetryCleanupInitializeTransport {
        fn send(
            &self,
            request: JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            Box::pin(async move {
                Ok(super::super::types::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: None,
                    error: Some(super::super::types::JsonRpcError {
                        code: -32000,
                        message: "initialization rejected".to_string(),
                        data: None,
                    }),
                })
            })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            self.close_count.fetch_add(1, Ordering::AcqRel);
            let fail = self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
            Box::pin(async move {
                if fail {
                    Err(ReactError::Other("injected cleanup failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    impl McpTransport for BlockingInitializeTransport {
        fn send(
            &self,
            _request: JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            let send_started = Arc::clone(&self.send_started);
            Box::pin(async move {
                send_started.notify_waiters();
                std::future::pending().await
            })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            self.close_count.fetch_add(1, Ordering::AcqRel);
            let fail = self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
            Box::pin(async move {
                if fail {
                    Err(ReactError::Other(
                        "injected cancelled-preparation cleanup failure".to_string(),
                    ))
                } else {
                    Ok(())
                }
            })
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    #[tokio::test]
    async fn from_transport_closes_after_initialization_failure() {
        let closed = Arc::new(AtomicBool::new(false));
        let transport: Arc<dyn McpTransport> = Arc::new(FailingInitializeTransport {
            closed: closed.clone(),
        });
        assert!(prepare_test_client("fixture", transport).await.is_err());
        assert!(closed.load(Ordering::Acquire));
    }

    #[test]
    fn from_transport_without_runtime_returns_cleanup_owner_synchronously() {
        let closed = Arc::new(AtomicBool::new(false));
        let transport: Arc<dyn McpTransport> = Arc::new(FailingInitializeTransport {
            closed: Arc::clone(&closed),
        });
        let error = McpClient::from_transport("no-runtime", transport)
            .err()
            .unwrap_or_else(|| {
                McpClientPreparationError::settled(ReactError::Other(
                    "preparation unexpectedly accepted without a runtime".to_string(),
                ))
            });

        assert!(error.cleanup_owner().is_some());
        assert!(error.to_string().contains("active Tokio runtime"));
        assert!(!closed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn preparation_error_retains_failed_cleanup_for_retry() -> std::result::Result<(), String>
    {
        let close_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport: Arc<dyn McpTransport> = Arc::new(RetryCleanupInitializeTransport {
            close_count: Arc::clone(&close_count),
            failures_remaining: std::sync::atomic::AtomicUsize::new(1),
        });
        let error = prepare_test_client("fixture", transport)
            .await
            .err()
            .ok_or_else(|| "failed initialization was accepted".to_string())?;
        assert!(error.cleanup_error().is_some());
        let owner = error
            .cleanup_owner()
            .ok_or_else(|| "failed cleanup owner was not retained".to_string())?;
        assert_eq!(owner.server_name(), "fixture");
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        owner
            .retry_cleanup()
            .await
            .map_err(|cleanup_error| cleanup_error.to_string())?;
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_preparation_transfers_cleanup_to_owned_retry_task()
    -> std::result::Result<(), String> {
        let send_started = Arc::new(tokio::sync::Notify::new());
        let close_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport: Arc<dyn McpTransport> = Arc::new(BlockingInitializeTransport {
            send_started: Arc::clone(&send_started),
            close_count: Arc::clone(&close_count),
            failures_remaining: std::sync::atomic::AtomicUsize::new(1),
        });
        let started = send_started.notified();
        let preparation_future =
            McpClient::from_transport("cancelled", transport).map_err(|error| error.to_string())?;
        let preparation = tokio::spawn(preparation_future);
        tokio::time::timeout(std::time::Duration::from_secs(1), started)
            .await
            .map_err(|_| "initialization did not start".to_string())?;
        preparation.abort();
        let join_error = preparation
            .await
            .err()
            .ok_or_else(|| "cancelled preparation unexpectedly completed".to_string())?;
        assert!(join_error.is_cancelled());

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while close_count.load(Ordering::Acquire) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "owned cleanup did not retry to settlement".to_string())?;
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        Ok(())
    }

    struct RecordingInitializeTransport {
        initialize: Arc<Mutex<Option<JsonRpcRequest>>>,
    }

    struct VersionInitializeTransport {
        protocol_version: String,
        initialized: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
    }

    impl McpTransport for VersionInitializeTransport {
        fn send(
            &self,
            request: JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            let protocol_version = self.protocol_version.clone();
            Box::pin(async move {
                Ok(super::super::types::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(serde_json::json!({
                        "protocolVersion": protocol_version,
                        "capabilities": {},
                        "serverInfo": {"name": "version-fixture", "version": "1.0"}
                    })),
                    error: None,
                })
            })
        }

        fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            if notification.method == "notifications/initialized" {
                self.initialized.store(true, Ordering::Release);
            }
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            self.closed.store(true, Ordering::Release);
            Box::pin(async { Ok(()) })
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    #[tokio::test]
    async fn client_accepts_supported_versions_and_rejects_unknown_selection()
    -> std::result::Result<(), String> {
        for version in SUPPORTED_PROTOCOL_VERSIONS {
            let initialized = Arc::new(AtomicBool::new(false));
            let closed = Arc::new(AtomicBool::new(false));
            let transport: Arc<dyn McpTransport> = Arc::new(VersionInitializeTransport {
                protocol_version: (*version).to_string(),
                initialized: initialized.clone(),
                closed: closed.clone(),
            });
            let client = prepare_test_client("supported", transport)
                .await
                .map_err(|error| error.to_string())?;
            assert_eq!(client.protocol_version(), *version);
            assert!(initialized.load(Ordering::Acquire));
            assert!(!closed.load(Ordering::Acquire));
        }

        let initialized = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let transport: Arc<dyn McpTransport> = Arc::new(VersionInitializeTransport {
            protocol_version: "2099-01-01".to_string(),
            initialized: initialized.clone(),
            closed: closed.clone(),
        });
        let error = prepare_test_client("unsupported", transport)
            .await
            .err()
            .ok_or_else(|| "unknown protocol version was accepted".to_string())?;
        match error.initialization_error() {
            ReactError::Mcp(error) => match error.as_ref() {
                McpError::InitializationFailed(message) => {
                    assert!(message.contains("2099-01-01"));
                    assert!(message.contains(MCP_PROTOCOL_VERSION));
                }
                other => return Err(format!("unexpected MCP error: {other}")),
            },
            other => return Err(format!("unexpected error: {other}")),
        }
        assert!(!initialized.load(Ordering::Acquire));
        assert!(closed.load(Ordering::Acquire));
        Ok(())
    }

    impl McpTransport for RecordingInitializeTransport {
        fn send(
            &self,
            request: JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            let initialize = self.initialize.clone();
            Box::pin(async move {
                {
                    let mut recorded = initialize.lock().await;
                    if request.method == "initialize" {
                        *recorded = Some(request.clone());
                    }
                }

                let result = if request.method == "initialize" {
                    serde_json::json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {},
                        "serverInfo": {"name": "fixture", "version": "1.0"}
                    })
                } else {
                    serde_json::json!({"tools": []})
                };

                Ok(super::super::types::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(result),
                    error: None,
                })
            })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    #[tokio::test]
    async fn initialize_wire_does_not_advertise_unimplemented_callbacks()
    -> std::result::Result<(), String> {
        let initialize = Arc::new(Mutex::new(None));
        let transport: Arc<dyn McpTransport> = Arc::new(RecordingInitializeTransport {
            initialize: initialize.clone(),
        });

        let client = prepare_test_client("fixture", transport)
            .await
            .map_err(|error| error.to_string())?;
        let request = initialize
            .lock()
            .await
            .clone()
            .ok_or_else(|| "initialize request was not recorded".to_string())?;
        let params = request
            .params
            .ok_or_else(|| "initialize params were missing".to_string())?;
        let capabilities = params
            .get("capabilities")
            .ok_or_else(|| "capabilities were missing".to_string())?;

        assert_eq!(request.method, "initialize");
        assert_eq!(
            params.get("protocolVersion"),
            Some(&serde_json::json!(MCP_PROTOCOL_VERSION))
        );
        assert_eq!(capabilities, &serde_json::json!({}));
        assert!(client.server_capabilities().tools.is_none());
        Ok(())
    }
}
