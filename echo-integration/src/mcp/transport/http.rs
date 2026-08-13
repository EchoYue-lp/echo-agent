use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::Mutex;

use super::super::types::{
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION,
};
use echo_core::error::{McpError, ReactError, Result};

use super::McpTransport;

/// HTTP 传输层（MCP Streamable HTTP）
///
/// 通过 HTTP POST 发送 JSON-RPC 请求，适用于远程 MCP 服务端。
/// 符合 MCP Streamable HTTP 规范：直接 POST 到端点 URL。
///
/// This request/response transport has no server-to-client receive channel.
/// Servers that return asynchronous `202 Accepted` responses must use the SSE
/// transport instead.
pub struct HttpTransport {
    client: reqwest::Client,
    /// MCP 服务端端点 URL
    endpoint: String,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
    /// 服务端在 initialize 响应中返回的会话 ID
    session_id: Arc<Mutex<Option<String>>>,
}

impl HttpTransport {
    pub fn new(endpoint: String, headers: HashMap<String, String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| {
                    reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(60))
                        .build()
                        .unwrap_or_default()
                }),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            headers,
            next_id: Arc::new(AtomicU64::new(1)),
            session_id: Arc::new(Mutex::new(None)),
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

            // Retry loop for transient HTTP errors (connection reset, timeout, DNS, TLS, 5xx)
            const MAX_RETRIES: u32 = 3;
            const BASE_DELAY_MS: u64 = 500;
            let mut retry_count: u32 = 0;
            let response = loop {
                let req = match builder.try_clone() {
                    Some(r) => r,
                    None => {
                        return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                            "无法复制 HTTP 请求".to_string(),
                        ))));
                    }
                };
                match req.send().await {
                    Ok(resp) => break resp,
                    Err(e) => {
                        if retry_count < MAX_RETRIES && is_retryable_error(&e) {
                            retry_count += 1;
                            let delay_ms = BASE_DELAY_MS * 2u64.pow(retry_count - 1);
                            // Add jitter: +/- 25%
                            let jitter = delay_ms / 4;
                            let delay = if jitter > 0 {
                                let r = (delay_ms as i64 - jitter as i64
                                    + (std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .subsec_nanos()
                                        as i64
                                        % (jitter * 2) as i64))
                                    .max(0) as u64;
                                Duration::from_millis(r)
                            } else {
                                Duration::from_millis(delay_ms)
                            };
                            tracing::warn!(
                                attempt = retry_count,
                                max = MAX_RETRIES,
                                delay_ms = delay.as_millis() as u64,
                                error = %e,
                                "MCP HTTP 请求失败，重试中..."
                            );
                            tokio::time::sleep(delay).await;
                        } else {
                            return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                                format!("HTTP 请求失败: {}", e),
                            ))));
                        }
                    }
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

            // Plain HTTP has no inbound stream that could ever deliver the
            // accepted response. Fail immediately instead of waiting on an
            // unreachable oneshot for 60 seconds.
            if status == 202 {
                return Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "HTTP request {id} returned 202 Accepted, but this endpoint has no server-to-client response stream; configure MCP SSE transport"
                )))));
            }

            // 非 2xx 错误
            if !response.status().is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                    format!("HTTP 错误 {}: {}", status, body),
                ))));
            }

            // 直接同步响应
            let rpc_response: JsonRpcResponse = response.json().await.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "解析 HTTP 响应失败: {}",
                    e
                ))))
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
        })
    }

    fn notification_rx(&self) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
        None
    }
}

/// Check if a reqwest error is transient and worth retrying.
///
/// Retries on: connection failures, timeouts, DNS errors, TLS handshake errors,
/// connection resets, EOF on connection, and HTTP 503/504 responses.
fn is_retryable_error(e: &reqwest::Error) -> bool {
    if e.is_timeout() || e.is_connect() {
        return true;
    }
    if let Some(status) = e.status() {
        // 5xx server errors (especially 502/503/504) are transient
        let code = status.as_u16();
        return code == 502 || code == 503 || code == 504;
    }
    // Check the error message for common transient patterns
    let msg = e.to_string().to_lowercase();
    msg.contains("connection reset")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("eof")
        || msg.contains("unexpected eof")
        || msg.contains("dns")
        || msg.contains("tls")
        || msg.contains("timed out")
}
