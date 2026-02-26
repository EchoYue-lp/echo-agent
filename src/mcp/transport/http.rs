use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{McpError, ReactError, Result};
use crate::mcp::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

use super::McpTransport;

/// HTTP 传输层
///
/// 通过 HTTP POST 发送 JSON-RPC 请求，适用于远程 MCP 服务端。
/// 默认端点：`{base_url}/message`（符合 MCP Streamable HTTP 规范）。
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
}

impl HttpTransport {
    pub fn new(base_url: String, headers: HashMap<String, String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            headers,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{}/message", base)
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(&self, mut request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        request.id = Some(Value::Number(id.into()));

        let mut builder = self.client.post(&self.endpoint()).json(&request);
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }

        let response = builder.send().await.map_err(|e| {
            ReactError::Mcp(McpError::ConnectionFailed(format!(
                "HTTP 请求失败: {}",
                e
            )))
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
    }

    async fn notify(&self, notification: JsonRpcNotification) -> Result<()> {
        let mut builder = self.client.post(&self.endpoint()).json(&notification);
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }
        // 通知是 fire-and-forget
        let _ = builder.send().await;
        Ok(())
    }

    async fn close(&self) {
        // HTTP 是无状态连接，无需显式关闭
    }
}
