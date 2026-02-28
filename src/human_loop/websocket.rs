use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{HumanLoopKind, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use crate::error::{ReactError, Result};

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
/// use echo_agent::human_loop::WebSocketHumanLoopProvider;
///
/// #[tokio::main]
/// async fn main() {
///     let provider = WebSocketHumanLoopProvider::bind(9000).await.unwrap();
///     // 客户端连接 ws://127.0.0.1:9000
/// }
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

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let clients: ClientSenders = Arc::new(Mutex::new(Vec::new()));

        let pending_bg = pending.clone();
        let clients_bg = clients.clone();

        tokio::spawn(async move {
            info!("WebSocket 人工介入服务器已启动: ws://127.0.0.1:{port}");
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        debug!("新的 WebSocket 客户端连接: {addr}");
                        let pending = pending_bg.clone();
                        let clients = clients_bg.clone();
                        tokio::spawn(handle_connection(stream, addr, pending, clients));
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
        })
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
) {
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            warn!("WebSocket 握手失败 ({addr}): {e}");
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    clients.lock().await.push(tx);

    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = write.send(Message::Text(msg)).await {
                warn!("WS 消息发送失败: {e}");
                break;
            }
        }
    });

    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(Message::Text(text)) => match serde_json::from_str::<ClientResponse>(&text) {
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
            },
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }

    write_task.abort();
    info!("WebSocket 客户端断开: {addr}");
}

#[async_trait]
impl HumanLoopProvider for WebSocketHumanLoopProvider {
    async fn request(&self, req: HumanLoopRequest) -> Result<HumanLoopResponse> {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let kind_str = match req.kind {
            HumanLoopKind::Approval => "approval",
            HumanLoopKind::Input => "input",
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
    }
}
