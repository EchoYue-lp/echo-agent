//! 本地沙箱执行器
//!
//! 使用本地进程执行命令，并在支持的平台启用操作系统原生隔离：
//! - **macOS**: `sandbox-exec` (Seatbelt)
//! - **Linux**: `bubblewrap` (user/mount/pid/network namespaces)
//! - **Windows**: unavailable until a Job Object owns the complete process tree
//! - **其他**: 仅进程隔离（超时 + 输出截断）
//! - 支持通过 `SandboxCommand::stdin` 传入标准输入
//!
//! 这是最轻量的沙箱层，适合受信代码和只读操作。

use super::{
    CommandKind, ExecutionResult, IsolationLevel, ResourceLimits, SandboxCommand, SandboxExecutor,
    SandboxOutputChannel, SandboxStreamEvent, stream_failure_from_error,
};
use echo_core::error::Result;
use echo_core::error::SandboxError;
use echo_core::utils::utf8::{IncrementalUtf8Decoder, split_utf8_chunks};
use futures::future::BoxFuture;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const STREAM_CHUNK_BYTES: usize = 16 * 1024;
const STREAM_CHANNEL_CAPACITY: usize = 32;

type LocalEventStream = Pin<Box<dyn Stream<Item = SandboxStreamEvent> + Send>>;

struct StartedLocalCommand {
    stream: LocalEventStream,
    task: tokio::sync::oneshot::Receiver<LocalChildTerminal>,
}

enum LocalChildTerminal {
    Completed,
    Failed(SandboxError),
    Unsettled(SandboxError, String),
    Abandoned(std::result::Result<(), String>),
}

struct LocalChildRequest {
    stdin: Option<String>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    process_group_id: Option<u32>,
    require_physical_resource_ownership: bool,
    events: mpsc::Sender<SandboxStreamEvent>,
    timeout: std::time::Duration,
    max_output_bytes: usize,
    sandbox_type: &'static str,
    cancel: Option<Arc<CancellationToken>>,
}

/// 本地沙箱配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    /// 是否启用本地沙箱后端（sandbox-exec / bubblewrap）
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
            enable_os_sandbox: cfg!(any(target_os = "macos", target_os = "linux")),
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
        self.config.enable_os_sandbox && cfg!(any(target_os = "macos", target_os = "linux"))
    }

    fn sandbox_type(&self) -> &'static str {
        if !self.effective_os_sandbox_enabled() {
            return "local";
        }
        if cfg!(target_os = "macos") {
            "local-seatbelt"
        } else if cfg!(target_os = "linux") {
            "local-bubblewrap"
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
        cancel: Option<Arc<CancellationToken>>,
        resource_ownership: (Vec<echo_core::tools::InvocationResourceGuard>, bool),
    ) -> Result<ExecutionResult> {
        ensure_local_execution_supported()?;
        self.run_command_controlled(
            command,
            timeout,
            stdin,
            sandbox_type,
            cancel,
            resource_ownership,
        )
        .await
    }

    async fn run_command_controlled(
        &self,
        command: Command,
        timeout: std::time::Duration,
        stdin: Option<&str>,
        sandbox_type: &'static str,
        cancel: Option<Arc<CancellationToken>>,
        resource_ownership: (Vec<echo_core::tools::InvocationResourceGuard>, bool),
    ) -> Result<ExecutionResult> {
        if cancel.as_ref().is_some_and(|token| token.is_cancelled()) {
            return Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::Cancelled(
                    "owning run was cancelled before process start".to_string(),
                ),
            )));
        }

        let StartedLocalCommand { mut stream, task } = self
            .start_command_stream(
                command,
                timeout,
                stdin,
                sandbox_type,
                cancel,
                resource_ownership,
            )
            .await?;
        let mut completion = None;
        while let Some(event) = stream.next().await {
            match event {
                SandboxStreamEvent::Complete(result) => {
                    completion = Some(result);
                    break;
                }
                SandboxStreamEvent::Failed { .. } => break,
                SandboxStreamEvent::Output { .. } => {}
            }
        }

        match task.await {
            Ok(LocalChildTerminal::Completed) => completion.ok_or_else(|| {
                echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(
                    "Local sandbox stream ended without a completion event".to_string(),
                )))
            }),
            Ok(LocalChildTerminal::Unsettled(_, operation_id)) => {
                Err(echo_core::error::ReactError::ExecutionNotSettled {
                    run_id: operation_id,
                    execution_id: None,
                })
            }
            Ok(LocalChildTerminal::Failed(error)) => {
                Err(echo_core::error::ReactError::Sandbox(Box::new(error)))
            }
            Ok(LocalChildTerminal::Abandoned(cleanup)) => {
                Err(echo_core::error::ReactError::Sandbox(Box::new(
                    SandboxError::IoError(match cleanup {
                        Ok(()) => "Local sandbox event receiver closed before terminal delivery"
                            .to_string(),
                        Err(error) => format!(
                            "Local sandbox event receiver closed before terminal delivery; cleanup failed: {error}"
                        ),
                    }),
                )))
            }
            Err(error) => Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!("Local sandbox execution task failed: {error}")),
            ))),
        }
    }

    async fn run_command_stream(
        &self,
        command: Command,
        timeout: std::time::Duration,
        stdin: Option<&str>,
        sandbox_type: &'static str,
        cancel: Option<Arc<CancellationToken>>,
        resource_ownership: (Vec<echo_core::tools::InvocationResourceGuard>, bool),
    ) -> Result<LocalEventStream> {
        let StartedLocalCommand { stream, task } = self
            .start_command_stream(
                command,
                timeout,
                stdin,
                sandbox_type,
                cancel,
                resource_ownership,
            )
            .await?;
        // 流调用者持有原事件 receiver；丢弃它唤醒同一个 child owner 做 kill + wait。
        // terminal receiver 只是观察，不能用它的退出冒充物理终结。
        drop(task);
        Ok(stream)
    }

    async fn start_command_stream(
        &self,
        mut command: Command,
        timeout: std::time::Duration,
        stdin: Option<&str>,
        sandbox_type: &'static str,
        cancel: Option<Arc<CancellationToken>>,
        resource_ownership: (Vec<echo_core::tools::InvocationResourceGuard>, bool),
    ) -> Result<StartedLocalCommand> {
        let (resource_guards, require_physical_resource_ownership) = resource_ownership;
        if tokio::time::Instant::now().checked_add(timeout).is_none() {
            return Err(SandboxError::StartFailed(
                "local command timeout exceeds the monotonic clock range".to_string(),
            )
            .into());
        }
        if stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        configure_command_process(&mut command, self.config.max_memory_bytes);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);

        let operation_id = format!("local-native:{}", uuid::Uuid::new_v4());
        let resources = local_native_resource_owners()
            .issue(operation_id.clone())
            .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?;
        let mut child = command.spawn().map_err(|e| {
            echo_core::error::ReactError::Sandbox(Box::new(SandboxError::StartFailed(format!(
                "Failed to spawn process: {e}"
            ))))
        })?;
        let process_group_id = child.id();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let stdin_input = stdin.map(ToString::to_string);
        let max_output_bytes = self.config.max_output_bytes;
        let (tx, rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let failure_events = tx.clone();
        let (terminal_tx, task) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            // 原 child task 继续拥有原 Child／PGID／资源；Unknown 只结束观察，不结束物理 owner。
            let held_guards = resource_guards.clone();
            let terminal = run_streaming_child(
                &mut child,
                LocalChildRequest {
                    stdin: stdin_input,
                    stdout: stdout_pipe,
                    stderr: stderr_pipe,
                    process_group_id,
                    require_physical_resource_ownership,
                    events: tx,
                    timeout,
                    max_output_bytes,
                    sandbox_type,
                    cancel,
                },
            )
            .await;
            let terminal = settle_local_command_resources(
                terminal,
                resources,
                resource_guards,
                operation_id.clone(),
            );
            let unsettled = matches!(&terminal, LocalChildTerminal::Unsettled(_, _));
            emit_local_terminal_failure(&failure_events, &terminal).await;
            let _ = terminal_tx.send(terminal);
            if unsettled {
                // 不重放命令、不另起 supervisor，也不再向未知目标重复发 kill。
                // 同一个原 owner 只观察原 child wait 与原进程组消失，再核销同一代 fence。
                reconcile_original_local_child(&mut child, process_group_id, &operation_id).await;
            }
            drop(held_guards);
        });
        let stream = Box::pin(futures::stream::unfold(rx, |mut receiver| async move {
            receiver.recv().await.map(|event| (event, receiver))
        }));
        Ok(StartedLocalCommand { stream, task })
    }
}

/// 仅复用原 child owner 给既有 Skill direct fallback／探测，不宣称 OS 隔离。
/// 原公开 LocalSandbox 平台限制仍在各 executor 入口，调用方的 env_clear／环境保持。
pub(crate) async fn run_direct_skill_command(
    command: Command,
    native_command: SandboxCommand,
    max_output_bytes: usize,
) -> Result<ExecutionResult> {
    native_command.ensure_physical_resource_ownership(cfg!(unix))?;
    LocalSandbox::new(LocalConfig {
        enable_os_sandbox: false,
        allow_network: true,
        max_output_bytes,
        ..LocalConfig::default()
    })
    .run_command_controlled(
        command,
        native_command.timeout,
        None,
        "direct",
        native_command.cancel,
        (
            native_command.resource_guards,
            native_command.require_physical_resource_ownership,
        ),
    )
    .await
}

async fn reconcile_original_local_child(
    child: &mut tokio::process::Child,
    process_group_id: Option<u32>,
    operation_id: &str,
) {
    loop {
        let waited = child.wait().await.is_ok();
        #[cfg(unix)]
        let absent = match process_group_id {
            Some(group) => verify_process_group_absent(group).await.is_ok(),
            None => false,
        };
        #[cfg(not(unix))]
        let absent = {
            let _ = process_group_id;
            true
        };
        if waited && absent {
            if let Some(fence) = local_native_resource_owners().uncertainty(operation_id) {
                local_native_resource_owners().confirm_uncertain_stopped(&fence);
            }
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn settle_local_command_resources(
    terminal: LocalChildTerminal,
    resources: echo_core::agent::admission::KeyedExecutionLease,
    resource_guards: Vec<echo_core::tools::InvocationResourceGuard>,
    operation_id: String,
) -> LocalChildTerminal {
    match terminal {
        LocalChildTerminal::Unsettled(error, _) => {
            tracing::error!(operation_id = %operation_id, error = %error,
                "local physical cleanup is unproved; original child owner and resources retained");
            resources.retain_unknown_with_resources(Some(Box::new(resource_guards)));
            LocalChildTerminal::Unsettled(error, operation_id)
        }
        LocalChildTerminal::Abandoned(Err(error)) => {
            tracing::error!(operation_id = %operation_id, error = %error,
                "local receiver detached with unproved cleanup; original child owner retained");
            resources.retain_unknown_with_resources(Some(Box::new(resource_guards)));
            LocalChildTerminal::Unsettled(SandboxError::IoError(error), operation_id)
        }
        settled => {
            drop(resource_guards);
            drop(resources);
            settled
        }
    }
}

fn local_native_resource_owners()
-> &'static Arc<echo_core::agent::admission::KeyedExecutionAdmission> {
    // 复用既有 keyed unknown fence；不配置容量、不调度、不重跑命令。
    static OWNERS: std::sync::LazyLock<Arc<echo_core::agent::admission::KeyedExecutionAdmission>> =
        std::sync::LazyLock::new(|| {
            Arc::new(echo_core::agent::admission::KeyedExecutionAdmission::default())
        });
    &OWNERS
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
// (cgroup)。embedding application 沿用同一设计:`max_memory_bytes` 字段保留是给 Docker/K8s 路径用的,
// local 后端(macOS/Linux/Windows)静默忽略它。Linux local 沙箱仍可用 bwrap 做
// namespace 隔离(见 `build_bubblewrap_command`),只是不限内存。
#[cfg(unix)]
fn configure_command_process(command: &mut Command, _max_memory_bytes: Option<u64>) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_command_process(_command: &mut Command, _max_memory_bytes: Option<u64>) {}

fn ensure_local_execution_supported() -> Result<()> {
    if cfg!(windows) {
        Err(echo_core::error::ReactError::Sandbox(Box::new(
            SandboxError::Unavailable(
                "Local sandbox execution is unavailable on Windows until the backend owns a Job Object"
                    .to_string(),
            ),
        )))
    } else {
        Ok(())
    }
}

pub(crate) async fn cleanup_child_process(
    child: &mut tokio::process::Child,
    process_group_id: Option<u32>,
) -> std::result::Result<(), String> {
    #[cfg(not(unix))]
    let _ = process_group_id;
    #[cfg(unix)]
    let group_signal = match process_group_id {
        Some(process_group_id) => signal_process_group(process_group_id, libc::SIGKILL),
        None => Err("missing captured process-group id during cleanup".to_string()),
    };
    #[cfg(not(unix))]
    let group_signal: std::result::Result<(), String> = Ok(());

    if let Err(error) = child.start_kill() {
        tracing::debug!("Child process was already exiting during cleanup: {error}");
    }
    let child_wait = child
        .wait()
        .await
        .map(|_| ())
        .map_err(|error| format!("failed to wait for child process termination: {error}"));

    #[cfg(unix)]
    let group_absent = match process_group_id {
        Some(process_group_id) => verify_process_group_absent(process_group_id).await,
        None => Err("cannot verify process-group cleanup without a captured id".to_string()),
    };
    #[cfg(not(unix))]
    let group_absent: std::result::Result<(), String> = Ok(());

    let failures = [group_signal.err(), child_wait.err(), group_absent.err()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(unix)]
fn signal_process_group(process_group_id: u32, signal: i32) -> std::result::Result<(), String> {
    let process_group_id = i32::try_from(process_group_id)
        .map_err(|error| format!("process-group id is out of range: {error}"))?;
    // SAFETY: `kill` receives a negated process-group id captured immediately
    // after spawning a child configured with `process_group(0)`. No pointers or
    // borrowed memory cross the FFI boundary.
    let result = unsafe { libc::kill(-process_group_id, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!(
            "failed to signal process group {process_group_id}: {error}"
        ))
    }
}

fn original_process_group_absent(
    process_group_id: Option<u32>,
) -> std::result::Result<bool, String> {
    #[cfg(unix)]
    {
        let group = process_group_id
            .and_then(|group| i32::try_from(group).ok())
            .filter(|group| *group > 1)
            .ok_or_else(|| "missing valid original process-group id".to_string())?;
        // SAFETY: signal 0 only observes the captured original process group, never a caller-supplied target.
        let observed = unsafe { libc::kill(-group, 0) };
        if observed == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(true)
        } else {
            Err(format!(
                "cannot establish original process-group absence: {error}"
            ))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = process_group_id;
        Err("original process-group observation unavailable on this platform".to_string())
    }
}

#[cfg(unix)]
async fn verify_process_group_absent(process_group_id: u32) -> std::result::Result<(), String> {
    let process_group_id = i32::try_from(process_group_id)
        .map_err(|error| format!("process-group id is out of range: {error}"))?;
    for _ in 0..100 {
        // SAFETY: signal 0 performs existence/permission validation only; the
        // negative id addresses the captured process group and no pointers cross
        // the FFI boundary.
        let result = unsafe { libc::kill(-process_group_id, 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            if error.raw_os_error() != Some(libc::EPERM) {
                return Err(format!(
                    "failed to verify process group {process_group_id} absence: {error}"
                ));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Err(format!(
        "process group {process_group_id} still exists after kill and child wait"
    ))
}

async fn run_streaming_child(
    child: &mut tokio::process::Child,
    request: LocalChildRequest,
) -> LocalChildTerminal {
    let LocalChildRequest {
        stdin,
        mut stdout,
        mut stderr,
        process_group_id,
        require_physical_resource_ownership,
        events: tx,
        timeout,
        max_output_bytes,
        sandbox_type,
        cancel,
    } = request;
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

    if let Some(input) = stdin {
        let Some(mut child_stdin) = child.stdin.take() else {
            let cleanup = cleanup_child_process(child, process_group_id).await;
            return local_failure_after_cleanup(
                "local sandbox did not provide the requested stdin pipe".to_string(),
                cleanup,
            );
        };
        let write = tokio::select! {
            biased;            _ = tx.closed() => {
                drop(child_stdin);
                let cleanup = cleanup_child_process(child, process_group_id).await;
                return abandoned_local_terminal(cleanup);
            }
            _ = wait_for_local_cancel(cancel.as_ref()) => {
                drop(child_stdin);
                let cleanup = cleanup_child_process(child, process_group_id).await;
                return local_cancellation_terminal(
                    "owning run was cancelled while writing process stdin",
                    cleanup,
                );
            }
            _ = &mut deadline => {
                drop(child_stdin);
                let cleanup = cleanup_child_process(child, process_group_id).await;
                return local_timeout_terminal(
                    cleanup,
                    retained_stdout,
                    retained_stderr,
                    start.elapsed(),
                    sandbox_type,
                    timeout,
                    &tx,
                ).await;
            }
            result = child_stdin.write_all(input.as_bytes()) => result,
        };
        if let Err(error) = write {
            drop(child_stdin);
            let cleanup = cleanup_child_process(child, process_group_id).await;
            return local_failure_after_cleanup(
                format!("failed to write local sandbox stdin: {error}"),
                cleanup,
            );
        }
        drop(child_stdin);
    }

    loop {
        if require_physical_resource_ownership
            && cancel.as_ref().is_some_and(|token| token.is_cancelled())
        {
            let cleanup = cleanup_child_process(child, process_group_id).await;
            return local_cancellation_terminal(
                "owning run cancelled before owned-group closure",
                cleanup,
            );
        }
        if require_physical_resource_ownership && tokio::time::Instant::now() >= deadline.deadline()
        {
            let cleanup = cleanup_child_process(child, process_group_id).await;
            return local_timeout_terminal(
                cleanup,
                retained_stdout,
                retained_stderr,
                start.elapsed(),
                sandbox_type,
                timeout,
                &tx,
            )
            .await;
        }
        if stdout.is_none()
            && stderr.is_none()
            && status.is_some()
            && (!require_physical_resource_ownership
                || original_process_group_absent(process_group_id).unwrap_or(false))
        {
            break;
        }
        tokio::select! {
            biased;
            _ = tx.closed() => {
                let cleanup = cleanup_child_process(child, process_group_id).await;
                return abandoned_local_terminal(cleanup);
            }
            _ = wait_for_local_cancel(cancel.as_ref()) => {
                let cleanup = cleanup_child_process(child, process_group_id).await;
                return local_cancellation_terminal(
                    "owning run was cancelled during local sandbox execution",
                    cleanup,
                );
            }
            _ = &mut deadline => {
                let cleanup = cleanup_child_process(child, process_group_id).await;
                return local_timeout_terminal(
                    cleanup,
                    retained_stdout,
                    retained_stderr,
                    start.elapsed(),
                    sandbox_type,
                    timeout,
                    &tx,
                ).await;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)),
                if require_physical_resource_ownership && status.is_some() && stdout.is_none() && stderr.is_none() => {}
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
                            let cleanup = cleanup_child_process(child, process_group_id).await;
                            return abandoned_local_terminal(cleanup);
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
                                let cleanup = cleanup_child_process(child, process_group_id).await;
                                return abandoned_local_terminal(cleanup);
                            }
                        }
                    }
                    Err(error) => {
                        let cleanup = cleanup_child_process(child, process_group_id).await;
                        return local_failure_after_cleanup(
                            format!("failed to read local sandbox stdout: {error}"),
                            cleanup,
                        );
                    }
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
                            let cleanup = cleanup_child_process(child, process_group_id).await;
                            return abandoned_local_terminal(cleanup);
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
                                let cleanup = cleanup_child_process(child, process_group_id).await;
                                return abandoned_local_terminal(cleanup);
                            }
                        }
                    }
                    Err(error) => {
                        let cleanup = cleanup_child_process(child, process_group_id).await;
                        return local_failure_after_cleanup(
                            format!("failed to read local sandbox stderr: {error}"),
                            cleanup,
                        );
                    }
                }
            }
            waited = child.wait(), if status.is_none() => {
                match waited {
                    Ok(exit_status) => status = Some(exit_status),
                    Err(error) => {
                        let cleanup = cleanup_child_process(child, process_group_id).await;
                        return local_failure_after_cleanup(
                            format!("failed to wait for local sandbox process: {error}"),
                            cleanup,
                        );
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
    if tx.send(SandboxStreamEvent::Complete(result)).await.is_ok() {
        LocalChildTerminal::Completed
    } else {
        LocalChildTerminal::Abandoned(Ok(()))
    }
}

fn local_failure_after_cleanup(
    primary: String,
    cleanup: std::result::Result<(), String>,
) -> LocalChildTerminal {
    let unsettled = cleanup.is_err();
    let message = match cleanup {
        Ok(()) => primary,
        Err(cleanup) => format!(
            "{}; local sandbox cleanup failed: {}",
            bounded_terminal_fact(&primary),
            bounded_terminal_fact(&cleanup)
        ),
    };
    if unsettled {
        LocalChildTerminal::Unsettled(SandboxError::IoError(message), String::new())
    } else {
        LocalChildTerminal::Failed(SandboxError::IoError(message))
    }
}

fn local_cancellation_terminal(
    primary: &str,
    cleanup: std::result::Result<(), String>,
) -> LocalChildTerminal {
    match cleanup {
        Ok(()) => LocalChildTerminal::Failed(SandboxError::Cancelled(primary.to_string())),
        Err(cleanup) => local_failure_after_cleanup(primary.to_string(), Err(cleanup)),
    }
}

async fn local_timeout_terminal(
    cleanup: std::result::Result<(), String>,
    stdout: RetainedPipeOutput,
    stderr: RetainedPipeOutput,
    duration: std::time::Duration,
    sandbox_type: &str,
    timeout: std::time::Duration,
    events: &mpsc::Sender<SandboxStreamEvent>,
) -> LocalChildTerminal {
    let result = local_execution_result(
        -1,
        stdout,
        stderr,
        duration,
        sandbox_type,
        true,
        Some(format!("Process timed out after {}s", timeout.as_secs())),
    );
    match cleanup {
        Ok(()) => {
            if events
                .send(SandboxStreamEvent::Complete(result))
                .await
                .is_ok()
            {
                LocalChildTerminal::Completed
            } else {
                LocalChildTerminal::Abandoned(Ok(()))
            }
        }
        Err(cleanup) => local_failure_after_cleanup(
            format!(
                "local sandbox timed out after {}s (stdout_bytes={}, stderr_bytes={})",
                timeout.as_secs(),
                result.stdout_bytes,
                result.stderr_bytes
            ),
            Err(cleanup),
        ),
    }
}

fn abandoned_local_terminal(cleanup: std::result::Result<(), String>) -> LocalChildTerminal {
    if let Err(error) = cleanup.as_ref() {
        tracing::error!(
            error,
            "local sandbox cleanup debt after event receiver closed"
        );
    }
    LocalChildTerminal::Abandoned(cleanup)
}

fn bounded_terminal_fact(value: &str) -> String {
    const MAX_FACT_CHARS: usize = 1_024;
    let mut fact = value.chars().take(MAX_FACT_CHARS).collect::<String>();
    if value.chars().count() > MAX_FACT_CHARS {
        fact.push_str("...");
    }
    fact
}

async fn emit_local_terminal_failure(
    events: &mpsc::Sender<SandboxStreamEvent>,
    terminal: &LocalChildTerminal,
) {
    if let LocalChildTerminal::Failed(error) | LocalChildTerminal::Unsettled(error, _) = terminal {
        let _ = events
            .send(SandboxStreamEvent::Failed {
                failure: stream_failure_from_error(error),
            })
            .await;
    }
}

async fn wait_for_local_cancel(cancel: Option<&Arc<CancellationToken>>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn send_sandbox_output(
    tx: &mpsc::Sender<SandboxStreamEvent>,
    channel: SandboxOutputChannel,
    text: String,
) -> std::result::Result<(), ()> {
    for chunk in split_utf8_chunks(text, STREAM_CHUNK_BYTES) {
        tx.send(SandboxStreamEvent::Output { channel, chunk })
            .await
            .map_err(|_| ())?;
    }
    Ok(())
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

    fn supports_physical_resource_ownership(&self) -> bool {
        // 仅此实现的原 Unix process-group owner：持 Child/guard 到 wait+drain，
        // cleanup 未知仍由同一 task 等实际组消失后核销；不是对任意 Unix 后端的推断。
        cfg!(unix)
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(async {
            if cfg!(windows) {
                false
            } else if self.effective_os_sandbox_enabled() && cfg!(target_os = "macos") {
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
            command
                .ensure_physical_resource_ownership(self.supports_physical_resource_ownership())?;
            ensure_local_execution_supported()?;
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
            self.run_command(
                cmd,
                timeout,
                command.stdin.as_deref(),
                self.sandbox_type(),
                command.cancel.clone(),
                (
                    command.resource_guards.clone(),
                    command.require_physical_resource_ownership,
                ),
            )
            .await
        })
    }

    fn execute_stream<'a>(
        &'a self,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = SandboxStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            command
                .ensure_physical_resource_ownership(self.supports_physical_resource_ownership())?;
            ensure_local_execution_supported()?;
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
            self.run_command_stream(
                cmd,
                timeout,
                command.stdin.as_deref(),
                self.sandbox_type(),
                command.cancel.clone(),
                (
                    command.resource_guards.clone(),
                    command.require_physical_resource_ownership,
                ),
            )
            .await
        })
    }

    fn supports_streaming(&self) -> bool {
        !cfg!(windows)
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

    fn execute_with_limits_and_cancel(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
        cancel: Option<Arc<CancellationToken>>,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move {
            command
                .ensure_physical_resource_ownership(self.supports_physical_resource_ownership())?;
            ensure_local_execution_supported()?;
            let timeout = limits
                .cpu_time_secs
                .map(std::time::Duration::from_secs)
                .unwrap_or(command.timeout);
            let sandbox = LocalSandbox::new(merge_limits_into_config(self.config.clone(), &limits));
            let command = SandboxCommand { timeout, ..command };
            let process = match &command.kind {
                CommandKind::Shell(shell) => sandbox.build_shell_command(shell, &command),
                CommandKind::Program { program, args } => {
                    sandbox.build_program_command(program, args, &command)
                }
                CommandKind::Code { language, code } => sandbox
                    .build_code_command(language, code, &command)
                    .map_err(|error| echo_core::error::ReactError::Sandbox(Box::new(error)))?,
            };
            sandbox
                .run_command_controlled(
                    process,
                    timeout,
                    command.stdin.as_deref(),
                    sandbox.sandbox_type(),
                    cancel.or_else(|| command.cancel.clone()),
                    (
                        command.resource_guards.clone(),
                        command.require_physical_resource_ownership,
                    ),
                )
                .await
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
    async fn unknown_cleanup_retains_the_original_guard_on_the_exact_fence()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        struct DropCounter(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let operation_id = format!("controlled-native-unknown:{}", uuid::Uuid::new_v4());
        let owners = local_native_resource_owners();
        let lease = owners.issue(operation_id.clone())?;
        let terminal = settle_local_command_resources(
            local_failure_after_cleanup(
                "controlled failure".to_string(),
                Err("cleanup unproved".to_string()),
            ),
            lease,
            vec![echo_core::tools::InvocationResourceGuard::new(DropCounter(
                dropped.clone(),
            ))],
            operation_id.clone(),
        );
        assert!(
            matches!(terminal, LocalChildTerminal::Unsettled(_, ref id) if *id == operation_id)
        );
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 0);
        let fence = owners
            .uncertainty(&operation_id)
            .ok_or("missing original uncertainty fence")?;
        // 纯资源负控，没有实际 process；不把模拟 cleanup failure 当真实 OS 失败。
        assert!(owners.confirm_uncertain_stopped(&fence));
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn original_unknown_owner_requires_real_child_exit_before_confirming_the_fence()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().acquire_owned().await?;
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30"])
            .process_group(0)
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let process_group_id = child.id().ok_or("missing original native child id")?;
        let operation_id = format!("controlled-native-owner:{}", uuid::Uuid::new_v4());
        let lease = local_native_resource_owners().issue(operation_id.clone())?;
        // 受控注入清理未知，实际 child 仍活着；不能把它误写成已发生的 OS 清理故障。
        let terminal = settle_local_command_resources(
            local_failure_after_cleanup(
                "controlled unknown".to_string(),
                Err("closure not yet established".to_string()),
            ),
            lease,
            vec![echo_core::tools::InvocationResourceGuard::new(permit)],
            operation_id.clone(),
        );
        assert!(matches!(terminal, LocalChildTerminal::Unsettled(_, _)));
        let owned_id = operation_id.clone();
        let mut owner = tokio::spawn(async move {
            reconcile_original_local_child(&mut child, Some(process_group_id), &owned_id).await;
        });
        let closed_while_live =
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut owner)
                .await
                .is_ok();
        let fence_while_live = local_native_resource_owners()
            .uncertainty(&operation_id)
            .is_some();
        let available_while_live = capacity.available_permits();
        signal_process_group(process_group_id, libc::SIGKILL)?;
        tokio::time::timeout(std::time::Duration::from_secs(3), owner).await??;
        verify_process_group_absent(process_group_id).await?;
        assert!(
            !closed_while_live,
            "an observation deadline was treated as physical closure"
        );
        assert!(fence_while_live);
        assert_eq!(available_while_live, 0);
        assert!(
            local_native_resource_owners()
                .uncertainty(&operation_id)
                .is_none()
        );
        assert_eq!(capacity.available_permits(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_skill_probe_discards_output_while_draining_both_real_pipes()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "head -c 1048576 /dev/zero; head -c 1048576 /dev/zero >&2",
        ]);
        let output = run_direct_skill_command(
            command,
            SandboxCommand::program("sh", vec![]).with_timeout(std::time::Duration::from_secs(5)),
            0,
        )
        .await?;
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert_eq!(output.stdout_bytes, 1_048_576);
        assert_eq!(output.stderr_bytes, 1_048_576);
        assert!(output.output_truncated);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_skill_owner_keeps_resources_through_cancel_timeout_and_caller_drop()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        struct Guard {
            pid: Arc<AtomicU32>,
            dropped: Arc<AtomicBool>,
            live_at_release: Arc<AtomicBool>,
            _permit: tokio::sync::OwnedSemaphorePermit,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                if let Ok(pid) = i32::try_from(self.pid.load(Ordering::SeqCst))
                    && pid > 0
                {
                    // SAFETY: signal 0 checks the original child-owned process group only.
                    let exists = unsafe { libc::kill(-pid, 0) } == 0;
                    self.live_at_release.store(exists, Ordering::SeqCst);
                }
                self.dropped.store(true, Ordering::SeqCst);
            }
        }
        for mode in ["cancel", "timeout", "caller-drop"] {
            let root =
                std::env::temp_dir().join(format!("direct-skill-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root)?;
            let capacity = Arc::new(tokio::sync::Semaphore::new(1));
            let pid = Arc::new(AtomicU32::new(0));
            let dropped = Arc::new(AtomicBool::new(false));
            let live_at_release = Arc::new(AtomicBool::new(false));
            let guard = Guard {
                pid: pid.clone(),
                dropped: dropped.clone(),
                live_at_release: live_at_release.clone(),
                _permit: capacity.clone().acquire_owned().await?,
            };
            let cancel = Arc::new(CancellationToken::new());
            let owner_cancel = cancel.clone();
            let mut command = Command::new("sh");
            command
                .args(["-c", "echo $$ > owner.pid; sleep 30"])
                .current_dir(&root);
            let timeout = if mode == "timeout" {
                std::time::Duration::from_millis(500)
            } else {
                std::time::Duration::from_secs(30)
            };
            let execution = tokio::spawn(async move {
                run_direct_skill_command(
                    command,
                    SandboxCommand::program("sh", vec![])
                        .with_timeout(timeout)
                        .with_cancel(Some(owner_cancel))
                        .with_resource_guards(vec![
                            echo_core::tools::InvocationResourceGuard::new(guard),
                        ]),
                    usize::MAX,
                )
                .await
            });
            let barrier = wait_for_pid(&root.join("owner.pid")).await;
            let owner_pid = match barrier {
                Ok(pid) => pid,
                Err(error) => {
                    cancel.cancel();
                    let _ = execution.await;
                    std::fs::remove_dir_all(root)?;
                    return Err(error.into());
                }
            };
            pid.store(owner_pid, Ordering::SeqCst);
            assert_eq!(capacity.available_permits(), 0);
            match mode {
                "cancel" => {
                    cancel.cancel();
                    let result = execution.await?;
                    assert!(
                        matches!(result, Err(echo_core::error::ReactError::Sandbox(error)) if matches!(*error, SandboxError::Cancelled(_)))
                    );
                }
                "timeout" => {
                    let result = execution.await??;
                    assert!(result.timed_out);
                }
                _ => {
                    execution.abort();
                    assert!(execution.await.is_err());
                }
            }
            verify_process_group_absent(owner_pid).await?;
            for _ in 0..100 {
                if dropped.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert!(
                dropped.load(Ordering::SeqCst),
                "mode {mode} lost its original physical resource owner"
            );
            assert!(
                !live_at_release.load(Ordering::SeqCst),
                "mode {mode} released resources before the original group disappeared"
            );
            assert_eq!(capacity.available_permits(), 1);
            std::fs::remove_dir_all(root)?;
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn normal_root_exit_and_pipe_eof_do_not_release_a_live_owned_group()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for mode in ["natural", "cancel", "deadline"] {
            let root = std::env::temp_dir()
                .join(format!("local-group-after-eof-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root)?;
            let capacity = Arc::new(tokio::sync::Semaphore::new(1));
            let permit = capacity.clone().acquire_owned().await?;
            let cancel = Arc::new(CancellationToken::new());
            let timeout = if mode == "deadline" {
                std::time::Duration::from_secs(2)
            } else {
                std::time::Duration::from_secs(10)
            };
            // 合法 nested sh + 重定向后台负载，父 exit/EOF 后后代仍在同组。
            let tail = if mode == "deadline" { "; sleep 1" } else { "" };
            let payload = format!(
                "echo $$ > leader.pid; sh -c 'while [ ! -e release ]; do sleep 0.02; done' >/dev/null 2>&1 & echo $! > descendant.pid{tail}"
            );
            let mut command = SandboxCommand::shell(payload)
                .with_working_dir(&root)
                .with_timeout(timeout)
                .with_cancel(Some(cancel.clone()))
                .with_resource_guards(vec![echo_core::tools::InvocationResourceGuard::new(permit)]);
            command.require_physical_resource_ownership = true;
            let sandbox = LocalSandbox::new(LocalConfig {
                enable_os_sandbox: false,
                ..LocalConfig::default()
            });
            let started = tokio::time::Instant::now();
            let mut execution = tokio::spawn(async move { sandbox.execute(command).await });
            let leader = wait_for_pid(&root.join("leader.pid")).await;
            let descendant = wait_for_pid(&root.join("descendant.pid")).await;
            let (leader, descendant) = match (leader, descendant) {
                (Ok(leader), Ok(descendant)) => (leader, descendant),
                _ => {
                    cancel.cancel();
                    let _ = execution.await;
                    std::fs::remove_dir_all(root)?;
                    return Err("owned group fixture did not publish its original pids".into());
                }
            };
            for _ in 0..300 {
                if !process_exists(leader)? {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let root_exited = !process_exists(leader)?;
            let descendant_live = process_exists(descendant)?;
            let capacity_while_live = capacity.available_permits();
            let early = tokio::time::timeout(std::time::Duration::from_millis(20), &mut execution)
                .await
                .ok();
            let completed_before_group = early.is_some();
            if early.is_some() || mode == "natural" {
                std::fs::write(root.join("release"), "release")?;
            } else if mode == "cancel" {
                cancel.cancel();
            } else {
                // 仅在原 deadline 后放行；若错误地在父退出时重启预算，会自然成功，不能蒙混成 timeout。
                tokio::time::sleep_until(
                    started
                        .checked_add(std::time::Duration::from_millis(2200))
                        .ok_or("test release deadline overflow")?,
                )
                .await;
                std::fs::write(root.join("release"), "release")?;
            }
            let result = match early {
                Some(result) => result,
                None => {
                    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut execution)
                        .await
                    {
                        Ok(result) => result,
                        Err(_) => {
                            cancel.cancel();
                            let _ = signal_process_group(leader, libc::SIGKILL);
                            let _ = std::fs::write(root.join("release"), "release");
                            execution.await
                        }
                    }
                }
            };
            let closure = verify_process_group_absent(leader).await;
            eprintln!(
                "owned local group: mode={mode}; root_exited={root_exited}; descendant_live={descendant_live}; capacity={capacity_while_live}; early={completed_before_group}; elapsed={:?}; result={result:?}",
                started.elapsed()
            );
            std::fs::remove_dir_all(root)?;
            closure?;
            assert!(
                root_exited && descendant_live,
                "fixture must reach real root-exit/EOF while original descendant remains alive"
            );
            assert_eq!(capacity_while_live, 0);
            assert!(
                !completed_before_group,
                "root wait/EOF was incorrectly treated as owned-group closure"
            );
            match mode {
                "natural" => {
                    let result = result??;
                    assert!(result.success());
                }
                "cancel" => {
                    assert!(
                        matches!(result?, Err(echo_core::error::ReactError::Sandbox(error)) if matches!(*error, SandboxError::Cancelled(_)))
                    );
                }
                _ => {
                    let result = result??;
                    assert!(result.timed_out, "deadline restarted after root exit/EOF");
                }
            }
            assert_eq!(capacity.available_permits(), 1);
        }
        Ok(())
    }

    #[cfg(unix)]
    async fn wait_for_pid(path: &std::path::Path) -> std::result::Result<u32, String> {
        for _ in 0..100 {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Ok(pid) = contents.trim().parse::<u32>()
            {
                return Ok(pid);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(format!("process did not publish pid at {}", path.display()))
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> std::result::Result<bool, String> {
        let pid = i32::try_from(pid).map_err(|error| format!("pid is out of range: {error}"))?;
        // SAFETY: signal 0 performs existence validation only and receives a
        // numeric pid parsed from a child-owned test barrier file.
        let result = unsafe { libc::kill(pid, 0) };
        if result == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else {
            Err(format!("failed to inspect process {pid}: {error}"))
        }
    }

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
        let owner_pid = temp_dir.join("owner.pid");
        let descendant_pid = temp_dir.join("descendant.pid");
        let command = SandboxCommand::shell(
            "printf '%s' \"$$\" > owner.pid; sh -c 'printf \"%s\" \"$$\" > descendant.pid; while :; do sleep 1; done' & while [ ! -s descendant.pid ]; do sleep 0.01; done; exit 0",
        )
        .with_working_dir(&temp_dir);
        let cancel = Arc::new(CancellationToken::new());
        let execution_cancel = cancel.clone();
        let execution = tokio::spawn(async move {
            sandbox
                .execute_with_limits_and_cancel(
                    command,
                    ResourceLimits::default(),
                    Some(execution_cancel),
                )
                .await
        });

        let pid = wait_for_pid(&owner_pid)
            .await
            .map_err(|error| format!("owner pid barrier failed: {error}"))?;
        let descendant = wait_for_pid(&descendant_pid)
            .await
            .map_err(|error| format!("descendant pid barrier failed: {error}"))?;
        for _ in 0..100 {
            if !process_exists(pid)? {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !process_exists(pid)?,
            "leader must exit before cancellation"
        );
        assert!(
            process_exists(descendant)?,
            "descendant must remain alive before cancellation"
        );
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), execution)
            .await
            .map_err(|_| "local sandbox cancellation did not reach a terminal state")?
            .map_err(|error| format!("local sandbox execution task failed: {error}"))?;
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Sandbox(error))
                if matches!(*error, SandboxError::Cancelled(_))
        ));

        verify_process_group_absent(pid).await?;
        assert!(!process_exists(descendant)?);
        std::fs::remove_dir_all(temp_dir)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn caller_abort_during_blocked_stdin_keeps_detached_process_owner()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        let temp_dir = std::env::temp_dir().join(format!(
            "echo-local-stdin-abort-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&temp_dir)?;
        let owner_pid = temp_dir.join("owner.pid");
        let command =
            SandboxCommand::shell("printf '%s' \"$$\" > owner.pid; while :; do sleep 1; done")
                .with_working_dir(&temp_dir)
                .with_stdin("x".repeat(8 * 1024 * 1024));
        let execution = tokio::spawn(async move { sandbox.execute(command).await });
        let pid = wait_for_pid(&owner_pid)
            .await
            .map_err(|error| format!("stdin owner pid barrier failed: {error}"))?;
        execution.abort();
        let join = execution.await;
        assert!(matches!(join, Err(error) if error.is_cancelled()));
        verify_process_group_absent(pid).await?;
        std::fs::remove_dir_all(temp_dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_failure_never_emits_fake_timeout_completion() {
        let (events, mut receiver) = mpsc::channel(1);
        let terminal = local_timeout_terminal(
            Err("process group still exists".to_string()),
            RetainedPipeOutput::new(),
            RetainedPipeOutput::new(),
            std::time::Duration::from_secs(1),
            "local",
            std::time::Duration::from_secs(1),
            &events,
        )
        .await;
        emit_local_terminal_failure(&events, &terminal).await;
        assert!(matches!(
            &terminal,
            LocalChildTerminal::Unsettled(SandboxError::IoError(message), _)
                if message.contains("timed out")
                    && message.contains("process group still exists")
        ));
        assert!(matches!(
            receiver.try_recv(),
            Ok(SandboxStreamEvent::Failed {
                failure: super::super::SandboxStreamFailure::IoError { message }
            }) if message.contains("timed out")
                && message.contains("process group still exists")
        ));
    }

    #[test]
    fn read_failure_preserves_primary_and_cleanup_debt() {
        let terminal = local_failure_after_cleanup(
            "failed to read stdout".to_string(),
            Err("process group still exists".to_string()),
        );
        assert!(matches!(
            terminal,
            LocalChildTerminal::Unsettled(SandboxError::IoError(message), _)
                if message.contains("failed to read stdout")
                    && message.contains("process group still exists")
        ));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn every_local_execution_path_fails_closed_without_job_object_owner() {
        let sandbox = LocalSandbox::new(LocalConfig {
            enable_os_sandbox: false,
            ..Default::default()
        });
        assert!(!sandbox.is_available().await);
        assert!(!sandbox.supports_streaming());
        let ordinary = sandbox
            .execute(SandboxCommand::shell("echo must-not-run"))
            .await;
        let streaming = sandbox
            .execute_stream(SandboxCommand::shell("echo must-not-run"))
            .await;
        let controlled = sandbox
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("echo must-not-run"),
                ResourceLimits::default(),
                Some(Arc::new(CancellationToken::new())),
            )
            .await;
        for result in [ordinary, controlled] {
            assert!(matches!(
                result,
                Err(echo_core::error::ReactError::Sandbox(error))
                    if matches!(&*error, SandboxError::Unavailable(message)
                        if message.contains("Job Object"))
            ));
        }
        assert!(matches!(
            streaming,
            Err(echo_core::error::ReactError::Sandbox(error))
                if matches!(&*error, SandboxError::Unavailable(message)
                    if message.contains("Job Object"))
        ));
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
    /// 会让 spawn 直接 EINVAL(`os error 22`),desktop UI 任何代码执行都失败。现在带 limits
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
    /// 验证 desktop UI 用例 `print('hello from sandbox')` 能成功执行 —— 之前 macOS 上传
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
