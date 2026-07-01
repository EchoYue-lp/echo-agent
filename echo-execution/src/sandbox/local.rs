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
            // Sprint 10b: R is a first-class language (arg-based, mirrors
            // python/ruby/perl). `Rscript -e` runs the inline expression.
            "r" => ("Rscript", "-e"),
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

        // 读取权限：validate paths to prevent Seatbelt profile injection
        for path in &self.config.allowed_read_paths {
            let path_str = path.display().to_string();
            if validate_sandbox_path(&path_str).is_err() {
                continue; // Skip invalid paths silently
            }
            let escaped = path_str.replace('"', "\\\"");
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
        }
        // 始终允许读取基本系统路径
        profile.push_str("(allow file-read* (subpath \"/usr\"))\n");
        profile.push_str("(allow file-read* (subpath \"/bin\"))\n");
        profile.push_str("(allow file-read* (subpath \"/Library\"))\n");
        profile.push_str("(allow file-read* (subpath \"/System\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/null\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/urandom\"))\n");

        // 写入权限：validate paths to prevent Seatbelt profile injection
        for path in &self.config.allowed_write_paths {
            let path_str = path.display().to_string();
            if validate_sandbox_path(&path_str).is_err() {
                continue; // Skip invalid paths silently
            }
            let escaped = path_str.replace('"', "\\\"");
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
        configure_command_process_group(&mut command);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

        let start = Instant::now();

        let mut child = command.spawn().map_err(|e| {
            echo_core::error::ReactError::Sandbox(Box::new(SandboxError::StartFailed(format!(
                "Failed to spawn process: {e}"
            ))))
        })?;

        // 写入 stdin 并处理写入失败时的进程清理
        if let Some(input) = stdin
            && let Some(mut child_stdin) = child.stdin.take()
        {
            if let Err(e) = child_stdin.write_all(input.as_bytes()).await {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(echo_core::error::ReactError::Sandbox(Box::new(
                    SandboxError::IoError(format!("Failed to write stdin: {e}")),
                )));
            }
            // 关闭 stdin 发送 EOF
            drop(child_stdin);
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
                cleanup_child_process(&mut child).await;
                Err(echo_core::error::ReactError::Sandbox(Box::new(
                    SandboxError::IoError(format!("Process wait error: {e}")),
                )))
            }
            Err(_) => {
                // 超时 — 显式 kill + wait 确保进程完全终止并被收割
                cleanup_child_process(&mut child).await;
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

/// Validate a path for safe inclusion in a macOS Seatbelt sandbox profile.
///
/// Rejects paths containing characters that could be used for Seatbelt profile
/// injection (parentheses for S-expression manipulation, newlines for rule injection,
/// null bytes for string truncation, semicolons and hash for comment injection).
/// Only allows: alphanumeric characters, `/`, `.`, `-`, `_`, and spaces.
fn validate_sandbox_path(path: &str) -> std::result::Result<(), String> {
    for c in path.chars() {
        if c.is_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | ' ') {
            continue;
        }
        return Err(format!(
            "Path '{}' contains disallowed character '{}' — only alphanumeric, /, ., -, _, and spaces are allowed in sandbox paths",
            path, c
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn configure_command_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_command_process_group(_command: &mut Command) {}

async fn cleanup_child_process(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        if let Err(e) = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .status()
        {
            tracing::warn!("Failed to send SIGKILL to process group {pid}: {e}");
        }
    }

    if let Err(e) = child.kill().await {
        tracing::warn!("Failed to kill child process: {e}");
    }
    let _ = child.wait().await;
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
        let safe_end = s.floor_char_boundary(max_bytes);
        s.truncate(safe_end);
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
                    .map_err(|e| echo_core::error::ReactError::Sandbox(Box::new(e)))?,
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

            // 将 ResourceLimits 的 network / path 约束翻译进 LocalConfig,
            // 这样 Seatbelt profile 才会真正强制它们。之前只翻译了
            // cpu_time / max_output_bytes,导致 policy 声明的 network /
            // writable_paths 形同虚设。
            let config = merge_limits_into_config(self.config.clone(), &limits);

            let cmd_with_timeout = SandboxCommand { timeout, ..command };
            // 使用更新后的配置执行
            let sandbox = LocalSandbox::new(config);
            sandbox.execute(cmd_with_timeout).await
        })
    }
}

/// 将 [`ResourceLimits`] 合并进 [`LocalConfig`],使 network / path 约束
/// 能流到 Seatbelt profile 真正生效。
///
/// 路径采用"合并"而非"替换":config 默认带 /usr /bin 读路径,
/// 替换会破坏基本执行;policy 声明的路径作为增量追加(去重保序)。
/// network / max_output_bytes 采用"覆盖":policy 显式声明则覆盖 config 默认。
fn merge_limits_into_config(mut config: LocalConfig, limits: &ResourceLimits) -> LocalConfig {
    // network: policy 显式声明则覆盖(EKO 默认 local trust = true)
    config.allow_network = limits.network;

    // writable_paths: 合并(去重保序)
    for path in &limits.writable_paths {
        if !config.allowed_write_paths.contains(path) {
            config.allowed_write_paths.push(path.clone());
        }
    }
    // read_only_paths: 同样合并
    for path in &limits.read_only_paths {
        if !config.allowed_read_paths.contains(path) {
            config.allowed_read_paths.push(path.clone());
        }
    }

    // max_output_bytes: policy 覆盖
    if let Some(max_bytes) = limits.max_output_bytes {
        config.max_output_bytes = max_bytes as usize;
    }

    config
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

    fn _count_processes_matching(pattern: &str) -> i32 {
        std::process::Command::new("sh")
            .args([
                "-c",
                &format!("ps -o args= | grep -F -- {pattern:?} | grep -v grep | wc -l"),
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(0)
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

    /// 验证 execute_with_limits 路径下,ResourceLimits 的 network /
    /// writable_paths 真正流进 LocalConfig(进而 Seatbelt profile 生效)。
    /// 这是 P2-3 修复的核心:之前 limits.network/writable_paths 被丢弃。
    #[test]
    fn merge_limits_translates_network_and_paths() {
        let base = LocalConfig::default();
        // 默认 allow_network=false
        assert!(!base.allow_network);

        let limits = ResourceLimits {
            network: true,
            writable_paths: vec![PathBuf::from("/tmp/skill-work")],
            read_only_paths: vec![PathBuf::from("/opt/assets")],
            cpu_time_secs: Some(60),
            max_output_bytes: Some(2 * 1024 * 1024),
            ..Default::default()
        };
        let merged = merge_limits_into_config(base, &limits);

        // network 翻译:base 的 false → limits 的 true
        assert!(
            merged.allow_network,
            "network must flow from limits to config"
        );

        // writable_paths 合并:policy 声明的路径出现
        assert!(
            merged
                .allowed_write_paths
                .contains(&PathBuf::from("/tmp/skill-work")),
            "writable_paths must be merged into config"
        );

        // read_only_paths 合并:policy 声明的路径出现,且默认 /usr /bin 保留
        assert!(
            merged
                .allowed_read_paths
                .contains(&PathBuf::from("/opt/assets")),
            "read_only_paths must be merged"
        );
        assert!(
            merged.allowed_read_paths.contains(&PathBuf::from("/usr")),
            "default read paths (/usr) must be preserved (merge, not replace)"
        );

        // max_output_bytes 覆盖
        assert_eq!(merged.max_output_bytes, 2 * 1024 * 1024);

        // 最终:用 merged config 构建 profile,network 放行应出现
        let sandbox = LocalSandbox::new(merged);
        let profile = sandbox.build_seatbelt_profile();
        assert!(
            profile.contains("(allow network*)"),
            "network=true from ResourceLimits must reach Seatbelt profile"
        );
        assert!(
            profile.contains("/tmp/skill-work"),
            "writable path from ResourceLimits must reach Seatbelt profile"
        );
    }

    /// 验证 network=false 的 policy 真的让 Seatbelt 不放行网络。
    #[test]
    fn merge_limits_network_false_keeps_seatbelt_strict() {
        let base = LocalConfig {
            allow_network: true, // 故意把 base 设为 true
            ..Default::default()
        };
        let limits = ResourceLimits {
            network: false,
            ..Default::default()
        };
        let merged = merge_limits_into_config(base, &limits);
        assert!(
            !merged.allow_network,
            "network=false from limits must override base's true"
        );
        let profile = LocalSandbox::new(merged).build_seatbelt_profile();
        assert!(
            !profile.contains("(allow network*)"),
            "network=false must keep Seatbelt strict"
        );
    }

    /// Sprint 10b: R must be a first-class language in the `Code` backend.
    ///
    /// Before the patch, R fell through to the `_` arm and returned
    /// `SandboxError::Unavailable("Unsupported language: r")`. After the patch
    /// it must map to `("Rscript", "-e")` like python/ruby/perl.
    ///
    /// We can't assume Rscript is installed in CI, so we only assert the error
    /// is NOT "Unsupported language" — i.e. the match arm fired and proceeded
    /// to interpreter resolution (a missing interpreter surfaces as a spawn
    /// error, not as "Unsupported language").
    #[tokio::test]
    async fn code_backend_supports_r_language_mapping() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        let cmd = SandboxCommand::code("r", "print(1+1)");
        match sandbox.execute(cmd).await {
            Ok(_) => { /* Rscript present + ran */ }
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(
                    !msg.contains("Unsupported language"),
                    "R should not hit the Unsupported-language arm. Got: {msg}"
                );
            }
        }
    }
}
