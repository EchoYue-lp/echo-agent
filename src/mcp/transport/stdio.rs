use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

use crate::error::{McpError, ReactError, Result};
use crate::mcp::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

use super::McpTransport;

/// 等待响应的发送端 Map：请求 ID → oneshot channel
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

/// stdio 传输层
///
/// 启动子进程，通过 stdin 发送 JSON-RPC 请求（每行一个 JSON），
/// 通过 stdout 读取响应，后台 task 负责将响应路由到对应的等待方。
pub struct StdioTransport {
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: PendingMap,
    next_id: Arc<AtomicU64>,
    _child: Arc<Mutex<Child>>,
}

impl StdioTransport {
    /// 启动 MCP 服务端进程并建立 stdio 传输
    pub async fn new(command: &str, args: &[String], env: &[(String, String)]) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // stderr 继承到父进程，方便调试
        cmd.stderr(Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            ReactError::Mcp(McpError::ConnectionFailed(format!(
                "无法启动 MCP 服务端 '{}': {}",
                command, e
            )))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ReactError::Mcp(McpError::ConnectionFailed(
                "无法获取子进程 stdin".to_string(),
            ))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ReactError::Mcp(McpError::ConnectionFailed(
                "无法获取子进程 stdout".to_string(),
            ))
        })?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        // 后台 task：持续读取 stdout，将响应路由到对应的 pending channel
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim().to_string();
                        if line.is_empty() {
                            continue;
                        }

                        // 解析为 JSON 值
                        let json: Value = match serde_json::from_str(&line) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(
                                    "MCP stdio: 解析 stdout 行失败: {} | 原始内容: {}",
                                    e,
                                    line
                                );
                                continue;
                            }
                        };

                        // 有 id → 这是对某个请求的响应
                        if let Some(id) = json.get("id").and_then(|id| id.as_u64()) {
                            match serde_json::from_value::<JsonRpcResponse>(json) {
                                Ok(response) => {
                                    let mut map = pending_clone.lock().await;
                                    if let Some(tx) = map.remove(&id) {
                                        // 忽略发送失败（调用方可能已超时取消）
                                        let _ = tx.send(response);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("MCP stdio: 解析响应失败: {}", e);
                                }
                            }
                        } else {
                            // 无 id → 服务端主动通知，记录日志后忽略
                            let method = json
                                .get("method")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown");
                            tracing::debug!("MCP stdio: 收到服务端通知: {}", method);
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("MCP stdio: stdout 已关闭");
                        // 唤醒所有等待中的调用方，让它们收到传输层关闭错误
                        let mut map = pending_clone.lock().await;
                        map.clear();
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("MCP stdio: 读取 stdout 出错: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            _child: Arc::new(Mutex::new(child)),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, mut request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        // 分配全局唯一 ID
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        request.id = Some(Value::Number(id.into()));

        // 注册等待 channel
        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        // 序列化并写入 stdin（每条消息一行）
        let line = serde_json::to_string(&request)
            .map_err(|e| ReactError::Mcp(McpError::ProtocolError(e.to_string())))?
            + "\n";

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await.map_err(|e| {
                ReactError::Mcp(McpError::ProtocolError(format!(
                    "写入 stdin 失败: {}",
                    e
                )))
            })?;
            stdin.flush().await.map_err(|e| {
                ReactError::Mcp(McpError::ProtocolError(format!("flush stdin 失败: {}", e)))
            })?;
        }

        // 等待后台 task 路由回响应
        rx.await
            .map_err(|_| ReactError::Mcp(McpError::TransportClosed))
    }

    async fn notify(&self, notification: JsonRpcNotification) -> Result<()> {
        let line = serde_json::to_string(&notification)
            .map_err(|e| ReactError::Mcp(McpError::ProtocolError(e.to_string())))?
            + "\n";

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| {
            ReactError::Mcp(McpError::ProtocolError(format!("写入通知失败: {}", e)))
        })?;
        stdin.flush().await.map_err(|e| {
            ReactError::Mcp(McpError::ProtocolError(format!("flush 通知失败: {}", e)))
        })?;
        Ok(())
    }

    async fn close(&self) {
        let mut child = self._child.lock().await;
        if let Err(e) = child.kill().await {
            tracing::warn!("MCP stdio: 终止服务端进程失败: {}", e);
        }
    }
}
