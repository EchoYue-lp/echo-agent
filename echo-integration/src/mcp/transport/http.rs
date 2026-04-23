use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, oneshot};

use super::super::types::{
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION,
    NotificationReceiver,
};
use echo_core::error::{McpError, ReactError, Result};

use super::McpTransport;

/// HTTP 传输层（MCP Streamable HTTP）
///
/// 通过 HTTP POST 发送 JSON-RPC 请求，适用于远程 MCP 服务端。
/// 符合 MCP Streamable HTTP 规范：直接 POST 到端点 URL。
///
/// 支持异步响应：当服务端返回 202 Accepted 时，
/// 通过通知通道等待实际响应。
pub struct HttpTransport {
    client: reqwest::Client,
    /// MCP 服务端端点 URL
    endpoint: String,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
    /// 服务端在 initialize 响应中返回的会话 ID
    session_id: Arc<Mutex<Option<String>>>,
    /// 通知通道：用于接收异步响应（如 202 Accepted 后的响应）
    notification_tx: broadcast::Sender<JsonRpcNotification>,
    /// 请求 ID 到 oneshot channel 的映射（用于 202 异步响应路由）
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
}

impl HttpTransport {
    pub fn new(endpoint: String, headers: HashMap<String, String>) -> Self {
        let (notification_tx, _) = broadcast::channel(256);
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            headers,
            next_id: Arc::new(AtomicU64::new(1)),
            session_id: Arc::new(Mutex::new(None)),
            notification_tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl McpTransport for HttpTransport {
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
        Box::pin(async move {
            let mut request = request;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            request.id = Some(Value::Number(id.into()));

            // 在 HTTP 请求发送前注册 pending channel，避免 202 异步响应竞态
            let (tx, rx) = oneshot::channel();
            {
                let mut pending = self.pending.lock().await;
                pending.insert(id, tx);
            }

            let mut builder = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION)
                .header("Accept", "application/json, text/event-stream")
                .json(&request);

            // 携带会话 ID（initialize 之后的请求必须带上）
            {
                let sid = self.session_id.lock().await;
                if let Some(ref session_id) = *sid {
                    builder = builder.header("Mcp-Session-Id", session_id.as_str());
                }
            }

            for (k, v) in &self.headers {
                builder = builder.header(k, v);
            }

            let response = match builder.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    self.pending.lock().await.remove(&id);
                    return Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                        "HTTP 请求失败: {}",
                        e
                    ))));
                }
            };

            // 从响应中保存会话 ID（通常在 initialize 响应中返回）
            if let Some(new_session_id) = response.headers().get("mcp-session-id")
                && let Ok(sid) = new_session_id.to_str()
            {
                let mut sid_guard = self.session_id.lock().await;
                *sid_guard = Some(sid.to_string());
                tracing::debug!("HTTP: 保存 Mcp-Session-Id: {}", sid);
            }

            let status = response.status().as_u16();

            // 202 Accepted：服务端已接受请求，响应将异步到达
            if status == 202 {
                tracing::debug!("HTTP: 收到 202 Accepted (id={})，等待异步响应", id);

                // 等待异步响应（60 秒超时）
                let result =
                    match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
                        Ok(Ok(response)) => Ok(response),
                        Ok(Err(_)) => Err(ReactError::Mcp(McpError::TransportClosed)),
                        Err(_) => {
                            self.pending.lock().await.remove(&id);
                            Err(ReactError::Mcp(McpError::ProtocolError(format!(
                                "等待 HTTP 异步响应超时 (id={})",
                                id
                            ))))
                        }
                    }?;

                return Ok(result);
            }

            // 非 202 响应，移除 pending 条目
            self.pending.lock().await.remove(&id);

            // 非 2xx 错误
            if !response.status().is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                    "HTTP 错误 {}: {}",
                    status, body
                ))));
            }

            // 直接同步响应
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

            // 携带会话 ID
            {
                let sid = self.session_id.lock().await;
                if let Some(ref session_id) = *sid {
                    builder = builder.header("Mcp-Session-Id", session_id.as_str());
                }
            }

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
            // 尝试发送 shutdown/notification 关闭通知
            let notification = JsonRpcNotification::new("notifications/cancelled", None);
            let _ = self.notify(notification).await;
            // 清理所有 pending channels
            self.pending.lock().await.clear();
        })
    }

    fn notification_rx(&self) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
        Some(Arc::new(NotificationReceiver::new(
            self.notification_tx.subscribe(),
        )))
    }
}
