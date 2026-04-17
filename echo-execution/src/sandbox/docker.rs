//! Docker 容器沙箱执行器
//!
//! 通过 Docker CLI 实现容器级隔离：
//! - 自动创建临时容器
//! - cgroups 资源限制
//! - 网络隔离
//! - 挂载控制
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::process::Command;

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
    /// 额外的 docker run 参数
    pub extra_args: Vec<String>,
}

/// 缓存 Docker 可用性检查结果
static DOCKER_AVAILABLE_CACHE: AtomicBool = AtomicBool::new(false);
static DOCKER_CHECKED: AtomicBool = AtomicBool::new(false);

/// 危险 Docker 参数，即使配置中有也应当过滤掉
const DANGEROUS_DOCKER_ARGS: &[&str] = &[
    "--privileged",
    "--pid=host",
    "--network=host",
    "--ipc=host",
    "--uts=host",
    "--cap-add",
    "--security-opt=seccomp=unconfined",
    "--security-opt=apparmor=unconfined",
];

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
            read_only_rootfs: false,
            extra_args: vec![],
        }
    }
}

/// Docker 容器沙箱
#[derive(Debug, Clone)]
pub struct DockerSandbox {
    config: DockerConfig,
}

impl DockerSandbox {
    pub fn new(config: DockerConfig) -> Self {
        Self { config }
    }

    /// 检测 Docker 是否安装且可用（带缓存）
    async fn check_docker() -> bool {
        if DOCKER_CHECKED.load(Ordering::Relaxed) {
            return DOCKER_AVAILABLE_CACHE.load(Ordering::Relaxed);
        }
        let available = Command::new("docker")
            .arg("info")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        DOCKER_AVAILABLE_CACHE.store(available, Ordering::Relaxed);
        DOCKER_CHECKED.store(true, Ordering::Relaxed);
        available
    }

    /// 为命令选择合适的镜像
    fn select_image(&self, command: &SandboxCommand) -> String {
        select_image_for_command(
            command,
            &self.config.language_images,
            &self.config.default_image,
        )
    }

    /// 构建 docker run 命令行参数
    fn build_docker_args(
        &self,
        command: &SandboxCommand,
        limits: Option<&ResourceLimits>,
    ) -> Vec<String> {
        let mut args = vec!["run".to_string()];

        if self.config.auto_remove {
            args.push("--rm".to_string());
        }

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

        // 环境变量
        for (k, v) in &command.env {
            args.push("-e".to_string());
            args.push(format!("{k}={v}"));
        }

        // 挂载卷（limits 中的路径），需验证安全性
        if let Some(limits) = limits {
            for path in &limits.read_only_paths {
                Self::validate_mount_paths(&[path.clone()])
                    .map_err(|e| {
                        echo_core::error::ReactError::Sandbox(SandboxError::PermissionDenied(e))
                    })
                    .unwrap();
                args.push("-v".to_string());
                args.push(format!("{}:{}:ro", path.display(), path.display()));
            }
            for path in &limits.writable_paths {
                Self::validate_mount_paths(&[path.clone()])
                    .map_err(|e| {
                        echo_core::error::ReactError::Sandbox(SandboxError::PermissionDenied(e))
                    })
                    .unwrap();
                args.push("-v".to_string());
                args.push(format!("{}:{}", path.display(), path.display()));
            }
        }

        // 工作目录
        if let Some(ref dir) = command.working_dir {
            args.push("-w".to_string());
            args.push(dir.display().to_string());
        }

        // 额外参数：过滤掉危险选项
        for arg in &self.config.extra_args {
            if !DANGEROUS_DOCKER_ARGS.iter().any(|d| arg.starts_with(d)) {
                args.push(arg.clone());
            }
        }

        // 镜像
        args.push(self.select_image(command));

        args
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
        let output = Command::new("docker")
            .args(["ps", "-aq", "--filter", "label=echo-sandbox=true"])
            .output()
            .await
            .map_err(|e| {
                echo_core::error::ReactError::Sandbox(SandboxError::IoError(format!(
                    "Failed to list sandbox containers: {e}"
                )))
            })?;

        let containers = String::from_utf8_lossy(&output.stdout);
        if !containers.trim().is_empty() {
            for container_id in containers.lines() {
                let _ = Command::new("docker")
                    .args(["rm", "-f", container_id])
                    .output()
                    .await;
            }
        }
        Ok(())
    }
}

impl SandboxExecutor for DockerSandbox {
    fn name(&self) -> &str {
        "docker"
    }

    fn isolation_level(&self) -> IsolationLevel {
        IsolationLevel::Container
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(Self::check_docker())
    }

    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(Self::cleanup_sandbox_containers())
    }

    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move {
            if !Self::check_docker().await {
                return Err(echo_core::error::ReactError::Sandbox(
                    SandboxError::Unavailable("Docker is not available".to_string()),
                ));
            }

            let timeout = command.timeout;
            let mut docker_args = self.build_docker_args(&command, None);
            docker_args.extend(Self::build_inner_command(&command));

            let start = Instant::now();

            // 先 create 容器获取 ID，再 start 执行，以便超时时能 kill
            let mut create_args = vec!["create".to_string()];
            // 从 docker_args 中分离镜像和命令：前部分是 run 参数，最后是镜像+命令
            let image_idx = docker_args
                .iter()
                .position(|a| !a.starts_with("-"))
                .unwrap_or(docker_args.len() - 3);
            let image_and_cmd = &docker_args[image_idx..];
            create_args.extend(docker_args[..image_idx].iter().cloned());
            create_args.extend(image_and_cmd.iter().cloned());

            let output = Command::new("docker")
                .args(&create_args)
                .output()
                .await
                .map_err(|e| {
                    echo_core::error::ReactError::Sandbox(SandboxError::StartFailed(format!(
                        "Failed to create docker container: {e}"
                    )))
                })?;

            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if container_id.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(echo_core::error::ReactError::Sandbox(
                    SandboxError::StartFailed(format!("Failed to get container ID: {stderr}")),
                ));
            }

            // 启动并等待结果
            let mut start_cmd = Command::new("docker");
            start_cmd
                .args(["start", "-a", &container_id])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            let child = start_cmd.spawn().map_err(|e| {
                echo_core::error::ReactError::Sandbox(SandboxError::StartFailed(format!(
                    "Failed to start docker container: {e}"
                )))
            })?;

            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    // 清理容器
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &container_id])
                        .output()
                        .await;

                    Ok(ExecutionResult {
                        exit_code: output.status.code().unwrap_or(-1),
                        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                        duration: start.elapsed(),
                        sandbox_type: "docker".to_string(),
                        timed_out: false,
                    })
                }
                Ok(Err(e)) => {
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &container_id])
                        .output()
                        .await;
                    Err(echo_core::error::ReactError::Sandbox(
                        SandboxError::IoError(format!("Docker IO error: {e}")),
                    ))
                }
                Err(_) => {
                    // 超时：强制 kill 容器
                    let _ = Command::new("docker")
                        .args(["kill", &container_id])
                        .output()
                        .await;
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &container_id])
                        .output()
                        .await;

                    Ok(ExecutionResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: format!("Docker execution timed out after {}s", timeout.as_secs()),
                        duration: start.elapsed(),
                        sandbox_type: "docker".to_string(),
                        timed_out: true,
                    })
                }
            }
        })
    }

    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move {
            if !Self::check_docker().await {
                return Err(echo_core::error::ReactError::Sandbox(
                    SandboxError::Unavailable("Docker is not available".to_string()),
                ));
            }

            let timeout = limits
                .cpu_time_secs
                .map(std::time::Duration::from_secs)
                .unwrap_or(command.timeout);

            let mut docker_args = self.build_docker_args(&command, Some(&limits));
            docker_args.extend(Self::build_inner_command(&command));

            let start = Instant::now();

            // 先 create 容器获取 ID
            let mut create_args = vec!["create".to_string()];
            let image_idx = docker_args
                .iter()
                .position(|a| !a.starts_with("-"))
                .unwrap_or(docker_args.len() - 3);
            let image_and_cmd = &docker_args[image_idx..];
            create_args.extend(docker_args[..image_idx].iter().cloned());
            create_args.extend(image_and_cmd.iter().cloned());

            let output = Command::new("docker")
                .args(&create_args)
                .output()
                .await
                .map_err(|e| {
                    echo_core::error::ReactError::Sandbox(SandboxError::StartFailed(format!(
                        "Failed to create docker container: {e}"
                    )))
                })?;

            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if container_id.is_empty() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(echo_core::error::ReactError::Sandbox(
                    SandboxError::StartFailed(format!("Failed to get container ID: {stderr}")),
                ));
            }

            let mut start_cmd = Command::new("docker");
            start_cmd
                .args(["start", "-a", &container_id])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            let child = start_cmd.spawn().map_err(|e| {
                echo_core::error::ReactError::Sandbox(SandboxError::StartFailed(format!(
                    "Failed to start docker container: {e}"
                )))
            })?;

            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &container_id])
                        .output()
                        .await;

                    let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    if let Some(max) = limits.max_output_bytes {
                        let max = max as usize;
                        if stdout.len() > max {
                            stdout.truncate(max);
                            stdout.push_str("\n... [output truncated]");
                        }
                        if stderr.len() > max {
                            stderr.truncate(max);
                            stderr.push_str("\n... [output truncated]");
                        }
                    }

                    Ok(ExecutionResult {
                        exit_code: output.status.code().unwrap_or(-1),
                        stdout,
                        stderr,
                        duration: start.elapsed(),
                        sandbox_type: "docker".to_string(),
                        timed_out: false,
                    })
                }
                Ok(Err(e)) => {
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &container_id])
                        .output()
                        .await;
                    Err(echo_core::error::ReactError::Sandbox(
                        SandboxError::IoError(format!("Docker IO error: {e}")),
                    ))
                }
                Err(_) => {
                    let _ = Command::new("docker")
                        .args(["kill", &container_id])
                        .output()
                        .await;
                    let _ = Command::new("docker")
                        .args(["rm", "-f", &container_id])
                        .output()
                        .await;

                    Ok(ExecutionResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: format!("Docker execution timed out after {}s", timeout.as_secs()),
                        duration: start.elapsed(),
                        sandbox_type: "docker".to_string(),
                        timed_out: true,
                    })
                }
            }
        })
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_docker_args_security() {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::shell("echo test");
        let args = sandbox.build_docker_args(&cmd, None);

        assert!(args.contains(&"--cap-drop=ALL".to_string()));
        assert!(args.contains(&"--security-opt=no-new-privileges".to_string()));
        assert!(args.contains(&"--network=none".to_string()));
    }

    #[test]
    fn test_docker_args_with_limits() {
        let sandbox = DockerSandbox::new(DockerConfig::default());
        let cmd = SandboxCommand::shell("echo test");
        let limits = ResourceLimits {
            max_processes: Some(16),
            network: true,
            ..Default::default()
        };
        let args = sandbox.build_docker_args(&cmd, Some(&limits));

        assert!(args.contains(&"--pids-limit=16".to_string()));
        // network=true in limits overrides config
        assert!(!args.contains(&"--network=none".to_string()));
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
}
