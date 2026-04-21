//! 三层沙箱执行系统
//!
//! 提供 Local / Docker / K8s 三种隔离级别的代码执行环境，
//! 统一通过 [`SandboxExecutor`] trait 抽象，由 [`SandboxManager`] 自动选择最佳执行层。
//!
//! 当前实现约定：
//! - [`ResourceLimits::cpu_time_secs`] 表示 wall-clock timeout，而不是 CPU request / quota
//! - [`SandboxCommand::stdin`] 会透传到 Local / Docker / K8s 三个执行器
//! - K8s 执行器会显式删除临时 Pod；`network=false` 目前只会记录能力告警，不能像 Docker 一样逐 Pod 强制断网
//!
//! ## 架构
//!
//! | 层 | 实现 | 隔离强度 | 开销 | 适用场景 |
//! |----|------|----------|------|----------|
//! | Local | [`LocalSandbox`] | OS 级（namespace/sandbox-exec） | 极低 | 开发调试、受信操作 |
//! | Docker | [`DockerSandbox`] | 容器级（namespace + cgroups） | 中等 | 不可信代码、环境隔离 |
//! | K8s | [`K8sSandbox`] | 编排工作负载级（Pod） | 较高 | 大规模并发、企业级 |
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_execution::sandbox::*;
//!
//! # async fn example() -> echo_core::error::Result<()> {
//! // 自动检测最佳执行环境
//! let manager = SandboxManager::auto_detect().await;
//! let result = manager.execute(
//!     SandboxCommand::shell("echo hello world"),
//! ).await?;
//! println!("stdout: {}", result.stdout);
//!
//! // 指定 Docker 执行
//! let docker = DockerSandbox::new(DockerConfig::default());
//! if docker.is_available().await {
//!     let result = docker.execute(SandboxCommand::shell("python3 -c 'print(1+1)'")).await?;
//!     println!("result: {}", result.stdout);
//! }
//! # Ok(())
//! # }
//! ```

pub mod docker;
pub mod k8s;
pub mod local;
pub mod manager;
pub mod policy;

use echo_core::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

// ── 重导出 ──────────────────────────────────────────────────────────────────

pub use docker::{DockerConfig, DockerSandbox};
pub use k8s::{K8sConfig, K8sSandbox};
pub use local::{LocalConfig, LocalSandbox};
pub use manager::SandboxManager;
pub use policy::{SandboxPolicy, SecurityLevel};

// ── 共享工具 ────────────────────────────────────────────────────────────────

/// 已知危险目录，禁止挂载到容器/Pod 中
pub static SENSITIVE_MOUNT_PATHS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        "/etc", "/proc", "/sys", "/dev", "/boot", "/run", "/var/run", "/var/log",
    ]
});

/// 默认语言到 Docker/K8s 镜像的映射
pub fn default_language_image(lang: &str) -> Option<&'static str> {
    match lang {
        "python" | "python3" => Some("python:3.12-slim"),
        "node" | "javascript" | "js" | "typescript" | "ts" => Some("node:20-slim"),
        "ruby" => Some("ruby:3.3-slim"),
        "go" | "golang" => Some("golang:1.22-alpine"),
        "rust" => Some("rust:1.77-slim"),
        "perl" => Some("perl:5.38-slim"),
        "php" => Some("php:8.3-cli"),
        "lua" => Some("alpine:3.19"),
        "r" => Some("rocker/r-base:latest"),
        "julia" => Some("julia:1.10-slim"),
        "swift" => Some("swift:5.9-slim"),
        _ => None,
    }
}

/// 为命令选择合适的镜像（通用逻辑）。
///
/// 按优先级：语言精确匹配 -> 语言前缀匹配 -> 默认镜像。
/// 对 `CommandKind::Shell`，如果首个 token 看起来像路径（包含 `/` 或 `\`），
/// 则直接回退到默认镜像，避免把宿主机路径误判成语言标识。
pub fn select_image_for_command(
    command: &SandboxCommand,
    language_images: &HashMap<String, String>,
    default_image: &str,
) -> String {
    match &command.kind {
        CommandKind::Code { language, .. } => {
            // 精确匹配
            if let Some(img) = language_images.get(language) {
                return img.clone();
            }
            // 默认语言映射
            if let Some(img) = default_language_image(language) {
                return img.to_string();
            }
            default_image.to_string()
        }
        CommandKind::Shell(cmd) => {
            let raw = cmd.split_whitespace().next().unwrap_or("");
            if raw.contains('/') || raw.contains('\\') {
                return default_image.to_string();
            }
            let base = std::path::Path::new(raw)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(raw);
            if let Some(img) = language_images.get(base) {
                return img.clone();
            }
            if let Some(img) = default_language_image(base) {
                return img.to_string();
            }
            default_image.to_string()
        }
        CommandKind::Program { program, .. } => {
            let name = std::path::Path::new(program)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(program);
            if let Some(img) = language_images.get(name) {
                return img.clone();
            }
            if let Some(img) = default_language_image(name) {
                return img.to_string();
            }
            default_image.to_string()
        }
    }
}

// ── 核心类型 ────────────────────────────────────────────────────────────────

/// 沙箱执行器统一接口
///
/// 所有三层沙箱（Local / Docker / K8s）都实现此 trait。
pub trait SandboxExecutor: Send + Sync {
    /// 执行器名称
    fn name(&self) -> &str;

    /// 当前隔离级别
    fn isolation_level(&self) -> IsolationLevel;

    /// 检测执行器是否可用
    fn is_available(&self) -> BoxFuture<'_, bool>;

    /// 执行命令
    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>>;

    /// 执行命令并限制资源
    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        // 默认实现：忽略 limits，直接执行
        let _ = limits;
        self.execute(command)
    }

    /// 清理沙箱资源（容器、临时文件等）
    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// 隔离级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IsolationLevel {
    /// 无隔离（直接宿主机执行）
    None = 0,
    /// 进程级隔离（资源限制、路径限制）
    Process = 1,
    /// OS 沙箱隔离（bubblewrap / sandbox-exec / seccomp）
    OsSandbox = 2,
    /// 容器隔离（Docker / Podman）
    Container = 3,
    /// 编排工作负载隔离（例如临时 K8s Pod；可由底层运行时进一步强化）
    Orchestrated = 4,
}

impl std::fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IsolationLevel::None => write!(f, "none"),
            IsolationLevel::Process => write!(f, "process"),
            IsolationLevel::OsSandbox => write!(f, "os-sandbox"),
            IsolationLevel::Container => write!(f, "container"),
            IsolationLevel::Orchestrated => write!(f, "orchestrated"),
        }
    }
}

/// 沙箱执行命令
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxCommand {
    /// 命令类型
    pub kind: CommandKind,
    /// 工作目录
    pub working_dir: Option<PathBuf>,
    /// 环境变量
    pub env: HashMap<String, String>,
    /// 执行超时
    pub timeout: Duration,
    /// 标准输入
    ///
    /// 当前会透传到 Local / Docker / K8s 三个执行器。
    pub stdin: Option<String>,
}

/// 命令类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandKind {
    /// Shell 命令（通过 sh -c / cmd /C 执行）
    Shell(String),
    /// 直接执行程序
    Program { program: String, args: Vec<String> },
    /// 执行代码片段（需指定语言运行时）
    Code { language: String, code: String },
}

impl SandboxCommand {
    /// 创建 Shell 命令
    pub fn shell(cmd: impl Into<String>) -> Self {
        Self {
            kind: CommandKind::Shell(cmd.into()),
            working_dir: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            stdin: None,
        }
    }

    /// 创建程序执行命令
    pub fn program(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            kind: CommandKind::Program {
                program: program.into(),
                args,
            },
            working_dir: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            stdin: None,
        }
    }

    /// 创建代码执行命令
    pub fn code(language: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            kind: CommandKind::Code {
                language: language.into(),
                code: code.into(),
            },
            working_dir: None,
            env: HashMap::new(),
            timeout: Duration::from_secs(30),
            stdin: None,
        }
    }

    /// 设置工作目录
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// 设置超时
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 添加环境变量
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// 设置标准输入
    pub fn with_stdin(mut self, stdin: impl Into<String>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

/// 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// 退出码（0 = 成功）
    pub exit_code: i32,
    /// 标准输出
    pub stdout: String,
    /// 标准错误
    pub stderr: String,
    /// 执行耗时
    pub duration: Duration,
    /// 使用的沙箱类型
    pub sandbox_type: String,
    /// 是否超时被终止
    pub timed_out: bool,
}

impl ExecutionResult {
    /// 是否执行成功
    pub fn success(&self) -> bool {
        self.exit_code == 0 && !self.timed_out
    }

    /// 合并 stdout + stderr 输出
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

/// 资源限制
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// 最大执行时间（秒，按 wall-clock timeout 处理）
    ///
    /// 该字段用于统一超时语义，不等价于容器 / K8s 的 CPU request 或 CPU quota。
    pub cpu_time_secs: Option<u64>,
    /// 最大内存（字节）
    pub memory_bytes: Option<u64>,
    /// 最大输出大小（字节）
    pub max_output_bytes: Option<u64>,
    /// 最大进程数
    pub max_processes: Option<u32>,
    /// 是否允许网络
    pub network: bool,
    /// 允许的挂载路径（只读）
    pub read_only_paths: Vec<PathBuf>,
    /// 允许的挂载路径（读写）
    pub writable_paths: Vec<PathBuf>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_time_secs: Some(30),
            memory_bytes: Some(256 * 1024 * 1024), // 256 MB
            max_output_bytes: Some(1024 * 1024),   // 1 MB
            max_processes: Some(64),
            network: false,
            read_only_paths: vec![],
            writable_paths: vec![],
        }
    }
}

impl ResourceLimits {
    /// 无限制（仅用于受信环境）
    pub fn unrestricted() -> Self {
        Self {
            cpu_time_secs: None,
            memory_bytes: None,
            max_output_bytes: None,
            max_processes: None,
            network: true,
            read_only_paths: vec![],
            writable_paths: vec![],
        }
    }

    /// 严格限制（适合不可信代码）
    pub fn strict() -> Self {
        Self {
            cpu_time_secs: Some(10),
            memory_bytes: Some(64 * 1024 * 1024), // 64 MB
            max_output_bytes: Some(256 * 1024),   // 256 KB
            max_processes: Some(8),
            network: false,
            read_only_paths: vec![],
            writable_paths: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_image_shell_path_uses_default_image() {
        let mut language_images = HashMap::new();
        language_images.insert("python3".to_string(), "python:3.12-slim".to_string());

        let cmd = SandboxCommand::shell("/usr/bin/python3 script.py");
        let image = select_image_for_command(&cmd, &language_images, "ubuntu:22.04");

        assert_eq!(image, "ubuntu:22.04");
    }

    #[test]
    fn test_select_image_shell_binary_name_mapping() {
        let mut language_images = HashMap::new();
        language_images.insert("python3".to_string(), "python:3.12-slim".to_string());

        let cmd = SandboxCommand::shell("python3 script.py");
        let image = select_image_for_command(&cmd, &language_images, "ubuntu:22.04");

        assert_eq!(image, "python:3.12-slim");
    }
}
