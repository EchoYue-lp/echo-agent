//! Kubernetes Pod 沙箱执行器
//!
//! 通过 `kubectl` CLI 在 K8s 集群中创建临时 Pod 执行代码：
//! - Pod 级隔离（SecurityContext + ResourceQuota）
//! - 显式删除临时 Pod（success / error / timeout 都会走清理）
//! - 支持 Pod SecurityPolicy / SecurityStandards
//!
//! 适用于大规模并发和企业级部署。需要 `kubectl` 已配置集群访问。
//! 当前实现不会像 Docker 一样逐 Pod 强制断网；当 `ResourceLimits.network=false`
//! 时，会保留默认网络行为并输出能力告警。

use super::{
    CommandKind, ExecutionResult, IsolationLevel, ResourceLimits, SandboxCommand, SandboxExecutor,
    select_image_for_command,
};
use echo_core::error::Result;
use echo_core::error::SandboxError;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::warn;

/// K8s 沙箱配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K8sConfig {
    /// kubectl 使用的 namespace
    pub namespace: String,
    /// 默认镜像
    pub default_image: String,
    /// 语言与镜像映射
    pub language_images: std::collections::HashMap<String, String>,
    /// Pod 的 service account
    pub service_account: Option<String>,
    /// CPU request（如 "100m"）
    pub cpu_request: String,
    /// CPU limit（如 "500m"）
    pub cpu_limit: String,
    /// Memory request（如 "64Mi"）
    pub memory_request: String,
    /// Memory limit（如 "256Mi"）
    pub memory_limit: String,
    /// 预留的集群侧清理时间（秒）
    ///
    /// 当前执行路径使用显式 `kubectl delete` 清理临时 Pod，
    /// 该字段保留给未来可能的 Job/TTL 控制策略。
    pub ttl_seconds: u32,
    /// 节点选择器
    pub node_selector: std::collections::HashMap<String, String>,
}

impl Default for K8sConfig {
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
            namespace: "echo-sandbox".to_string(),
            default_image: "ubuntu:22.04".to_string(),
            language_images,
            service_account: None,
            cpu_request: "100m".to_string(),
            cpu_limit: "500m".to_string(),
            memory_request: "64Mi".to_string(),
            memory_limit: "256Mi".to_string(),
            ttl_seconds: 300,
            node_selector: std::collections::HashMap::new(),
        }
    }
}

/// Kubernetes Pod 沙箱
#[derive(Debug, Clone)]
pub struct K8sSandbox {
    config: K8sConfig,
}

impl K8sSandbox {
    pub fn new(config: K8sConfig) -> Self {
        Self { config }
    }

    /// 检测 kubectl 是否可用
    async fn check_kubectl() -> bool {
        Command::new("kubectl")
            .args(["version", "--client"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// 为命令选择合适的镜像
    fn select_image(&self, command: &SandboxCommand) -> String {
        select_image_for_command(
            command,
            &self.config.language_images,
            &self.config.default_image,
        )
    }

    /// 构建容器内执行的命令
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

    async fn delete_pod(&self, pod_name: &str) {
        let _ = Command::new("kubectl")
            .args([
                "delete",
                "pod",
                pod_name,
                "-n",
                &self.config.namespace,
                "--grace-period=0",
                "--force",
                "--ignore-not-found=true",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }

    /// 生成 Pod JSON spec 并通过 kubectl run 执行
    async fn run_pod(
        &self,
        command: &SandboxCommand,
        limits: Option<&ResourceLimits>,
    ) -> Result<ExecutionResult> {
        let pod_name = format!("echo-sandbox-{}", uuid::Uuid::new_v4().simple());
        let image = self.select_image(command);
        let inner_cmd = Self::build_inner_command(command);
        let timeout = limits
            .and_then(|l| l.cpu_time_secs)
            .map(std::time::Duration::from_secs)
            .unwrap_or(command.timeout);

        // 根据 limits 动态设置资源
        let (cpu_req, cpu_lim, mem_req, mem_lim) = if let Some(l) = limits {
            let mr = l
                .memory_bytes
                .map(|b| format!("{b}"))
                .unwrap_or(self.config.memory_request.clone());
            let ml = l
                .memory_bytes
                .map(|b| format!("{}", b * 2))
                .unwrap_or(self.config.memory_limit.clone());
            (
                self.config.cpu_request.clone(),
                self.config.cpu_limit.clone(),
                mr,
                ml,
            )
        } else {
            (
                self.config.cpu_request.clone(),
                self.config.cpu_limit.clone(),
                self.config.memory_request.clone(),
                self.config.memory_limit.clone(),
            )
        };

        if let Some(l) = limits
            && !l.network
        {
            warn!(
                pod = %pod_name,
                "K8sSandbox cannot fully disable per-pod network access; continuing with cluster defaults"
            );
        }

        // 构建 kubectl run 命令
        let mut args = vec![
            "run".to_string(),
            pod_name.clone(),
            format!("--image={image}"),
            format!("--namespace={}", self.config.namespace),
            "--restart=Never".to_string(),
            "--attach".to_string(),
            format!(
                "--stdin={}",
                if command.stdin.is_some() {
                    "true"
                } else {
                    "false"
                }
            ),
            format!("--requests=cpu={cpu_req},memory={mem_req}"),
            format!("--limits=cpu={cpu_lim},memory={mem_lim}"),
        ];

        // 注入环境变量作为 Pod env（使用 --env）
        for (k, v) in &command.env {
            args.push(format!("--env={k}={v}"));
        }

        if let Some(ref sa) = self.config.service_account {
            args.push(format!("--serviceaccount={sa}"));
        }

        // SecurityContext overrides
        args.push("--overrides".to_string());
        args.push(
            serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "labels": {
                        "echo-sandbox": "true"
                    }
                },
                "spec": {
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 65534,
                        "fsGroup": 65534
                    },
                    "containers": [{
                        "name": pod_name,
                        "securityContext": {
                            "allowPrivilegeEscalation": false,
                            "readOnlyRootFilesystem": false,
                            "capabilities": { "drop": ["ALL"] }
                        }
                    }],
                    "automountServiceAccountToken": false,
                    "enableServiceLinks": false
                }
            })
            .to_string(),
        );

        // 命令分隔
        args.push("--command".to_string());
        args.push("--".to_string());
        args.extend(inner_cmd);

        let mut cmd = Command::new("kubectl");
        cmd.args(&args);
        cmd.stdin(if command.stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let start = Instant::now();

        let mut child = cmd.spawn().map_err(|e| {
            echo_core::error::ReactError::Sandbox(Box::new(SandboxError::StartFailed(format!(
                "Failed to run kubectl: {e}"
            ))))
        })?;

        if let Some(input) = command.stdin.as_deref()
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin.write_all(input.as_bytes()).await.map_err(|e| {
                echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(format!(
                    "Failed to write kubectl stdin: {e}"
                ))))
            })?;
        }

        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                self.delete_pod(&pod_name).await;
                Ok(ExecutionResult {
                    exit_code: output.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    duration: start.elapsed(),
                    sandbox_type: "k8s".to_string(),
                    timed_out: false,
                })
            }
            Ok(Err(e)) => {
                self.delete_pod(&pod_name).await;
                Err(echo_core::error::ReactError::Sandbox(Box::new(
                    SandboxError::IoError(format!("kubectl IO error: {e}")),
                )))
            }
            Err(_) => {
                // 超时时删除 Pod 并等待确认
                self.delete_pod(&pod_name).await;

                Ok(ExecutionResult {
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("K8s Pod execution timed out after {}s", timeout.as_secs()),
                    duration: start.elapsed(),
                    sandbox_type: "k8s".to_string(),
                    timed_out: true,
                })
            }
        }
    }

    /// 清理所有带有 echo-sandbox 标签的 Pod
    pub async fn cleanup_sandbox_pods(&self) -> Result<()> {
        let output = Command::new("kubectl")
            .args([
                "get",
                "pods",
                "-n",
                &self.config.namespace,
                "-l",
                "echo-sandbox=true",
                "-o",
                "name",
            ])
            .output()
            .await
            .map_err(|e| {
                echo_core::error::ReactError::Sandbox(Box::new(SandboxError::IoError(format!(
                    "Failed to list sandbox pods: {e}"
                ))))
            })?;

        let pods = String::from_utf8_lossy(&output.stdout);
        if !pods.trim().is_empty() {
            let _ = Command::new("kubectl")
                .args([
                    "delete",
                    "pods",
                    "-n",
                    &self.config.namespace,
                    "-l",
                    "echo-sandbox=true",
                    "--grace-period=0",
                    "--force",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .await;
        }
        Ok(())
    }
}

impl SandboxExecutor for K8sSandbox {
    fn name(&self) -> &str {
        "k8s"
    }

    fn isolation_level(&self) -> IsolationLevel {
        IsolationLevel::Orchestrated
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(Self::check_kubectl())
    }

    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.cleanup_sandbox_pods())
    }

    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move { self.run_pod(&command, None).await })
    }

    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move { self.run_pod(&command, Some(&limits)).await })
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_k8s_config_default() {
        let config = K8sConfig::default();
        assert_eq!(config.namespace, "echo-sandbox");
        assert_eq!(config.cpu_limit, "500m");
        assert_eq!(config.memory_limit, "256Mi");
    }

    #[test]
    fn test_select_image_default() {
        let sandbox = K8sSandbox::new(K8sConfig::default());
        let cmd = SandboxCommand::shell("echo test");
        assert_eq!(sandbox.select_image(&cmd), "ubuntu:22.04");
    }

    #[test]
    fn test_inner_command_code() {
        let cmd = SandboxCommand::code("python", "print('hello')");
        let inner = K8sSandbox::build_inner_command(&cmd);
        assert_eq!(inner, vec!["python3", "-c", "print('hello')"]);
    }

    #[test]
    fn test_inner_command_program() {
        let cmd = SandboxCommand::program("ls", vec!["-la".to_string()]);
        let inner = K8sSandbox::build_inner_command(&cmd);
        assert_eq!(inner, vec!["ls", "-la"]);
    }

    #[test]
    fn test_inner_command_php() {
        let cmd = SandboxCommand::code("php", "echo 'hi';");
        let inner = K8sSandbox::build_inner_command(&cmd);
        assert_eq!(inner, vec!["php", "-r", "echo 'hi';"]);
    }
}
