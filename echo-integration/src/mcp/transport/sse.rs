//! 旧版 HTTP+SSE 传输层（MCP 2024-11-05 协议）
//!
//! 适用于 旧版 SDK 的服务端。
//!
//! - SSE 连接：`GET {base_url}/sse`
//! - 发送请求：`POST {base_url}/message`（注意：单数）

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_util::sync::CancellationToken;

use super::super::types::{
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, NotificationReceiver,
};
use echo_core::error::{McpError, ReactError, Result};

use super::McpTransport;

/// HTTP/SSE 传输层
pub struct SseTransport {
    client: reqwest::Client,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    notification_tx: broadcast::Sender<JsonRpcNotification>,
    message_endpoint: Arc<Mutex<Option<String>>>,
    cancel_token: CancellationToken,
    _sse_task: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    pub async fn new(base_url: String, headers: HashMap<String, String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                    "创建 HTTP 客户端失败: {}",
                    e
                ))))
            })?;

        let next_id = Arc::new(AtomicU64::new(1));
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
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

                loop {
                    // 检查取消信号
                    if cancel.is_cancelled() {
                        tracing::debug!("SSE: 收到取消信号，退出重连循环");
                        break;
                    }

                    // 超过最大重试次数后停止重连
                    if retry_count >= MAX_RETRIES {
                        tracing::error!("SSE: 达到最大重试次数 ({})，停止重连", MAX_RETRIES);
                        break;
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
                            break;
                        }
                        Err(e) => {
                            retry_count += 1;
                            if cancel.is_cancelled() {
                                tracing::debug!("SSE: 收到取消信号，退出");
                                break;
                            }
                            tracing::warn!(
                                "SSE: 连接断开（{}），{}ms 后重试 ({}/{})（Last-Event-ID={:?}）",
                                e,
                                retry_ms,
                                retry_count,
                                MAX_RETRIES,
                                last_event_id
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(std::time::Duration::from_millis(retry_ms)) => {}
                                _ = cancel.cancelled() => {
                                    tracing::debug!("SSE: 等待重连时收到取消信号");
                                    break;
                                }
                            }
                            // 指数退避（最大 30 秒）
                            retry_ms = (retry_ms * 2).min(30_000);
                        }
                    }
                }
            })
        };

        // 给 SSE 连接留出建立时间
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        Ok(Self {
            client,
            headers,
            next_id,
            pending,
            notification_tx,
            message_endpoint,
            cancel_token,
            _sse_task: sse_task,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_sse_loop(
        client: &reqwest::Client,
        sse_url: &str,
        headers: &HashMap<String, String>,
        pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
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
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!("SSE 连接失败: {}", e))))
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

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                    "SSE 读取错误: {}",
                    e
                ))))
            })?;

            let text = std::str::from_utf8(&chunk).map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "SSE 编码错误: {}",
                    e
                ))))
            })?;

            buffer.push_str(text);

            while let Some(pos) = buffer.find("\n\n") {
                let event_block = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

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
                        tracing::info!("SSE: 获取到 POST 端点 URI: {}", uri);
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
                    tracing::debug!("SSE: 忽略非 JSON 数据: {}", data);
                    continue;
                };

                let has_rpc_id = value.get("id").is_some() && !value["id"].is_null();
                let has_result = value.get("result").is_some();
                let has_error = value.get("error").is_some();
                let has_method = value.get("method").is_some();

                if has_rpc_id && (has_result || has_error) {
                    match serde_json::from_value::<JsonRpcResponse>(value) {
                        Ok(resp) => {
                            if let Some(id_val) = &resp.id {
                                let id_u64 = match id_val {
                                    Value::Number(n) => n.as_u64().unwrap_or(0),
                                    Value::String(s) => s.parse().unwrap_or(0),
                                    _ => 0,
                                };
                                let mut pending_guard = pending.lock().await;
                                if let Some(sender) = pending_guard.remove(&id_u64) {
                                    tracing::debug!("SSE: 分发响应 id={}", id_u64);
                                    let _ = sender.send(resp);
                                } else {
                                    tracing::debug!("SSE: 未找到等待方 id={}，丢弃响应", id_u64);
                                }
                            }
                        }
                        Err(e) => tracing::warn!("SSE: 解析响应失败: {}", e),
                    }
                } else if has_method && !has_rpc_id {
                    match serde_json::from_value::<JsonRpcNotification>(value) {
                        Ok(notif) => {
                            tracing::debug!("SSE: 收到通知 method={}", notif.method);
                            let _ = notification_tx.send(notif);
                        }
                        Err(e) => tracing::warn!("SSE: 解析通知失败: {}", e),
                    }
                } else {
                    tracing::debug!("SSE: 收到未知格式数据，已忽略");
                }
            }
        }

        Ok(())
    }
}

impl McpTransport for SseTransport {
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
        Box::pin(async move {
            let mut request = request;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            request.id = Some(Value::Number(id.into()));

            let (tx, rx) = oneshot::channel();
            {
                let mut pending = self.pending.lock().await;
                pending.insert(id, tx);
            }

            let endpoint_uri = {
                let guard = self.message_endpoint.lock().await;
                guard.clone().ok_or_else(|| {
                    ReactError::Mcp(Box::new(McpError::ProtocolError(
                        "SSE: 尚未获取到 POST 端点 URI，请等待连接建立".to_string(),
                    )))
                })?
            };

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

            let post_resp = builder.send().await.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                    "POST {} 失败: {}",
                    endpoint_uri, e
                ))))
            })?;

            if post_resp.status().is_server_error() {
                let status = post_resp.status().as_u16();
                let body = post_resp.text().await.unwrap_or_default();
                self.pending.lock().await.remove(&id);
                return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                    format!("POST {} 返回服务器错误 {}: {}", endpoint_uri, status, body),
                ))));
            }

            tracing::debug!("SSE: POST 成功（id={}），等待 SSE 响应…", id);

            let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
                .await
                .map_err(|_| {
                    ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                        "等待 SSE 响应超时（id={}）",
                        id
                    ))))
                })?
                .map_err(|_| {
                    ReactError::Mcp(Box::new(McpError::ProtocolError(
                        "响应 channel 已关闭".to_string(),
                    )))
                })?;

            Ok(response)
        })
    }

    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
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
            let _ = builder.send().await;
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // 使用 CancellationToken 取消后台 SSE 任务
            self.cancel_token.cancel();
            tracing::debug!("SSE: 已发送取消信号");
        })
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
    }
}
