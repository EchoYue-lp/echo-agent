use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION};
use echo_core::error::{McpError, ReactError, Result};

use super::McpTransport;

/// HTTP 传输层（MCP Streamable HTTP）
///
/// 通过 HTTP POST 发送 JSON-RPC 请求，适用于远程 MCP 服务端。
/// 符合 MCP Streamable HTTP 规范：直接 POST 到端点 URL。
pub struct HttpTransport {
    client: reqwest::Client,
    /// MCP 服务端端点 URL
    endpoint: String,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
}

impl HttpTransport {
    pub fn new(endpoint: String, headers: HashMap<String, String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            headers,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl McpTransport for HttpTransport {
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
        Box::pin(async move {
            let mut request = request;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            request.id = Some(Value::Number(id.into()));

            let mut builder = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
                .json(&request);
            for (k, v) in &self.headers {
                builder = builder.header(k, v);
            }

            let response = builder.send().await.map_err(|e| {
                ReactError::Mcp(McpError::ConnectionFailed(format!("HTTP 请求失败: {}", e)))
            })?;

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_default();
                return Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                    "HTTP 错误 {}: {}",
                    status, body
                ))));
            }

            let rpc_response: JsonRpcResponse = response.json().await.map_err(|e| {
                ReactError::Mcp(McpError::ProtocolError(format!(
                    "解析 HTTP 响应失败: {}",
                    e
                )))
            })?;

            Ok(rpc_response)
        })
    }

    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut builder = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
                .json(&notification);
            for (k, v) in &self.headers {
                builder = builder.header(k, v);
            }
            // 通知是 fire-and-forget
            let _ = builder.send().await;
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // HTTP 是无状态连接，无需显式关闭
        })
    }

    fn notification_rx(&self) -> Option<Arc<dyn crate::types::JsonRpcNotificationReceiver>> {
        None
    }
}
