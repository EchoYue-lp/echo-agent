//! 权限服务 (PermissionService)
//!
//! 统一的权限检查入口，整合：
//! - PermissionMode: 权限模式
//! - RuleRegistry: 规则注册表
//! - SessionApprovalCache: 会话级审批缓存
//! - DenialTracker: 连续拒绝升级
//! - Classifier: AI 分类器（auto 模式）
//! - PermissionRequestHandler: 权限请求处理
//!
//! ## 审批管线
//!
//! ```text
//! check() → check_with_permissions()
//!   1. BypassPermissions → Allow
//!   2. Plan 模式 → 按 permissions 过滤
//!   3. 规则匹配 → Allow/Deny/Ask
//!   4. 缓存检查 → 命中则 AutoApprove
//!   5. DenialTracker → 连续拒绝则升级
//!   6. 模式分发:
//!      - Auto → Classifier
//!      - Default → RequestHandler
//!   7. 缓存写入（带 scope 的审批）
//! ```
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use echo_core::error::Result;
//! use echo_core::tools::permission::PermissionMode;
//! use echo_orchestration::human_loop::{
//!     PermissionRequest, PermissionRequestHandler, PermissionResponse, PermissionService,
//! };
//! use std::sync::Arc;
//!
//! struct AllowAllHandler;
//!
//! #[async_trait]
//! impl PermissionRequestHandler for AllowAllHandler {
//!     async fn handle(&self, _request: PermissionRequest) -> Result<PermissionResponse> {
//!         Ok(PermissionResponse::allowed())
//!     }
//! }
//!
//! # async fn example(handler: Arc<dyn PermissionRequestHandler>) -> Result<()> {
//! let service = PermissionService::new()
//!     .with_mode(PermissionMode::Auto)
//!     .with_request_handler(handler);
//!
//! let decision = service.check("Bash", &serde_json::json!({"command": "ls"})).await?;
//! # Ok(())
//! # }
//! # let handler: Arc<dyn PermissionRequestHandler> = Arc::new(AllowAllHandler);
//! # let _ = example(handler);
//! ```

use async_trait::async_trait;
use echo_core::tools::permission::{
    PermissionDecision, PermissionMode, PermissionRule, RuleBehavior, RuleMatcher, RuleRegistry,
    RuleSource, ToolPermission,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::approval_cache::SessionApprovalCache;
use super::audit::{PermissionAuditEntry, PermissionAuditSink};
use super::classifier::{Classifier, ClassifierContext, DenialTracker};
use super::permission::{
    PermissionContext, PermissionRequest, PermissionRequestHandler, PermissionResponse,
    PermissionResponseDecision, PermissionUpdate, RiskLevel,
};
use super::policy::ApprovalScope;
use super::protected::{ProtectedPathChecker, ProtectedPathResult};
use echo_core::error::Result;

// ── 权限服务配置 ────────────────────────────────────────────────────────────────

/// 权限服务配置
#[derive(Debug, Clone)]
pub struct PermissionServiceConfig {
    /// 权限模式
    pub mode: PermissionMode,
    /// 最大连续拒绝次数（超过则升级为人工审批）
    pub max_consecutive_denials: u32,
    /// 是否禁止 BypassPermissions 模式（企业部署用）
    pub bypass_disabled: bool,
    /// 审批缓存 TTL（None = 永不过期，参考 Claude Code: 1h Max / 5min Pro）
    pub cache_ttl: Option<Duration>,
}

impl Default for PermissionServiceConfig {
    fn default() -> Self {
        Self {
            mode: PermissionMode::Default,
            max_consecutive_denials: DenialTracker::DEFAULT_MAX_CONSECUTIVE,
            bypass_disabled: false,
            cache_ttl: Some(Duration::from_secs(30 * 60)), // 默认 30 分钟
        }
    }
}

// ── 权限服务 ────────────────────────────────────────────────────────────────────

/// 权限服务 - 统一的权限检查入口
///
/// 审批管线：
/// 1. BypassPermissions → Allow
/// 2. Plan 模式 → 按 permissions 过滤
/// 3. 规则匹配 → Allow/Deny/Ask
/// 4. 缓存检查 → 命中则 AutoApprove
/// 5. DenialTracker → 连续拒绝则升级
/// 6. 模式分发 → Classifier / Handler
/// 7. 缓存写入（带 scope 的审批）
pub struct PermissionService {
    /// 配置
    config: RwLock<PermissionServiceConfig>,
    /// 规则注册表
    rules: RwLock<RuleRegistry>,
    /// 会话级审批缓存
    cache: SessionApprovalCache,
    /// 连续拒绝跟踪器
    denial_tracker: tokio::sync::Mutex<DenialTracker>,
    /// Classifier（auto 模式）
    classifier: Option<Arc<dyn Classifier>>,
    /// 权限请求处理器（可原地替换，避免重建整个服务导致配置丢失）
    request_handler: Arc<std::sync::RwLock<Arc<dyn PermissionRequestHandler>>>,
    /// 受保护路径检查器
    protected_paths: ProtectedPathChecker,
    /// 审计 Sink（可选）
    audit_sink: Option<Arc<dyn PermissionAuditSink>>,
}

/// Atomic result of one permission check.
#[derive(Debug, Clone)]
pub struct PermissionCheck {
    pub decision: PermissionDecision,
    pub updated_input: Option<Value>,
}

/// Immutable identity and environment snapshot for one permission decision.
#[derive(Debug, Clone, Default)]
pub struct PermissionInvocationContext {
    pub scope_id: Option<String>,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_name: Option<String>,
    pub timeout: Option<Duration>,
    pub permission: PermissionContext,
    pub classifier: ClassifierContext,
}

impl PermissionCheck {
    fn from_decision(decision: PermissionDecision) -> Self {
        Self {
            decision,
            updated_input: None,
        }
    }
}

impl PermissionService {
    fn default_confirmation_required(permissions: &[ToolPermission]) -> bool {
        permissions.contains(&ToolPermission::Write)
            || permissions.contains(&ToolPermission::Execute)
            || permissions.contains(&ToolPermission::Network)
            || permissions.contains(&ToolPermission::Sensitive)
    }

    fn accept_edits_confirmation_required(permissions: &[ToolPermission]) -> bool {
        permissions.contains(&ToolPermission::Execute)
            || permissions.contains(&ToolPermission::Network)
            || permissions.contains(&ToolPermission::Sensitive)
    }

    fn strict_confirmation_required(permissions: &[ToolPermission]) -> bool {
        permissions.contains(&ToolPermission::Write)
            || permissions.contains(&ToolPermission::Execute)
            || permissions.contains(&ToolPermission::Network)
            || permissions.contains(&ToolPermission::Sensitive)
    }

    /// 创建新的权限服务
    pub fn new() -> Self {
        let config = PermissionServiceConfig::default();
        let max_denials = config.max_consecutive_denials;
        let cache = match config.cache_ttl {
            Some(ttl) => SessionApprovalCache::with_ttl(ttl),
            None => SessionApprovalCache::new(),
        };
        Self {
            config: RwLock::new(config),
            rules: RwLock::new(RuleRegistry::new()),
            cache,
            denial_tracker: tokio::sync::Mutex::new(DenialTracker::with_max_consecutive(
                max_denials,
            )),
            classifier: None,
            request_handler: Arc::new(std::sync::RwLock::new(Arc::new(
                NullPermissionRequestHandler,
            ))),
            protected_paths: ProtectedPathChecker::new(),
            audit_sink: None,
        }
    }

    /// 从 `HumanLoopProvider` 创建权限服务
    ///
    /// 便捷构造方法，自动将 Provider 适配为 `PermissionRequestHandler`。
    pub fn from_provider(provider: Arc<dyn super::HumanLoopProvider>) -> Self {
        let handler: Arc<dyn PermissionRequestHandler> = Arc::new(DynProviderHandler { provider });
        Self::new().with_request_handler(handler)
    }

    /// 设置权限模式
    pub fn with_mode(self, mode: PermissionMode) -> Self {
        if let Ok(mut config) = self.config.try_write() {
            config.mode = mode;
        }
        self
    }

    /// 设置 Classifier
    pub fn with_classifier(mut self, classifier: Arc<dyn Classifier>) -> Self {
        self.classifier = Some(classifier);
        self
    }

    /// 设置权限请求处理器
    pub fn with_request_handler(mut self, handler: Arc<dyn PermissionRequestHandler>) -> Self {
        self.request_handler = Arc::new(std::sync::RwLock::new(handler));
        self
    }

    /// 是否已配置真实的权限请求处理器（非 NullHandler）
    /// 使用 trait method 标记而非 type_name 字符串匹配
    fn has_real_handler(&self) -> bool {
        if let Ok(handler) = self.request_handler.read() {
            return !handler.is_null_handler();
        }
        true // lock poisoned — assume real handler
    }

    /// 原地替换权限请求处理器（provider），不重建整个 PermissionService。
    ///
    /// 这是 `set_human_loop_provider` 的正确底层调用：它保留了 mode、
    /// bypass_disabled、classifier、audit_sink、protected_paths、缓存等所有配置，
    /// 只替换 handler。避免了 `build_permission_service` 全量重建导致的状态丢失。
    pub fn replace_provider(&self, provider: Arc<dyn super::HumanLoopProvider>) {
        let handler: Arc<dyn PermissionRequestHandler> = Arc::new(DynProviderHandler { provider });
        if let Ok(mut guard) = self.request_handler.write() {
            *guard = handler;
        }
        // 切换 provider 时清除审批缓存——旧 provider 的审批不应延续到新 provider
        self.cache.clear();
    }

    /// Replace only the UI/provider transport and keep existing session approvals.
    ///
    /// desktop UI/Tauri installs a per-run provider before each message so concurrent
    /// conversations stay isolated. That provider swap is not a permission
    /// boundary and must not erase approvals such as "approve for this session".
    pub fn replace_provider_preserving_cache(&self, provider: Arc<dyn super::HumanLoopProvider>) {
        let handler: Arc<dyn PermissionRequestHandler> = Arc::new(DynProviderHandler { provider });
        if let Ok(mut guard) = self.request_handler.write() {
            *guard = handler;
        }
    }

    /// 设置最大连续拒绝次数
    pub fn with_max_consecutive_denials(mut self, max: u32) -> Self {
        if let Ok(mut config) = self.config.try_write() {
            config.max_consecutive_denials = max;
        }
        self.denial_tracker = tokio::sync::Mutex::new(DenialTracker::with_max_consecutive(max));
        self
    }

    /// 设置受保护路径检查器
    pub fn with_protected_paths(mut self, checker: ProtectedPathChecker) -> Self {
        self.protected_paths = checker;
        self
    }

    /// 设置审计 Sink
    pub fn with_audit_sink(mut self, sink: Arc<dyn PermissionAuditSink>) -> Self {
        self.audit_sink = Some(sink);
        self
    }

    /// 添加规则
    pub async fn add_rule(&self, rule: PermissionRule) {
        let mut rules = self.rules.write().await;
        rules.add_rule(rule);
    }

    /// 批量添加规则
    pub async fn add_rules(&self, rules: Vec<PermissionRule>) {
        let mut registry = self.rules.write().await;
        registry.add_rules(rules);
    }

    /// Remove one exact rule while preserving unrelated session and policy rules.
    pub async fn remove_rule(&self, rule: &PermissionRule) -> bool {
        self.rules.write().await.remove_rule(rule)
    }

    /// 应用权限更新
    pub async fn apply_update(&self, update: PermissionUpdate) {
        let mut rules = self.rules.write().await;
        match update {
            PermissionUpdate::AddRule {
                matcher,
                behavior,
                source,
            } => {
                let rule = Self::parse_rule(matcher, behavior, source);
                rules.add_rule(rule);
            }
            PermissionUpdate::RemoveRule { matcher } => {
                rules.remove_by_matcher(&matcher);
            }
            PermissionUpdate::SetMode { mode } => {
                self.config.write().await.mode = mode;
            }
        }
    }

    /// 批量应用更新
    pub async fn apply_updates(&self, updates: Vec<PermissionUpdate>) {
        for update in updates {
            self.apply_update(update).await;
        }
    }

    /// 设置权限模式
    pub async fn set_mode(&self, mode: PermissionMode) {
        self.config.write().await.mode = mode;
    }

    /// 同步设置权限模式（用于非 async 上下文，如 slash 命令回调）。
    ///
    /// 使用 `try_write()` 避免阻塞 — 在 agent 锁内调用时 PermissionService
    /// 不会有并发写入竞争，因此 `try_write` 几乎不会失败。
    pub fn set_mode_sync(&self, mode: PermissionMode) {
        if let Ok(mut cfg) = self.config.try_write() {
            cfg.mode = mode;
        }
    }

    /// 获取当前权限模式
    pub async fn mode(&self) -> PermissionMode {
        self.config.read().await.mode
    }

    /// Pure, side-effect-free prediction used only to split tool batches into
    /// concurrent vs approval-serialized groups.
    ///
    /// This must never call the request handler: batch planning does not have
    /// the real tool arguments and must not show a human approval dialog.
    /// The execution path still calls [`Self::check_with_permissions`] with the real
    /// input before running the tool.
    pub async fn would_request_human_for_permissions(
        &self,
        tool_name: &str,
        permissions: &[ToolPermission],
    ) -> bool {
        let config = self.config.read().await;

        if matches!(
            config.mode,
            PermissionMode::BypassPermissions
                | PermissionMode::Auto
                | PermissionMode::DontAsk
                | PermissionMode::Plan
        ) {
            return false;
        }

        {
            let rules = self.rules.read().await;
            if let Some(behavior) = rules.check(tool_name, permissions) {
                return matches!(
                    behavior.to_decision(),
                    PermissionDecision::RequireApproval | PermissionDecision::Ask { .. }
                );
            }
        }

        match config.mode {
            PermissionMode::Default => Self::default_confirmation_required(permissions),
            PermissionMode::AcceptEdits => Self::accept_edits_confirmation_required(permissions),
            PermissionMode::StrictConfirm => Self::strict_confirmation_required(permissions),
            PermissionMode::Bubble => true,
            _ => false,
        }
    }

    /// 撤销某工具的会话级审批缓存
    pub fn revoke_cache(&self, scope_id: &str, tool_name: &str) {
        self.cache.revoke(scope_id, tool_name);
    }

    /// 清空所有审批缓存
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// 记录审计条目（fire-and-forget，不阻塞管线）
    #[allow(clippy::too_many_arguments)]
    fn record_audit(
        &self,
        tool_name: &str,
        tool_input: &Value,
        decision: &PermissionDecision,
        reason: &str,
        source: &str,
        pipeline_start: std::time::Instant,
        decision_duration: std::time::Duration,
    ) {
        if let Some(sink) = &self.audit_sink {
            let entry = PermissionAuditEntry::new(
                tool_name,
                tool_input,
                decision,
                reason,
                source,
                pipeline_start,
                decision_duration,
            );
            let sink = sink.clone();
            tokio::spawn(async move {
                sink.record(entry).await;
            });
        }
    }

    /// 统一权限检查入口
    pub async fn check(&self, tool_name: &str, tool_input: &Value) -> Result<PermissionDecision> {
        self.check_with_permissions(tool_name, tool_input, &[])
            .await
    }

    /// 带权限类型的检查
    ///
    /// 审批管线：
    /// 1. BypassPermissions → Allow
    /// 2. Plan 模式 → 按 permissions 过滤
    /// 3. 规则匹配 → Allow/Deny/Ask
    /// 4. 缓存检查 → 命中则 AutoApprove
    /// 5. DenialTracker → 连续拒绝则升级
    /// 6. 模式分发（Classifier / Handler）
    /// 7. 结果后处理（缓存写入、拒绝跟踪、modified args）
    pub async fn check_with_permissions(
        &self,
        tool_name: &str,
        tool_input: &Value,
        permissions: &[ToolPermission],
    ) -> Result<PermissionDecision> {
        self.check_with_permissions_in_mode(tool_name, tool_input, permissions, None)
            .await
    }

    /// Check permissions with an optional call-scoped mode override.
    ///
    /// The override is not written into the service configuration, so
    /// concurrent tool calls cannot leak a hook-selected mode into each other.
    pub async fn check_with_permissions_in_mode(
        &self,
        tool_name: &str,
        tool_input: &Value,
        permissions: &[ToolPermission],
        mode_override: Option<PermissionMode>,
    ) -> Result<PermissionDecision> {
        Ok(self
            .check_with_permissions_result_in_mode(
                tool_name,
                tool_input,
                permissions,
                mode_override,
            )
            .await?
            .decision)
    }

    /// Check permissions and keep any user-updated input bound to this decision.
    pub async fn check_with_permissions_result_in_mode(
        &self,
        tool_name: &str,
        tool_input: &Value,
        permissions: &[ToolPermission],
        mode_override: Option<PermissionMode>,
    ) -> Result<PermissionCheck> {
        self.check_with_permissions_result_in_mode_and_context(
            tool_name,
            tool_input,
            permissions,
            mode_override,
            None,
        )
        .await
    }

    /// Check permissions with invocation context for automatic classification.
    pub async fn check_with_permissions_result_in_mode_and_context(
        &self,
        tool_name: &str,
        tool_input: &Value,
        permissions: &[ToolPermission],
        mode_override: Option<PermissionMode>,
        invocation: Option<&PermissionInvocationContext>,
    ) -> Result<PermissionCheck> {
        let pipeline_start = std::time::Instant::now();
        let config = self.config.read().await;
        let effective_mode = mode_override.unwrap_or(config.mode);
        let scope_id = invocation.and_then(|context| context.scope_id.as_deref());

        // 辅助闭包：审计 + 返回
        macro_rules! audit_return {
            ($decision:expr, $reason:expr, $source:expr) => {{
                let d = $decision;
                self.record_audit(
                    tool_name,
                    tool_input,
                    &d,
                    $reason,
                    $source,
                    pipeline_start,
                    pipeline_start.elapsed(),
                );
                return Ok(PermissionCheck::from_decision(d));
            }};
        }

        // 0. 受保护路径检查（最高优先级，在任何权限模式之前）
        // 即使在 BypassPermissions 模式下，.git/.ssh/.env 等也必须被保护
        match self.protected_paths.check(tool_name, tool_input) {
            ProtectedPathResult::Protected {
                matched_pattern,
                path,
            } => {
                audit_return!(
                    PermissionDecision::Deny {
                        reason: format!("受保护路径 '{}'（匹配规则 '{}'）", path, matched_pattern),
                    },
                    "protected_path",
                    "protected_paths"
                );
            }
            ProtectedPathResult::Safe => {}
        }

        // 1. Bypass 模式（可被管理员禁用）
        if effective_mode == PermissionMode::BypassPermissions {
            if config.bypass_disabled {
                audit_return!(
                    PermissionDecision::Deny {
                        reason: "BypassPermissions 模式已被管理员禁用".to_string(),
                    },
                    "bypass_disabled",
                    "bypass_mode"
                );
            }
            audit_return!(PermissionDecision::Allow, "bypass", "bypass_mode");
        }

        // 2. Plan 模式检查
        if effective_mode == PermissionMode::Plan {
            if permissions.contains(&ToolPermission::Write)
                || permissions.contains(&ToolPermission::Execute)
                || permissions.contains(&ToolPermission::Sensitive)
            {
                audit_return!(
                    PermissionDecision::Deny {
                        reason: "Plan 模式不允许写入或执行操作".to_string(),
                    },
                    "plan_mode",
                    "plan_mode"
                );
            }
            audit_return!(PermissionDecision::Allow, "plan_mode", "plan_mode");
        }

        // 4. 检查规则注册表
        let rule_decision = {
            let rules = self.rules.read().await;
            rules
                .check(tool_name, permissions)
                .map(|behavior| behavior.to_decision())
        };
        if let Some(decision) = rule_decision {
            if matches!(
                decision,
                PermissionDecision::RequireApproval | PermissionDecision::Ask { .. }
            ) && self.has_real_handler()
            {
                return self
                    .check_with_handler(tool_name, tool_input, permissions, invocation)
                    .await;
            }
            audit_return!(decision, "rule_match", "rules");
        }

        // 5. 缓存检查
        if scope_id.is_some_and(|scope_id| self.cache.is_approved(scope_id, tool_name, tool_input))
        {
            audit_return!(PermissionDecision::Allow, "cache_hit", "approval_cache");
        }

        // 6. DenialTracker 检查 — 连续拒绝过多则升级为人工审批
        {
            let tracker = self.denial_tracker.lock().await;
            if tracker.should_fallback() {
                drop(tracker);
                if self.has_real_handler() {
                    return self
                        .check_with_handler(tool_name, tool_input, permissions, invocation)
                        .await;
                }
                audit_return!(
                    PermissionDecision::RequireApproval,
                    "denial_tracker_fallback",
                    "denial_tracker"
                );
            }
        }

        // 5.5 未配置 handler 时直接返回 RequireApproval（而非静默拒绝）
        let needs_handler = match effective_mode {
            PermissionMode::Default => Self::default_confirmation_required(permissions),
            PermissionMode::AcceptEdits => Self::accept_edits_confirmation_required(permissions),
            PermissionMode::StrictConfirm => Self::strict_confirmation_required(permissions),
            _ => false,
        };
        if needs_handler && !self.has_real_handler() {
            audit_return!(
                PermissionDecision::RequireApproval,
                "no_handler",
                "handler_check"
            );
        }

        // 6. 模式分发
        let check = match effective_mode {
            PermissionMode::Auto => {
                let decision = self
                    .check_with_classifier(
                        tool_name,
                        tool_input,
                        invocation
                            .map(|context| context.classifier.clone())
                            .unwrap_or_default(),
                    )
                    .await?;
                if matches!(
                    decision,
                    PermissionDecision::RequireApproval | PermissionDecision::Ask { .. }
                ) && self.has_real_handler()
                {
                    self.check_with_handler(tool_name, tool_input, permissions, invocation)
                        .await?
                } else {
                    PermissionCheck::from_decision(decision)
                }
            }
            PermissionMode::Default => {
                if Self::default_confirmation_required(permissions) {
                    self.check_with_handler(tool_name, tool_input, permissions, invocation)
                        .await?
                } else {
                    PermissionCheck::from_decision(PermissionDecision::Allow)
                }
            }
            PermissionMode::Plan => {
                // Plan 已在步骤 2 处理，此处不应到达
                PermissionCheck::from_decision(PermissionDecision::Allow)
            }
            PermissionMode::AcceptEdits => {
                if Self::accept_edits_confirmation_required(permissions) {
                    self.check_with_handler(tool_name, tool_input, permissions, invocation)
                        .await?
                } else {
                    PermissionCheck::from_decision(PermissionDecision::Allow)
                }
            }
            PermissionMode::StrictConfirm => {
                if Self::strict_confirmation_required(permissions) {
                    self.check_with_handler(tool_name, tool_input, permissions, invocation)
                        .await?
                } else {
                    PermissionCheck::from_decision(PermissionDecision::Allow)
                }
            }
            PermissionMode::DontAsk => PermissionCheck::from_decision(
                // 静默模式：只放行有明确 allow 规则的操作，其他静默拒绝
                // 注意：到这里规则已检查过且无匹配，直接拒绝
                PermissionDecision::Deny {
                    reason: format!(
                        "DontAsk 模式下工具 '{}' 未匹配任何允许规则，已静默拒绝",
                        tool_name
                    ),
                },
            ),
            PermissionMode::Bubble => {
                if self.has_real_handler() {
                    self.check_with_handler(tool_name, tool_input, permissions, invocation)
                        .await?
                } else {
                    PermissionCheck::from_decision(PermissionDecision::RequireApproval)
                }
            }
            PermissionMode::BypassPermissions => {
                // Bypass 已在步骤 1 处理，此处不应到达
                PermissionCheck::from_decision(PermissionDecision::Allow)
            }
        };
        let decision = &check.decision;

        // 7. 结果后处理
        match decision {
            PermissionDecision::Allow => {
                let mut tracker = self.denial_tracker.lock().await;
                tracker.reset();
            }
            PermissionDecision::Deny { .. } => {
                let mut tracker = self.denial_tracker.lock().await;
                tracker.record_denial();
            }
            _ => {}
        }

        // 8. 审计记录（最终决策）
        let reason = match decision {
            PermissionDecision::Allow => "allowed",
            PermissionDecision::Deny { .. } => "denied",
            PermissionDecision::RequireApproval => "require_approval",
            PermissionDecision::Ask { .. } => "ask",
        };
        self.record_audit(
            tool_name,
            tool_input,
            decision,
            reason,
            "mode_dispatch",
            pipeline_start,
            pipeline_start.elapsed(),
        );

        Ok(check)
    }

    /// 使用 Classifier 检查（Auto 模式）
    async fn check_with_classifier(
        &self,
        tool_name: &str,
        tool_input: &Value,
        context: ClassifierContext,
    ) -> Result<PermissionDecision> {
        if let Some(classifier) = &self.classifier {
            let result = classifier.classify(tool_name, tool_input, &context).await?;

            if result.confidence < 0.8 {
                Ok(PermissionDecision::RequireApproval)
            } else if result.should_block {
                Ok(PermissionDecision::Deny {
                    reason: result.reason,
                })
            } else {
                Ok(PermissionDecision::Allow)
            }
        } else {
            // 没有 Classifier，回退到用户确认
            Ok(PermissionDecision::RequireApproval)
        }
    }

    /// 使用 PermissionRequestHandler 检查
    async fn check_with_handler(
        &self,
        tool_name: &str,
        tool_input: &Value,
        permissions: &[ToolPermission],
        invocation: Option<&PermissionInvocationContext>,
    ) -> Result<PermissionCheck> {
        let risk_level = RiskLevel::from_permissions(permissions);

        let mut request = PermissionRequest::new(tool_name, tool_input.clone())
            .with_permissions(permissions.to_vec())
            .with_risk_level(risk_level)
            .with_risk_based_suggestions();
        if let Some(invocation) = invocation {
            request.context = invocation.permission.clone();
            request.request_id = invocation.request_id.clone();
            request.session_id = invocation.session_id.clone();
            request.agent_name = invocation.agent_name.clone();
            request.timeout = invocation.timeout;
        }

        // 通过 RwLock 读取当前 handler（支持运行时原地替换 provider）
        let handler = self
            .request_handler
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let response = handler.handle(request).await?;

        let updated_input = response.updated_input.clone();
        let final_input = updated_input.as_ref().unwrap_or(tool_input);
        if let ProtectedPathResult::Protected {
            matched_pattern,
            path,
        } = self.protected_paths.check(tool_name, final_input)
        {
            return Ok(PermissionCheck::from_decision(PermissionDecision::Deny {
                reason: format!(
                    "修改后的输入指向受保护路径 '{}'（匹配规则 '{}'）",
                    path, matched_pattern
                ),
            }));
        }

        // 处理审批缓存。缓存范围是响应中的显式字段，不能从通用规则更新猜测。
        if let Some(scope_id) = invocation.and_then(|context| context.scope_id.as_deref())
            && matches!(response.decision, PermissionResponseDecision::Allowed)
        {
            self.cache.record_approval(
                scope_id,
                tool_name,
                final_input,
                response.approval_scope.unwrap_or(ApprovalScope::Once),
            );
        }

        // 处理规则更新
        if !response.rule_updates.is_empty() {
            self.apply_updates(response.rule_updates).await;
        }

        let decision = match response.decision {
            PermissionResponseDecision::Allowed => PermissionDecision::Allow,
            PermissionResponseDecision::Denied { reason } => PermissionDecision::Deny {
                reason: reason.unwrap_or_else(|| "用户拒绝".to_string()),
            },
            PermissionResponseDecision::NeedMoreInfo { question } => PermissionDecision::Ask {
                suggestions: vec![question],
            },
        };
        Ok(PermissionCheck {
            decision,
            updated_input,
        })
    }

    /// 解析规则
    fn parse_rule(matcher: String, behavior: String, source: String) -> PermissionRule {
        let rule_matcher = RuleMatcher::Pattern { pattern: matcher };
        let rule_behavior = match behavior.as_str() {
            "allow" => RuleBehavior::Allow,
            "deny" => RuleBehavior::Deny {
                reason: "规则拒绝".to_string(),
            },
            "ask" => RuleBehavior::Ask {
                suggestions: vec!["允许".to_string(), "拒绝".to_string()],
            },
            _ => RuleBehavior::Allow,
        };
        let rule_source = match source.as_str() {
            "session" => RuleSource::Session,
            "cliArg" => RuleSource::CliArg,
            "userSettings" => RuleSource::UserSettings,
            "projectSettings" => RuleSource::ProjectSettings,
            "localSettings" => RuleSource::LocalSettings,
            _ => RuleSource::Default,
        };

        PermissionRule {
            matcher: rule_matcher,
            behavior: rule_behavior,
            source: rule_source,
            description: None,
        }
    }

    /// 清空规则
    pub async fn clear_rules(&self) {
        let mut rules = self.rules.write().await;
        rules.clear();
    }

    /// 获取所有规则
    pub async fn all_rules(&self) -> Vec<PermissionRule> {
        let rules = self.rules.read().await;
        rules.all_rules().to_vec()
    }
}

impl Default for PermissionService {
    fn default() -> Self {
        Self::new()
    }
}

// ── Null Handler ────────────────────────────────────────────────────────────────

/// 空权限请求处理器（默认占位实现）
///
/// 不应该被直接调用 — `check_with_permissions` 会检测到此 handler 并短路返回
/// `RequireApproval`，避免静默拒绝所有工具调用。
struct NullPermissionRequestHandler;

#[async_trait]
impl PermissionRequestHandler for NullPermissionRequestHandler {
    async fn handle(&self, _request: PermissionRequest) -> Result<PermissionResponse> {
        // 不应到达此路径，但作为安全保障返回拒绝
        Ok(PermissionResponse::denied(Some(
            "没有配置权限请求处理器".to_string(),
        )))
    }

    fn is_null_handler(&self) -> bool {
        true
    }
}

// ── Dyn Provider Handler (桥接 dyn HumanLoopProvider) ─────────────────────────

/// 将 `dyn HumanLoopProvider` 桥接到 `PermissionRequestHandler`
///
/// 解决 `DefaultPermissionRequestHandler<P>` 无法直接接受 `dyn HumanLoopProvider` 的问题。
struct DynProviderHandler {
    provider: Arc<dyn super::HumanLoopProvider>,
}

#[async_trait]
impl PermissionRequestHandler for DynProviderHandler {
    async fn handle(&self, request: PermissionRequest) -> Result<PermissionResponse> {
        use super::HumanLoopResponse;

        let tool_name = request.tool_name.clone();
        let req = request.into_human_loop_request();

        match self.provider.request(req).await? {
            HumanLoopResponse::Approved => Ok(PermissionResponse::allowed()),
            HumanLoopResponse::ApprovedWithScope { scope } => {
                Ok(response_with_scope(&tool_name, scope))
            }
            HumanLoopResponse::ModifiedArgs { args, scope } => {
                // 保留用户修改的参数，传递给调用方
                let mut response = response_with_scope(&tool_name, scope);
                response.updated_input = Some(args);
                Ok(response)
            }
            HumanLoopResponse::Rejected { reason } => Ok(PermissionResponse::denied(reason)),
            HumanLoopResponse::Text(text) => Ok(PermissionResponse::allowed().with_feedback(text)),
            HumanLoopResponse::Timeout => {
                Ok(PermissionResponse::denied(Some("请求超时".to_string())))
            }
            HumanLoopResponse::Deferred => {
                Ok(PermissionResponse::denied(Some("审批被推迟".to_string())))
            }
            HumanLoopResponse::Selection { .. } => Ok(PermissionResponse::denied(Some(
                "收到意外的 Selection 响应".to_string(),
            ))),
        }
    }
}

fn response_with_scope(_tool_name: &str, scope: ApprovalScope) -> PermissionResponse {
    match scope {
        ApprovalScope::Once => PermissionResponse::allowed(),
        ApprovalScope::Session => PermissionResponse {
            decision: PermissionResponseDecision::Allowed,
            rule_updates: Vec::new(),
            feedback: None,
            updated_input: None,
            approval_scope: Some(ApprovalScope::Session),
        },
        ApprovalScope::SessionTool => PermissionResponse {
            decision: PermissionResponseDecision::Allowed,
            rule_updates: Vec::new(),
            feedback: None,
            updated_input: None,
            approval_scope: Some(ApprovalScope::SessionTool),
        },
    }
}

// ── 权限服务构建器 ──────────────────────────────────────────────────────────────

/// 权限服务构建器
pub struct PermissionServiceBuilder {
    config: PermissionServiceConfig,
    rules: RuleRegistry,
    classifier: Option<Arc<dyn Classifier>>,
    request_handler: Option<Arc<dyn PermissionRequestHandler>>,
    protected_paths: ProtectedPathChecker,
}

impl PermissionServiceBuilder {
    pub fn new() -> Self {
        Self {
            config: PermissionServiceConfig::default(),
            rules: RuleRegistry::new(),
            classifier: None,
            request_handler: None,
            protected_paths: ProtectedPathChecker::new(),
        }
    }

    pub fn mode(mut self, mode: PermissionMode) -> Self {
        self.config.mode = mode;
        self
    }

    pub fn max_consecutive_denials(mut self, max: u32) -> Self {
        self.config.max_consecutive_denials = max;
        self
    }

    pub fn rule(mut self, rule: PermissionRule) -> Self {
        self.rules.add_rule(rule);
        self
    }

    pub fn classifier(mut self, classifier: Arc<dyn Classifier>) -> Self {
        self.classifier = Some(classifier);
        self
    }

    pub fn request_handler(mut self, handler: Arc<dyn PermissionRequestHandler>) -> Self {
        self.request_handler = Some(handler);
        self
    }

    pub fn protected_paths(mut self, checker: ProtectedPathChecker) -> Self {
        self.protected_paths = checker;
        self
    }

    pub fn build(self) -> PermissionService {
        let max_denials = self.config.max_consecutive_denials;
        PermissionService {
            config: RwLock::new(self.config),
            rules: RwLock::new(self.rules),
            cache: SessionApprovalCache::new(),
            denial_tracker: tokio::sync::Mutex::new(DenialTracker::with_max_consecutive(
                max_denials,
            )),
            classifier: self.classifier,
            request_handler: Arc::new(std::sync::RwLock::new(
                self.request_handler
                    .unwrap_or_else(|| Arc::new(NullPermissionRequestHandler)),
            )),
            protected_paths: self.protected_paths,
            audit_sink: None,
        }
    }
}

impl Default for PermissionServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── 单元测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::error::Result as EchoResult;
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn invocation(scope_id: &str) -> PermissionInvocationContext {
        PermissionInvocationContext {
            scope_id: Some(scope_id.to_string()),
            ..PermissionInvocationContext::default()
        }
    }

    struct CountingAllowHandler {
        count: Arc<AtomicUsize>,
        response: PermissionResponse,
    }

    struct EchoModifiedInputHandler;
    struct ProtectedModifiedInputHandler;

    #[async_trait::async_trait]
    impl PermissionRequestHandler for CountingAllowHandler {
        async fn handle(&self, _request: PermissionRequest) -> EchoResult<PermissionResponse> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    #[async_trait::async_trait]
    impl PermissionRequestHandler for EchoModifiedInputHandler {
        async fn handle(&self, request: PermissionRequest) -> EchoResult<PermissionResponse> {
            tokio::task::yield_now().await;
            Ok(PermissionResponse {
                decision: PermissionResponseDecision::Allowed,
                rule_updates: Vec::new(),
                feedback: None,
                updated_input: Some(serde_json::json!({
                    "approved_for": request.tool_input.get("request_id").cloned()
                })),
                approval_scope: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl PermissionRequestHandler for ProtectedModifiedInputHandler {
        async fn handle(&self, _request: PermissionRequest) -> EchoResult<PermissionResponse> {
            Ok(PermissionResponse {
                decision: PermissionResponseDecision::Allowed,
                rule_updates: Vec::new(),
                feedback: None,
                updated_input: Some(serde_json::json!({"path": ".git/config"})),
                approval_scope: Some(ApprovalScope::Session),
            })
        }
    }

    struct TestHumanLoopProvider;

    impl super::super::HumanLoopProvider for TestHumanLoopProvider {
        fn request(
            &self,
            _req: super::super::HumanLoopRequest,
        ) -> BoxFuture<'_, EchoResult<super::super::HumanLoopResponse>> {
            Box::pin(async { Ok(super::super::HumanLoopResponse::Approved) })
        }
    }

    #[tokio::test]
    async fn test_permission_service_new() {
        let service = PermissionService::new();
        assert_eq!(service.mode().await, PermissionMode::Default);
    }

    #[tokio::test]
    async fn test_permission_service_bypass() {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::BypassPermissions).await;

        let decision = service.check("Bash", &serde_json::json!({})).await.unwrap();
        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn test_permission_service_plan_mode() {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::Plan).await;

        // 读操作应该允许
        let decision = service
            .check_with_permissions("Read", &serde_json::json!({}), &[ToolPermission::Read])
            .await
            .unwrap();
        assert!(decision.is_allowed());

        // 写操作应该拒绝
        let decision = service
            .check_with_permissions("Write", &serde_json::json!({}), &[ToolPermission::Write])
            .await
            .unwrap();
        assert!(decision.is_denied());
    }

    #[tokio::test]
    async fn call_scoped_mode_override_does_not_mutate_service_mode() -> EchoResult<()> {
        let service = PermissionService::new().with_mode(PermissionMode::Default);

        let decision = service
            .check_with_permissions_in_mode(
                "Write",
                &serde_json::json!({}),
                &[ToolPermission::Write],
                Some(PermissionMode::Plan),
            )
            .await?;

        assert!(decision.is_denied());
        assert_eq!(service.mode().await, PermissionMode::Default);
        Ok(())
    }

    #[tokio::test]
    async fn test_default_mode_allows_read_without_handler() -> EchoResult<()> {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::Default).await;

        let decision = service
            .check_with_permissions("Read", &serde_json::json!({}), &[ToolPermission::Read])
            .await?;

        assert!(decision.is_allowed());
        Ok(())
    }

    #[tokio::test]
    async fn test_default_mode_requires_handler_for_execute() -> EchoResult<()> {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::Default).await;

        let decision = service
            .check_with_permissions("Bash", &serde_json::json!({}), &[ToolPermission::Execute])
            .await?;

        assert!(matches!(decision, PermissionDecision::RequireApproval));
        Ok(())
    }

    #[tokio::test]
    async fn test_batch_prediction_does_not_call_handler() -> EchoResult<()> {
        let count = Arc::new(AtomicUsize::new(0));
        let service =
            PermissionService::new().with_request_handler(Arc::new(CountingAllowHandler {
                count: count.clone(),
                response: PermissionResponse::allowed(),
            }));
        service.set_mode(PermissionMode::Default).await;

        let needs_human = service
            .would_request_human_for_permissions("Bash", &[ToolPermission::Execute])
            .await;

        assert!(needs_human);
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "batch prediction must not show approval UI"
        );
        Ok(())
    }

    #[tokio::test]
    async fn modified_input_is_returned_with_its_permission_decision() -> EchoResult<()> {
        let service = PermissionService::new()
            .with_request_handler(Arc::new(EchoModifiedInputHandler))
            .with_mode(PermissionMode::StrictConfirm);

        let check = service
            .check_with_permissions_result_in_mode(
                "Bash",
                &serde_json::json!({"request_id": "one"}),
                &[ToolPermission::Execute],
                None,
            )
            .await?;

        assert!(check.decision.is_allowed());
        assert_eq!(
            check.updated_input,
            Some(serde_json::json!({"approved_for": "one"}))
        );
        Ok(())
    }

    #[tokio::test]
    async fn modified_input_is_rechecked_for_protected_paths() -> EchoResult<()> {
        let service = PermissionService::new()
            .with_request_handler(Arc::new(ProtectedModifiedInputHandler))
            .with_mode(PermissionMode::StrictConfirm);

        let check = service
            .check_with_permissions_result_in_mode_and_context(
                "Write",
                &serde_json::json!({"path": "notes.txt"}),
                &[ToolPermission::Write],
                None,
                Some(&invocation("agent-a:conversation-a")),
            )
            .await?;

        assert!(check.decision.is_denied());
        assert!(check.updated_input.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_modified_inputs_remain_call_scoped() -> EchoResult<()> {
        let service = PermissionService::new()
            .with_request_handler(Arc::new(EchoModifiedInputHandler))
            .with_mode(PermissionMode::StrictConfirm);
        let first_input = serde_json::json!({"request_id": "first"});
        let second_input = serde_json::json!({"request_id": "second"});
        let first = service.check_with_permissions_result_in_mode(
            "Bash",
            &first_input,
            &[ToolPermission::Execute],
            None,
        );
        let second = service.check_with_permissions_result_in_mode(
            "Bash",
            &second_input,
            &[ToolPermission::Execute],
            None,
        );

        let (first, second) = tokio::join!(first, second);
        assert_eq!(
            first?.updated_input,
            Some(serde_json::json!({"approved_for": "first"}))
        );
        assert_eq!(
            second?.updated_input,
            Some(serde_json::json!({"approved_for": "second"}))
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_accept_edits_allows_read_and_write_without_handler() -> EchoResult<()> {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::AcceptEdits).await;

        let read = service
            .check_with_permissions("Read", &serde_json::json!({}), &[ToolPermission::Read])
            .await?;
        let write = service
            .check_with_permissions("Write", &serde_json::json!({}), &[ToolPermission::Write])
            .await?;

        assert!(read.is_allowed());
        assert!(write.is_allowed());
        Ok(())
    }

    #[tokio::test]
    async fn test_accept_edits_still_requires_handler_for_execute() -> EchoResult<()> {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::AcceptEdits).await;

        let decision = service
            .check_with_permissions("Bash", &serde_json::json!({}), &[ToolPermission::Execute])
            .await?;

        assert!(matches!(decision, PermissionDecision::RequireApproval));
        Ok(())
    }

    #[tokio::test]
    async fn test_strict_confirm_allows_read_without_handler() {
        let service = PermissionService::new();
        service.set_mode(PermissionMode::StrictConfirm).await;

        let decision = service
            .check_with_permissions("Read", &serde_json::json!({}), &[ToolPermission::Read])
            .await
            .unwrap();

        assert!(decision.is_allowed());
    }

    #[tokio::test]
    async fn test_strict_confirm_write_uses_handler_and_session_scope() {
        let count = Arc::new(AtomicUsize::new(0));
        let service =
            PermissionService::new().with_request_handler(Arc::new(CountingAllowHandler {
                count: count.clone(),
                response: PermissionResponse {
                    decision: PermissionResponseDecision::Allowed,
                    rule_updates: Vec::new(),
                    feedback: None,
                    updated_input: None,
                    approval_scope: Some(ApprovalScope::SessionTool),
                },
            }));
        service.set_mode(PermissionMode::StrictConfirm).await;

        let first_input = serde_json::json!({"path": "first"});
        let decision = service
            .check_with_permissions_result_in_mode_and_context(
                "Write",
                &first_input,
                &[ToolPermission::Write],
                None,
                Some(&invocation("agent-a:conversation-a")),
            )
            .await
            .unwrap();
        assert!(decision.decision.is_allowed());
        assert_eq!(count.load(Ordering::SeqCst), 1);

        let decision = service
            .check_with_permissions_result_in_mode_and_context(
                "Write",
                &serde_json::json!({"path": "second"}),
                &[ToolPermission::Write],
                None,
                Some(&invocation("agent-a:conversation-a")),
            )
            .await
            .unwrap();
        assert!(decision.decision.is_allowed());
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "session tool approval should use the explicit cache scope"
        );
    }

    #[tokio::test]
    async fn test_provider_transport_swap_preserves_session_approval_cache() {
        let count = Arc::new(AtomicUsize::new(0));
        let service =
            PermissionService::new().with_request_handler(Arc::new(CountingAllowHandler {
                count: count.clone(),
                response: PermissionResponse {
                    decision: PermissionResponseDecision::Allowed,
                    rule_updates: Vec::new(),
                    feedback: None,
                    updated_input: None,
                    approval_scope: Some(ApprovalScope::SessionTool),
                },
            }));
        service.set_mode(PermissionMode::StrictConfirm).await;

        let decision = service
            .check_with_permissions_result_in_mode_and_context(
                "Bash",
                &serde_json::json!({}),
                &[ToolPermission::Execute],
                None,
                Some(&invocation("agent-a:conversation-a")),
            )
            .await
            .unwrap();
        assert!(decision.decision.is_allowed());
        assert_eq!(count.load(Ordering::SeqCst), 1);

        service.replace_provider_preserving_cache(Arc::new(TestHumanLoopProvider));

        let decision = service
            .check_with_permissions_result_in_mode_and_context(
                "Bash",
                &serde_json::json!({"command": "pwd"}),
                &[ToolPermission::Execute],
                None,
                Some(&invocation("agent-a:conversation-a")),
            )
            .await
            .unwrap();
        assert!(decision.decision.is_allowed());
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "desktop UI provider transport swaps must not erase session approvals"
        );
    }

    #[tokio::test]
    async fn test_permission_service_add_rule() {
        let service = PermissionService::new();

        service
            .add_rule(PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "Read".to_string(),
                },
                RuleSource::UserSettings,
            ))
            .await;

        let rules = service.all_rules().await;
        assert_eq!(rules.len(), 1);
    }

    #[tokio::test]
    async fn test_permission_service_apply_update() {
        let service = PermissionService::new();

        let update = PermissionUpdate::add_session_rule("Read".to_string());
        service.apply_update(update).await;

        let rules = service.all_rules().await;
        assert_eq!(rules.len(), 1);
    }

    #[tokio::test]
    async fn test_permission_service_builder() {
        let service = PermissionServiceBuilder::new()
            .mode(PermissionMode::Auto)
            .max_consecutive_denials(5)
            .build();

        assert_eq!(service.mode().await, PermissionMode::Auto);
    }

    #[tokio::test]
    async fn test_permission_service_default_handler() {
        let service = PermissionService::new();

        // 没有配置 handler，应该返回 RequireApproval（而非静默拒绝）
        let decision = service
            .check_with_permissions("Bash", &serde_json::json!({}), &[ToolPermission::Execute])
            .await
            .unwrap();

        assert!(matches!(decision, PermissionDecision::RequireApproval));
    }

    #[test]
    fn test_parse_rule() {
        let rule = PermissionService::parse_rule(
            "Bash".to_string(),
            "allow".to_string(),
            "session".to_string(),
        );

        assert!(matches!(rule.behavior, RuleBehavior::Allow));
        assert!(matches!(rule.source, RuleSource::Session));
    }
}
