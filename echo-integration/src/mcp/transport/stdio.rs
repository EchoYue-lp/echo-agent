use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use super::super::types::JsonRpcError;
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
    ///
    /// # 安全模型
    ///
    /// MCP 服务端是任意外部进程，`command`/`args`/`env` 来自 `mcp.json`（项目本地
    /// 配置文件）。本函数做基础校验（拒绝空命令、以 `-` 开头的参数注入、危险元字符），
    /// 但**不**做可执行文件白名单——因为合法服务端可能是 `npx`、`uvx`、`python -m …` 等任意解释器。
    ///
    /// 调用方（应用层）应：从可信目录加载 `mcp.json`，并在首次连接每个服务端时
    /// 要求人工确认（`PermissionService`）。
    pub async fn new(command: &str, args: &[String], env: &[(String, String)]) -> Result<Self> {
        validate_mcp_command(command, args)?;
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
                        // Send error responses to all pending requests before clearing,
                        // so callers know which specific request failed due to transport close
                        for (id, tx) in map.drain() {
                            let error_response = JsonRpcResponse {
                                jsonrpc: "2.0".to_string(),
                                id: Some(serde_json::Value::Number(id.into())),
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32000,
                                    message: "MCP transport closed: stdout reached EOF".to_string(),
                                    data: None,
                                }),
                            };
                            let _ = tx.send(error_response);
                        }
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

/// Validate the command used to spawn an MCP server.
///
/// Defense-in-depth against a malicious `mcp.json`: rejects empty commands,
/// argument injection (args starting with `-`, which could become flags to
/// the spawned binary), and obvious shell-meta abuse. This does **not**
/// whitelist executables — MCP servers are legitimately arbitrary
/// interpreters (`npx`, `uvx`, `python -m …`); the real trust boundary is
/// "where did this mcp.json come from", enforced by the caller.
fn validate_mcp_command(command: &str, args: &[String]) -> Result<()> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
            "MCP server command is empty".to_string(),
        ))));
    }

    // Reject a bare shell as the command — that turns every arg into a shell
    // script and bypasses any argv-level control. Legitimate servers invoke a
    // real interpreter binary directly.
    let basename = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    if matches!(
        basename,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "cmd" | "powershell" | "pwsh"
    ) {
        tracing::warn!(
            command = %command,
            "MCP server uses a bare shell as its command; this allows arbitrary shell \
             execution from mcp.json and is strongly discouraged"
        );
    }

    // Argument-injection guard: block options that are known to enable
    // arbitrary-code execution or credential exfiltration when passed to
    // certain binaries (e.g. `--upload-pack=` to git-over-ssh, ssh `-o`
    // proxy commands). Normal interpreter flags (`-y`, `-m`) are allowed
    // because they are the standard way to launch MCP servers.
    const DANGEROUS_ARG_PREFIXES: &[&str] = &["--upload-pack", "--config"];
    for arg in args {
        let lower = arg.to_lowercase();
        if lower.starts_with("--upload-pack") || lower.starts_with("-o") {
            return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!(
                    "MCP server argument '{arg}' may enable arbitrary command execution \
                     (git/ssh option injection) and is rejected",
                ),
            ))));
        }
        for prefix in DANGEROUS_ARG_PREFIXES {
            if lower.starts_with(prefix) {
                return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                    format!(
                        "MCP server argument '{arg}' matches a dangerous option prefix \
                         ('{prefix}') and is rejected",
                    ),
                ))));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_mcp_command;

    #[test]
    fn rejects_empty_command() {
        assert!(validate_mcp_command("   ", &[]).is_err());
    }

    #[test]
    fn rejects_dangerous_option_injection() {
        // git-over-ssh upload-pack injection
        assert!(validate_mcp_command("git", &["--upload-pack=evil".into()]).is_err());
        // ssh -o ProxyCommand injection
        assert!(validate_mcp_command("ssh", &["-o".into(), "ProxyCommand=evil".into()]).is_err());
    }

    #[test]
    fn accepts_legitimate_interpreter_flags() {
        // These are the standard ways real MCP servers are launched.
        assert!(
            validate_mcp_command("npx", &["-y".into(), "@modelcontextprotocol/server".into()])
                .is_ok()
        );
        assert!(validate_mcp_command("python", &["-m".into(), "mcp_server".into()]).is_ok());
        assert!(validate_mcp_command("node", &["server.js".into()]).is_ok());
    }

    #[test]
    fn accepts_plain_command_and_paths() {
        assert!(validate_mcp_command("/usr/local/bin/my-mcp-server", &[]).is_ok());
        assert!(validate_mcp_command("uvx", &["mcp-server".into()]).is_ok());
    }
}
