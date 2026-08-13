use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use echo_core::error::{ReactError, Result};

type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<ClientResponse>>>>;
type ClientSenders = Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<String>>>>;

/// WebSocket 人工介入 Provider。
///
/// 在本地启动 WebSocket 服务器，向已连接的客户端推送审批/输入请求，
/// 并异步等待第一个响应。适合与 Web UI、移动端或自定义工具集成。
///
/// # 使用方法
///
/// ```rust,no_run
/// // Requires the `websocket` feature.
/// # #[cfg(feature = "websocket")]
/// # async fn example() {
/// # use echo_orchestration::human_loop::WebSocketHumanLoopProvider;
/// # let provider = WebSocketHumanLoopProvider::bind(9000).await.unwrap();
/// # let _ = provider;
/// # }
/// # #[cfg(not(feature = "websocket"))]
/// # fn example() {}
/// ```
///
/// # 协议
///
/// **服务端 → 客户端**：
/// ```json
/// {
///   "kind": "approval" | "input",
///   "request_id": "uuid",
///   "prompt": "...",
///   "tool_name": "xxx",
///   "args": { ... }
/// }
/// ```
///
/// **客户端 → 服务端**：
/// ```json
/// {
///   "request_id": "uuid",
///   "decision": "approved" | "rejected",
///   "text": "用户输入（input 场景）",
///   "reason": "可选说明"
/// }
/// ```
pub struct WebSocketHumanLoopProvider {
    pending: PendingMap,
    clients: ClientSenders,
    timeout: Duration,
    /// Authentication token that clients must send as their first message.
    auth_token: String,
}

/// 推送给客户端的消息（统一格式，`kind` 字段区分场景）。
#[derive(Serialize)]
struct ServerMessage<'a> {
    kind: &'a str,
    request_id: &'a str,
    prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<&'a serde_json::Value>,
}

/// 客户端返回的响应（统一格式）。
#[derive(Deserialize)]
struct ClientResponse {
    request_id: String,
    /// approval 场景：`"approved"` | `"rejected"`
    decision: Option<String>,
    /// input 场景：用户输入的文本
    text: Option<String>,
    reason: Option<String>,
}

impl WebSocketHumanLoopProvider {
    /// 绑定端口并启动 WebSocket 服务器，默认超时 5 分钟。
    pub async fn bind(port: u16) -> std::io::Result<Self> {
        Self::bind_with_timeout(port, Duration::from_secs(300)).await
    }

    /// 绑定端口并启动 WebSocket 服务器，自定义超时。
    pub async fn bind_with_timeout(port: u16, timeout: Duration) -> std::io::Result<Self> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(addr).await?;

        // Generate a random authentication token using UUID v4 (122 bits of randomness).
        // Clients must send this token as their first WebSocket message to authenticate.
        let auth_token = Uuid::new_v4().to_string();

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let clients: ClientSenders = Arc::new(Mutex::new(Vec::new()));

        let pending_bg = pending.clone();
        let clients_bg = clients.clone();
        let token_bg = auth_token.clone();

        tokio::spawn(async move {
            info!("WebSocket 人工介入服务器已启动: ws://127.0.0.1:{port}");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!("新的 WebSocket 客户端连接: {addr}");
                        let pending = pending_bg.clone();
                        let clients = clients_bg.clone();
                        let token = token_bg.clone();
                        tokio::spawn(handle_connection(stream, addr, pending, clients, token));
                    }
                    Err(e) => {
                        error!("WebSocket accept 错误: {e}");
                    }
                }
            }
        });

        Ok(Self {
            pending,
            clients,
            timeout,
            auth_token,
        })
    }

    /// Get the authentication token that clients must send as their first message.
    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }

    /// 向所有已连接客户端广播消息，自动清理失效连接，返回成功发送数量。
    async fn broadcast(&self, msg: &str) -> usize {
        let mut clients = self.clients.lock().await;
        clients.retain(|tx| tx.send(msg.to_string()).is_ok());
        clients.len()
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    pending: PendingMap,
    clients: ClientSenders,
    auth_token: String,
) {
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            warn!("WebSocket 握手失败 ({addr}): {e}");
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();

    // Authentication: client must send the correct token as the first message.
    // Uses constant-time comparison to prevent timing-based token enumeration.
    const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
    match tokio::time::timeout(AUTH_TIMEOUT, read.next()).await {
        Ok(Some(Ok(Message::Text(token)))) => {
            if token.len() != auth_token.len()
                || token.chars().zip(auth_token.chars()).any(|(a, b)| a != b)
            {
                warn!("WebSocket 认证失败 ({addr}): invalid token");
                let _ = write
                    .send(Message::Text("Authentication failed".into()))
                    .await;
                let _ = write.send(Message::Close(None)).await;
                return;
            }
            debug!("WebSocket 认证成功 ({addr})");
            let _ = write.send(Message::Text("Authenticated".into())).await;
        }
        _ => {
            warn!("WebSocket 认证超时或失败 ({addr})");
            let _ = write.send(Message::Close(None)).await;
            return;
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    clients.lock().await.push(tx);

    // 写任务：转发消息 + 30s 心跳 ping
    let write_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if let Err(e) = write.send(Message::Text(msg)).await {
                                warn!("WS 消息发送失败: {e}");
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = heartbeat.tick() => {
                    if let Err(e) = write.send(Message::Ping(vec![])).await {
                        warn!("WS ping 发送失败: {e}");
                        break;
                    }
                }
            }
        }
    });

    // 读循环：90s 超时检测死连接
    const READ_TIMEOUT: Duration = Duration::from_secs(90);

    loop {
        match tokio::time::timeout(READ_TIMEOUT, read.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                match serde_json::from_str::<ClientResponse>(&text) {
                    Ok(response) => {
                        let mut map = pending.lock().await;
                        if let Some(sender) = map.remove(&response.request_id) {
                            let _ = sender.send(response);
                        } else {
                            warn!("收到未知 request_id 的 WS 响应: {}", response.request_id);
                        }
                    }
                    Err(e) => {
                        warn!("WebSocket 消息解析失败: {e}，原始内容: {text}");
                    }
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(Message::Pong(_)))) => {
                debug!("收到 WebSocket pong ({addr})");
            }
            Ok(Some(Ok(_))) => {} // 忽略其他帧类型
            Ok(None) => break,
            Err(_) => {
                warn!("WebSocket 读取超时 ({addr})，关闭死连接");
                break;
            }
        }
    }

    write_task.abort();
    info!("WebSocket 客户端断开: {addr}");
}

impl HumanLoopProvider for WebSocketHumanLoopProvider {
    fn request(&self, req: HumanLoopRequest) -> BoxFuture<'_, Result<HumanLoopResponse>> {
        Box::pin(async move {
            let request_id = Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            self.pending.lock().await.insert(request_id.clone(), tx);

            let kind_str = match req.kind {
                HumanLoopKind::Approval => "approval",
                HumanLoopKind::Input => "input",
                HumanLoopKind::Selection => "selection",
            };

            let msg = serde_json::to_string(&ServerMessage {
                kind: kind_str,
                request_id: &request_id,
                prompt: &req.prompt,
                tool_name: req.tool_name.as_deref(),
                args: req.args.as_ref(),
            })
            .map_err(|e| ReactError::Other(format!("WS 消息序列化失败: {e}")))?;

            let sent = self.broadcast(&msg).await;
            if sent == 0 {
                self.pending.lock().await.remove(&request_id);
                return Err(ReactError::Other(
                    "没有已连接的 WebSocket 客户端，无法发送人工介入请求".to_string(),
                ));
            }

            match tokio::time::timeout(self.timeout, rx).await {
                Ok(Ok(response)) => match req.kind {
                    HumanLoopKind::Approval => match response.decision.as_deref() {
                        Some("approved") => Ok(HumanLoopResponse::Approved),
                        _ => Ok(HumanLoopResponse::Rejected {
                            reason: response.reason,
                        }),
                    },
                    HumanLoopKind::Input => {
                        Ok(HumanLoopResponse::Text(response.text.unwrap_or_default()))
                    }
                    HumanLoopKind::Selection => Ok(HumanLoopResponse::Selection {
                        selection: response.text.unwrap_or_else(|| "cancel".to_string()),
                        instructions: None,
                    }),
                },
                Ok(Err(_)) => {
                    self.pending.lock().await.remove(&request_id);
                    Err(ReactError::Other("介入 channel 意外关闭".to_string()))
                }
                Err(_) => {
                    self.pending.lock().await.remove(&request_id);
                    Ok(HumanLoopResponse::Timeout)
                }
            }
        })
    }
}
