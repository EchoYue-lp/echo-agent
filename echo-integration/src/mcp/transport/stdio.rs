use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::redaction::text_with_secrets;
use echo_core::error::{McpError, ReactError, Result};

use super::{
    CloseCoordinator, McpTransport, PendingRequests, close_error, redact_response_error,
    transport_closed_error,
};

#[derive(Clone, Copy)]
struct StdioTimeouts {
    response: Duration,
    graceful_child: Duration,
    close: Duration,
}

impl Default for StdioTimeouts {
    fn default() -> Self {
        Self {
            response: Duration::from_secs(120),
            graceful_child: Duration::from_secs(2),
            close: Duration::from_secs(5),
        }
    }
}

/// stdio 传输层
///
/// 启动子进程，通过 stdin 发送 JSON-RPC 请求（每行一个 JSON），
/// 通过 stdout 读取响应，后台 task 负责将响应路由到对应的等待方。
pub struct StdioTransport {
    stdin: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    pending: Arc<PendingRequests>,
    next_id: Arc<AtomicU64>,
    child: Arc<Mutex<Option<Child>>>,
    stdout_task: Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
    stderr_task: Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
    close_coordinator: Arc<CloseCoordinator>,
    timeouts: StdioTimeouts,
}

impl StdioTransport {
    /// 启动 MCP 服务端进程并建立 stdio 传输
    ///
    /// # 安全模型
    ///
    /// MCP servers are user-selected local extensions. This transport checks
    /// only framework invariants; product policy remains application-owned.
    pub async fn new(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&std::path::Path>,
    ) -> Result<Self> {
        Self::new_with_timeouts(command, args, env, cwd, StdioTimeouts::default()).await
    }

    async fn new_with_timeouts(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&std::path::Path>,
        timeouts: StdioTimeouts,
    ) -> Result<Self> {
        validate_mcp_command(command)?;
        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // stderr 重定向到 pipe，通过后台 task 转发到 tracing
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

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
        let configured_secrets = args
            .iter()
            .cloned()
            .chain(env.iter().map(|(_, value)| value.clone()))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();

        let pending = PendingRequests::new();
        let pending_clone = pending.clone();
        let child = Arc::new(Mutex::new(Some(child)));
        let child_clone = Arc::clone(&child);
        let stdout_secrets = configured_secrets.clone();

        // 后台 task：持续读取 stdout，将响应路由到对应的 pending channel
        let stdout_task = tokio::spawn(Self::route_stdout(
            stdout,
            pending_clone,
            child_clone,
            timeouts,
            stdout_secrets,
        ));

        // 后台 task：读取 stderr 并转发到 tracing
        let stderr_task =
            stderr.map(|stderr| tokio::spawn(Self::drain_stderr(stderr, configured_secrets)));

        Ok(Self {
            stdin: Arc::new(Mutex::new(Some(stdin))),
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            child,
            stdout_task: Arc::new(Mutex::new(Some(stdout_task))),
            stderr_task: Arc::new(Mutex::new(stderr_task)),
            close_coordinator: CloseCoordinator::new(),
            timeouts,
        })
    }

    async fn route_stdout<R>(
        stdout: R,
        pending: Arc<PendingRequests>,
        child: Arc<Mutex<Option<Child>>>,
        timeouts: StdioTimeouts,
        secrets: Vec<String>,
    ) -> Result<()>
    where
        R: AsyncRead + Unpin,
    {
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
                        Ok(value) => value,
                        Err(error) => {
                            tracing::warn!(
                                "MCP stdio: 解析 stdout 行失败: {} | 原始内容: {}",
                                error,
                                text_with_secrets(&line, secrets.iter().map(String::as_str))
                            );
                            continue;
                        }
                    };

                    if let Some(id) = json.get("id").and_then(Value::as_u64) {
                        match serde_json::from_value::<JsonRpcResponse>(json) {
                            Ok(mut response) => {
                                redact_response_error(&mut response, &secrets);
                                pending.resolve(id, response);
                            }
                            Err(error) => {
                                tracing::warn!(
                                    "MCP stdio: 解析响应失败: {}",
                                    text_with_secrets(
                                        &error.to_string(),
                                        secrets.iter().map(String::as_str)
                                    )
                                );
                            }
                        }
                    } else {
                        let method = json
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        tracing::debug!("MCP stdio: 收到服务端通知: {}", method);
                    }
                }
                Ok(None) => {
                    tracing::debug!("MCP stdio: stdout 已关闭");
                    pending.close();
                    return Self::settle_child(&child, timeouts).await;
                }
                Err(error) => {
                    tracing::warn!("MCP stdio: 读取 stdout 出错: {}", error);
                    pending.close();
                    return match Self::settle_child(&child, timeouts).await {
                        Ok(()) => Err(close_error(format!(
                            "MCP stdio stdout read failed: {error}"
                        ))),
                        Err(cleanup_error) => Err(close_error(format!(
                            "MCP stdio stdout read failed: {error}; child cleanup also failed: {cleanup_error}"
                        ))),
                    };
                }
            }
        }
    }

    async fn drain_stderr<R>(stderr: R, secrets: Vec<String>) -> Result<()>
    where
        R: AsyncRead + Unpin,
    {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        tracing::debug!(
                            "MCP stderr: {}",
                            text_with_secrets(&line, secrets.iter().map(String::as_str))
                        );
                    }
                }
                Ok(None) => return Ok(()),
                Err(error) => {
                    return Err(close_error(format!(
                        "MCP stdio stderr read failed: {}",
                        text_with_secrets(&error.to_string(), secrets.iter().map(String::as_str))
                    )));
                }
            }
        }
    }

    async fn settle_child(
        child_slot: &Arc<Mutex<Option<Child>>>,
        timeouts: StdioTimeouts,
    ) -> Result<()> {
        let started_at = Instant::now();
        let close_deadline = started_at + timeouts.close;
        let graceful_deadline = (started_at + timeouts.graceful_child).min(close_deadline);
        Self::settle_child_until(child_slot, graceful_deadline, close_deadline).await
    }

    async fn settle_child_until(
        child_slot: &Arc<Mutex<Option<Child>>>,
        graceful_deadline: Instant,
        close_deadline: Instant,
    ) -> Result<()> {
        let mut child_slot = tokio::time::timeout_at(close_deadline, child_slot.lock())
            .await
            .map_err(|_| close_error("MCP stdio child owner lock timed out during close"))?;
        let Some(child) = child_slot.as_mut() else {
            return Ok(());
        };

        match tokio::time::timeout_at(graceful_deadline, child.wait()).await {
            Ok(Ok(status)) => {
                child_slot.take();
                tracing::debug!(?status, "MCP stdio: 子进程已退出");
                Ok(())
            }
            Ok(Err(error)) => Err(close_error(format!(
                "MCP stdio failed to wait for child: {error}"
            ))),
            Err(_) => {
                let kill_error = child.start_kill().err();
                match tokio::time::timeout_at(close_deadline, child.wait()).await {
                    Ok(Ok(status)) => {
                        child_slot.take();
                        if let Some(error) = kill_error {
                            Err(close_error(format!(
                                "MCP stdio child exited as {status}, but kill failed: {error}"
                            )))
                        } else {
                            tracing::debug!(?status, "MCP stdio: 超时后已终止并回收子进程");
                            Ok(())
                        }
                    }
                    Ok(Err(error)) => Err(close_error(format!(
                        "MCP stdio failed to reap killed child: {error}"
                    ))),
                    Err(_) => {
                        let kill_detail = kill_error
                            .map(|error| format!("; kill failed: {error}"))
                            .unwrap_or_default();
                        Err(close_error(format!(
                            "MCP stdio child did not settle before close deadline{kill_detail}"
                        )))
                    }
                }
            }
        }
    }

    async fn await_task_until(
        task_slot: &Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
        task_name: &str,
        deadline: Instant,
    ) -> Result<()> {
        let mut slot = task_slot.lock().await;
        let Some(mut task) = slot.take() else {
            return Ok(());
        };
        match tokio::time::timeout_at(deadline, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Err(error)) => Err(close_error(format!(
                "MCP stdio {task_name} task join failed: {error}"
            ))),
            Err(_) => {
                *slot = Some(task);
                Err(close_error(format!(
                    "MCP stdio {task_name} task did not settle before close deadline"
                )))
            }
        }
    }

    async fn close_owned(
        stdin: &Arc<Mutex<Option<tokio::process::ChildStdin>>>,
        pending: &Arc<PendingRequests>,
        child: &Arc<Mutex<Option<Child>>>,
        stdout_task: &Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
        stderr_task: &Arc<Mutex<Option<tokio::task::JoinHandle<Result<()>>>>>,
        timeouts: StdioTimeouts,
        deadline: Instant,
    ) -> Result<()> {
        let graceful_deadline = (Instant::now() + timeouts.graceful_child).min(deadline);
        pending.close();

        let mut errors = Vec::new();
        let mut stdin_closed = false;
        if let Ok(mut stdin) = tokio::time::timeout_at(graceful_deadline, stdin.lock()).await {
            stdin.take();
            stdin_closed = true;
        }
        if let Err(error) = Self::settle_child_until(child, graceful_deadline, deadline).await {
            errors.push(error.to_string());
        }
        if !stdin_closed {
            match tokio::time::timeout_at(deadline, stdin.lock()).await {
                Ok(mut stdin) => {
                    stdin.take();
                }
                Err(_) => errors
                    .push("MCP stdio writer did not settle before the close deadline".to_string()),
            }
        }
        if let Err(error) = Self::await_task_until(stdout_task, "stdout", deadline).await {
            errors.push(error.to_string());
        }
        if let Err(error) = Self::await_task_until(stderr_task, "stderr", deadline).await {
            errors.push(error.to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(close_error(format!(
                "MCP stdio close did not fully settle: {}",
                errors.join("; ")
            )))
        }
    }
}

impl McpTransport for StdioTransport {
    fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
        Box::pin(async move {
            let mut request = request;
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            request.id = Some(Value::Number(id.into()));

            let (rx, _registration) = self.pending.register(id)?;

            let line = serde_json::to_string(&request)
                .map_err(|e| ReactError::Mcp(Box::new(McpError::ProtocolError(e.to_string()))))?
                + "\n";

            {
                let mut stdin = self.stdin.lock().await;
                self.pending.ensure_open()?;
                let stdin = stdin.as_mut().ok_or_else(transport_closed_error)?;
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

            match tokio::time::timeout(self.timeouts.response, rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(transport_closed_error()),
                Err(_) => Err(ReactError::Mcp(Box::new(McpError::ProtocolError(format!(
                    "等待响应超时 (id={}, 超时 {:?})",
                    id, self.timeouts.response
                ))))),
            }
        })
    }

    fn notify(&self, notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let line = serde_json::to_string(&notification)
                .map_err(|e| ReactError::Mcp(Box::new(McpError::ProtocolError(e.to_string()))))?
                + "\n";

            let mut stdin = self.stdin.lock().await;
            self.pending.ensure_open()?;
            let stdin = stdin.as_mut().ok_or_else(transport_closed_error)?;
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

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let deadline = Instant::now() + self.timeouts.close;
            let (start, receipt) = self.close_coordinator.begin();
            if start {
                let coordinator = Arc::clone(&receipt);
                let stdin = Arc::clone(&self.stdin);
                let pending = Arc::clone(&self.pending);
                let child = Arc::clone(&self.child);
                let stdout_task = Arc::clone(&self.stdout_task);
                let stderr_task = Arc::clone(&self.stderr_task);
                let timeouts = self.timeouts;
                let producer = tokio::spawn(async move {
                    let result = Self::close_owned(
                        &stdin,
                        &pending,
                        &child,
                        &stdout_task,
                        &stderr_task,
                        timeouts,
                        deadline,
                    )
                    .await;
                    coordinator.complete(&result);
                });
                receipt.retain_producer(producer);
            }
            receipt.wait(deadline, "MCP stdio transport").await
        })
    }

    fn notification_rx(&self) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
        None
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.pending.close();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (start, coordinator) = self.close_coordinator.begin();
        if !start {
            return;
        }
        let stdin = Arc::clone(&self.stdin);
        let pending = Arc::clone(&self.pending);
        let child = Arc::clone(&self.child);
        let stdout_task = Arc::clone(&self.stdout_task);
        let stderr_task = Arc::clone(&self.stderr_task);
        let timeouts = self.timeouts;
        let deadline = Instant::now() + timeouts.close;
        runtime.spawn(async move {
            let result = Self::close_owned(
                &stdin,
                &pending,
                &child,
                &stdout_task,
                &stderr_task,
                timeouts,
                deadline,
            )
            .await;
            if let Err(error) = &result {
                tracing::warn!("MCP stdio drop cleanup failed: {error}");
            }
            coordinator.complete(&result);
        });
    }
}

/// Validate the command used to spawn an MCP server.
///
/// Commands are executed directly without a shell, so only an empty command
/// is a framework error. Users remain responsible for extensions they load.
fn validate_mcp_command(command: &str) -> Result<()> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
            "MCP server command is empty".to_string(),
        ))));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    struct ErrorReader;

    impl AsyncRead for ErrorReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::Error::other("injected read failure")))
        }
    }

    fn short_timeouts() -> StdioTimeouts {
        StdioTimeouts {
            response: Duration::from_millis(20),
            graceful_child: Duration::from_millis(20),
            close: Duration::from_millis(250),
        }
    }

    #[test]
    fn rejects_empty_command() {
        assert!(validate_mcp_command("   ").is_err());
    }

    #[test]
    fn accepts_user_selected_commands() {
        assert!(validate_mcp_command("npx").is_ok());
        assert!(validate_mcp_command("/usr/local/bin/my-mcp-server").is_ok());
        assert!(validate_mcp_command("sh").is_ok());
    }

    #[tokio::test]
    async fn stdout_eof_fails_every_pending_request() -> Result<()> {
        let pending = PendingRequests::new();
        let (receiver, _registration) = pending.register(1)?;
        let child = Arc::new(Mutex::new(None));

        StdioTransport::route_stdout(
            tokio::io::empty(),
            Arc::clone(&pending),
            child,
            short_timeouts(),
            Vec::new(),
        )
        .await?;

        assert!(matches!(receiver.await, Ok(Err(ReactError::Mcp(_)))));
        assert_eq!(pending.len(), 0);
        assert!(pending.register(2).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn stdout_read_error_fails_every_pending_request() -> Result<()> {
        let pending = PendingRequests::new();
        let (receiver, _registration) = pending.register(1)?;
        let child = Arc::new(Mutex::new(None));

        let result = StdioTransport::route_stdout(
            ErrorReader,
            Arc::clone(&pending),
            child,
            short_timeouts(),
            Vec::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(receiver.await, Ok(Err(ReactError::Mcp(_)))));
        assert_eq!(pending.len(), 0);
        assert!(pending.register(2).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_drains_request_and_awaits_real_child_and_io_tasks() -> Result<()> {
        let mut timeouts = short_timeouts();
        timeouts.response = Duration::from_secs(1);
        let transport = Arc::new(
            StdioTransport::new_with_timeouts(
                "/bin/sh",
                &[
                    "-c".to_string(),
                    "while IFS= read -r line; do :; done".to_string(),
                ],
                &[],
                None,
                timeouts,
            )
            .await?,
        );
        let child_id = transport
            .child
            .lock()
            .await
            .as_ref()
            .and_then(Child::id)
            .ok_or_else(|| close_error("test child has no process id"))?;
        let send_task = tokio::spawn({
            let transport = Arc::clone(&transport);
            async move { transport.send(JsonRpcRequest::new("test", None)).await }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while transport.pending.len() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| close_error("request did not enter pending registry"))?;

        transport.close().await?;

        let send_result = send_task
            .await
            .map_err(|error| close_error(format!("send task join failed: {error}")))?;
        assert!(send_result.is_err());
        assert_eq!(transport.pending.len(), 0);
        assert!(transport.stdin.lock().await.is_none());
        assert!(transport.child.lock().await.is_none());
        assert!(transport.stdout_task.lock().await.is_none());
        assert!(transport.stderr_task.lock().await.is_none());
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(child_id.to_string())
            .status()?;
        assert!(!status.success());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_forces_and_reaps_child_that_ignores_stdin() -> Result<()> {
        let transport = StdioTransport::new_with_timeouts(
            "/bin/sh",
            &["-c".to_string(), "exec sleep 30".to_string()],
            &[],
            None,
            short_timeouts(),
        )
        .await?;
        let child_id = transport
            .child
            .lock()
            .await
            .as_ref()
            .and_then(Child::id)
            .ok_or_else(|| close_error("test child has no process id"))?;
        let started_at = std::time::Instant::now();

        transport.close().await?;

        assert!(started_at.elapsed() < Duration::from_secs(1));
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(child_id.to_string())
            .status()?;
        assert!(!status.success());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_close_future_keeps_child_owner_until_retry_waits() -> Result<()> {
        let transport = Arc::new(
            StdioTransport::new_with_timeouts(
                "/bin/sh",
                &["-c".to_string(), "exec sleep 30".to_string()],
                &[],
                None,
                short_timeouts(),
            )
            .await?,
        );
        let child_guard = transport.child.lock().await;
        let first_close = tokio::spawn({
            let transport = Arc::clone(&transport);
            async move { transport.close().await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        first_close.abort();
        let _ = first_close.await;
        drop(child_guard);

        transport.close().await?;

        assert!(transport.child.lock().await.is_none());
        assert!(transport.stdout_task.lock().await.is_none());
        assert!(transport.stderr_task.lock().await.is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn close_retries_child_after_owning_runtime_stops_mid_wait() -> Result<()> {
        let first_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (transport, child_id) = first_runtime.block_on(async {
            let transport = Arc::new(
                StdioTransport::new_with_timeouts(
                    "/bin/sh",
                    &["-c".to_string(), "exec sleep 30".to_string()],
                    &[],
                    None,
                    short_timeouts(),
                )
                .await?,
            );
            let child_id = transport
                .child
                .lock()
                .await
                .as_ref()
                .and_then(Child::id)
                .ok_or_else(|| close_error("test child has no process id"))?;
            let close = tokio::spawn({
                let transport = Arc::clone(&transport);
                async move { transport.close().await }
            });
            for _ in 0..100 {
                if transport.child.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(transport.child.try_lock().is_err());
            assert!(!close.is_finished());
            Ok::<_, ReactError>((transport, child_id))
        })?;
        drop(first_runtime);

        let second_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        second_runtime.block_on(async { transport.close().await })?;
        assert!(transport.child.blocking_lock().is_none());
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(child_id.to_string())
            .status()?;
        assert!(!status.success());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn server_stdout_eof_settles_real_child_before_close_returns() -> Result<()> {
        let transport = StdioTransport::new_with_timeouts(
            "/bin/sh",
            &["-c".to_string(), "exec 1>&-; exec sleep 30".to_string()],
            &[],
            None,
            short_timeouts(),
        )
        .await?;
        let child_id = transport
            .child
            .lock()
            .await
            .as_ref()
            .and_then(Child::id)
            .ok_or_else(|| close_error("test child has no process id"))?;

        transport.close().await?;

        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(child_id.to_string())
            .status()?;
        assert!(!status.success());
        assert!(transport.stdout_task.lock().await.is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_unblocks_and_awaits_a_writer_stalled_on_child_stdin() -> Result<()> {
        let mut timeouts = short_timeouts();
        timeouts.response = Duration::from_secs(2);
        let transport = Arc::new(
            StdioTransport::new_with_timeouts(
                "/bin/sh",
                &["-c".to_string(), "exec sleep 30".to_string()],
                &[],
                None,
                timeouts,
            )
            .await?,
        );
        let payload = "x".repeat(2 * 1024 * 1024);
        let send_task = tokio::spawn({
            let transport = Arc::clone(&transport);
            async move {
                transport
                    .send(JsonRpcRequest::new(
                        "test",
                        Some(serde_json::json!({"payload": payload})),
                    ))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if transport.stdin.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| close_error("request writer did not block on child stdin"))?;
        let started_at = std::time::Instant::now();

        transport.close().await?;

        assert!(started_at.elapsed() < Duration::from_secs(1));
        let result = send_task
            .await
            .map_err(|error| close_error(format!("send task join failed: {error}")))?;
        assert!(result.is_err());
        assert!(transport.stdin.lock().await.is_none());
        assert!(transport.child.lock().await.is_none());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn response_timeout_removes_pending_request() -> Result<()> {
        let transport = StdioTransport::new_with_timeouts(
            "/bin/sh",
            &[
                "-c".to_string(),
                "while IFS= read -r line; do :; done".to_string(),
            ],
            &[],
            None,
            short_timeouts(),
        )
        .await?;

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
}
