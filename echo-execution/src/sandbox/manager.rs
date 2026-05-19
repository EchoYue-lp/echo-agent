//! 沙箱管理器
//!
//! 自动检测可用的沙箱执行环境，按安全策略路由命令到合适的沙箱层：
//! - 优先使用满足安全策略的最轻量沙箱
//! - 自动降级：如 Docker 不可用则回退到本地沙箱
//! - 统一的 `execute()` 接口屏蔽底层差异

use super::{
    DockerSandbox, ExecutionResult, IsolationLevel, K8sSandbox, LocalSandbox, ResourceLimits,
    SandboxCommand, SandboxExecutor, docker::DockerConfig, k8s::K8sConfig, local::LocalConfig,
    policy::SandboxPolicy,
};
use echo_core::error::Result;
use echo_core::error::SandboxError;
use futures::future::BoxFuture;
use std::sync::Arc;
use tracing::{info, warn};

/// 沙箱管理器：统一调度三层沙箱
#[derive(Debug)]
pub struct SandboxManager {
    /// 本地沙箱（始终可用）
    local: Arc<LocalSandbox>,
    /// Docker 沙箱
    docker: Option<Arc<DockerSandbox>>,
    /// K8s 沙箱
    k8s: Option<Arc<K8sSandbox>>,
    /// 安全策略
    policy: SandboxPolicy,
    /// 是否允许降级执行
    allow_fallback: bool,
}

impl SandboxExecutor for SandboxManager {
    fn name(&self) -> &str {
        "manager"
    }

    fn isolation_level(&self) -> IsolationLevel {
        self.available_levels()
            .into_iter()
            .max()
            .unwrap_or(IsolationLevel::None)
    }

    fn is_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { true })
    }

    fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move { SandboxManager::execute(self, command).await })
    }

    fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> BoxFuture<'_, Result<ExecutionResult>> {
        Box::pin(async move { SandboxManager::execute_with_limits(self, command, limits).await })
    }

    fn cleanup(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.local.cleanup().await?;
            if let Some(docker) = &self.docker {
                docker.cleanup().await?;
            }
            if let Some(k8s) = &self.k8s {
                k8s.cleanup().await?;
            }
            Ok(())
        })
    }
}

impl SandboxManager {
    /// 自动检测可用沙箱环境
    pub async fn auto_detect() -> Self {
        let local = Arc::new(LocalSandbox::new(LocalConfig::default()));
        let docker = Arc::new(DockerSandbox::new(DockerConfig::default()));
        let k8s = Arc::new(K8sSandbox::new(K8sConfig::default()));

        let docker_available = docker.is_available().await;
        let k8s_available = k8s.is_available().await;

        info!(
            docker = docker_available,
            k8s = k8s_available,
            "Sandbox auto-detection complete"
        );

        Self {
            local,
            docker: if docker_available { Some(docker) } else { None },
            k8s: if k8s_available { Some(k8s) } else { None },
            policy: SandboxPolicy::default(),
            allow_fallback: true,
        }
    }

    /// 使用自定义配置创建
    pub fn with_configs(
        local_config: LocalConfig,
        docker_config: Option<DockerConfig>,
        k8s_config: Option<K8sConfig>,
        policy: SandboxPolicy,
    ) -> Self {
        Self {
            local: Arc::new(LocalSandbox::new(local_config)),
            docker: docker_config.map(|c| Arc::new(DockerSandbox::new(c))),
            k8s: k8s_config.map(|c| Arc::new(K8sSandbox::new(c))),
            policy,
            allow_fallback: true,
        }
    }

    /// 仅本地沙箱（开发用，禁用 OS 沙箱）
    pub fn local_only() -> Self {
        Self {
            local: Arc::new(LocalSandbox::new(LocalConfig {
                enable_os_sandbox: false,
                ..Default::default()
            })),
            docker: None,
            k8s: None,
            policy: SandboxPolicy::trusted(),
            allow_fallback: false,
        }
    }

    /// 设置是否允许降级
    pub fn set_allow_fallback(&mut self, allow: bool) {
        self.allow_fallback = allow;
    }

    /// 设置安全策略
    pub fn set_policy(&mut self, policy: SandboxPolicy) {
        self.policy = policy;
    }

    /// 执行命令（自动选择沙箱层）
    pub async fn execute(&self, command: SandboxCommand) -> Result<ExecutionResult> {
        let required_level = self.policy.evaluate(&command);
        self.execute_at_level(command, required_level, None).await
    }

    /// 执行命令并限制资源
    pub async fn execute_with_limits(
        &self,
        command: SandboxCommand,
        limits: ResourceLimits,
    ) -> Result<ExecutionResult> {
        let required_level = self.policy.evaluate_with_limits(&command, Some(&limits));
        self.execute_at_level(command, required_level, Some(limits))
            .await
    }

    /// 在指定隔离级别执行，必要时降级
    async fn execute_at_level(
        &self,
        command: SandboxCommand,
        required: IsolationLevel,
        limits: Option<ResourceLimits>,
    ) -> Result<ExecutionResult> {
        // 选择满足要求的最佳执行器
        let executor = self.select_executor(required)?;

        // 当允许 fallback 时，可能会选择低于所需隔离级别的执行器。
        let actual = executor.isolation_level();
        if actual < required {
            if !self.allow_fallback {
                return Err(echo_core::error::ReactError::Sandbox(
                    Box::new(SandboxError::PermissionDenied(format!(
                        "Cannot downgrade from {} to {}: no executor meets the required isolation level",
                        required, actual
                    ))),
                ));
            }

            warn!(
                required = %required,
                actual = %actual,
                executor = executor.name(),
                "Falling back to lower isolation executor"
            );
        }

        info!(
            required = %required,
            actual = %actual,
            executor = executor.name(),
            "Sandbox routing"
        );

        match limits {
            Some(limits) => executor.execute_with_limits(command, limits).await,
            None => executor.execute(command).await,
        }
    }

    /// 选择最佳执行器
    ///
    /// 优先选择满足隔离级别要求的最轻量执行器（Local -> Docker -> K8s）。
    /// 如果没有执行器满足要求，根据 `allow_fallback` 决定：
    /// - `true`: 返回可用执行器中隔离最强的一层（实际隔离级别可能低于要求）
    /// - `false`: 返回错误
    fn select_executor(
        &self,
        required: IsolationLevel,
    ) -> std::result::Result<Arc<dyn SandboxExecutor>, echo_core::error::ReactError> {
        // 优先选择满足要求的最轻量执行器，避免把低风险命令路由到更重的层。
        if self.local.isolation_level() >= required {
            return Ok(self.local.clone());
        }
        if let Some(ref docker) = self.docker
            && docker.isolation_level() >= required
        {
            return Ok(docker.clone());
        }
        if let Some(ref k8s) = self.k8s
            && k8s.isolation_level() >= required
        {
            return Ok(k8s.clone());
        }

        // 没有执行器满足要求
        if self.allow_fallback {
            // 降级时选择可用执行器中隔离最强的一层。
            let best_available: Arc<dyn SandboxExecutor> = if let Some(ref k8s) = self.k8s {
                k8s.clone()
            } else if let Some(ref docker) = self.docker {
                docker.clone()
            } else {
                self.local.clone()
            };

            warn!(
                required = %required,
                available = %best_available.isolation_level(),
                executor = best_available.name(),
                "No executor meets required isolation level, returning best available for fallback decision"
            );

            return Ok(best_available);
        }

        // 不允许降级，直接拒绝
        Err(echo_core::error::ReactError::Sandbox(
            Box::new(SandboxError::Unavailable(format!(
                "Isolation level {} required but no executor meets the requirement",
                required
            ))),
        ))
    }

    /// 列出可用的沙箱层
    pub fn available_levels(&self) -> Vec<IsolationLevel> {
        let mut levels = vec![IsolationLevel::None, IsolationLevel::Process];

        if self.local.isolation_level() >= IsolationLevel::OsSandbox {
            levels.push(IsolationLevel::OsSandbox);
        }

        if self.docker.is_some() {
            levels.push(IsolationLevel::Container);
        }

        if self.k8s.is_some() {
            levels.push(IsolationLevel::Orchestrated);
        }

        levels
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_only() {
        let manager = SandboxManager::local_only();
        let levels = manager.available_levels();
        assert!(levels.contains(&IsolationLevel::None));
        assert!(levels.contains(&IsolationLevel::Process));
        assert!(!levels.contains(&IsolationLevel::Container));
        assert!(!levels.contains(&IsolationLevel::Orchestrated));
    }

    #[tokio::test]
    async fn test_execute_simple_command() {
        let manager = SandboxManager::local_only();
        let cmd = SandboxCommand::shell("echo sandbox_test");
        let result = manager.execute(cmd).await.unwrap();
        assert!(result.success());
        assert_eq!(result.stdout.trim(), "sandbox_test");
    }

    #[test]
    fn test_select_executor_none() {
        let manager = SandboxManager::local_only();
        let executor = manager.select_executor(IsolationLevel::None).unwrap();
        assert_eq!(executor.name(), "local");
    }

    #[test]
    fn test_select_executor_container_fallback() {
        let mut manager = SandboxManager::local_only();
        manager.allow_fallback = true;
        // No docker available, should fallback to local
        let executor = manager.select_executor(IsolationLevel::Container).unwrap();
        assert_eq!(executor.name(), "local");
    }

    #[test]
    fn test_select_executor_container_no_fallback() {
        let mut manager = SandboxManager::local_only();
        manager.allow_fallback = false;
        let result = manager.select_executor(IsolationLevel::Container);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_with_policy() {
        let manager = SandboxManager::local_only();
        // Safe command: policy should evaluate to None, execute locally
        let cmd = SandboxCommand::shell("echo safe");
        let result = manager.execute(cmd).await.unwrap();
        assert!(result.success());
    }

    #[tokio::test]
    async fn test_execute_with_limits_uses_limit_aware_policy() {
        let mut manager = SandboxManager::local_only();
        manager.set_policy(SandboxPolicy::default());

        let err = manager
            .execute_with_limits(
                SandboxCommand::shell("echo safe"),
                ResourceLimits {
                    network: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("Isolation level"));
    }

    #[tokio::test]
    async fn test_execute_with_limits_falls_back_when_enabled() {
        let mut manager = SandboxManager::local_only();
        manager.set_policy(SandboxPolicy::default());
        manager.set_allow_fallback(true);

        let result = manager
            .execute_with_limits(
                SandboxCommand::shell("echo downgraded"),
                ResourceLimits {
                    network: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(result.success());
        assert_eq!(result.stdout.trim(), "downgraded");
        assert_eq!(result.sandbox_type, "local");
    }
}
