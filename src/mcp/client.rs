use std::sync::Arc;

use serde_json::Value;

use crate::error::{McpError, ReactError, Result};
use crate::mcp::server_config::{McpServerConfig, TransportConfig};
use crate::mcp::transport::http::HttpTransport;
use crate::mcp::transport::stdio::StdioTransport;
use crate::mcp::transport::McpTransport;
use crate::mcp::types::{
    ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcNotification,
    JsonRpcRequest, McpContent, McpTool, McpToolCallParams, McpToolCallResult, McpToolsListResult,
};

/// MCP 客户端
///
/// 管理与单个 MCP 服务端的完整生命周期：
/// 1. 连接 → 2. 握手（initialize） → 3. 工具发现（tools/list） → 4. 工具调用（tools/call）
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    server_name: String,
    tools: Vec<McpTool>,
}

impl McpClient {
    /// 连接到 MCP 服务端，完成握手和工具发现后返回 Arc<McpClient>
    pub async fn new(config: McpServerConfig) -> Result<Arc<Self>> {
        let transport: Arc<dyn McpTransport> = match config.transport {
            TransportConfig::Stdio { command, args, env } => {
                Arc::new(StdioTransport::new(&command, &args, &env).await?)
            }
            TransportConfig::Http { base_url, headers } => {
                Arc::new(HttpTransport::new(base_url, headers))
            }
        };

        tracing::info!("MCP: 正在连接服务端 '{}'", config.name);

        // ── Step 1: initialize 握手 ───────────────────────────────────────────
        let init_params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "echo-agent".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let init_req = JsonRpcRequest::new(
            "initialize",
            Some(serde_json::to_value(init_params)?),
        );

        let init_resp = transport.send(init_req).await?;

        if let Some(err) = init_resp.error {
            return Err(ReactError::Mcp(McpError::InitializationFailed(err.message)));
        }

        let init_result: InitializeResult = serde_json::from_value(
            init_resp
                .result
                .ok_or_else(|| {
                    ReactError::Mcp(McpError::InitializationFailed(
                        "initialize 响应为空".to_string(),
                    ))
                })?,
        )?;

        tracing::info!(
            "MCP: 已连接 '{}' (协议版本: {})",
            config.name,
            init_result.protocol_version
        );
        if let Some(info) = &init_result.server_info {
            tracing::info!("MCP: 服务端信息: {} v{}", info.name, info.version);
        }

        // ── Step 2: 发送 initialized 通知 ────────────────────────────────────
        transport
            .notify(JsonRpcNotification::new("notifications/initialized", None))
            .await?;

        // ── Step 3: 发现工具 ─────────────────────────────────────────────────
        let tools = Self::fetch_tools(&transport, &config.name).await?;
        tracing::info!(
            "MCP: 从 '{}' 发现 {} 个工具",
            config.name,
            tools.len()
        );
        for tool in &tools {
            tracing::debug!(
                "MCP:   工具 '{}' - {}",
                tool.name,
                tool.description.as_deref().unwrap_or("(无描述)")
            );
        }

        Ok(Arc::new(McpClient {
            transport,
            server_name: config.name,
            tools,
        }))
    }

    /// 从传输层获取工具列表（支持分页）
    async fn fetch_tools(
        transport: &Arc<dyn McpTransport>,
        server_name: &str,
    ) -> Result<Vec<McpTool>> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = cursor
                .as_ref()
                .map(|c| serde_json::json!({ "cursor": c }));

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

    // ── 公共 API ─────────────────────────────────────────────────────────────

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

        let result: McpToolCallResult =
            serde_json::from_value(resp.result.unwrap_or(Value::Null))?;

        Ok(result)
    }

    /// 获取此服务端提供的工具列表
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// 服务端标识名称
    pub fn server_name(&self) -> &str {
        &self.server_name
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
                McpContent::Resource { resource } => format!("[资源: {}]", resource),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
