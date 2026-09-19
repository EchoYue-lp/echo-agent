//! 旧版 HTTP+SSE 传输层（MCP 2024-11-05 协议）
//!
//! 适用于 旧版 SDK 的服务端。
//!
//! - SSE 连接：`GET {base_url}/sse`
//! - 发送请求：`POST {base_url}/message`（注意：单数）

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::super::types::{
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, NotificationReceiver,
};
use crate::redaction::{
    header_secrets, request_error, text as redact_text, text_with_secrets, url as redact_url,
};
use echo_core::error::{McpError, ReactError, Result};

use super::{
    CloseCoordinator, McpTransport, PendingRequests, close_error, redact_response_error,
    transport_closed_error,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// HTTP/SSE 传输层
pub struct SseTransport {
    client: reqwest::Client,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
    pending: Arc<PendingRequests>,
    notification_tx: broadcast::Sender<JsonRpcNotification>,
    message_endpoint: Arc<Mutex<Option<String>>>,
    post_gate: Arc<RwLock<()>>,
    cancel_token: CancellationToken,
    sse_task: Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
    close_coordinator: Arc<CloseCoordinator>,
    response_timeout: Duration,
}

struct SseConstructionTask {
    cancel: CancellationToken,
    pending: Arc<PendingRequests>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl Drop for SseConstructionTask {
    fn drop(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        self.cancel.cancel();
        self.pending.close();
        task.abort();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = task.await;
            });
        }
    }
}

impl SseTransport {
    pub async fn new(base_url: String, headers: HashMap<String, String>) -> Result<Self> {
        Self::new_with_warmup(base_url, headers, Duration::from_millis(300)).await
    }

    async fn new_with_warmup(
        base_url: String,
        headers: HashMap<String, String>,
        warmup: Duration,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                    "创建 HTTP 客户端失败: {}",
                    request_error(e, std::iter::empty::<&str>())
                ))))
            })?;

        let next_id = Arc::new(AtomicU64::new(1));
        let pending = PendingRequests::new();
        let (notification_tx, _) = broadcast::channel(64);
        let message_endpoint: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let cancel_token = CancellationToken::new();

        let sse_task = {
            let client = client.clone();
            let base_url_clone = base_url.clone();
            let headers_clone = headers.clone();
            let pending_clone = pending.clone();
            let notification_tx_clone = notification_tx.clone();
            let message_endpoint_clone = message_endpoint.clone();
            let cancel = cancel_token.clone();

            tokio::spawn(async move {
                let sse_url = format!("{}/sse", base_url_clone.trim_end_matches('/'));
                let mut last_event_id: Option<String> = None;
                let mut retry_ms: u64 = 2_000;
                let mut retry_count: u32 = 0;
                const MAX_RETRIES: u32 = 5;

                let result = loop {
                    // 检查取消信号
                    if cancel.is_cancelled() {
                        tracing::debug!("SSE: 收到取消信号，退出重连循环");
                        break Ok(());
                    }

                    match Self::run_sse_loop(
                        &client,
                        &sse_url,
                        &headers_clone,
                        &pending_clone,
                        &notification_tx_clone,
                        &message_endpoint_clone,
                        &mut last_event_id,
                        &mut retry_ms,
                        &cancel,
                    )
                    .await
                    {
                        Ok(_) => {
                            tracing::debug!("SSE: 连接正常关闭");
                            break Ok(());
                        }
                        Err(e) => {
                            retry_count += 1;
                            if cancel.is_cancelled() {
                                tracing::debug!("SSE: 收到取消信号，退出");
                                break Ok(());
                            }
                            if retry_count >= MAX_RETRIES {
                                tracing::error!(
                                    "SSE: 达到最大重试次数 ({})，停止重连",
                                    MAX_RETRIES
                                );
                                break Err(e);
                            }
                            tracing::warn!(
                                "SSE: 连接断开（{}），{}ms 后重试 ({}/{})（Last-Event-ID 已隐藏={}）",
                                redact_text(&e.to_string()),
                                retry_ms,
                                retry_count,
                                MAX_RETRIES,
                                last_event_id.is_some()
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(retry_ms)) => {}
                                _ = cancel.cancelled() => {
                                    tracing::debug!("SSE: 等待重连时收到取消信号");
                                    break Ok(());
                                }
                            }
                            // 指数退避（最大 30 秒）
                            retry_ms = (retry_ms * 2).min(30_000);
                        }
                    }
                };
                pending_clone.close();
                result
            })
        };

        let mut construction = SseConstructionTask {
            cancel: cancel_token.clone(),
            pending: Arc::clone(&pending),
            task: Some(sse_task),
        };

        // Keep the original endpoint warm-up period while the task has an
        // owner that can settle cancellation before Self is returned.
        tokio::time::sleep(warmup).await;
        let sse_task = construction.task.take().ok_or_else(|| {
            ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                "SSE initialization lost its receive task".to_string(),
            )))
        })?;

        Ok(Self {
            client,
            headers,
            next_id,
            pending,
            notification_tx,
            message_endpoint,
            post_gate: Arc::new(RwLock::new(())),
            cancel_token,
            sse_task: Arc::new(Mutex::new(Some(sse_task))),
            close_coordinator: CloseCoordinator::new(),
            response_timeout: RESPONSE_TIMEOUT,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_sse_loop(
        client: &reqwest::Client,
        sse_url: &str,
        headers: &HashMap<String, String>,
        pending: &Arc<PendingRequests>,
        notification_tx: &broadcast::Sender<JsonRpcNotification>,
        message_endpoint: &Arc<Mutex<Option<String>>>,
        last_event_id: &mut Option<String>,
        retry_ms: &mut u64,
        cancel: &CancellationToken,
    ) -> Result<()> {
        // 重连时重置 message_endpoint（服务端可能重新分配）
        {
            let mut ep = message_endpoint.lock().await;
            if ep.is_some() {
                tracing::debug!("SSE: 重连，重置 message_endpoint");
                *ep = None;
            }
        }

        let mut builder = client
            .get(sse_url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive");

        if let Some(ref eid) = *last_event_id {
            builder = builder.header("Last-Event-ID", eid);
        }

        for (k, v) in headers {
            builder = builder.header(k, v);
        }

        let response = tokio::select! {
            resp = builder.send() => resp.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                    "SSE 连接失败: {}",
                    request_error(e, header_secrets(headers))
                ))))
            })?,
            _ = cancel.cancelled() => {
                return Ok(());
            }
        };

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!("SSE 连接返回 HTTP {}", status),
            ))));
        }

        tracing::debug!("SSE: 连接已建立");

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        loop {
            let next_chunk = tokio::select! {
                chunk = stream.next() => chunk,
                _ = cancel.cancelled() => return Ok(()),
            };
            let Some(chunk) = next_chunk else {
                break;
            };
            let chunk = chunk.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                    "SSE 读取错误: {}",
                    request_error(e, header_secrets(headers))
                ))))
            })?;

            let text = std::str::from_utf8(&chunk).map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "SSE 编码错误: {}",
                    text_with_secrets(&e.to_string(), header_secrets(headers))
                ))))
            })?;

            buffer.push_str(text);

            while let Some(pos) = buffer.find("\n\n") {
                let event_block = buffer.get(..pos).unwrap_or_default().to_string();
                buffer = buffer.get(pos + 2..).unwrap_or_default().to_string();

                let mut data_lines: Vec<&str> = Vec::new();
                let mut event_id_field: Option<&str> = None;
                let mut event_type: Option<&str> = None;

                for line in event_block.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        data_lines.push(data.trim());
                    } else if let Some(id) = line.strip_prefix("id: ") {
                        event_id_field = Some(id.trim());
                    } else if let Some(ms_str) = line.strip_prefix("retry: ") {
                        if let Ok(ms) = ms_str.trim().parse::<u64>() {
                            *retry_ms = ms;
                            tracing::debug!("SSE: retry 更新为 {}ms", ms);
                        }
                    } else if let Some(et) = line.strip_prefix("event: ") {
                        event_type = Some(et.trim());
                    }
                }

                if event_type == Some("endpoint") {
                    let data = data_lines.join("\n");
                    if let Ok(endpoint_value) = serde_json::from_str::<Value>(&data)
                        && let Some(uri) = endpoint_value.get("uri").and_then(|v| v.as_str())
                    {
                        let mut endpoint_guard = message_endpoint.lock().await;
                        *endpoint_guard = Some(uri.to_string());
                        tracing::info!("SSE: 获取到 POST 端点 URI: {}", redact_url(uri));
                        continue;
                    }
                }

                if let Some(eid) = event_id_field {
                    *last_event_id = if eid.is_empty() {
                        None
                    } else {
                        Some(eid.to_string())
                    };
                }

                let data = data_lines.join("\n");
                if data.is_empty() {
                    continue;
                }

                let Ok(value) = serde_json::from_str::<Value>(&data) else {
                    tracing::debug!(
                        "SSE: 忽略非 JSON 数据: {}",
                        text_with_secrets(&data, header_secrets(headers))
                    );
                    continue;
                };

                let has_rpc_id = value.get("id").is_some_and(|id| !id.is_null());
                let has_result = value.get("result").is_some();
                let has_error = value.get("error").is_some();
                let has_method = value.get("method").is_some();

                if has_rpc_id && (has_result || has_error) {
                    match serde_json::from_value::<JsonRpcResponse>(value) {
                        Ok(mut resp) => {
                            let secrets = header_secrets(headers);
                            redact_response_error(&mut resp, &secrets);
                            if let Some(id_val) = &resp.id {
                                let id_u64 = match id_val {
                                    Value::Number(n) => n.as_u64().unwrap_or(0),
                                    Value::String(s) => s.parse().unwrap_or(0),
                                    _ => 0,
                                };
                                tracing::debug!("SSE: 分发响应 id={}", id_u64);
                                pending.resolve(id_u64, resp);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("SSE: 解析响应失败: {}", redact_text(&e.to_string()))
                        }
                    }
                } else if has_method && !has_rpc_id {
                    match serde_json::from_value::<JsonRpcNotification>(value) {
                        Ok(notif) => {
                            tracing::debug!("SSE: 收到通知 method={}", notif.method);
                            let _ = notification_tx.send(notif);
                        }
                        Err(e) => {
                            tracing::warn!("SSE: 解析通知失败: {}", redact_text(&e.to_string()))
                        }
                    }
                } else {
                    tracing::debug!("SSE: 收到未知格式数据，已忽略");
                }
            }
        }

        Ok(())
    }

    async fn close_owned(
        cancel_token: CancellationToken,
        pending: Arc<PendingRequests>,
        message_endpoint: Arc<Mutex<Option<String>>>,
        post_gate: Arc<RwLock<()>>,
        sse_task: Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
        deadline: Instant,
    ) -> Result<()> {
        cancel_token.cancel();
        let drained = pending.close();
        let mut errors = Vec::new();
        let post_guard = tokio::time::timeout_at(deadline, post_gate.write()).await;
        if post_guard.is_err() {
            errors.push("SSE POST operations did not settle before close deadline".to_string());
        }
        match tokio::time::timeout_at(deadline, message_endpoint.lock()).await {
            Ok(mut endpoint) => *endpoint = None,
            Err(_) => errors.push("SSE endpoint owner lock timed out during close".to_string()),
        }
        drop(post_guard);
        tracing::debug!(drained, "SSE: 已发送取消信号并结算 pending 请求");

        match tokio::time::timeout_at(deadline, sse_task.lock()).await {
            Ok(mut task_slot) => {
                if let Some(mut task) = task_slot.take() {
                    match tokio::time::timeout_at(deadline, &mut task).await {
                        Ok(Ok(Ok(()))) => {}
                        Ok(Ok(Err(error))) => errors.push(error.to_string()),
                        Ok(Err(error)) => errors.push(format!("SSE task join failed: {error}")),
                        Err(_) => {
                            task.abort();
                            let _ = task.await;
                            errors.push("SSE close timed out; task was aborted".to_string());
                        }
                    }
                }
            }
            Err(_) => errors.push("SSE task owner lock timed out during close".to_string()),
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(close_error(errors.join("; ")))
        }
    }

    async fn close_with_timeout(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let (start, receipt) = self.close_coordinator.begin();
        if start {
            let coordinator = Arc::clone(&receipt);
            let cancel_token = self.cancel_token.clone();
            let pending = Arc::clone(&self.pending);
            let message_endpoint = Arc::clone(&self.message_endpoint);
            let post_gate = Arc::clone(&self.post_gate);
            let sse_task = Arc::clone(&self.sse_task);
            tokio::spawn(async move {
                let result = Self::close_owned(
                    cancel_token,
                    pending,
                    message_endpoint,
                    post_gate,
                    sse_task,
                    deadline,
                )
                .await;
                coordinator.complete(&result);
            });
        }
        receipt.wait(deadline, "SSE transport").await
    }
}

impl McpTransport for SseTransport {
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
        Box::pin(async move {
            let mut request = request;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            request.id = Some(Value::Number(id.into()));

            let endpoint_uri = {
                let guard = self.message_endpoint.lock().await;
                guard.clone().ok_or_else(|| {
                    ReactError::Mcp(Box::new(McpError::ProtocolError(
                        "SSE: 尚未获取到 POST 端点 URI，请等待连接建立".to_string(),
                    )))
                })?
            };
            let (rx, _registration) = self.pending.register(id)?;
            self.pending.ensure_open()?;
            let _post_guard = tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => return Err(transport_closed_error()),
                guard = self.post_gate.read() => guard,
            };
            self.pending.ensure_open()?;

            // POST 请求携带 session ID 到动态端点
            let mut builder = self
                .client
                .post(&endpoint_uri)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream");

            // 从 headers 中携带自定义 headers
            for (k, v) in &self.headers {
                builder = builder.header(k, v);
            }

            // 如果 message_endpoint 之前保存过，尝试在 header 中携带
            // 注意：session ID 在 POST 请求 headers 中携带
            builder = builder.json(&request);

            let post_resp = tokio::select! {
                response = builder.send() => response.map_err(|e| {
                    ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                        "POST {} 失败: {}",
                        redact_url(&endpoint_uri),
                        request_error(e, header_secrets(&self.headers))
                    ))))
                })?,
                _ = self.cancel_token.cancelled() => return Err(transport_closed_error()),
            };

            if !post_resp.status().is_success() {
                let status = post_resp.status().as_u16();
                let body = tokio::select! {
                    biased;
                    _ = self.cancel_token.cancelled() => return Err(transport_closed_error()),
                    body = post_resp.text() => body.unwrap_or_default(),
                };
                return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                    format!(
                        "POST {} 返回 HTTP {}: {}",
                        redact_url(&endpoint_uri),
                        status,
                        text_with_secrets(&body, header_secrets(&self.headers))
                    ),
                ))));
            }

            tracing::debug!("SSE: POST 成功（id={}），等待 SSE 响应…", id);

            match tokio::time::timeout(self.response_timeout, rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(transport_closed_error()),
                Err(_) => Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "等待 SSE 响应超时（id={}）",
                    id
                ))))),
            }
        })
    }

    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let _post_guard = tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => return Err(transport_closed_error()),
                guard = self.post_gate.read() => guard,
            };
            self.pending.ensure_open()?;
            let endpoint_uri = {
                let guard = self.message_endpoint.lock().await;
                match guard.clone() {
                    Some(uri) => uri,
                    None => {
                        tracing::warn!("SSE: 尚未获取到 POST 端点 URI，跳过通知发送");
                        return Ok(());
                    }
                }
            };

            let mut builder = self
                .client
                .post(&endpoint_uri)
                .header("Content-Type", "application/json")
                .json(&notification);
            for (k, v) in &self.headers {
                builder = builder.header(k, v);
            }
            let response = tokio::select! {
                biased;
                _ = self.cancel_token.cancelled() => return Err(transport_closed_error()),
                response = builder.send() => response.map_err(|error| {
                    ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                        "SSE notification POST failed: {}",
                        request_error(error, header_secrets(&self.headers))
                    ))))
                })?,
            };
            if !response.status().is_success() {
                return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                    format!("SSE notification POST returned HTTP {}", response.status()),
                ))));
            }
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.close_with_timeout(CLOSE_TIMEOUT).await })
    }

    fn notification_rx(&self) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
        Some(Arc::new(NotificationReceiver::new(
            self.notification_tx.subscribe(),
        )))
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        self.pending.close();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    fn test_transport(
        endpoint: Option<String>,
        response_timeout: Duration,
        ignore_cancel: bool,
    ) -> Result<(SseTransport, Arc<AtomicBool>)> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .map_err(|error| close_error(format!("test HTTP client failed: {error}")))?;
        let pending = PendingRequests::new();
        let cancel_token = CancellationToken::new();
        let cancel = cancel_token.clone();
        let pending_for_task = Arc::clone(&pending);
        let settled = Arc::new(AtomicBool::new(false));
        let settled_for_task = Arc::clone(&settled);
        let task = tokio::spawn(async move {
            if ignore_cancel {
                std::future::pending::<()>().await;
            } else {
                cancel.cancelled().await;
                pending_for_task.close();
                settled_for_task.store(true, AtomicOrdering::Release);
            }
            Ok(())
        });
        Ok((
            SseTransport {
                client,
                headers: HashMap::new(),
                next_id: Arc::new(AtomicU64::new(1)),
                pending,
                notification_tx: broadcast::channel(4).0,
                message_endpoint: Arc::new(Mutex::new(endpoint)),
                post_gate: Arc::new(RwLock::new(())),
                cancel_token,
                sse_task: Arc::new(Mutex::new(Some(task))),
                close_coordinator: CloseCoordinator::new(),
                response_timeout,
            },
            settled,
        ))
    }

    #[tokio::test]
    async fn cancelled_construction_settles_its_started_receive_task() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let construction = tokio::spawn(async move {
            SseTransport::new_with_warmup(base_url, HashMap::new(), Duration::from_secs(5)).await
        });
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .map_err(|_| close_error("SSE construction did not start its receive request"))??;
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await?;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await?;
        construction.abort();
        assert!(construction.await.is_err_and(|error| error.is_cancelled()));

        let mut probe = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut probe))
            .await
            .map_err(|_| close_error("cancelled SSE construction left its receive task alive"))?;
        assert!(
            matches!(&read, Ok(0))
                || matches!(&read, Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset)
        );
        Ok(())
    }

    #[tokio::test]
    async fn close_drains_pending_awaits_task_and_is_idempotent() -> Result<()> {
        let (transport, settled) = test_transport(None, Duration::from_secs(1), false)?;
        let (receiver, _registration) = transport.pending.register(41)?;

        transport.close_with_timeout(Duration::from_secs(1)).await?;

        assert!(settled.load(AtomicOrdering::Acquire));
        assert_eq!(transport.pending.len(), 0);
        assert!(matches!(receiver.await, Ok(Err(ReactError::Mcp(_)))));
        transport.close_with_timeout(Duration::from_secs(1)).await?;
        Ok(())
    }

    #[tokio::test]
    async fn close_timeout_aborts_and_awaits_unresponsive_task() -> Result<()> {
        let (transport, _) = test_transport(None, Duration::from_secs(1), true)?;
        let started_at = std::time::Instant::now();

        let result = transport
            .close_with_timeout(Duration::from_millis(20))
            .await;

        assert!(result.is_err());
        assert!(started_at.elapsed() < Duration::from_secs(1));
        assert!(transport.sse_task.lock().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn missing_endpoint_never_registers_pending_request() -> Result<()> {
        let (transport, _) = test_transport(None, Duration::from_secs(1), false)?;

        assert!(
            transport
                .send(JsonRpcRequest::new("test", None))
                .await
                .is_err()
        );
        assert_eq!(transport.pending.len(), 0);
        transport.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn post_failure_removes_pending_request() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/message", listener.local_addr()?);
        drop(listener);
        let (transport, _) = test_transport(Some(endpoint), Duration::from_secs(1), false)?;

        assert!(
            transport
                .send(JsonRpcRequest::new("test", None))
                .await
                .is_err()
        );
        assert_eq!(transport.pending.len(), 0);
        transport.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn notification_post_failure_is_observable() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/message", listener.local_addr()?);
        drop(listener);
        let (transport, _) = test_transport(Some(endpoint), Duration::from_secs(1), false)?;

        assert!(
            transport
                .notify(JsonRpcNotification::new("test", None))
                .await
                .is_err()
        );
        transport.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn close_cancels_and_awaits_inflight_notification_post() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/message", listener.local_addr()?);
        let (release_server, server_release) = tokio::sync::oneshot::channel();
        let (request_ready, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await?;
            let _ = request_ready.send(());
            let _ = server_release.await;
            Ok::<(), std::io::Error>(())
        });
        let (transport, _) = test_transport(Some(endpoint), Duration::from_secs(1), false)?;
        let transport = Arc::new(transport);
        let notify_task = tokio::spawn({
            let transport = Arc::clone(&transport);
            async move {
                transport
                    .notify(JsonRpcNotification::new("test", None))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), ready)
            .await
            .map_err(|_| close_error("notification POST did not reach the test server"))?
            .map_err(|_| close_error("notification POST readiness channel closed"))?;

        transport.close_with_timeout(Duration::from_secs(1)).await?;

        let notify_result = notify_task
            .await
            .map_err(|error| close_error(format!("notify task join failed: {error}")))?;
        assert!(notify_result.is_err());
        let _ = release_server.send(());
        server
            .await
            .map_err(|error| close_error(format!("test server join failed: {error}")))??;
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_close_future_keeps_owned_task_for_retry_receipt() -> Result<()> {
        let (transport, _) = test_transport(None, Duration::from_secs(1), true)?;
        let transport = Arc::new(transport);
        let first_close = tokio::spawn({
            let transport = Arc::clone(&transport);
            async move {
                transport
                    .close_with_timeout(Duration::from_millis(100))
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        first_close.abort();
        let _ = first_close.await;

        assert!(
            transport
                .close_with_timeout(Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(transport.sse_task.lock().await.is_none());
        transport.close_with_timeout(Duration::from_secs(1)).await?;
        Ok(())
    }

    #[tokio::test]
    async fn close_cancels_stalled_error_body_and_settles_sse_task() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/message", listener.local_addr()?);
        let (transport, settled) = test_transport(Some(endpoint), Duration::from_secs(30), false)?;
        let (mut socket, mut send) = {
            let send = transport.send(JsonRpcRequest::new("test", None));
            let accept = listener.accept();
            tokio::pin!(accept);
            let mut send = send;
            let socket = tokio::select! {
                socket = &mut accept => socket?.0,
                _ = &mut send => return Err(close_error("request finished before accept")),
            };
            (socket, send)
        };
        let mut request = [0_u8; 2048];
        tokio::select! {
            read = socket.read(&mut request) => { read?; }
            _ = &mut send => return Err(close_error("request finished before headers")),
        }
        socket
            .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 1000\r\n\r\n")
            .await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut send)
                .await
                .is_err()
        );
        let close = transport.close_with_timeout(Duration::from_millis(200));
        let (result, sent) = tokio::join!(close, send);
        result?;
        assert!(sent.is_err());
        assert!(settled.load(AtomicOrdering::Acquire));
        assert!(transport.sse_task.lock().await.is_none());
        assert_eq!(transport.pending.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn post_gate_failure_still_aborts_and_joins_sse_task() -> Result<()> {
        let (transport, _) = test_transport(None, Duration::from_secs(1), true)?;
        let _gate = transport.post_gate.read().await;
        let result = SseTransport::close_owned(
            transport.cancel_token.clone(),
            Arc::clone(&transport.pending),
            Arc::clone(&transport.message_endpoint),
            Arc::clone(&transport.post_gate),
            Arc::clone(&transport.sse_task),
            Instant::now() + Duration::from_millis(20),
        )
        .await;
        assert!(result.is_err());
        assert!(transport.sse_task.lock().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn response_timeout_removes_pending_request() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/message", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await?;
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            Ok::<(), std::io::Error>(())
        });
        let (transport, _) = test_transport(Some(endpoint), Duration::from_millis(20), false)?;

        assert!(
            transport
                .send(JsonRpcRequest::new("test", None))
                .await
                .is_err()
        );
        assert_eq!(transport.pending.len(), 0);
        server
            .await
            .map_err(|error| close_error(format!("test server join failed: {error}")))??;
        transport.close().await?;
        Ok(())
    }
}
