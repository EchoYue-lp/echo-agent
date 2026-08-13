//! 三层沙箱执行系统
//!
//! 提供 Local / Docker / K8s 三种隔离级别的代码执行环境，
//! 统一通过 [`SandboxExecutor`] trait 抽象，由 [`SandboxManager`] 自动选择最佳执行层。
//!
//! 当前实现约定：
//! - [`ResourceLimits::cpu_time_secs`] 表示 wall-clock timeout，而不是 CPU request / quota
//! - [`SandboxCommand::stdin`] 会透传到 Local / Docker / K8s 三个执行器
//! - K8s 执行器会显式删除临时 Pod；无法逐 Pod 强制断网时，`network=false` 会明确拒绝执行
//!
//! ## 架构
//!
//! | 层 | 实现 | 隔离强度 | 开销 | 适用场景 |
//! |----|------|----------|------|----------|
//! | Local | [`LocalSandbox`] | macOS/Linux OS 级；Windows 进程级 | 极低 | 开发调试、受信操作 |
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

use std::collections::HashMap;
use std::sync::LazyLock;

// ── 重导出 ──────────────────────────────────────────────────────────────────

pub use docker::{DockerConfig, DockerSandbox};
pub use k8s::{K8sConfig, K8sSandbox};
pub use local::{LocalConfig, LocalSandbox};
pub use manager::SandboxManager;
pub use policy::{SandboxPolicy, SecurityLevel};

// Re-export core types from echo_core
pub use echo_core::sandbox::{
    CommandKind, ExecutionResult, IsolationLevel, ResourceLimits, SandboxCommand, SandboxExecutor,
    SandboxOutputChannel, SandboxStreamEvent,
};

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

// ── 核心类型 (defined in echo_core::sandbox, re-exported above) ───────────

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
