//! 本地沙箱执行器
//!
//! 使用本地进程执行命令，并在支持的平台启用操作系统原生隔离：
//! - **macOS**: `sandbox-exec` (Seatbelt)
//! - **Linux**: `bubblewrap` (user/mount/pid/network namespaces)
//! - **Windows**: minimal process backend (`cmd /C` + timeout/output limits)
//! - **其他**: 仅进程隔离（超时 + 输出截断）
//! - 支持通过 `SandboxCommand::stdin` 传入标准输入
//!
//! 这是最轻量的沙箱层，适合受信代码和只读操作。

use super::{
    CommandKind, ExecutionResult, IsolationLevel, ResourceLimits, SandboxCommand, SandboxExecutor,
    SandboxOutputChannel, SandboxStreamEvent,
};
use echo_core::error::Result;
use echo_core::error::SandboxError;
use futures::future::BoxFuture;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;

const STREAM_CHUNK_BYTES: usize = 16 * 1024;
const STREAM_CHANNEL_CAPACITY: usize = 32;

/// 本地沙箱配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    /// 是否启用本地沙箱后端（sandbox-exec / bubblewrap / Windows process backend）
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
    /// 最大内存大小（字节）。Linux/WSL2 限制虚拟地址空间；macOS 限制 data segment。
    pub max_memory_bytes: Option<u64>,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            enable_os_sandbox: cfg!(any(
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            )),
            allowed_read_paths: vec![PathBuf::from("/usr"), PathBuf::from("/bin")],
            allowed_write_paths: vec![],
            allow_network: false,
            default_timeout_secs: 30,
            max_output_bytes: 1024 * 1024, // 1 MB
            max_memory_bytes: None,
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
        self.config.enable_os_sandbox
            && cfg!(any(
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            ))
    }

    fn sandbox_type(&self) -> &'static str {
        if !self.effective_os_sandbox_enabled() {
            return "local";
        }
        if cfg!(target_os = "macos") {
            "local-seatbelt"
        } else if cfg!(target_os = "linux") {
            "local-bubblewrap"
        } else if cfg!(target_os = "windows") {
            "local-windows-process"
        } else {
            "local"
        }
    }

    fn effective_working_dir(&self, sandbox_cmd: &SandboxCommand) -> Option<PathBuf> {
        sandbox_cmd
            .working_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
    }

    /// 构建 shell 命令
    fn build_shell_command(&self, cmd: &str, sandbox_cmd: &SandboxCommand) -> Command {
        let mut command = if self.effective_os_sandbox_enabled() && cfg!(target_os = "macos") {
            // macOS: 使用 sandbox-exec
            let profile = self.build_seatbelt_profile_for(sandbox_cmd);
            let mut c = Command::new("sandbox-exec");
            c.arg("-p").arg(profile).arg("sh").arg("-c").arg(cmd);
            c
        } else if self.effective_os_sandbox_enabled() && cfg!(target_os = "linux") {
            self.build_bubblewrap_command("sh", &["-c".to_string(), cmd.to_string()], sandbox_cmd)
        } else if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(cmd);
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
        let mut command = if self.effective_os_sandbox_enabled() && cfg!(target_os = "macos") {
            let profile = self.build_seatbelt_profile_for(sandbox_cmd);
            let mut c = Command::new("sandbox-exec");
            c.arg("-p").arg(profile).arg(program);
            c.args(args);
            c
        } else if self.effective_os_sandbox_enabled() && cfg!(target_os = "linux") {
            self.build_bubblewrap_command(program, args, sandbox_cmd)
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
        let (interpreter, invocation_args) = code_invocation(language, code)?;

        let mut command = if self.effective_os_sandbox_enabled() && cfg!(target_os = "macos") {
            let profile = self.build_seatbelt_profile_for(sandbox_cmd);
            let mut c = Command::new("sandbox-exec");
            c.arg("-p").arg(profile).arg(interpreter);
            c.args(&invocation_args);
            c
        } else if self.effective_os_sandbox_enabled() && cfg!(target_os = "linux") {
            self.build_bubblewrap_command(interpreter, &invocation_args, sandbox_cmd)
        } else {
            let mut c = Command::new(interpreter);
            c.args(&invocation_args);
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
    #[cfg(test)]
    fn build_seatbelt_profile(&self) -> String {
        self.build_seatbelt_profile_with_working_dir(None)
    }

    fn build_seatbelt_profile_for(&self, sandbox_cmd: &SandboxCommand) -> String {
        self.build_seatbelt_profile_with_working_dir(
            self.effective_working_dir(sandbox_cmd).as_ref(),
        )
    }

    fn build_seatbelt_profile_with_working_dir(&self, working_dir: Option<&PathBuf>) -> String {
        let mut profile = String::from("(version 1)\n(deny default)\n");

        self.append_seatbelt_base_policy(&mut profile);
        self.append_seatbelt_platform_read_defaults(&mut profile);
        self.append_seatbelt_filesystem_policy(&mut profile, working_dir);

        // 网络
        if self.config.allow_network {
            profile.push_str("(allow network*)\n");
        }

        profile
    }

    fn append_seatbelt_base_policy(&self, profile: &mut String) {
        // Shells and language runtimes need broader process metadata access than
        // fork+exec. File and network access remain constrained by later layers.
        profile.push_str("(allow process*)\n");
        profile.push_str("(allow sysctl-read)\n");
        profile.push_str("(allow mach-lookup)\n");

        // Python multiprocessing, OpenMP, and native libraries commonly touch
        // these IPC and pseudo-terminal primitives during startup.
        profile.push_str("(allow ipc-posix-sem)\n");
        profile.push_str("(allow ipc-posix-shm)\n");
        profile.push_str("(allow file-read* (literal \"/dev/null\"))\n");
        profile.push_str("(allow file-write* (literal \"/dev/null\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/urandom\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/random\"))\n");
        profile.push_str("(allow file-read* (literal \"/dev/ptmx\"))\n");
        profile.push_str("(allow file-write* (literal \"/dev/ptmx\"))\n");
    }

    fn append_seatbelt_platform_read_defaults(&self, profile: &mut String) {
        // Claude Code's sandbox defaults to broad reads so common compilers,
        // interpreters, and package managers can discover system dependencies.
        // Sensitive paths are denied again in the filesystem layer.
        profile.push_str("(allow file-read*)\n");
    }

    fn append_seatbelt_filesystem_policy(
        &self,
        profile: &mut String,
        working_dir: Option<&PathBuf>,
    ) {
        for path in &self.config.allowed_read_paths {
            append_seatbelt_subpath_rule(profile, "file-read*", path);
        }
        for path in &self.config.allowed_write_paths {
            append_seatbelt_subpath_rule(profile, "file-write*", path);
        }
        if let Some(path) = working_dir {
            append_seatbelt_subpath_rule(profile, "file-read*", path);
            append_seatbelt_subpath_rule(profile, "file-write*", path);
        }

        // Match Claude Code's default write boundary: project working directory
        // plus the session temp directory, not arbitrary parent/home paths.
        let session_temp_dir = std::env::temp_dir();
        append_seatbelt_subpath_rule(profile, "file-write*", &session_temp_dir);

        for path in credential_deny_defaults() {
            append_seatbelt_subpath_deny(profile, path);
        }
    }

    #[cfg(target_os = "linux")]
    fn build_bubblewrap_command(
        &self,
        program: &str,
        args: &[String],
        sandbox_cmd: &SandboxCommand,
    ) -> Command {
        let mut command = Command::new("bwrap");
        command
            .arg("--unshare-user-try")
            .arg("--unshare-ipc")
            .arg("--unshare-pid")
            .arg("--unshare-uts")
            .arg("--unshare-cgroup-try")
            .arg("--die-with-parent")
            .arg("--new-session")
            .arg("--clearenv")
            .arg("--ro-bind")
            .arg("/")
            .arg("/")
            .arg("--dev")
            .arg("/dev")
            .arg("--proc")
            .arg("/proc")
            .arg("--tmpfs")
            .arg("/tmp")
            .arg("--dir")
            .arg("/run");

        if !self.config.allow_network {
            command.arg("--unshare-net");
        }

        for (key, value) in default_sandbox_env(&sandbox_cmd.env) {
            command.arg("--setenv").arg(key).arg(value);
        }

        for path in &self.config.allowed_read_paths {
            command.arg("--ro-bind-try").arg(path).arg(path);
        }
        for path in &self.config.allowed_write_paths {
            command.arg("--bind-try").arg(path).arg(path);
        }

        let session_temp_dir = std::env::temp_dir();
        if !is_generic_temp_dir(&session_temp_dir) {
            command
                .arg("--bind-try")
                .arg(&session_temp_dir)
                .arg(&session_temp_dir);
        }

        if let Some(working_dir) = self.effective_working_dir(sandbox_cmd) {
            command
                .arg("--bind-try")
                .arg(&working_dir)
                .arg(&working_dir)
                .arg("--chdir")
                .arg(working_dir);
        }

        for path in credential_deny_defaults() {
            append_bubblewrap_credential_deny(&mut command, &path);
        }

        command.arg("--").arg(program).args(args);
        command
    }

    #[cfg(not(target_os = "linux"))]
    fn build_bubblewrap_command(
        &self,
        program: &str,
        args: &[String],
        _sandbox_cmd: &SandboxCommand,
    ) -> Command {
        let mut command = Command::new(program);
        command.args(args);
        command
    }

    /// 执行命令并收集输出
    ///
    /// 超时时显式 kill + wait 清理子进程，避免僵尸/孤儿进程残留。
    async fn run_command(
        &self,
        command: Command,
        timeout: std::time::Duration,
        stdin: Option<&str>,
        sandbox_type: &'static str,
    ) -> Result<ExecutionResult> {
        let mut stream = self
            .run_command_stream(command, timeout, stdin, sandbox_type)
            .await?;
        while let Some(event) = stream.next().await {
            if let SandboxStreamEvent::Complete(result) = event {
                return Ok(result);
            }
        }
        Err(echo_core::error::ReactError::Sandbox(Box::new(
            SandboxError::IoError(
                "Local sandbox stream ended without a completion event".to_string(),
            ),
        )))
    }

    async fn run_command_stream(
        &self,
        mut command: Command,
        timeout: std::time::Duration,
        stdin: Option<&str>,
        sandbox_type: &'static str,
    ) -> Result<Pin<Box<dyn Stream<Item = SandboxStreamEvent> + Send>>> {
        if stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        configure_command_process(&mut command, self.config.max_memory_bytes);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

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

        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let max_output_bytes = self.config.max_output_bytes;
        let (tx, rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            run_streaming_child(
                &mut child,
                stdout_pipe,
                stderr_pipe,
                tx,
                timeout,
                max_output_bytes,
                sandbox_type,
            )
            .await;
        });
        Ok(Box::pin(futures::stream::unfold(
            rx,
            |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) },
        )))
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

fn append_seatbelt_subpath_rule(profile: &mut String, operation: &str, path: &PathBuf) {
    let path = normalize_seatbelt_profile_path(path);
    let path_str = path.display().to_string();
    if validate_sandbox_path(&path_str).is_err() {
        return;
    }
    let escaped = path_str.replace('"', "\\\"");
    profile.push_str(&format!("(allow {operation} (subpath \"{escaped}\"))\n"));
}

fn append_seatbelt_subpath_deny(profile: &mut String, path: PathBuf) {
    let path = normalize_seatbelt_profile_path(&path);
    let path_str = path.display().to_string();
    if validate_sandbox_path(&path_str).is_err() {
        return;
    }
    let escaped = path_str.replace('"', "\\\"");
    profile.push_str(&format!("(deny file-read* (subpath \"{escaped}\"))\n"));
    profile.push_str(&format!("(deny file-write* (subpath \"{escaped}\"))\n"));
}

fn normalize_seatbelt_profile_path(path: &PathBuf) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())
}

fn credential_deny_defaults() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    vec![
        home.join(".ssh"),
        home.join(".aws"),
        home.join(".azure"),
        home.join(".config/gcloud"),
        home.join(".docker"),
        home.join(".kube"),
        home.join(".gnupg"),
        home.join(".netrc"),
    ]
}

#[cfg(target_os = "linux")]
fn append_bubblewrap_credential_deny(command: &mut Command, path: &PathBuf) {
    if path.is_dir() {
        command.arg("--tmpfs").arg(path);
    } else if path.is_file() {
        command.arg("--ro-bind-try").arg("/dev/null").arg(path);
    }
}

#[cfg(target_os = "linux")]
fn is_generic_temp_dir(path: &PathBuf) -> bool {
    path == &PathBuf::from("/tmp") || path == &PathBuf::from("/private/tmp")
}

fn code_invocation(
    language: &str,
    code: &str,
) -> std::result::Result<(&'static str, Vec<String>), SandboxError> {
    let code_arg = code.to_string();
    let invocation = if cfg!(target_os = "windows") {
        match language {
            "python" | "python3" => ("python", vec!["-c".to_string(), code_arg]),
            "node" | "javascript" | "js" => ("node", vec!["-e".to_string(), code_arg]),
            "ruby" => ("ruby", vec!["-e".to_string(), code_arg]),
            "r" => ("Rscript", vec!["-e".to_string(), code_arg]),
            "perl" => ("perl", vec!["-e".to_string(), code_arg]),
            "lua" => ("lua", vec!["-e".to_string(), code_arg]),
            "php" => ("php", vec!["-r".to_string(), code_arg]),
            "bash" | "sh" => ("cmd", vec!["/C".to_string(), code_arg]),
            _ => {
                return Err(SandboxError::Unavailable(format!(
                    "Unsupported language: {language}"
                )));
            }
        }
    } else {
        match language {
            "python" | "python3" => ("python3", vec!["-c".to_string(), code_arg]),
            "node" | "javascript" | "js" => ("node", vec!["-e".to_string(), code_arg]),
            "ruby" => ("ruby", vec!["-e".to_string(), code_arg]),
            "r" => ("Rscript", vec!["-e".to_string(), code_arg]),
            "perl" => ("perl", vec!["-e".to_string(), code_arg]),
            "lua" => ("lua", vec!["-e".to_string(), code_arg]),
            "php" => ("php", vec!["-r".to_string(), code_arg]),
            "bash" | "sh" => ("sh", vec!["-c".to_string(), code_arg]),
            _ => {
                return Err(SandboxError::Unavailable(format!(
                    "Unsupported language: {language}"
                )));
            }
        }
    };
    Ok(invocation)
}

// Local sandbox 的 `max_memory_bytes` 字段在 **所有 local 后端(macOS / Linux / Windows)
// 上都不被强制**(对齐 Claude Code / Codex —— 它们在沙箱层都不设内存上限)。
//
// 本地场景下为何保留字段但不强制(AGENTS.md 要求注明):
//
// - **macOS**:XNU kernel 对任何 < RLIM_INFINITY 的 `RLIMIT_DATA` 都返回 EINVAL(已实测
//   验证,见 https://github.com/hacksider/Deep-Live-Cam/issues/1848);`RLIMIT_AS` 又会
//   在 dyld/framework 加载阶段拒绝无害的内存映射。macOS 没有可靠的用户态 per-process
//   内存上限机制(jetsam 系统级不可控、无 cgroup)。
// - **Linux**:`RLIMIT_AS` 限制整个虚拟地址空间,glibc `ld.so` 用 mmap 加载共享库,
//   设低了会让进程还没跑到用户代码就因 ENOMEM 崩掉(import 重库 / JIT 运行时尤其严重,
//   见 https://stackoverflow.com/q/39755928)。设大了又等于没限。cgroup 才是 Linux 上
//   可靠的内存上限机制,而 cgroup 走 Docker/K8s 路径(`docker.rs`/`k8s.rs`),不经过这里。
// - **Windows**:local sandbox 本就是 `cmd /C` + 超时/输出截断的进程级后端,无 rlimit 概念。
//
// Claude Code 和 Codex 在所有平台的沙箱层都不设内存上限,真正的内存上限交给容器层
// (cgroup)。EKO 沿用同一设计:`max_memory_bytes` 字段保留是给 Docker/K8s 路径用的,
// local 后端(macOS/Linux/Windows)静默忽略它。Linux local 沙箱仍可用 bwrap 做
// namespace 隔离(见 `build_bubblewrap_command`),只是不限内存。
#[cfg(unix)]
fn configure_command_process(command: &mut Command, _max_memory_bytes: Option<u64>) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_command_process(_command: &mut Command, _max_memory_bytes: Option<u64>) {}

async fn cleanup_child_process(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id()
        && let Err(e) = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
    {
        tracing::warn!("Failed to send SIGKILL to process group {pid}: {e}");
    }

    if let Err(e) = child.kill().await {
        tracing::warn!("Failed to kill child process: {e}");
    }
    let _ = child.wait().await;
}

async fn run_streaming_child(
    child: &mut tokio::process::Child,
    mut stdout: Option<tokio::process::ChildStdout>,
    mut stderr: Option<tokio::process::ChildStderr>,
    tx: mpsc::Sender<SandboxStreamEvent>,
    timeout: std::time::Duration,
    max_output_bytes: usize,
    sandbox_type: &'static str,
) {
    let start = Instant::now();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut stdout_buffer = [0_u8; STREAM_CHUNK_BYTES];
    let mut stderr_buffer = [0_u8; STREAM_CHUNK_BYTES];
    let mut stdout_decoder = IncrementalUtf8Decoder::default();
    let mut stderr_decoder = IncrementalUtf8Decoder::default();
    let mut retained_stdout = RetainedPipeOutput::new();
    let mut retained_stderr = RetainedPipeOutput::new();
    let mut status = None;

    loop {
        if stdout.is_none() && stderr.is_none() && status.is_some() {
            break;
        }
        tokio::select! {
            _ = tx.closed() => {
                cleanup_child_process(child).await;
                return;
            }
            _ = &mut deadline => {
                cleanup_child_process(child).await;
                let result = local_execution_result(
                    -1,
                    retained_stdout,
                    retained_stderr,
                    start.elapsed(),
                    sandbox_type,
                    true,
                    Some(format!("Process timed out after {}s", timeout.as_secs())),
                );
                let _ = tx.send(SandboxStreamEvent::Complete(result)).await;
                return;
            }
            read = async {
                match stdout.as_mut() {
                    Some(pipe) => pipe.read(&mut stdout_buffer).await,
                    None => Ok(0),
                }
            }, if stdout.is_some() => {
                match read {
                    Ok(0) => {
                        stdout = None;
                        if let Some(chunk) = stdout_decoder.finish()
                            && send_sandbox_output(&tx, SandboxOutputChannel::Stdout, chunk).await.is_err()
                        {
                            cleanup_child_process(child).await;
                            return;
                        }
                    }
                    Ok(count) => {
                        let bytes = stdout_buffer.get(..count).unwrap_or_default();
                        let retained = retained_stdout
                            .bytes
                            .len()
                            .saturating_add(retained_stderr.bytes.len());
                        retained_stdout.push(bytes, max_output_bytes.saturating_sub(retained));
                        for chunk in stdout_decoder.push(bytes) {
                            if send_sandbox_output(&tx, SandboxOutputChannel::Stdout, chunk).await.is_err() {
                                cleanup_child_process(child).await;
                                return;
                            }
                        }
                    }
                    Err(_) => stdout = None,
                }
            }
            read = async {
                match stderr.as_mut() {
                    Some(pipe) => pipe.read(&mut stderr_buffer).await,
                    None => Ok(0),
                }
            }, if stderr.is_some() => {
                match read {
                    Ok(0) => {
                        stderr = None;
                        if let Some(chunk) = stderr_decoder.finish()
                            && send_sandbox_output(&tx, SandboxOutputChannel::Stderr, chunk).await.is_err()
                        {
                            cleanup_child_process(child).await;
                            return;
                        }
                    }
                    Ok(count) => {
                        let bytes = stderr_buffer.get(..count).unwrap_or_default();
                        let retained = retained_stdout
                            .bytes
                            .len()
                            .saturating_add(retained_stderr.bytes.len());
                        retained_stderr.push(bytes, max_output_bytes.saturating_sub(retained));
                        for chunk in stderr_decoder.push(bytes) {
                            if send_sandbox_output(&tx, SandboxOutputChannel::Stderr, chunk).await.is_err() {
                                cleanup_child_process(child).await;
                                return;
                            }
                        }
                    }
                    Err(_) => stderr = None,
                }
            }
            waited = child.wait(), if status.is_none() => {
                match waited {
                    Ok(exit_status) => status = Some(exit_status),
                    Err(error) => {
                        cleanup_child_process(child).await;
                        let result = local_execution_result(
                            -1,
                            retained_stdout,
                            retained_stderr,
                            start.elapsed(),
                            sandbox_type,
                            false,
                            Some(format!("Process wait error: {error}")),
                        );
                        let _ = tx.send(SandboxStreamEvent::Complete(result)).await;
                        return;
                    }
                }
            }
        }
    }

    let exit_code = status
        .as_ref()
        .and_then(std::process::ExitStatus::code)
        .unwrap_or(-1);
    let termination_message = status
        .as_ref()
        .filter(|status| status.code().is_none())
        .map(platform_termination_message);
    let result = local_execution_result(
        exit_code,
        retained_stdout,
        retained_stderr,
        start.elapsed(),
        sandbox_type,
        false,
        termination_message,
    );
    let _ = tx.send(SandboxStreamEvent::Complete(result)).await;
}

async fn send_sandbox_output(
    tx: &mpsc::Sender<SandboxStreamEvent>,
    channel: SandboxOutputChannel,
    text: String,
) -> std::result::Result<(), ()> {
    for chunk in split_stream_chunks(text) {
        tx.send(SandboxStreamEvent::Output { channel, chunk })
            .await
            .map_err(|_| ())?;
    }
    Ok(())
}

fn split_stream_chunks(text: String) -> Vec<String> {
    if text.len() <= STREAM_CHUNK_BYTES {
        return vec![text];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if current.len().saturating_add(character.len_utf8()) > STREAM_CHUNK_BYTES
            && !current.is_empty()
        {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

struct RetainedPipeOutput {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
}

impl RetainedPipeOutput {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            total_bytes: 0,
            truncated: false,
        }
    }

    fn push(&mut self, bytes: &[u8], remaining_capacity: usize) {
        self.total_bytes = self
            .total_bytes
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let retain = remaining_capacity.min(bytes.len());
        if let Some(prefix) = bytes.get(..retain) {
            self.bytes.extend_from_slice(prefix);
        }
        self.truncated |= retain < bytes.len();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).to_string()
    }
}

#[derive(Default)]
struct IncrementalUtf8Decoder {
    pending: Vec<u8>,
}

impl IncrementalUtf8Decoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    output.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_len = error.valid_up_to();
                    if let Some(valid_bytes) = self.pending.get(..valid_len)
                        && let Ok(valid) = std::str::from_utf8(valid_bytes)
                    {
                        output.push_str(valid);
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            output.push('\u{FFFD}');
                            let consumed = valid_len.saturating_add(invalid_len);
                            self.pending.drain(..consumed.min(self.pending.len()));
                        }
                        None => {
                            self.pending.drain(..valid_len.min(self.pending.len()));
                            break;
                        }
                    }
                }
            }
        }
        if output.is_empty() {
            Vec::new()
        } else {
            split_stream_chunks(output)
        }
    }

    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let output = String::from_utf8_lossy(&self.pending).to_string();
        self.pending.clear();
        Some(output)
    }
}

fn local_execution_result(
    exit_code: i32,
    stdout: RetainedPipeOutput,
    stderr: RetainedPipeOutput,
    duration: std::time::Duration,
    sandbox_type: &str,
    timed_out: bool,
    fallback_stderr: Option<String>,
) -> ExecutionResult {
    let mut stderr_text = stderr.text();
    if stderr_text.is_empty()
        && let Some(message) = fallback_stderr
    {
        stderr_text = message;
    }
    ExecutionResult {
        exit_code,
        stdout: stdout.text(),
        stderr: stderr_text,
        duration,
        sandbox_type: sandbox_type.to_string(),
        timed_out,
        cancelled: false,
        output_truncated: stdout.truncated || stderr.truncated,
        stdout_bytes: stdout.total_bytes,
        stderr_bytes: stderr.total_bytes,
    }
}

#[cfg(unix)]
fn platform_termination_message(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    match status.signal() {
        Some(signal) => format!("Process terminated by signal {signal}"),
        None => "Process terminated without an exit code".to_string(),
    }
}

#[cfg(not(unix))]
fn platform_termination_message(_status: &std::process::ExitStatus) -> String {
    "Process terminated without an exit code".to_string()
}

#[cfg(target_os = "linux")]
fn default_sandbox_env(extra: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "PATH".to_string(),
            "/usr/local/bin:/usr/bin:/bin".to_string(),
        ),
        ("HOME".to_string(), "/tmp".to_string()),
        ("TMPDIR".to_string(), "/tmp".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
    ];
    for (key, value) in extra {
        env.push((key.clone(), value.clone()));
    }
    env
}

impl SandboxExecutor for LocalSandbox {
    fn name(&self) -> &str {
        "local"
    }

    fn isolation_level(&self) -> IsolationLevel {
        if self.effective_os_sandbox_enabled()
            && cfg!(any(target_os = "macos", target_os = "linux"))
        {
            IsolationLevel::OsSandbox
        } else {
            IsolationLevel::Process
        }
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(async {
            if self.effective_os_sandbox_enabled() && cfg!(target_os = "macos") {
                Command::new("sandbox-exec")
                    .arg("-p")
                    .arg("(version 1)\n(allow default)")
                    .arg("true")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false)
            } else if self.effective_os_sandbox_enabled() && cfg!(target_os = "linux") {
                Command::new("bwrap")
                    .arg("--version")
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
            self.run_command(cmd, timeout, command.stdin.as_deref(), self.sandbox_type())
                .await
        })
    }

    fn execute_stream<'a>(
        &'a self,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = SandboxStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            let timeout = command.timeout;
            let cmd = match &command.kind {
                CommandKind::Shell(cmd) => self.build_shell_command(cmd, &command),
                CommandKind::Program { program, args } => {
                    self.build_program_command(program, args, &command)
                }
                CommandKind::Code { language, code } => self
                    .build_code_command(language, code, &command)
                    .map_err(|error| echo_core::error::ReactError::Sandbox(Box::new(error)))?,
            };
            self.run_command_stream(cmd, timeout, command.stdin.as_deref(), self.sandbox_type())
                .await
        })
    }

    fn supports_streaming(&self) -> bool {
        true
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
/// network / max_output_bytes / memory_bytes 采用"覆盖":policy 显式声明则覆盖 config 默认。
fn merge_limits_into_config(mut config: LocalConfig, limits: &ResourceLimits) -> LocalConfig {
    // network: policy 显式声明则覆盖 base config
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
    config.max_memory_bytes = limits.memory_bytes;

    config
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::CancellationToken;
    use echo_core::sandbox::{SandboxOutputChannel, SandboxStreamEvent};
    use futures::StreamExt;
    use std::sync::Arc;

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

    #[test]
    fn local_sandbox_declares_live_streaming_support() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        assert!(sandbox.supports_streaming());
    }

    #[tokio::test]
    async fn local_sandbox_streams_before_command_completes() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        let command = SandboxCommand::shell("printf first; sleep 1; printf second");
        let started = Instant::now();
        let mut stream = sandbox.execute_stream(command).await.unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_millis(700), stream.next())
            .await
            .unwrap()
            .unwrap();

        assert!(started.elapsed() < std::time::Duration::from_millis(900));
        assert!(matches!(
            first,
            SandboxStreamEvent::Output {
                channel: SandboxOutputChannel::Stdout,
                ref chunk,
            } if chunk == "first"
        ));
    }

    #[tokio::test]
    async fn local_sandbox_stream_decoder_preserves_split_unicode() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        let command = SandboxCommand::shell(
            "printf '\\342'; sleep 0.05; printf '\\202'; sleep 0.05; printf '\\254'",
        );
        let mut stream = sandbox.execute_stream(command).await.unwrap();
        let mut streamed = String::new();
        let mut complete = None;
        while let Some(event) = stream.next().await {
            match event {
                SandboxStreamEvent::Output {
                    channel: SandboxOutputChannel::Stdout,
                    chunk,
                } => streamed.push_str(&chunk),
                SandboxStreamEvent::Complete(result) => complete = Some(result),
                _ => {}
            }
        }

        assert_eq!(streamed, "€");
        assert_eq!(complete.unwrap().stdout, "€");
    }

    #[tokio::test]
    async fn local_sandbox_stream_caps_retained_output_but_reports_total_bytes() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            max_output_bytes: 4,
            ..Default::default()
        });
        let mut stream = sandbox
            .execute_stream(SandboxCommand::shell("printf abcdef"))
            .await
            .unwrap();
        let mut complete = None;
        while let Some(event) = stream.next().await {
            if let SandboxStreamEvent::Complete(result) = event {
                complete = Some(result);
            }
        }

        let result = complete.unwrap();
        assert_eq!(result.stdout, "abcd");
        assert!(result.output_truncated);
        assert_eq!(result.stdout_bytes, 6);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn controlled_execution_cancellation_stops_process_group()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        let temp_dir = std::env::temp_dir().join(format!(
            "echo-local-cancel-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&temp_dir)?;
        let marker = temp_dir.join("should-not-exist");
        let command =
            SandboxCommand::shell("sleep 0.2; touch should-not-exist").with_working_dir(&temp_dir);
        let cancel = Arc::new(CancellationToken::new());
        let cancel_after_start = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel_after_start.cancel();
        });

        let result = sandbox
            .execute_with_limits_and_cancel(command, ResourceLimits::default(), Some(cancel))
            .await;
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Sandbox(error))
                if matches!(*error, SandboxError::Cancelled(_))
        ));

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!marker.exists());
        std::fs::remove_dir_all(temp_dir)?;
        Ok(())
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

    /// 验证 `execute_with_limits` 带 `memory_bytes` 时命令能成功执行。
    ///
    /// 平台语义(对齐 Claude Code / Codex —— 沙箱层不设内存上限):
    /// 所有 local 后端(macOS/Linux/Windows)都**静默忽略** `memory_bytes` —— rlimit 在
    /// 现代动态运行时上不可靠(macOS RLIMIT_DATA 必 EINVAL;Linux RLIMIT_AS 会卡 glibc
    /// ld.so 的 mmap),真正的内存上限交给容器层(Docker/K8s 的 cgroup)。字段保留是
    /// 给容器路径用的,local 路径上被忽略,见 `configure_command_process` 注释。
    ///
    /// 本测试的历史意义:它守护的是一次回归 —— 之前 macOS 上 `memory_bytes: Some`
    /// 会让 spawn 直接 EINVAL(`os error 22`),GUI 任何代码执行都失败。现在带 limits
    /// 也必须能成功 spawn。
    #[tokio::test]
    async fn execute_with_memory_limit_starts_os_sandbox_shell()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: true,
            ..Default::default()
        });
        if !sandbox.is_available().await {
            return Ok(());
        }

        let result = sandbox
            .execute_with_limits(
                SandboxCommand::shell("echo hello"),
                ResourceLimits {
                    memory_bytes: Some(512 * 1024 * 1024),
                    ..Default::default()
                },
            )
            .await?;
        assert!(result.success(), "stderr: {}", result.stderr);
        assert_eq!(result.stdout.trim(), "hello");
        Ok(())
    }

    /// 同 [`execute_with_memory_limit_starts_os_sandbox_shell`],但走 Code 路径(python3 -c)。
    /// 验证 GUI 用例 `print('hello from sandbox')` 能成功执行 —— 之前 macOS 上传
    /// `memory_bytes: Some` 会让它 EINVAL。
    #[tokio::test]
    async fn execute_with_memory_limit_starts_os_sandbox_python_when_available()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let python_available = std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or_else(|_| false);
        if !python_available {
            return Ok(());
        }

        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: true,
            ..Default::default()
        });
        if !sandbox.is_available().await {
            return Ok(());
        }

        let result = sandbox
            .execute_with_limits(
                SandboxCommand::code("python", "print('hello from sandbox')"),
                ResourceLimits {
                    memory_bytes: Some(512 * 1024 * 1024),
                    ..Default::default()
                },
            )
            .await?;
        assert!(result.success(), "stderr: {}", result.stderr);
        assert_eq!(result.stdout.trim(), "hello from sandbox");
        Ok(())
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
    fn test_linux_reports_os_sandbox_when_enabled() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: true,
            ..Default::default()
        });
        assert_eq!(sandbox.isolation_level(), IsolationLevel::OsSandbox);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_bubblewrap_command_contains_local_isolation() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: true,
            allow_network: false,
            allowed_write_paths: vec![PathBuf::from("/tmp/eko-work")],
            ..Default::default()
        });
        let cmd =
            SandboxCommand::shell("echo hello").with_working_dir(PathBuf::from("/tmp/eko-work"));
        let command = sandbox.build_bubblewrap_command(
            "sh",
            &["-c".to_string(), "echo hello".to_string()],
            &cmd,
        );
        let rendered = format!("{command:?}");
        assert!(rendered.contains("bwrap"));
        assert!(rendered.contains("--ro-bind"));
        assert!(rendered.contains("\"/\""));
        assert!(rendered.contains("--unshare-net"));
        assert!(rendered.contains("--tmpfs"));
        assert!(rendered.contains("/tmp/eko-work"));
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
        assert!(profile.contains("(allow process*)"));
        assert!(profile.contains("(allow ipc-posix-sem)"));
        assert!(profile.contains("(allow ipc-posix-shm)"));
        assert!(profile.contains("/dev/ptmx"));
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("/opt/data"));
        assert!(profile.contains("/tmp/sandbox"));
        assert!(profile.contains("(deny file-read* (subpath"));
        assert!(profile.contains(".ssh"));
        assert!(!profile.contains("(allow network*)"));
    }

    #[test]
    fn test_seatbelt_path_validation_rejects_profile_injection_chars() {
        assert!(validate_sandbox_path("/tmp/valid path_1-2.3").is_ok());
        assert!(validate_sandbox_path("/tmp/bad\"quote").is_err());
        assert!(validate_sandbox_path("/tmp/bad\nnewline").is_err());
        assert!(validate_sandbox_path("/tmp/bad(paren)").is_err());
        assert!(validate_sandbox_path("/tmp/bad;comment").is_err());
        assert!(validate_sandbox_path("/tmp/bad#comment").is_err());
        assert!(validate_sandbox_path("/tmp/bad\0null").is_err());
    }

    #[test]
    fn test_seatbelt_rule_builder_skips_invalid_paths() {
        let mut profile = String::new();
        append_seatbelt_subpath_rule(
            &mut profile,
            "file-write*",
            &PathBuf::from("/tmp/bad\")\n(allow file-write* (subpath \"/\""),
        );
        assert!(profile.is_empty());
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
        assert_eq!(merged.max_memory_bytes, Some(256 * 1024 * 1024));

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
