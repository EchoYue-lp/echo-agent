//! 旧版 HTTP+SSE 传输层（MCP 2024-11-05 协议）
//!
//! 适用于 旧版 SDK 的服务端。
//!
//!
//! - SSE 连接：`GET {base_url}/sse`
//! - 发送请求：`POST {base_url}/message`（注意：单数）
//!
//! 数据流：
//! ```text
//! Client                          Server
//!   |--- GET /sse (keep-alive) --->|
//!   |<-- SSE stream (responses) ---|
//!   |--- POST /message (req) ----->|
//!   |<-- 202 Accepted -------------|  ← 仅确认收到
//!   |<-- data: {rpc-response}\n\n -|  ← 真正的响应通过 SSE 推送
//! ```
//!
//! 与 Streamable HTTP（新版）的区别：
//! - 此传输需要两个独立端点（SSE + message）
//! - 响应异步到达，需要关联 request id
//! - 协议版本固定为 `2024-11-05`（服务端协商后可能降级）

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::{Mutex, broadcast, oneshot};

use crate::error::{McpError, ReactError, Result};
use crate::mcp::types::{
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, NotificationReceiver,
};

use super::McpTransport;

/// HTTP/SSE 传输层
///
/// 符合 MCP 规范的 HTTP+SSE 双通道传输实现：
/// - 服务端 → 客户端：通过 SSE 长连接推送（响应、通知）
/// - 客户端 → 服务端：通过 HTTP POST 发送请求（端点 URI 从服务器的 endpoint event 动态获取）
pub struct SseTransport {
    client: reqwest::Client,
    headers: HashMap<String, String>,
    next_id: Arc<AtomicU64>,
    /// 待响应请求表：request_id → response channel
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// 服务端主动推送通知的广播频道
    notification_tx: broadcast::Sender<JsonRpcNotification>,
    /// 动态获取的 POST 端点 URI（从服务器的 endpoint event）
    message_endpoint: Arc<Mutex<Option<String>>>,
    _sse_task: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    /// 建立 HTTP/SSE 传输连接
    ///
    /// 创建时即启动后台 SSE 监听任务，自动重连。
    pub async fn new(base_url: String, headers: HashMap<String, String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| {
                ReactError::Mcp(McpError::ConnectionFailed(format!(
                    "创建 HTTP 客户端失败: {}",
                    e
                )))
            })?;

        let next_id = Arc::new(AtomicU64::new(1));
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (notification_tx, _) = broadcast::channel(64);
        let message_endpoint: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let sse_task = {
            let client = client.clone();
            let base_url_clone = base_url.clone();
            let headers_clone = headers.clone();
            let pending_clone = pending.clone();
            let notification_tx_clone = notification_tx.clone();
            let message_endpoint_clone = message_endpoint.clone();

            tokio::spawn(async move {
                let sse_url = format!("{}/sse", base_url_clone.trim_end_matches('/'));
                // 跨重连持久化的状态（2025-11-25 changelog minor #6 #7）
                let mut last_event_id: Option<String> = None;
                let mut retry_ms: u64 = 2_000;

                loop {
                    tracing::debug!("SSE: 正在连接 {}", sse_url);
                    match Self::run_sse_loop(
                        &client,
                        &sse_url,
                        &headers_clone,
                        &pending_clone,
                        &notification_tx_clone,
                        &message_endpoint_clone,
                        &mut last_event_id,
                        &mut retry_ms,
                    )
                    .await
                    {
                        Ok(_) => {
                            tracing::debug!("SSE: 连接正常关闭");
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "SSE: 连接断开（{}），{}ms 后重试（Last-Event-ID={:?}）",
                                e,
                                retry_ms,
                                last_event_id
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(retry_ms)).await;
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
            _sse_task: sse_task,
        })
    }

    /// SSE 连接主循环：解析数据帧并分发到对应 channel
    ///
    /// `last_event_id` / `retry_ms` 跨重连共享，每次调用可能被服务端下发的
    /// `id:` / `retry:` 字段更新（2025-11-25 changelog minor #6 #7）。
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
    ) -> Result<()> {
        let mut builder = client
            .get(sse_url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive");

        // 携带 Last-Event-ID 实现断点续传
        if let Some(ref eid) = *last_event_id {
            builder = builder.header("Last-Event-ID", eid);
        }

        for (k, v) in headers {
            builder = builder.header(k, v);
        }

        let response = builder.send().await.map_err(|e| {
            ReactError::Mcp(McpError::ConnectionFailed(format!("SSE 连接失败: {}", e)))
        })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                "SSE 连接返回 HTTP {}",
                status
            ))));
        }

        tracing::debug!("SSE: 连接已建立");

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                ReactError::Mcp(McpError::ConnectionFailed(format!("SSE 读取错误: {}", e)))
            })?;

            let text = std::str::from_utf8(&chunk).map_err(|e| {
                ReactError::Mcp(McpError::ProtocolError(format!("SSE 编码错误: {}", e)))
            })?;

            buffer.push_str(text);

            // SSE 事件以 \n\n 为分隔符
            while let Some(pos) = buffer.find("\n\n") {
                let event_block = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                // 解析 SSE 事件的所有字段
                let mut data_lines: Vec<&str> = Vec::new();
                let mut event_id_field: Option<&str> = None;
                let mut event_type: Option<&str> = None;

                for line in event_block.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        data_lines.push(data.trim());
                    } else if let Some(id) = line.strip_prefix("id: ") {
                        // id: 字段：记录 Last-Event-ID（可为空，表示清空游标）
                        event_id_field = Some(id.trim());
                    } else if let Some(ms_str) = line.strip_prefix("retry: ") {
                        // retry: 字段：更新重连等待时间（客户端 MUST 遵守）
                        if let Ok(ms) = ms_str.trim().parse::<u64>() {
                            *retry_ms = ms;
                            tracing::debug!("SSE: retry 更新为 {}ms", ms);
                        }
                    } else if let Some(et) = line.strip_prefix("event: ") {
                        // event: 字段：MCP 协议使用此字段来区分事件类型
                        // 特别是 `event: endpoint` 表示服务端发送的 POST 端点 URI
                        event_type = Some(et.trim());
                    }
                }

                // 处理 endpoint 事件：服务端告知客户端 POST 端点 URI
                if event_type == Some("endpoint") {
                    let data = data_lines.join("\n");
                    if let Ok(endpoint_value) = serde_json::from_str::<Value>(&data)
                        && let Some(uri) = endpoint_value.get("uri").and_then(|v| v.as_str())
                    {
                        let mut endpoint_guard = message_endpoint.lock().await;
                        *endpoint_guard = Some(uri.to_string());
                        tracing::info!("SSE: 获取到 POST 端点 URI: {}", uri);
                        continue; // endpoint 事件不是 JSON-RPC 消息，跳过
                    }
                }

                // 更新 last_event_id
                if let Some(eid) = event_id_field {
                    *last_event_id = if eid.is_empty() {
                        None
                    } else {
                        Some(eid.to_string())
                    };
                }

                // 合并多行 data（SSE 规范允许 data 跨行）
                let data = data_lines.join("\n");
                if data.is_empty() {
                    // 空 data 事件：服务端用于 priming（让客户端记住 Last-Event-ID）
                    continue;
                }

                tracing::debug!("SSE: 收到数据: {}", data);

                let Ok(value) = serde_json::from_str::<Value>(&data) else {
                    tracing::debug!("SSE: 忽略非 JSON 数据: {}", data);
                    continue;
                };

                let has_rpc_id = value.get("id").is_some() && !value["id"].is_null();
                let has_result = value.get("result").is_some();
                let has_error = value.get("error").is_some();
                let has_method = value.get("method").is_some();

                if has_rpc_id && (has_result || has_error) {
                    // JSON-RPC 响应 → 分发给等待该 id 的 send() 调用
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
                    // JSON-RPC 通知 → 广播
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

#[async_trait]
impl McpTransport for SseTransport {
    async fn send(&self, mut request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        request.id = Some(Value::Number(id.into()));

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        // 获取动态端点 URI（从服务器的 endpoint event 获取）
        let endpoint_uri = {
            let guard = self.message_endpoint.lock().await;
            guard.clone().ok_or_else(|| {
                ReactError::Mcp(McpError::ProtocolError(
                    "SSE: 尚未获取到 POST 端点 URI，请等待连接建立".to_string(),
                ))
            })?
        };

        // POST 请求到动态端点
        let mut builder = self
            .client
            .post(&endpoint_uri)
            .header("Content-Type", "application/json")
            .json(&request);
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }

        let post_resp = builder.send().await.map_err(|e| {
            ReactError::Mcp(McpError::ConnectionFailed(format!(
                "POST {} 失败: {}",
                endpoint_uri, e
            )))
        })?;

        // 202 Accepted 是正常的，5xx 才算错误
        if post_resp.status().is_server_error() {
            let status = post_resp.status().as_u16();
            let body = post_resp.text().await.unwrap_or_default();
            self.pending.lock().await.remove(&id);
            return Err(ReactError::Mcp(McpError::ConnectionFailed(format!(
                "POST {} 返回服务器错误 {}: {}",
                endpoint_uri, status, body
            ))));
        }

        tracing::debug!("SSE: POST /message 成功（id={}），等待 SSE 响应…", id);

        // 通过 SSE 等待服务端推送响应（30 秒超时）
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                ReactError::Mcp(McpError::ProtocolError(format!(
                    "等待 SSE 响应超时（id={}）",
                    id
                )))
            })?
            .map_err(|_| {
                ReactError::Mcp(McpError::ProtocolError("响应 channel 已关闭".to_string()))
            })?;

        Ok(response)
    }

    async fn notify(&self, notification: JsonRpcNotification) -> Result<()> {
        // 获取动态端点 URI
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
        // fire-and-forget，忽略错误
        let _ = builder.send().await;
        Ok(())
    }

    async fn close(&self) {
        // _sse_task 随 SseTransport drop 时自动取消
    }

    fn notification_rx(&self) -> Option<Arc<dyn crate::mcp::types::JsonRpcNotificationReceiver>> {
        Some(Arc::new(NotificationReceiver::new(
            self.notification_tx.subscribe(),
        )))
    }
}
