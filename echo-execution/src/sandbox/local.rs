//! 本地沙箱执行器
//!
//! 使用本地进程执行命令，并在支持的平台启用操作系统原生隔离：
//! - **macOS**: `sandbox-exec` (Seatbelt)
//! - **Linux**: 当前仍是进程级隔离 + 资源限制（尚未接入 `bubblewrap`）
//! - **其他**: 仅进程隔离（超时 + 输出截断）
//! - 支持通过 `SandboxCommand::stdin` 传入标准输入
//!
//! 这是最轻量的沙箱层，适合受信代码和只读操作。

use super::{
    CommandKind, ExecutionResult, IsolationLevel, ResourceLimits, SandboxCommand, SandboxExecutor,
};
use echo_core::error::Result;
use echo_core::error::SandboxError;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// 本地沙箱配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    /// 是否启用 OS 级沙箱（sandbox-exec / bubblewrap）
    pub enable_os_sandbox: bool,
    /// 允许访问的路径（只读）
    pub allowed_read_paths: Vec<PathBuf>,
    /// 允许访问的路径（读写）
    pub allowed_write_paths: Vec<PathBuf>,
    /// 是否允许网络访问
    pub allow_network: bool,
    /// 默认超时（秒）
    pub default_timeout_secs: u64,
    /// 最大输出大小（字节）
    pub max_output_bytes: usize,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            enable_os_sandbox: cfg!(target_os = "macos"),
            allowed_read_paths: vec![PathBuf::from("/usr"), PathBuf::from("/bin")],
            allowed_write_paths: vec![],
            allow_network: false,
            default_timeout_secs: 30,
            max_output_bytes: 1024 * 1024, // 1 MB
        }
    }
}

/// 本地沙箱执行器
#[derive(Debug, Clone)]
pub struct LocalSandbox {
    config: LocalConfig,
}

impl LocalSandbox {
    /// 使用默认配置创建
    pub fn new(config: LocalConfig) -> Self {
        Self { config }
    }

    fn effective_os_sandbox_enabled(&self) -> bool {
        self.config.enable_os_sandbox && cfg!(target_os = "macos")
    }

    /// 构建 shell 命令
    fn build_shell_command(&self, cmd: &str, sandbox_cmd: &SandboxCommand) -> Command {
        let mut command = if self.effective_os_sandbox_enabled() {
            // macOS: 使用 sandbox-exec
            let profile = self.build_seatbelt_profile();
            let mut c = Command::new("sandbox-exec");
            c.arg("-p").arg(profile).arg("sh").arg("-c").arg(cmd);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(cmd);
            c
        };

        // 设置工作目录
        if let Some(ref dir) = sandbox_cmd.working_dir {
            command.current_dir(dir);
        }

        // 设置环境变量
        for (k, v) in &sandbox_cmd.env {
            command.env(k, v);
        }

        command
    }

    /// 构建程序执行命令
    fn build_program_command(
        &self,
        program: &str,
        args: &[String],
        sandbox_cmd: &SandboxCommand,
    ) -> Command {
        let mut command = if self.effective_os_sandbox_enabled() {
            let profile = self.build_seatbelt_profile();
            let mut c = Command::new("sandbox-exec");
            c.arg("-p").arg(profile).arg(program);
            c.args(args);
            c
        } else {
            let mut c = Command::new(program);
            c.args(args);
            c
        };

        if let Some(ref dir) = sandbox_cmd.working_dir {
            command.current_dir(dir);
        }
        for (k, v) in &sandbox_cmd.env {
            command.env(k, v);
        }

        command
    }

    /// 构建代码执行命令
    fn build_code_command(
        &self,
        language: &str,
        code: &str,
        sandbox_cmd: &SandboxCommand,
    ) -> std::result::Result<Command, SandboxError> {
        let (interpreter, flag) = match language {
            "python" | "python3" => ("python3", "-c"),
            "node" | "javascript" | "js" => ("node", "-e"),
            "ruby" => ("ruby", "-e"),
            "perl" => ("perl", "-e"),
            "lua" => ("lua", "-e"),
            "php" => ("php", "-r"),
            "bash" | "sh" => ("sh", "-c"),
            _ => {
                return Err(SandboxError::Unavailable(format!(
                    "Unsupported language: {language}"
                )));
            }
        };

        let mut command = if self.effective_os_sandbox_enabled() {
            let profile = self.build_seatbelt_profile();
            let mut c = Command::new("sandbox-exec");
            c.arg("-p")
                .arg(profile)
                .arg(interpreter)
                .arg(flag)
                .arg(code);
            c
        } else {
            let mut c = Command::new(interpreter);
            c.arg(flag).arg(code);
            c
        };

        if let Some(ref dir) = sandbox_cmd.working_dir {
            command.current_dir(dir);
        }
        for (k, v) in &sandbox_cmd.env {
            command.env(k, v);
        }

        Ok(command)
    }

    /// 生成 macOS Seatbelt profile
    fn build_seatbelt_profile(&self) -> String {
        let mut profile = String::from("(version 1)\n(deny default)\n");

        // 基本权限
        profile.push_str("(allow process-exec)\n");
        profile.push_str("(allow process-fork)\n");
        profile.push_str("(allow sysctl-read)\n");
        profile.push_str("(allow mach-lookup)\n");

        // 读取权限：转义路径中的引号防止 sandbox profile 语法破坏
        for path in &self.config.allowed_read_paths {
            let escaped = path.display().to_string().replace('"', "\\\"");
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
        }
        // 始终允许读取基本系统路径
        profile.push_str("(allow file-read* (subpath \"/usr\"))\n");
        profile.push_str("(allow file-read* (subpath \"/bin\"))\n");
        profile.push_str("(allow file-read* (subpath \"/Library\"))\n");
        profile.push_str("(allow file-read* (subpath \"/System\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/null\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/urandom\"))\n");

        // 写入权限：转义路径中的引号
        for path in &self.config.allowed_write_paths {
            let escaped = path.display().to_string().replace('"', "\\\"");
            profile.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", escaped));
        }
        // 允许写 /dev/null
        profile.push_str("(allow file-write* (literal \"/dev/null\"))\n");

        // 临时文件
        profile.push_str("(allow file-read* (subpath \"/tmp\"))\n");
        profile.push_str("(allow file-write* (subpath \"/tmp\"))\n");
        profile.push_str("(allow file-read* (subpath \"/private/tmp\"))\n");
        profile.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");

        // 网络
        if self.config.allow_network {
            profile.push_str("(allow network*)\n");
        }

        profile
    }

    /// 执行命令并收集输出
    ///
    /// 超时时显式 kill + wait 清理子进程，避免僵尸/孤儿进程残留。
    async fn run_command(
        &self,
        mut command: Command,
        timeout: std::time::Duration,
        stdin: Option<&str>,
    ) -> Result<ExecutionResult> {
        if stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

        let start = Instant::now();

        let mut child = command.spawn().map_err(|e| {
            echo_core::error::ReactError::Sandbox(SandboxError::StartFailed(format!(
                "Failed to spawn process: {e}"
            )))
        })?;

        // 写入 stdin 并处理写入失败时的进程清理
        if let Some(input) = stdin {
            if let Some(mut child_stdin) = child.stdin.take() {
                if let Err(e) = child_stdin.write_all(input.as_bytes()).await {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(echo_core::error::ReactError::Sandbox(
                        SandboxError::IoError(format!("Failed to write stdin: {e}")),
                    ));
                }
                // 关闭 stdin 发送 EOF
                drop(child_stdin);
            }
        }

        // 提前取出 stdout/stderr 管道，避免 child.wait() 消耗后无法读取
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        // 使用 &mut self 等待，保留 Child 句柄的控制权
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => {
                let duration = start.elapsed();
                let stdout = read_pipe_output(stdout_pipe, self.config.max_output_bytes).await;
                let stderr = read_pipe_output(stderr_pipe, self.config.max_output_bytes).await;

                Ok(ExecutionResult {
                    exit_code: status.code().unwrap_or(-1),
                    stdout,
                    stderr,
                    duration,
                    sandbox_type: "local".to_string(),
                    timed_out: false,
                })
            }
            Ok(Err(e)) => {
                // wait() 自身失败 — 清理进程
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(echo_core::error::ReactError::Sandbox(
                    SandboxError::IoError(format!("Process wait error: {e}")),
                ))
            }
            Err(_) => {
                // 超时 — 显式 kill + wait 确保进程完全终止并被收割
                let _ = child.kill().await;
                let _ = child.wait().await;
                let duration = start.elapsed();
                Ok(ExecutionResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("Process timed out after {}s", timeout.as_secs()),
                    duration,
                    sandbox_type: "local".to_string(),
                    timed_out: true,
                })
            }
        }
    }
}

/// 从管道句柄中读取全部输出，并截断超过 max_bytes 的部分。
async fn read_pipe_output<R: AsyncReadExt + Unpin>(
    mut pipe: Option<R>,
    max_bytes: usize,
) -> String {
    let Some(ref mut reader) = pipe else {
        return String::new();
    };
    let cap = max_bytes.min(4096);
    let mut buf = Vec::with_capacity(cap);
    if reader.read_to_end(&mut buf).await.is_err() {
        return String::new();
    }
    let mut s = String::from_utf8_lossy(&buf).to_string();
    if s.len() > max_bytes {
        s.truncate(max_bytes);
        s.push_str("\n... [output truncated]");
    }
    s
}

impl SandboxExecutor for LocalSandbox {
    fn name(&self) -> &str {
        "local"
    }

    fn isolation_level(&self) -> IsolationLevel {
        if self.effective_os_sandbox_enabled() {
            IsolationLevel::OsSandbox
        } else {
            IsolationLevel::Process
        }
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(async {
            if self.effective_os_sandbox_enabled() {
                Command::new("sandbox-exec")
                    .arg("-n")
                    .arg("default")
                    .arg("true")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false)
            } else {
                true
            }
        })
    }

    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move {
            let timeout = command.timeout;
            let cmd = match &command.kind {
                CommandKind::Shell(cmd) => self.build_shell_command(cmd, &command),
                CommandKind::Program { program, args } => {
                    self.build_program_command(program, args, &command)
                }
                CommandKind::Code { language, code } => self
                    .build_code_command(language, code, &command)
                    .map_err(echo_core::error::ReactError::Sandbox)?,
            };
            self.run_command(cmd, timeout, command.stdin.as_deref())
                .await
        })
    }

    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move {
            // 将 ResourceLimits 合并到命令超时中
            let timeout = if let Some(cpu_secs) = limits.cpu_time_secs {
                std::time::Duration::from_secs(cpu_secs)
            } else {
                command.timeout
            };

            // 如果 limits 指定了 max_output_bytes，则覆盖 config 中的默认值
            let config = if let Some(max_bytes) = limits.max_output_bytes {
                LocalConfig {
                    max_output_bytes: max_bytes as usize,
                    ..self.config.clone()
                }
            } else {
                self.config.clone()
            };

            let cmd_with_timeout = SandboxCommand { timeout, ..command };
            // 使用更新后的配置执行
            let sandbox = LocalSandbox::new(config);
            sandbox.execute(cmd_with_timeout).await
        })
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_sandbox_echo() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        let cmd = SandboxCommand::shell("echo hello");
        let result = sandbox.execute(cmd).await.unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.sandbox_type, "local");
    }

    #[tokio::test]
    async fn test_local_sandbox_exit_code() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        let cmd = SandboxCommand::shell("exit 42");
        let result = sandbox.execute(cmd).await.unwrap();
        assert_eq!(result.exit_code, 42);
        assert!(!result.success());
    }

    #[tokio::test]
    async fn test_local_sandbox_timeout() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        let cmd =
            SandboxCommand::shell("sleep 60").with_timeout(std::time::Duration::from_millis(100));
        let result = sandbox.execute(cmd).await.unwrap();
        assert!(result.timed_out);
        assert!(!result.success());
    }

    #[tokio::test]
    async fn test_local_sandbox_env() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        let cmd = SandboxCommand::shell("echo $MY_VAR").with_env("MY_VAR", "test_value");
        let result = sandbox.execute(cmd).await.unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "test_value");
    }

    #[tokio::test]
    async fn test_local_sandbox_program() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        let cmd = SandboxCommand::program("echo", vec!["hello".into(), "world".into()]);
        let result = sandbox.execute(cmd).await.unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "hello world");
    }

    #[tokio::test]
    async fn test_local_sandbox_stdin() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        let cmd = SandboxCommand::shell("read value; echo $value").with_stdin("from-stdin\n");
        let result = sandbox.execute(cmd).await.unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "from-stdin");
    }

    #[tokio::test]
    async fn test_local_sandbox_is_available() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        assert!(sandbox.is_available().await);
    }

    #[tokio::test]
    async fn test_timeout_no_zombie_process() {
        // 验证超时后子进程被完全清理（无僵尸/孤儿进程残留）
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });

        // 获取当前 sleep 进程数
        let _pid = std::process::id();
        let count_before = std::process::Command::new("sh")
            .args([
                "-c",
                &format!("ps -o pid= -o comm= | grep -c '[s]leep' || echo 0"),
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);

        let cmd =
            SandboxCommand::shell("sleep 9999").with_timeout(std::time::Duration::from_millis(200));
        let result = sandbox.execute(cmd).await.unwrap();
        assert!(result.timed_out);

        // 等待一小段时间确保进程被收割
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let count_after = std::process::Command::new("sh")
            .args(["-c", "ps -o pid= -o comm= | grep -c '[s]leep' || echo 0"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0);

        // 超时后不应增加 sleep 进程数
        assert!(
            count_after <= count_before,
            "Found {} sleep processes after timeout (was {})",
            count_after,
            count_before
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_reports_process_isolation_without_os_sandbox_backend() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: true,
            ..Default::default()
        });
        assert_eq!(sandbox.isolation_level(), IsolationLevel::Process);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_seatbelt_profile_generation() {
        let config = LocalConfig {
            allow_network: false,
            allowed_read_paths: vec![PathBuf::from("/opt/data")],
            allowed_write_paths: vec![PathBuf::from("/tmp/sandbox")],
            ..Default::default()
        };
        let sandbox = LocalSandbox::new(config);
        let profile = sandbox.build_seatbelt_profile();

        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow process-exec)"));
        assert!(profile.contains("/opt/data"));
        assert!(profile.contains("/tmp/sandbox"));
        assert!(!profile.contains("(allow network*)"));
    }
}
