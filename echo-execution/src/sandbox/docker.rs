//! Docker 容器沙箱执行器
//!
//! 通过 Docker CLI 实现容器级隔离：
//! - 自动创建临时容器
//! - cgroups 资源限制
//! - 网络隔离
//! - 挂载控制
//! - 显式 label + 清理路径，避免孤立容器长期残留
//!
//! 需要宿主机安装 Docker Engine。

use super::{
    CommandKind, ExecutionResult, IsolationLevel, ResourceLimits, SENSITIVE_MOUNT_PATHS,
    SandboxCommand, SandboxExecutor, select_image_for_command,
};
use echo_core::error::Result;
use echo_core::error::SandboxError;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const SANDBOX_LABEL: &str = "echo-sandbox=true";
const DOCKER_CACHE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_DOCKER_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_DOCKER_OUTPUT_BYTES: usize = 1024 * 1024;
const DOCKER_CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(50);
const DOCKER_CLEANUP_RETRY_ATTEMPTS: usize = 3;

/// Docker 沙箱配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// 默认镜像
    pub default_image: String,
    /// 语言与镜像映射
    pub language_images: std::collections::HashMap<String, String>,
    /// 是否移除已完成的容器
    pub auto_remove: bool,
    /// 是否禁用网络
    pub disable_network: bool,
    /// 内存限制（字节），默认 256MB
    pub memory_limit: Option<u64>,
    /// CPU 配额（纳秒/周期），如 50000 = 50%
    pub cpu_quota: Option<u64>,
    /// 只读根文件系统
    pub read_only_rootfs: bool,
    /// 额外的 Docker create 参数。仅允许 user/hostname/platform/entrypoint。
    pub extra_args: Vec<String>,
}

const ALLOWED_DOCKER_EXTRA_OPTIONS: &[&str] =
    &["--entrypoint", "--hostname", "--platform", "--user"];

impl Default for DockerConfig {
    fn default() -> Self {
        let mut language_images = std::collections::HashMap::new();
        language_images.insert("python".to_string(), "python:3.12-slim".to_string());
        language_images.insert("python3".to_string(), "python:3.12-slim".to_string());
        language_images.insert("node".to_string(), "node:20-slim".to_string());
        language_images.insert("javascript".to_string(), "node:20-slim".to_string());
        language_images.insert("ruby".to_string(), "ruby:3.3-slim".to_string());
        language_images.insert("go".to_string(), "golang:1.22-alpine".to_string());
        language_images.insert("rust".to_string(), "rust:1.77-slim".to_string());

        Self {
            default_image: "ubuntu:22.04".to_string(),
            language_images,
            auto_remove: true,
            disable_network: true,
            memory_limit: Some(256 * 1024 * 1024), // 256 MB
            cpu_quota: Some(50_000),
            read_only_rootfs: true,
            extra_args: vec![],
        }
    }
}

/// Docker 容器沙箱
#[derive(Debug, Clone)]
pub struct DockerSandbox {
    config: DockerConfig,
    docker_program: PathBuf,
    control_timeout: Duration,
    availability_cache: Arc<Mutex<Option<(Instant, bool)>>>,
}

struct DockerOwnerRequest {
    command: SandboxCommand,
    limits: Option<ResourceLimits>,
    cancel: Option<Arc<CancellationToken>>,
    timeout: Duration,
    container_name: String,
    create_args: Vec<String>,
    caller_abandoned: CancellationToken,
    max_output_bytes: usize,
}

struct DockerCallerAbandonmentGuard {
    token: CancellationToken,
    armed: bool,
}

impl DockerCallerAbandonmentGuard {
    fn new(token: CancellationToken) -> Self {
        Self { token, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DockerCallerAbandonmentGuard {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}

#[derive(Clone, Copy)]
enum DockerInterruption {
    TimedOut,
    Cancelled,
}

enum DockerStdinOutcome {
    Written(std::io::Result<()>),
    TimedOut,
    Cancelled,
}

enum DockerWaitOutcome {
    Completed(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

#[derive(Default)]
struct DockerPipeCapture {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
}

enum DockerCliStage {
    Completed {
        status: std::process::ExitStatus,
        stdout: DockerPipeCapture,
        stderr: DockerPipeCapture,
    },
    Cancelled {
        output: DockerPipeOutput,
        cleanup: std::result::Result<(), String>,
    },
    TimedOut {
        output: DockerPipeOutput,
        cleanup: std::result::Result<(), String>,
    },
}

type DockerPipeReader = Option<tokio::task::JoinHandle<std::io::Result<DockerPipeCapture>>>;
type DockerPipeOutput = std::result::Result<(DockerPipeCapture, DockerPipeCapture), String>;

impl DockerSandbox {
    pub fn new(config: DockerConfig) -> Self {
        Self {
            config,
            docker_program: PathBuf::from("docker"),
            control_timeout: DEFAULT_DOCKER_CONTROL_TIMEOUT,
            availability_cache: Arc::new(Mutex::new(None)),
        }
    }

    fn cached_availability(&self) -> Option<bool> {
        if let Ok(guard) = self.availability_cache.lock()
            && let Some((checked_at, available)) = *guard
            && checked_at.elapsed() < DOCKER_CACHE_TTL
        {
            Some(available)
        } else {
            None
        }
    }

    fn record_availability(&self, available: bool) {
        if let Ok(mut guard) = self.availability_cache.lock() {
            *guard = Some((Instant::now(), available));
        }
    }

    async fn check_docker_controlled(
        &self,
        cancel: Option<&Arc<CancellationToken>>,
        caller_abandoned: Option<&CancellationToken>,
    ) -> Result<Option<bool>> {
        if let Some(available) = self.cached_availability() {
            return Ok(Some(available));
        }
        let available = self
            .probe_docker_controlled(cancel, caller_abandoned)
            .await?;
        if let Some(available) = available {
            self.record_availability(available);
        }
        Ok(available)
    }

    /// 检测 Docker 是否安装且可用（带实例共享短 TTL 缓存）
    async fn check_docker(&self) -> bool {
        matches!(
            self.check_docker_controlled(None, None).await,
            Ok(Some(true))
        )
    }

    async fn probe_docker_controlled(
        &self,
        cancel: Option<&Arc<CancellationToken>>,
        caller_abandoned: Option<&CancellationToken>,
    ) -> Result<Option<bool>> {
        match run_docker_cli_stage(
            &self.docker_program,
            &["info".to_string()],
            self.control_timeout,
            cancel,
            caller_abandoned,
            8 * 1024,
        )
        .await
        {
            Ok(DockerCliStage::Completed { status, .. }) => Ok(Some(status.success())),
            Ok(DockerCliStage::Cancelled { cleanup, output }) => {
                if cleanup.is_ok() && output.is_ok() {
                    Ok(None)
                } else {
                    Err(docker_control_failure(
                        "Docker availability probe was cancelled",
                        cleanup,
                        output,
                    ))
                }
            }
            Ok(DockerCliStage::TimedOut { cleanup, output }) => Err(docker_control_failure(
                "Docker availability probe timed out",
                cleanup,
                output,
            )),
            Err(error) => Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!(
                    "Docker availability probe failed: {}",
                    bounded_docker_fact(&error)
                )),
            ))),
        }
    }

    fn docker_command(&self) -> Command {
        Command::new(&self.docker_program)
    }

    #[cfg(all(test, unix))]
    fn with_program(config: DockerConfig, docker_program: PathBuf) -> Self {
        Self {
            config,
            docker_program,
            control_timeout: Duration::from_millis(100),
            availability_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// 为命令选择合适的镜像
    fn select_image(&self, command: &SandboxCommand) -> String {
        select_image_for_command(
            command,
            &self.config.language_images,
            &self.config.default_image,
        )
    }

    /// 构建 docker create 命令行参数（不含 `docker` 可执行文件本身）。
    ///
    /// 这里直接面向 `docker create` 组装参数，而不是先拼 `docker run`
    /// 再反推 image / command 边界，避免 create/start 路径漂移。
    fn build_docker_create_args(
        &self,
        command: &SandboxCommand,
        limits: Option<&ResourceLimits>,
        container_name: &str,
    ) -> Result<Vec<String>> {
        let mut args = vec!["create".to_string()];
        args.push("--label".to_string());
        args.push(SANDBOX_LABEL.to_string());
        args.push("--name".to_string());
        args.push(container_name.to_string());

        // 网络隔离
        let network_allowed = limits
            .map(|l| l.network)
            .unwrap_or(!self.config.disable_network);
        if !network_allowed {
            args.push("--network=none".to_string());
        }

        // 内存限制
        let mem = limits
            .and_then(|l| l.memory_bytes)
            .or(self.config.memory_limit);
        if let Some(mem) = mem {
            args.push(format!("--memory={mem}"));
            args.push(format!("--memory-swap={mem}")); // 禁用 swap
        }

        // CPU 限制
        if let Some(quota) = self.config.cpu_quota {
            args.push(format!("--cpu-quota={quota}"));
        }

        // 进程限制
        if let Some(limits) = limits
            && let Some(max_procs) = limits.max_processes
        {
            args.push(format!("--pids-limit={max_procs}"));
        }

        // 只读根文件系统
        if self.config.read_only_rootfs {
            args.push("--read-only".to_string());
            // 需要 tmpfs 给 /tmp
            args.push("--tmpfs=/tmp:rw,noexec,nosuid,size=64m".to_string());
        }

        // 安全选项：移除所有 capabilities
        args.push("--cap-drop=ALL".to_string());
        // 禁止提权
        args.push("--security-opt=no-new-privileges".to_string());
        // 防止僵尸进程累积
        args.push("--init".to_string());
        // 防止容器意外重启
        args.push("--restart=no".to_string());
        // 限制资源使用上限
        args.push("--ulimit=nofile=256:512".to_string());
        args.push("--ulimit=nproc=64:128".to_string());

        // 环境变量
        for (k, v) in &command.env {
            args.push("-e".to_string());
            args.push(format!("{k}={v}"));
        }

        // 挂载卷（limits 中的路径），需验证安全性
        if let Some(limits) = limits {
            for path in &limits.read_only_paths {
                Self::validate_mount_paths(std::slice::from_ref(path)).map_err(|e| {
                    echo_core::error::ReactError::Sandbox(Box::new(SandboxError::PermissionDenied(
                        e,
                    )))
                })?;
                args.push("-v".to_string());
                args.push(format!("{}:{}:ro", path.display(), path.display()));
            }
            for path in &limits.writable_paths {
                Self::validate_mount_paths(std::slice::from_ref(path)).map_err(|e| {
                    echo_core::error::ReactError::Sandbox(Box::new(SandboxError::PermissionDenied(
                        e,
                    )))
                })?;
                args.push("-v".to_string());
                args.push(format!("{}:{}", path.display(), path.display()));
            }
        }

        // 工作目录
        if let Some(ref dir) = command.working_dir {
            args.push("-w".to_string());
            args.push(dir.display().to_string());
        }

        args.extend(
            validate_docker_extra_args(&self.config.extra_args).map_err(|error| {
                echo_core::error::ReactError::Sandbox(Box::new(SandboxError::PermissionDenied(
                    error,
                )))
            })?,
        );

        // 镜像
        args.push(self.select_image(command));
        args.extend(Self::build_inner_command(command));

        Ok(args)
    }

    /// 验证挂载路径，拒绝敏感目录
    fn validate_mount_paths(paths: &[std::path::PathBuf]) -> std::result::Result<(), String> {
        for path in paths {
            let path_str = path.display().to_string();
            for sensitive in SENSITIVE_MOUNT_PATHS.iter() {
                if path_str == *sensitive || path_str.starts_with(&format!("{sensitive}/")) {
                    return Err(format!(
                        "Mount path '{}' accesses sensitive directory '{}'",
                        path_str, sensitive
                    ));
                }
            }
        }
        Ok(())
    }

    /// 构建容器内的执行命令
    fn build_inner_command(command: &SandboxCommand) -> Vec<String> {
        match &command.kind {
            CommandKind::Shell(cmd) => vec!["sh".to_string(), "-c".to_string(), cmd.clone()],
            CommandKind::Program { program, args } => {
                let mut v = vec![program.clone()];
                v.extend(args.clone());
                v
            }
            CommandKind::Code { language, code } => {
                let (interpreter, flag) = match language.as_str() {
                    "python" | "python3" => ("python3", "-c"),
                    "node" | "javascript" | "js" => ("node", "-e"),
                    "ruby" => ("ruby", "-e"),
                    // Sprint 10b: R is a first-class language. Without this
                    // arm R silently fell through to ("sh","-c") and was
                    // mis-run as shell (no error, wrong interpreter).
                    // Image `rocker/r-base:latest` is mapped in mod.rs:84;
                    // a missing image surfaces as Docker's ImageNotFound.
                    "r" => ("Rscript", "-e"),
                    "perl" => ("perl", "-e"),
                    "php" => ("php", "-r"),
                    _ => ("sh", "-c"),
                };
                vec![interpreter.to_string(), flag.to_string(), code.clone()]
            }
        }
    }

    /// 清理所有带有 echo-sandbox 标签的容器
    pub async fn cleanup_sandbox_containers() -> Result<()> {
        Self::cleanup_sandbox_containers_with_program(
            Path::new("docker"),
            DEFAULT_DOCKER_CONTROL_TIMEOUT,
        )
        .await
    }

    async fn cleanup_sandbox_containers_with_program(
        program: &Path,
        control_timeout: Duration,
    ) -> Result<()> {
        let args = vec![
            "ps".to_string(),
            "-aq".to_string(),
            "--filter".to_string(),
            format!("label={SANDBOX_LABEL}"),
        ];
        let (status, stdout, stderr) = match run_docker_cli_stage(
            program,
            &args,
            control_timeout,
            None,
            None,
            64 * 1024,
        )
        .await
        {
            Ok(DockerCliStage::Completed {
                status,
                stdout,
                stderr,
            }) => (status, stdout, stderr),
            Ok(DockerCliStage::TimedOut { cleanup, output }) => {
                return docker_control_error(
                    "listing sandbox containers timed out",
                    cleanup,
                    output,
                );
            }
            Ok(DockerCliStage::Cancelled { cleanup, output }) => {
                return docker_control_error(
                    "listing sandbox containers was cancelled",
                    cleanup,
                    output,
                );
            }
            Err(error) => {
                return Err(echo_core::error::ReactError::Sandbox(Box::new(
                    SandboxError::IoError(format!(
                        "Failed to list sandbox containers: {}",
                        bounded_docker_fact(&error)
                    )),
                )));
            }
        };
        if !status.success() {
            return Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!(
                    "Failed to list sandbox containers ({}): {}",
                    format_exit_status(&status),
                    bounded_docker_fact(String::from_utf8_lossy(&stderr.bytes).trim())
                )),
            )));
        }

        let containers = String::from_utf8_lossy(&stdout.bytes);
        let mut container_ids = containers.lines().collect::<Vec<_>>();
        if stdout.truncated && !stdout.bytes.ends_with(b"\n") {
            let _ = container_ids.pop();
        }
        let mut failures = Vec::new();
        for container_id in container_ids.into_iter().filter(|id| !id.trim().is_empty()) {
            if let Err(error) =
                Self::remove_container_with_program(program, container_id, control_timeout).await
            {
                failures.push(format!(
                    "{}: {}",
                    bounded_docker_fact(container_id),
                    bounded_docker_fact(&error.to_string())
                ));
            }
        }
        if stdout.truncated {
            failures.push(format!(
                "container listing was truncated after {} logical bytes",
                stdout.total_bytes
            ));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            let failure_count = failures.len();
            Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!(
                    "Failed to clean {failure_count} sandbox container(s): {}",
                    bounded_docker_fact(&failures.join("; "))
                )),
            )))
        }
    }

    async fn remove_container(&self, container_id: &str) -> Result<()> {
        let mut last_error = None;
        for attempt in 0..DOCKER_CLEANUP_RETRY_ATTEMPTS {
            match Self::remove_container_with_program(
                &self.docker_program,
                container_id,
                self.control_timeout,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
            if attempt.saturating_add(1) < DOCKER_CLEANUP_RETRY_ATTEMPTS {
                tokio::time::sleep(DOCKER_CLEANUP_RETRY_DELAY).await;
            }
        }
        Err(last_error.unwrap_or_else(|| {
            echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(
                "Docker cleanup exhausted without an attempt result".to_string(),
            )))
        }))
    }

    async fn remove_container_with_program(
        program: &Path,
        container_id: &str,
        control_timeout: Duration,
    ) -> Result<()> {
        let args = vec!["rm".to_string(), "-f".to_string(), container_id.to_string()];
        let (status, stderr) = match run_docker_cli_stage(
            program,
            &args,
            control_timeout,
            None,
            None,
            64 * 1024,
        )
        .await
        {
            Ok(DockerCliStage::Completed { status, stderr, .. }) => (status, stderr),
            Ok(DockerCliStage::TimedOut { cleanup, output }) => {
                return docker_control_error(
                    &format!("Docker container cleanup timed out for {container_id}"),
                    cleanup,
                    output,
                );
            }
            Ok(DockerCliStage::Cancelled { cleanup, output }) => {
                return docker_control_error(
                    &format!("Docker container cleanup was cancelled for {container_id}"),
                    cleanup,
                    output,
                );
            }
            Err(error) => {
                return Err(echo_core::error::ReactError::Sandbox(Box::new(
                    SandboxError::IoError(format!(
                        "Failed to start docker container cleanup for {container_id}: {}",
                        bounded_docker_fact(&error)
                    )),
                )));
            }
        };
        if !status.success() {
            return Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!(
                    "Docker container cleanup failed for {container_id} ({}): {}",
                    format_exit_status(&status),
                    bounded_docker_fact(String::from_utf8_lossy(&stderr.bytes).trim())
                )),
            )));
        }
        Ok(())
    }

    async fn finish_with_cleanup(
        &self,
        container_name: &str,
        outcome: Result<ExecutionResult>,
    ) -> Result<ExecutionResult> {
        match (outcome, self.remove_container(container_name).await) {
            (outcome, Ok(())) => outcome,
            (Ok(primary), Err(cleanup)) => Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!(
                    "Docker terminal [{}]; container cleanup also failed: {}",
                    docker_result_facts(&primary),
                    bounded_docker_fact(&cleanup.to_string())
                )),
            ))),
            (Err(primary), Err(cleanup)) => Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::IoError(format!(
                    "Docker execution failed: {}; container cleanup also failed: {}",
                    bounded_docker_fact(&primary.to_string()),
                    bounded_docker_fact(&cleanup.to_string())
                )),
            ))),
        }
    }

    async fn execute_with_limits_controlled(
        &self,
        command: SandboxCommand,
        limits: Option<ResourceLimits>,
        cancel: Option<Arc<CancellationToken>>,
    ) -> Result<ExecutionResult> {
        if cancel.as_ref().is_some_and(|token| token.is_cancelled()) {
            return Ok(empty_docker_interrupted_result(
                DockerInterruption::Cancelled,
                Duration::ZERO,
            ));
        }

        let available = self.check_docker_controlled(cancel.as_ref(), None).await?;
        if available.is_none() {
            return Ok(empty_docker_interrupted_result(
                DockerInterruption::Cancelled,
                Duration::ZERO,
            ));
        }
        let available = available.unwrap_or(false);
        if !available {
            return Err(echo_core::error::ReactError::Sandbox(Box::new(
                SandboxError::Unavailable("Docker is not available".to_string()),
            )));
        }

        let timeout = limits
            .as_ref()
            .and_then(|value| value.cpu_time_secs)
            .map(Duration::from_secs)
            .unwrap_or(command.timeout);
        let container_name = format!("echo-sandbox-{}", uuid::Uuid::new_v4().simple());
        let create_args =
            self.build_docker_create_args(&command, limits.as_ref(), &container_name)?;
        let max_output_bytes = limits
            .as_ref()
            .and_then(|limits| limits.max_output_bytes)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(DEFAULT_DOCKER_OUTPUT_BYTES);
        let caller_abandoned = CancellationToken::new();
        let mut caller_guard = DockerCallerAbandonmentGuard::new(caller_abandoned.clone());
        let recovery_name = container_name.clone();
        let owner = self.clone();
        let task = tokio::spawn(async move {
            owner
                .run_container_owner(DockerOwnerRequest {
                    command,
                    limits,
                    cancel,
                    timeout,
                    container_name,
                    create_args,
                    caller_abandoned,
                    max_output_bytes,
                })
                .await
        });
        let outcome = match task.await {
            Ok(outcome) => outcome,
            Err(error) => {
                self.finish_with_cleanup(
                    &recovery_name,
                    Err(echo_core::error::ReactError::Sandbox(Box::new(
                        SandboxError::IoError(format!(
                            "Docker lifecycle owner failed to join: {error}"
                        )),
                    ))),
                )
                .await
            }
        };
        caller_guard.disarm();
        outcome
    }

    async fn run_container_owner(&self, request: DockerOwnerRequest) -> Result<ExecutionResult> {
        let DockerOwnerRequest {
            command,
            limits,
            cancel,
            timeout,
            container_name,
            create_args,
            caller_abandoned,
            max_output_bytes,
        } = request;
        let start = Instant::now();
        if docker_cancelled(cancel.as_ref(), Some(&caller_abandoned)) {
            return Ok(empty_docker_interrupted_result(
                DockerInterruption::Cancelled,
                start.elapsed(),
            ));
        }

        let create = run_docker_cli_stage(
            &self.docker_program,
            &create_args,
            self.control_timeout,
            cancel.as_ref(),
            Some(&caller_abandoned),
            64 * 1024,
        )
        .await;
        let (status, stdout, stderr) = match create {
            Ok(DockerCliStage::Completed {
                status,
                stdout,
                stderr,
            }) => (status, stdout, stderr),
            Ok(DockerCliStage::Cancelled { output, cleanup }) => {
                let terminal = docker_interrupted_terminal(
                    DockerInterruption::Cancelled,
                    start.elapsed(),
                    cleanup,
                    output,
                    max_output_bytes,
                );
                return self.finish_with_cleanup(&container_name, terminal).await;
            }
            Ok(DockerCliStage::TimedOut { output, cleanup }) => {
                let terminal =
                    docker_client_failure("docker create control stage timed out", cleanup, output);
                return self.finish_with_cleanup(&container_name, terminal).await;
            }
            Err(error) => {
                return self
                    .finish_with_cleanup(
                        &container_name,
                        Err(echo_core::error::ReactError::Sandbox(Box::new(
                            SandboxError::StartFailed(format!(
                                "Failed to create docker container: {}",
                                bounded_docker_fact(&error)
                            )),
                        ))),
                    )
                    .await;
            }
        };

        let container_id = String::from_utf8_lossy(&stdout.bytes).trim().to_string();
        if docker_cancelled(cancel.as_ref(), Some(&caller_abandoned)) {
            return self
                .finish_with_cleanup(
                    &container_name,
                    Ok(docker_interrupted_result(
                        DockerInterruption::Cancelled,
                        start.elapsed(),
                        stdout,
                        stderr,
                    )),
                )
                .await;
        }
        if !status.success() {
            return self
                .finish_with_cleanup(
                    &container_name,
                    Err(echo_core::error::ReactError::Sandbox(Box::new(
                        SandboxError::StartFailed(format!(
                            "Docker container creation failed ({}): {}",
                            format_exit_status(&status),
                            bounded_docker_fact(String::from_utf8_lossy(&stderr.bytes).trim())
                        )),
                    ))),
                )
                .await;
        }
        if !valid_docker_container_id(&container_id) {
            return self
                .finish_with_cleanup(
                    &container_name,
                    Err(echo_core::error::ReactError::Sandbox(Box::new(
                        SandboxError::StartFailed(format!(
                            "Docker create returned an invalid container ID: {}",
                            bounded_docker_fact(&container_id)
                        )),
                    ))),
                )
                .await;
        }

        let mut start_cmd = self.docker_command();
        let attach_flag = if command.stdin.is_some() { "-ai" } else { "-a" };
        start_cmd
            .args(["start", attach_flag, &container_name])
            .stdin(if command.stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        start_cmd.process_group(0);
        let mut child = match start_cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                return self
                    .finish_with_cleanup(
                        &container_name,
                        Err(echo_core::error::ReactError::Sandbox(Box::new(
                            SandboxError::StartFailed(format!(
                                "Failed to start docker container: {error}"
                            )),
                        ))),
                    )
                    .await;
            }
        };
        let client_process_group_id = child.id();
        let output_budget = Arc::new(parking_lot::Mutex::new(0_usize));
        let stdout_reader =
            spawn_docker_pipe_reader(child.stdout.take(), output_budget.clone(), max_output_bytes);
        let stderr_reader =
            spawn_docker_pipe_reader(child.stderr.take(), output_budget, max_output_bytes);
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        if let Some(input) = command.stdin {
            let Some(mut stdin) = child.stdin.take() else {
                let client_cleanup =
                    cleanup_docker_client(&mut child, client_process_group_id).await;
                let output = collect_docker_pipes(stdout_reader, stderr_reader).await;
                let terminal = docker_client_failure(
                    "docker start did not provide the requested stdin pipe",
                    client_cleanup,
                    output,
                );
                return self.finish_with_cleanup(&container_name, terminal).await;
            };
            let stdin_outcome = tokio::select! {
                _ = wait_for_docker_cancel(cancel.as_ref(), Some(&caller_abandoned)) => DockerStdinOutcome::Cancelled,
                _ = &mut deadline => DockerStdinOutcome::TimedOut,
                result = stdin.write_all(input.as_bytes()) => DockerStdinOutcome::Written(result),
            };
            drop(stdin);
            match stdin_outcome {
                DockerStdinOutcome::Written(Ok(())) => {}
                DockerStdinOutcome::Written(Err(error)) => {
                    let client_cleanup =
                        cleanup_docker_client(&mut child, client_process_group_id).await;
                    let output = collect_docker_pipes(stdout_reader, stderr_reader).await;
                    let terminal = docker_client_failure(
                        &format!("failed to write docker stdin: {error}"),
                        client_cleanup,
                        output,
                    );
                    return self.finish_with_cleanup(&container_name, terminal).await;
                }
                interrupted @ (DockerStdinOutcome::TimedOut | DockerStdinOutcome::Cancelled) => {
                    let interruption = match interrupted {
                        DockerStdinOutcome::TimedOut => DockerInterruption::TimedOut,
                        _ => DockerInterruption::Cancelled,
                    };
                    let client_cleanup =
                        cleanup_docker_client(&mut child, client_process_group_id).await;
                    let output = collect_docker_pipes(stdout_reader, stderr_reader).await;
                    let terminal = docker_interrupted_terminal(
                        interruption,
                        start.elapsed(),
                        client_cleanup,
                        output,
                        max_output_bytes,
                    );
                    return self.finish_with_cleanup(&container_name, terminal).await;
                }
            }
        }

        let wait_outcome = tokio::select! {
            _ = wait_for_docker_cancel(cancel.as_ref(), Some(&caller_abandoned)) => DockerWaitOutcome::Cancelled,
            _ = &mut deadline => DockerWaitOutcome::TimedOut,
            result = child.wait() => DockerWaitOutcome::Completed(result),
        };
        let terminal = match wait_outcome {
            DockerWaitOutcome::Completed(status) => {
                let output = collect_docker_pipes(stdout_reader, stderr_reader).await;
                docker_completed_terminal(status, output, start.elapsed(), limits.as_ref())
            }
            interrupted @ (DockerWaitOutcome::TimedOut | DockerWaitOutcome::Cancelled) => {
                let interruption = match interrupted {
                    DockerWaitOutcome::TimedOut => DockerInterruption::TimedOut,
                    _ => DockerInterruption::Cancelled,
                };
                let client_cleanup =
                    cleanup_docker_client(&mut child, client_process_group_id).await;
                let output = collect_docker_pipes(stdout_reader, stderr_reader).await;
                docker_interrupted_terminal(
                    interruption,
                    start.elapsed(),
                    client_cleanup,
                    output,
                    max_output_bytes,
                )
            }
        };
        self.finish_with_cleanup(&container_name, terminal).await
    }
}

async fn run_docker_cli_stage(
    program: &Path,
    args: &[String],
    timeout: Duration,
    cancel: Option<&Arc<CancellationToken>>,
    caller_abandoned: Option<&CancellationToken>,
    max_output_bytes: usize,
) -> std::result::Result<DockerCliStage, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn Docker CLI: {error}"))?;
    let process_group_id = child.id();
    let output_budget = Arc::new(parking_lot::Mutex::new(0_usize));
    let stdout =
        spawn_docker_pipe_reader(child.stdout.take(), output_budget.clone(), max_output_bytes);
    let stderr = spawn_docker_pipe_reader(child.stderr.take(), output_budget, max_output_bytes);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let outcome = tokio::select! {
        _ = wait_for_docker_cancel(cancel, caller_abandoned) => DockerWaitOutcome::Cancelled,
        _ = &mut deadline => DockerWaitOutcome::TimedOut,
        result = child.wait() => DockerWaitOutcome::Completed(result),
    };
    match outcome {
        DockerWaitOutcome::Completed(Ok(status)) => {
            let (stdout, stderr) = collect_docker_pipes(stdout, stderr).await?;
            Ok(DockerCliStage::Completed {
                status,
                stdout,
                stderr,
            })
        }
        DockerWaitOutcome::Completed(Err(error)) => {
            let cleanup = cleanup_docker_client(&mut child, process_group_id).await;
            let output = collect_docker_pipes(stdout, stderr).await;
            Err(format!(
                "failed to wait for Docker CLI: {}; cleanup={}; output={}",
                bounded_docker_fact(&error.to_string()),
                cleanup
                    .err()
                    .map(|error| bounded_docker_fact(&error))
                    .unwrap_or_else(|| "ok".to_string()),
                docker_pipe_output_facts(&output)
            ))
        }
        DockerWaitOutcome::Cancelled => {
            let cleanup = cleanup_docker_client(&mut child, process_group_id).await;
            let output = collect_docker_pipes(stdout, stderr).await;
            Ok(DockerCliStage::Cancelled { output, cleanup })
        }
        DockerWaitOutcome::TimedOut => {
            let cleanup = cleanup_docker_client(&mut child, process_group_id).await;
            let output = collect_docker_pipes(stdout, stderr).await;
            Ok(DockerCliStage::TimedOut { output, cleanup })
        }
    }
}

fn docker_cancelled(
    cancel: Option<&Arc<CancellationToken>>,
    caller_abandoned: Option<&CancellationToken>,
) -> bool {
    cancel.is_some_and(|cancel| cancel.is_cancelled())
        || caller_abandoned.is_some_and(CancellationToken::is_cancelled)
}

async fn wait_for_docker_cancel(
    cancel: Option<&Arc<CancellationToken>>,
    caller_abandoned: Option<&CancellationToken>,
) {
    match (cancel, caller_abandoned) {
        (Some(cancel), Some(caller_abandoned)) => {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = caller_abandoned.cancelled() => {}
            }
        }
        (Some(cancel), None) => cancel.cancelled().await,
        (None, Some(caller_abandoned)) => caller_abandoned.cancelled().await,
        (None, None) => std::future::pending().await,
    }
}

fn spawn_docker_pipe_reader<R>(
    pipe: Option<R>,
    retained_budget: Arc<parking_lot::Mutex<usize>>,
    max_output_bytes: usize,
) -> DockerPipeReader
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    pipe.map(|mut pipe| {
        tokio::spawn(async move {
            let mut capture = DockerPipeCapture::default();
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                let count = pipe.read(&mut buffer).await?;
                if count == 0 {
                    return Ok(capture);
                }
                capture.total_bytes = capture
                    .total_bytes
                    .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
                let mut retained = retained_budget.lock();
                let available = max_output_bytes.saturating_sub(*retained);
                let keep = available.min(count);
                if let Some(bytes) = buffer.get(..keep) {
                    capture.bytes.extend_from_slice(bytes);
                }
                *retained = retained.saturating_add(keep);
                capture.truncated |= keep < count;
            }
        })
    })
}

async fn collect_docker_pipes(
    stdout: DockerPipeReader,
    stderr: DockerPipeReader,
) -> DockerPipeOutput {
    let stdout = collect_docker_pipe("stdout", stdout).await?;
    let stderr = collect_docker_pipe("stderr", stderr).await?;
    Ok((stdout, stderr))
}

async fn collect_docker_pipe(
    name: &str,
    reader: DockerPipeReader,
) -> std::result::Result<DockerPipeCapture, String> {
    let Some(reader) = reader else {
        return Ok(DockerPipeCapture::default());
    };
    reader
        .await
        .map_err(|error| format!("docker {name} reader failed to join: {error}"))?
        .map_err(|error| format!("failed to read docker {name}: {error}"))
}

async fn cleanup_docker_client(
    child: &mut tokio::process::Child,
    process_group_id: Option<u32>,
) -> std::result::Result<(), String> {
    super::local::cleanup_child_process(child, process_group_id).await
}

fn docker_control_failure(
    primary: &str,
    cleanup: std::result::Result<(), String>,
    output: DockerPipeOutput,
) -> echo_core::error::ReactError {
    let cleanup = cleanup
        .err()
        .map(|error| bounded_docker_fact(&error))
        .unwrap_or_else(|| "ok".to_string());
    echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(format!(
        "{}; cleanup={}; output={}",
        bounded_docker_fact(primary),
        cleanup,
        docker_pipe_output_facts(&output)
    ))))
}

fn docker_control_error(
    primary: &str,
    cleanup: std::result::Result<(), String>,
    output: DockerPipeOutput,
) -> Result<()> {
    Err(docker_control_failure(primary, cleanup, output))
}

fn docker_client_failure(
    primary: &str,
    client_cleanup: std::result::Result<(), String>,
    output: DockerPipeOutput,
) -> Result<ExecutionResult> {
    let mut facts = vec![bounded_docker_fact(primary)];
    if let Err(error) = client_cleanup {
        facts.push(format!(
            "docker client cleanup failed: {}",
            bounded_docker_fact(&error)
        ));
    }
    match output {
        Ok((stdout, stderr)) => facts.push(format!(
            "stdout_bytes={}, stderr_bytes={}, output_truncated={}, stderr={}",
            stdout.total_bytes,
            stderr.total_bytes,
            stdout.truncated || stderr.truncated,
            bounded_docker_fact(&String::from_utf8_lossy(&stderr.bytes))
        )),
        Err(error) => facts.push(format!(
            "docker output drain failed: {}",
            bounded_docker_fact(&error)
        )),
    }
    Err(echo_core::error::ReactError::Sandbox(Box::new(
        SandboxError::IoError(facts.join("; ")),
    )))
}

fn docker_interrupted_terminal(
    interruption: DockerInterruption,
    duration: Duration,
    client_cleanup: std::result::Result<(), String>,
    output: DockerPipeOutput,
    max_output_bytes: usize,
) -> Result<ExecutionResult> {
    match (client_cleanup, output) {
        (Ok(()), Ok((stdout, stderr))) => {
            let mut result = docker_interrupted_result(interruption, duration, stdout, stderr);
            result.enforce_output_limit(u64::try_from(max_output_bytes).unwrap_or(u64::MAX));
            Ok(result)
        }
        (client_cleanup, output) => docker_client_failure(
            match interruption {
                DockerInterruption::TimedOut => "docker execution timed out",
                DockerInterruption::Cancelled => "docker execution was cancelled",
            },
            client_cleanup,
            output,
        ),
    }
}

fn docker_completed_terminal(
    status: std::io::Result<std::process::ExitStatus>,
    output: DockerPipeOutput,
    duration: Duration,
    limits: Option<&ResourceLimits>,
) -> Result<ExecutionResult> {
    let status = status.map_err(|error| {
        echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(format!(
            "Docker IO error while waiting for start client: {error}"
        ))))
    })?;
    let (stdout, stderr) = output.map_err(|error| {
        echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(error)))
    })?;
    let mut result = ExecutionResult {
        exit_code: status.code().unwrap_or(-1),
        stdout_bytes: stdout.total_bytes,
        stderr_bytes: stderr.total_bytes,
        stdout: String::from_utf8_lossy(&stdout.bytes).to_string(),
        stderr: String::from_utf8_lossy(&stderr.bytes).to_string(),
        duration,
        sandbox_type: "docker".to_string(),
        timed_out: false,
        cancelled: false,
        output_truncated: stdout.truncated || stderr.truncated,
    };
    if let Some(max_output_bytes) = limits.and_then(|value| value.max_output_bytes) {
        result.enforce_output_limit(max_output_bytes);
    }
    Ok(result)
}

fn docker_interrupted_result(
    interruption: DockerInterruption,
    duration: Duration,
    stdout: DockerPipeCapture,
    stderr: DockerPipeCapture,
) -> ExecutionResult {
    let message = match interruption {
        DockerInterruption::TimedOut => "Docker execution timed out",
        DockerInterruption::Cancelled => "Docker execution cancelled by owning run",
    };
    let stderr_text = if stderr.bytes.is_empty() {
        message.to_string()
    } else {
        String::from_utf8_lossy(&stderr.bytes).to_string()
    };
    ExecutionResult {
        exit_code: -1,
        stdout_bytes: stdout.total_bytes,
        stderr_bytes: stderr.total_bytes,
        stdout: String::from_utf8_lossy(&stdout.bytes).to_string(),
        stderr: stderr_text,
        duration,
        sandbox_type: "docker".to_string(),
        timed_out: matches!(interruption, DockerInterruption::TimedOut),
        cancelled: matches!(interruption, DockerInterruption::Cancelled),
        output_truncated: stdout.truncated || stderr.truncated,
    }
}

fn empty_docker_interrupted_result(
    interruption: DockerInterruption,
    duration: Duration,
) -> ExecutionResult {
    docker_interrupted_result(
        interruption,
        duration,
        DockerPipeCapture::default(),
        DockerPipeCapture::default(),
    )
}

fn docker_pipe_output_facts(output: &DockerPipeOutput) -> String {
    match output {
        Ok((stdout, stderr)) => format!(
            "stdout_bytes={}, stderr_bytes={}, output_truncated={}",
            stdout.total_bytes,
            stderr.total_bytes,
            stdout.truncated || stderr.truncated
        ),
        Err(error) => format!("drain_error={}", bounded_docker_fact(error)),
    }
}

fn docker_result_facts(result: &ExecutionResult) -> String {
    format!(
        "exit_code={}, timed_out={}, cancelled={}, stdout_bytes={}, stderr_bytes={}, stderr={}",
        result.exit_code,
        result.timed_out,
        result.cancelled,
        result.stdout_bytes,
        result.stderr_bytes,
        bounded_docker_fact(&result.stderr)
    )
}

fn bounded_docker_fact(value: &str) -> String {
    const MAX_FACT_CHARS: usize = 1_024;
    let mut fact = value.chars().take(MAX_FACT_CHARS).collect::<String>();
    if value.chars().count() > MAX_FACT_CHARS {
        fact.push_str("...");
    }
    fact
}

fn valid_docker_container_id(value: &str) -> bool {
    (12..=64).contains(&value.len()) && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn validate_docker_extra_args(args: &[String]) -> std::result::Result<Vec<String>, String> {
    let mut validated = Vec::with_capacity(args.len());
    let mut index = 0_usize;
    while let Some(argument) = args.get(index) {
        if let Some((option, value)) = argument.split_once('=') {
            if !ALLOWED_DOCKER_EXTRA_OPTIONS.contains(&option) || value.is_empty() {
                return Err(format!(
                    "Docker extra option is not allowed: {}",
                    bounded_docker_fact(argument)
                ));
            }
            validated.push(argument.clone());
            index = index.saturating_add(1);
            continue;
        }
        if !ALLOWED_DOCKER_EXTRA_OPTIONS.contains(&argument.as_str()) {
            return Err(format!(
                "Docker extra option is not allowed: {}",
                bounded_docker_fact(argument)
            ));
        }
        let value = args
            .get(index.saturating_add(1))
            .ok_or_else(|| format!("Docker extra option requires a value: {argument}"))?;
        if value.is_empty() || value.starts_with('-') {
            return Err(format!(
                "Docker extra option has an invalid value: {} {}",
                bounded_docker_fact(argument),
                bounded_docker_fact(value)
            ));
        }
        validated.push(argument.clone());
        validated.push(value.clone());
        index = index.saturating_add(2);
    }
    Ok(validated)
}

fn format_exit_status(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("exit code {code}"))
        .unwrap_or_else(|| "terminated without an exit code".to_string())
}

impl SandboxExecutor for DockerSandbox {
    fn name(&self) -> &str {
        "docker"
    }

    fn isolation_level(&self) -> IsolationLevel {
        IsolationLevel::Container
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(self.check_docker())
    }

    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(Self::cleanup_sandbox_containers_with_program(
            &self.docker_program,
            self.control_timeout,
        ))
    }

    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(self.execute_with_limits_controlled(command, None, None))
    }

    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(self.execute_with_limits_controlled(command, Some(limits), None))
    }

    fn execute_with_limits_and_cancel(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
        cancel: Option<Arc<CancellationToken>>,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(self.execute_with_limits_controlled(command, Some(limits), cancel))
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    struct FakeDocker {
        directory: PathBuf,
        program: PathBuf,
        log: PathBuf,
    }

    #[cfg(unix)]
    impl FakeDocker {
        fn new(mode: &str) -> std::result::Result<Self, Box<dyn std::error::Error>> {
            use std::os::unix::fs::PermissionsExt;

            let directory = std::env::temp_dir().join(format!(
                "echo-fake-docker-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&directory)?;
            let program = directory.join("docker");
            let log = directory.join("docker.log");
            let script = format!(
                r#"#!/bin/sh
LOG="$0.log"
printf '%s\n' "$1" >> "$LOG"
case "$1" in
  info)
    if [ "{mode}" = "hung-info" ]; then exec sleep 10; fi
    exit 0
    ;;
  create)
    case "{mode}" in
      hung-create) exec sleep 10 ;;
      create-empty) exit 0 ;;
      create-bad) printf 'not-a-container-id\n'; exit 0 ;;
      *) printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n'; exit 0 ;;
    esac
    ;;
  start)
    case "{mode}" in
      normal|cleanup-fail) printf 'completed\n'; exit 0 ;;
      nonzero-cleanup-fail) printf 'command failed\n' >&2; exit 17 ;;
      large-normal) yes x | head -c 262144; exit 0 ;;
      large-timeout|large-cancel)
        yes x | head -c 65536
        printf 'large-ready\n' >> "$LOG"
        exec yes x
        ;;
      timeout|cancel|timeout-cleanup-fail|cancel-cleanup-fail|abort|blocked-stdin) exec sleep 10 ;;
    esac
    ;;
  rm)
    case "{mode}" in
      hung-rm) exec sleep 10 ;;
      global-first-fail)
        if [ "$3" = "first-container" ]; then
          printf 'forced first cleanup failure\n' >&2
          exit 24
        fi
        exit 0
        ;;
      cleanup-fail|*-cleanup-fail)
        printf 'forced cleanup failure\n' >&2
        exit 23
        ;;
      *) exit 0 ;;
    esac
    ;;
  ps)
    if [ "{mode}" = "global-first-fail" ]; then
      printf 'first-container\nsecond-container\n'
    elif [ "{mode}" = "global-truncated" ]; then
      printf 'first-container\n'
      yes a | tr -d '\n' | head -c 70000
    fi
    exit 0
    ;;
esac
exit 64
"#
            );
            std::fs::write(&program, script)?;
            let mut permissions = std::fs::metadata(&program)?.permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&program, permissions)?;
            Ok(Self {
                directory,
                program,
                log,
            })
        }

        fn sandbox(&self) -> DockerSandbox {
            DockerSandbox::with_program(DockerConfig::default(), self.program.clone())
        }

        fn operations(&self) -> std::result::Result<Vec<String>, std::io::Error> {
            std::fs::read_to_string(&self.log).map(|contents| {
                contents
                    .lines()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
        }

        fn operations_or_empty(&self) -> std::result::Result<Vec<String>, std::io::Error> {
            match self.operations() {
                Ok(operations) => Ok(operations),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(error) => Err(error),
            }
        }
    }

    #[cfg(unix)]
    impl Drop for FakeDocker {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[cfg(unix)]
    async fn wait_for_operation(log: &Path, operation: &str) -> std::result::Result<(), String> {
        for _ in 0..100 {
            if let Ok(contents) = tokio::fs::read_to_string(log).await
                && contents.lines().any(|line| line == operation)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(format!("fake docker never observed operation {operation}"))
    }

    #[test]
    fn docker_streaming_is_explicitly_buffered_fallback() {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        assert!(!sandbox.supports_streaming());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn normal_execution_removes_container_before_return()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("normal")?;
        let result = fake
            .sandbox()
            .execute(SandboxCommand::shell("printf completed"))
            .await?;
        assert!(result.success());
        assert_eq!(fake.operations()?, ["info", "create", "start", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn consecutive_execution_reuses_instance_availability_cache()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("normal")?;
        let sandbox = fake.sandbox();
        for _ in 0..2 {
            let result = sandbox
                .execute(SandboxCommand::shell("printf completed"))
                .await?;
            assert!(result.success());
        }
        assert_eq!(
            fake.operations()?,
            ["info", "create", "start", "rm", "create", "start", "rm"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pre_cancel_does_not_probe_or_create_container()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("normal")?;
        let cancel = Arc::new(CancellationToken::new());
        cancel.cancel();
        let result = fake
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("never starts"),
                ResourceLimits::default(),
                Some(cancel),
            )
            .await?;
        assert!(result.cancelled);
        assert!(fake.operations_or_empty()?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn outer_abort_after_start_keeps_detached_cleanup_owner()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("abort")?;
        let sandbox = fake.sandbox();
        let execution = tokio::spawn(async move {
            sandbox
                .execute(
                    SandboxCommand::shell("sleep forever").with_timeout(Duration::from_secs(30)),
                )
                .await
        });
        wait_for_operation(&fake.log, "start")
            .await
            .map_err(|error| format!("docker start barrier failed: {error}"))?;
        execution.abort();
        let join = execution.await;
        assert!(matches!(join, Err(error) if error.is_cancelled()));
        wait_for_operation(&fake.log, "rm")
            .await
            .map_err(|error| format!("detached docker cleanup failed: {error}"))?;
        assert_eq!(fake.operations()?, ["info", "create", "start", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_stdin_cancellation_reaches_owner_cleanup()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("blocked-stdin")?;
        let cancel = Arc::new(CancellationToken::new());
        let cancellation = cancel.clone();
        let log = fake.log.clone();
        let trigger = tokio::spawn(async move {
            wait_for_operation(&log, "start").await?;
            cancellation.cancel();
            Ok::<(), String>(())
        });
        let result = fake
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("does not read stdin")
                    .with_stdin("x".repeat(8 * 1024 * 1024)),
                ResourceLimits::default(),
                Some(cancel),
            )
            .await?;
        trigger
            .await
            .map_err(|error| format!("blocked stdin trigger failed to join: {error}"))?
            .map_err(|error| format!("blocked stdin trigger failed: {error}"))?;
        assert!(result.cancelled);
        assert_eq!(fake.operations()?, ["info", "create", "start", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_or_invalid_create_output_still_uses_named_cleanup_authority()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for mode in ["create-empty", "create-bad"] {
            let fake = FakeDocker::new(mode)?;
            let result = fake
                .sandbox()
                .execute(SandboxCommand::shell("never starts"))
                .await;
            assert!(matches!(
                result,
                Err(echo_core::error::ReactError::Sandbox(error))
                    if matches!(*error, SandboxError::StartFailed(_))
            ));
            assert_eq!(fake.operations()?, ["info", "create", "rm"]);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_removes_container_before_return()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("timeout")?;
        let result = fake
            .sandbox()
            .execute(SandboxCommand::shell("sleep forever").with_timeout(Duration::from_millis(30)))
            .await?;
        assert!(result.timed_out);
        assert_eq!(fake.operations()?, ["info", "create", "start", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_removes_container_before_return()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("cancel")?;
        let cancel = Arc::new(CancellationToken::new());
        let cancellation = cancel.clone();
        let log = fake.log.clone();
        let trigger = tokio::spawn(async move {
            wait_for_operation(&log, "start").await?;
            cancellation.cancel();
            Ok::<(), String>(())
        });
        let result = fake
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("sleep forever"),
                ResourceLimits::default(),
                Some(cancel),
            )
            .await?;
        trigger
            .await
            .map_err(|error| format!("cancellation trigger failed to join: {error}"))?
            .map_err(|error| format!("cancellation trigger failed: {error}"))?;
        assert!(result.cancelled);
        assert_eq!(fake.operations()?, ["info", "create", "start", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_nonzero_status_is_a_typed_terminal_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("cleanup-fail")?;
        let result = fake
            .sandbox()
            .execute(SandboxCommand::shell("printf completed"))
            .await;
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Sandbox(error))
                if matches!(*error, SandboxError::IoError(ref message)
                    if message.contains("exit code 23")
                        && message.contains("forced cleanup failure"))
        ));
        assert_eq!(
            fake.operations()?,
            ["info", "create", "start", "rm", "rm", "rm"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_failure_preserves_nonzero_timeout_and_cancel_facts()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let nonzero = FakeDocker::new("nonzero-cleanup-fail")?;
        let nonzero_error = nonzero
            .sandbox()
            .execute(SandboxCommand::shell("exit 17"))
            .await
            .err()
            .ok_or("nonzero execution with cleanup failure unexpectedly succeeded")?;
        assert!(nonzero_error.to_string().contains("exit_code=17"));
        assert!(nonzero_error.to_string().contains("cleanup"));

        let timed_out = FakeDocker::new("timeout-cleanup-fail")?;
        let timeout_error = timed_out
            .sandbox()
            .execute(SandboxCommand::shell("sleep forever").with_timeout(Duration::from_millis(30)))
            .await
            .err()
            .ok_or("timeout with cleanup failure unexpectedly succeeded")?;
        assert!(timeout_error.to_string().contains("timed_out=true"));
        assert!(timeout_error.to_string().contains("cleanup"));

        let cancelled = FakeDocker::new("cancel-cleanup-fail")?;
        let cancel = Arc::new(CancellationToken::new());
        let cancellation = cancel.clone();
        let log = cancelled.log.clone();
        let trigger = tokio::spawn(async move {
            wait_for_operation(&log, "start").await?;
            cancellation.cancel();
            Ok::<(), String>(())
        });
        let cancel_error = cancelled
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("sleep forever"),
                ResourceLimits::default(),
                Some(cancel),
            )
            .await
            .err()
            .ok_or("cancellation with cleanup failure unexpectedly succeeded")?;
        trigger
            .await
            .map_err(|error| format!("cancel trigger failed to join: {error}"))?
            .map_err(|error| format!("cancel trigger failed: {error}"))?;
        assert!(cancel_error.to_string().contains("cancelled=true"));
        assert!(cancel_error.to_string().contains("cleanup"));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_spawn_failure_is_a_typed_terminal_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!(
            "echo-missing-docker-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let missing_program = directory.join("docker");
        let result = DockerSandbox::remove_container_with_program(
            &missing_program,
            "container-1",
            Duration::from_millis(100),
        )
        .await;
        assert!(matches!(
            result,
            Err(echo_core::error::ReactError::Sandbox(error))
                if matches!(*error, SandboxError::IoError(ref message)
                    if message.contains("Failed to start docker container cleanup"))
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hung_control_stages_are_bounded_and_typed()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let hung_info = FakeDocker::new("hung-info")?;
        let info_started = Instant::now();
        let info_error = hung_info
            .sandbox()
            .execute(SandboxCommand::shell("never starts"))
            .await
            .err()
            .ok_or("hung info unexpectedly succeeded")?;
        assert!(info_started.elapsed() < Duration::from_secs(2));
        assert!(info_error.to_string().contains("probe timed out"));
        assert_eq!(hung_info.operations()?, ["info"]);

        let hung_create = FakeDocker::new("hung-create")?;
        let create_started = Instant::now();
        let create_error = hung_create
            .sandbox()
            .execute(SandboxCommand::shell("never starts"))
            .await
            .err()
            .ok_or("hung create unexpectedly succeeded")?;
        assert!(create_started.elapsed() < Duration::from_secs(2));
        assert!(
            create_error
                .to_string()
                .contains("create control stage timed out")
        );
        assert_eq!(hung_create.operations()?, ["info", "create", "rm"]);

        let hung_rm = FakeDocker::new("hung-rm")?;
        let rm_started = Instant::now();
        let rm_error = hung_rm
            .sandbox()
            .execute(SandboxCommand::shell("completes"))
            .await
            .err()
            .ok_or("hung rm unexpectedly succeeded")?;
        assert!(rm_started.elapsed() < Duration::from_secs(2));
        assert!(rm_error.to_string().contains("cleanup timed out"));
        assert_eq!(
            hung_rm.operations()?,
            ["info", "create", "start", "rm", "rm", "rm"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn global_cleanup_attempts_every_container_before_returning_error()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("global-first-fail")?;
        let error = DockerSandbox::cleanup_sandbox_containers_with_program(
            &fake.program,
            Duration::from_millis(100),
        )
        .await
        .err()
        .ok_or("partial global cleanup unexpectedly succeeded")?;
        assert!(error.to_string().contains("first-container"));
        assert_eq!(fake.operations()?, ["ps", "rm", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn truncated_global_listing_cleans_complete_prefix_but_never_returns_success()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let fake = FakeDocker::new("global-truncated")?;
        let error = DockerSandbox::cleanup_sandbox_containers_with_program(
            &fake.program,
            Duration::from_millis(100),
        )
        .await
        .err()
        .ok_or("truncated global cleanup unexpectedly succeeded")?;
        assert!(error.to_string().contains("listing was truncated"));
        assert_eq!(fake.operations()?, ["ps", "rm"]);
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_budget_is_shared_for_normal_timeout_and_cancel()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let output_limits = || ResourceLimits {
            cpu_time_secs: None,
            max_output_bytes: Some(1024),
            ..ResourceLimits::default()
        };
        let assert_bounded = |result: &ExecutionResult| {
            assert!(
                result.stdout.len().saturating_add(result.stderr.len()) <= 1024,
                "retained Docker output exceeded shared cap"
            );
            assert!(result.stdout_bytes.saturating_add(result.stderr_bytes) > 1024);
            assert!(result.output_truncated);
        };

        let normal = FakeDocker::new("large-normal")?;
        let normal_result = normal
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("large output"),
                output_limits(),
                None,
            )
            .await?;
        assert_bounded(&normal_result);

        let timed_out = FakeDocker::new("large-timeout")?;
        let timeout_result = timed_out
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("large output").with_timeout(Duration::from_millis(200)),
                output_limits(),
                None,
            )
            .await?;
        assert!(timeout_result.timed_out);
        assert_bounded(&timeout_result);

        let cancelled = FakeDocker::new("large-cancel")?;
        let cancel = Arc::new(CancellationToken::new());
        let cancellation = cancel.clone();
        let log = cancelled.log.clone();
        let trigger = tokio::spawn(async move {
            wait_for_operation(&log, "large-ready").await?;
            cancellation.cancel();
            Ok::<(), String>(())
        });
        let cancel_result = cancelled
            .sandbox()
            .execute_with_limits_and_cancel(
                SandboxCommand::shell("large output"),
                output_limits(),
                Some(cancel),
            )
            .await?;
        trigger
            .await
            .map_err(|error| format!("large output cancel trigger failed to join: {error}"))?
            .map_err(|error| format!("large output cancel trigger failed: {error}"))?;
        assert!(cancel_result.cancelled);
        assert_bounded(&cancel_result);
        Ok(())
    }

    #[test]
    fn extra_args_allowlist_cannot_override_cleanup_or_isolation_authority()
    -> std::result::Result<(), String> {
        let reserved = [
            "--name",
            "--label",
            "--restart",
            "--network",
            "--pid",
            "--ipc",
            "--uts",
            "--device",
            "--volume",
            "--mount",
            "--security-opt",
            "--cap-add",
            "--cap-drop",
        ];
        for option in reserved {
            for extra_args in [
                vec![format!("{option}=override")],
                vec![option.to_string(), "override".to_string()],
            ] {
                let sandbox = DockerSandbox::new(DockerConfig {
                    extra_args,
                    ..DockerConfig::default()
                });
                let result = sandbox.build_docker_create_args(
                    &SandboxCommand::shell("true"),
                    None,
                    "echo-sandbox-generated",
                );
                assert!(matches!(
                    result,
                    Err(echo_core::error::ReactError::Sandbox(error))
                        if matches!(*error, SandboxError::PermissionDenied(_))
                ));
            }
        }

        let sandbox = DockerSandbox::new(DockerConfig {
            extra_args: vec![
                "--user=1000:1000".to_string(),
                "--hostname".to_string(),
                "sandbox-host".to_string(),
            ],
            ..DockerConfig::default()
        });
        let args = sandbox
            .build_docker_create_args(
                &SandboxCommand::shell("true"),
                None,
                "echo-sandbox-generated",
            )
            .map_err(|error| error.to_string())?;
        assert_eq!(args.iter().filter(|arg| *arg == "--name").count(), 1);
        assert!(args.contains(&"echo-sandbox-generated".to_string()));
        assert!(args.contains(&SANDBOX_LABEL.to_string()));
        assert!(args.contains(&"--restart=no".to_string()));
        assert!(args.contains(&"--network=none".to_string()));
        assert!(args.contains(&"--user=1000:1000".to_string()));
        Ok(())
    }

    #[test]
    fn test_docker_config_default() {
        let config = DockerConfig::default();
        assert_eq!(config.default_image, "ubuntu:22.04");
        assert!(config.auto_remove);
        assert!(config.disable_network);
    }

    #[test]
    fn test_select_image_python() {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::code("python", "print(1)");
        let image = sandbox.select_image(&cmd);
        assert_eq!(image, "python:3.12-slim");
    }

    #[test]
    fn test_select_image_fallback() {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::shell("echo hello");
        let image = sandbox.select_image(&cmd);
        assert_eq!(image, "ubuntu:22.04");
    }

    #[test]
    fn test_docker_args_security() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::shell("echo test");
        let args = sandbox.build_docker_create_args(&cmd, None, "echo-sandbox-test")?;

        assert_eq!(args.first().map(String::as_str), Some("create"));
        assert!(args.contains(&"--label".to_string()));
        assert!(args.contains(&SANDBOX_LABEL.to_string()));
        assert!(args.contains(&"--cap-drop=ALL".to_string()));
        assert!(args.contains(&"--security-opt=no-new-privileges".to_string()));
        assert!(args.contains(&"--network=none".to_string()));
        assert!(args.contains(&"--init".to_string()));
        assert!(args.contains(&"--restart=no".to_string()));
        assert!(args.contains(&"--read-only".to_string()));
        assert!(args.contains(&"--ulimit=nofile=256:512".to_string()));
        assert!(args.contains(&"--ulimit=nproc=64:128".to_string()));
        Ok(())
    }

    #[test]
    fn test_docker_args_with_limits() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::shell("echo test");
        let limits = ResourceLimits {
            max_processes: Some(16),
            network: true,
            ..Default::default()
        };
        let args = sandbox.build_docker_create_args(&cmd, Some(&limits), "echo-sandbox-test")?;

        assert!(args.contains(&"--pids-limit=16".to_string()));
        // network=true in limits overrides config
        assert!(!args.contains(&"--network=none".to_string()));
        Ok(())
    }

    #[test]
    fn test_inner_command_shell() {
        let cmd = SandboxCommand::shell("ls -la");
        let inner = DockerSandbox::build_inner_command(&cmd);
        assert_eq!(inner, vec!["sh", "-c", "ls -la"]);
    }

    #[test]
    fn test_inner_command_code() {
        let cmd = SandboxCommand::code("python", "print('hi')");
        let inner = DockerSandbox::build_inner_command(&cmd);
        assert_eq!(inner, vec!["python3", "-c", "print('hi')"]);
    }

    #[test]
    fn test_docker_args_include_full_command() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::program("python3", vec!["-V".to_string()]);
        let args = sandbox.build_docker_create_args(&cmd, None, "echo-sandbox-test")?;

        assert_eq!(args.first().map(String::as_str), Some("create"));
        assert!(args.contains(&"python3".to_string()));
        assert!(args.contains(&"-V".to_string()));
        assert!(!args.contains(&"run".to_string()));
        Ok(())
    }

    /// Sprint 10b: R must map to `Rscript -e` in the docker Code backend.
    ///
    /// Before the patch, R fell through to `_ => ("sh", "-c")` and was
    /// SILENTLY mis-run as a shell command (no error, wrong interpreter —
    /// the worst kind of bug). After the patch it must map to
    /// `("Rscript", "-e")` like python/node.
    #[test]
    fn test_inner_command_code_r_maps_to_rscript() {
        let cmd = SandboxCommand::code("r", "print(1+1)");
        let inner = DockerSandbox::build_inner_command(&cmd);
        assert_eq!(
            inner,
            vec!["Rscript", "-e", "print(1+1)"],
            "R must map to Rscript -e, not silently fall through to sh -c"
        );
    }
}
