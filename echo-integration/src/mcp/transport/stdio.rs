use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use super::super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use echo_core::error::{McpError, ReactError, Result};

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
        // stderr 重定向到 pipe，通过后台 task 转发到 tracing
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                "无法启动 MCP 服务端 '{}': {}",
                command, e
            ))))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                "无法获取子进程 stdin".to_string(),
            )))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                "无法获取子进程 stdout".to_string(),
            )))
        })?;

        let stderr = child.stderr.take();

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

                        if let Some(id) = json.get("id").and_then(|id| id.as_u64()) {
                            match serde_json::from_value::<JsonRpcResponse>(json) {
                                Ok(response) => {
                                    let mut map = pending_clone.lock().await;
                                    if let Some(tx) = map.remove(&id) {
                                        let _ = tx.send(response);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("MCP stdio: 解析响应失败: {}", e);
                                }
                            }
                        } else {
                            let method = json
                                .get("method")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown");
                            tracing::debug!("MCP stdio: 收到服务端通知: {}", method);
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("MCP stdio: stdout 已关闭");
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

        // 后台 task：读取 stderr 并转发到 tracing
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        tracing::debug!("MCP stderr: {}", line);
                    }
                }
            });
        }

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            _child: Arc::new(Mutex::new(child)),
        })
    }
}

impl McpTransport for StdioTransport {
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
        Box::pin(async move {
            let mut request = request;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            request.id = Some(Value::Number(id.into()));

            let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
            {
                let mut pending = self.pending.lock().await;
                pending.insert(id, tx);
            }

            let line = serde_json::to_string(&request)
                .map_err(|e| ReactError::Mcp(Box::new(McpError::ProtocolError(e.to_string()))))?
                + "\n";

            {
                let mut stdin = self.stdin.lock().await;
                stdin.write_all(line.as_bytes()).await.map_err(|e| {
                    ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                        "写入 stdin 失败: {}",
                        e
                    ))))
                })?;
                stdin.flush().await.map_err(|e| {
                    ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                        "flush stdin 失败: {}",
                        e
                    ))))
                })?;
            }

            // 使用超时等待响应，超时时清理 pending entry 防止泄漏
            const RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

            match tokio::time::timeout(RESPONSE_TIMEOUT, rx).await {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(_)) => {
                    // oneshot 发送端被丢弃（后台 task 崩溃）
                    self.pending.lock().await.remove(&id);
                    Err(ReactError::Mcp(Box::new(McpError::TransportClosed)))
                }
                Err(_) => {
                    // 超时，清理 pending entry 防止泄漏
                    self.pending.lock().await.remove(&id);
                    Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                        "等待响应超时 (id={}, 超时 {:?})",
                        id, RESPONSE_TIMEOUT
                    )))))
                }
            }
        })
    }

    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let line = serde_json::to_string(&notification)
                .map_err(|e| ReactError::Mcp(Box::new(McpError::ProtocolError(e.to_string()))))?
                + "\n";

            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "写入通知失败: {}",
                    e
                ))))
            })?;
            stdin.flush().await.map_err(|e| {
                ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "flush 通知失败: {}",
                    e
                ))))
            })?;
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut child = self._child.lock().await;
            if let Err(e) = child.kill().await {
                tracing::warn!("MCP stdio: 终止子进程失败: {}", e);
            }
            // 等待子进程退出，避免僵尸进程
            let _ = child.wait().await;
            tracing::debug!("MCP stdio: 子进程已退出");
        })
    }

    fn notification_rx(&self) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
        None
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Drop 时尝试关闭，清理子进程
        let child = self._child.clone();
        tokio::spawn(async move {
            let mut child = child.lock().await;
            if let Err(e) = child.kill().await {
                tracing::debug!("MCP stdio drop: kill 失败: {}", e);
            }
            let _ = child.wait().await;
        });
    }
}
