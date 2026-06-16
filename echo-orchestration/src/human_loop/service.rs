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
    PermissionRequest, PermissionRequestHandler, PermissionResponse, PermissionResponseDecision,
    PermissionUpdate, RiskLevel,
};
use super::policy::ApprovalScope;
use super::protected::{ProtectedPathChecker, ProtectedPathResult};
use echo_core::error::Result;

// ── 超时策略 ────────────────────────────────────────────────────────────────

/// 审批超时策略
///
/// 当审批请求超过指定超时时间后，决定系统如何处理。
#[derive(Debug, Clone, Default)]
pub enum TimeoutStrategy {
    /// 超时后拒绝执行（默认行为）
    #[default]
    Reject,
    /// 超时后自动批准
    AutoApprove {
        /// 自动批准原因
        reason: String,
    },
    /// 超时后升级为人工交互
    Escalate,
}

// ── 权限服务配置 ────────────────────────────────────────────────────────────────

/// 权限服务配置
#[derive(Debug, Clone)]
pub struct PermissionServiceConfig {
    /// 权限模式
    pub mode: PermissionMode,
    /// 最大连续拒绝次数（超过则升级为人工审批）
    pub max_consecutive_denials: u32,
    /// 最大总拒绝次数（超过则升级为人工审批）
    pub max_total_denials: u32,
    /// 是否启用 classifier（auto 模式）
    pub enable_classifier: bool,
    /// 超时策略
    pub timeout_strategy: TimeoutStrategy,
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
            max_total_denials: DenialTracker::DEFAULT_MAX_TOTAL,
            enable_classifier: true,
            timeout_strategy: TimeoutStrategy::Reject,
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
    /// 最近一次审批中用户修改的参数（side channel）
    last_modified_args: RwLock<Option<Value>>,
}

impl PermissionService {
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
            request_handler: Arc::new(std::sync::RwLock::new(Arc::new(NullPermissionRequestHandler))),
            protected_paths: ProtectedPathChecker::new(),
            audit_sink: None,
            last_modified_args: RwLock::new(None),
        }
    }

    /// 从 `HumanLoopProvider` 创建权限服务
    ///
    /// 便捷构造方法，自动将 Provider 适配为 `PermissionRequestHandler`。
    pub fn from_provider(provider: Arc<dyn super::HumanLoopProvider>) -> Self {
        let handler: Arc<dyn PermissionRequestHandler> = Arc::new(DynProviderHandler { provider });
        Self::new().with_request_handler(handler)
    }

    /// 设置超时策略
    pub fn with_timeout_strategy(self, strategy: TimeoutStrategy) -> Self {
        if let Ok(mut config) = self.config.try_write() {
            config.timeout_strategy = strategy;
        }
        self
    }

    /// 设置权限模式
    pub fn with_mode(self, mode: PermissionMode) -> Self {
        let mut config = self
            .config
            .try_write()
            .expect("PermissionService not yet shared");
        config.mode = mode;
        drop(config);
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
    /// GUI/Tauri installs a per-run provider before each message so concurrent
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

    /// 取出最近一次审批中用户修改的参数
    ///
    /// 返回 `Some(modified_args)` 表示用户在审批时修改了参数，
    /// 调用方应用修改后的参数执行工具。调用后自动清除。
    pub async fn take_modified_args(&self) -> Option<Value> {
        self.last_modified_args.write().await.take()
    }

    /// 撤销某工具的会话级审批缓存
    pub fn revoke_cache(&self, tool_name: &str) {
        self.cache.revoke(tool_name);
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
        let pipeline_start = std::time::Instant::now();
        let config = self.config.read().await;

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
                return Ok(d);
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
        if config.mode == PermissionMode::BypassPermissions {
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
        if config.mode == PermissionMode::Plan {
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
        {
            let rules = self.rules.read().await;
            if let Some(behavior) = rules.check(tool_name, permissions) {
                let decision = behavior.to_decision();
                audit_return!(decision, "rule_match", "rules");
            }
        }

        // 5. 缓存检查
        if self.cache.is_approved(tool_name, tool_input) {
            audit_return!(PermissionDecision::Allow, "cache_hit", "approval_cache");
        }

        // 6. DenialTracker 检查 — 连续拒绝过多则升级为人工审批
        {
            let tracker = self.denial_tracker.lock().await;
            if tracker.should_fallback() {
                audit_return!(
                    PermissionDecision::RequireApproval,
                    "denial_tracker_fallback",
                    "denial_tracker"
                );
            }
        }

        // 5.5 未配置 handler 时直接返回 RequireApproval（而非静默拒绝）
        let needs_handler = matches!(config.mode, PermissionMode::Default | PermissionMode::AcceptEdits)
            || (config.mode == PermissionMode::StrictConfirm
                && Self::strict_confirmation_required(permissions));
        if needs_handler && !self.has_real_handler() {
            audit_return!(
                PermissionDecision::RequireApproval,
                "no_handler",
                "handler_check"
            );
        }

        // 6. 模式分发
        let decision = match config.mode {
            PermissionMode::Auto => self.check_with_classifier(tool_name, tool_input).await?,
            PermissionMode::Default => {
                self.check_with_handler(tool_name, tool_input, permissions)
                    .await?
            }
            PermissionMode::Plan => {
                // Plan 已在步骤 2 处理，此处不应到达
                PermissionDecision::Allow
            }
            PermissionMode::AcceptEdits => {
                if permissions.contains(&ToolPermission::Write) {
                    PermissionDecision::Allow
                } else {
                    self.check_with_handler(tool_name, tool_input, permissions)
                        .await?
                }
            }
            PermissionMode::StrictConfirm => {
                if Self::strict_confirmation_required(permissions) {
                    self.check_with_handler(tool_name, tool_input, permissions)
                        .await?
                } else {
                    PermissionDecision::Allow
                }
            }
            PermissionMode::DontAsk => {
                // 静默模式：只放行有明确 allow 规则的操作，其他静默拒绝
                // 注意：到这里规则已检查过且无匹配，直接拒绝
                PermissionDecision::Deny {
                    reason: format!(
                        "DontAsk 模式下工具 '{}' 未匹配任何允许规则，已静默拒绝",
                        tool_name
                    ),
                }
            }
            PermissionMode::Bubble => PermissionDecision::RequireApproval,
            PermissionMode::BypassPermissions => {
                // Bypass 已在步骤 1 处理，此处不应到达
                PermissionDecision::Allow
            }
        };

        // 7. 结果后处理
        match &decision {
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
        let reason = match &decision {
            PermissionDecision::Allow => "allowed",
            PermissionDecision::Deny { .. } => "denied",
            PermissionDecision::RequireApproval => "require_approval",
            PermissionDecision::Ask { .. } => "ask",
        };
        self.record_audit(
            tool_name,
            tool_input,
            &decision,
            reason,
            "mode_dispatch",
            pipeline_start,
            pipeline_start.elapsed(),
        );

        Ok(decision)
    }

    /// 使用 Classifier 检查（Auto 模式）
    async fn check_with_classifier(
        &self,
        tool_name: &str,
        tool_input: &Value,
    ) -> Result<PermissionDecision> {
        if let Some(classifier) = &self.classifier {
            let context = ClassifierContext::new("agent".to_string(), "session".to_string());
            let result = classifier.classify(tool_name, tool_input, &context).await?;

            if result.should_block {
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
    ) -> Result<PermissionDecision> {
        let risk_level = RiskLevel::from_permissions(permissions);

        let request = PermissionRequest::new(tool_name, tool_input.clone())
            .with_permissions(permissions.to_vec())
            .with_risk_level(risk_level)
            .with_risk_based_suggestions();

        // 通过 RwLock 读取当前 handler（支持运行时原地替换 provider）
        let handler = self
            .request_handler
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let response = handler.handle(request).await?;

        // 处理用户修改的参数
        if let Some(modified) = &response.updated_input {
            *self.last_modified_args.write().await = Some(modified.clone());
        }

        // 处理审批缓存 — Approved 时写入缓存
        // 根据 rule_updates 推断缓存 scope
        if matches!(response.decision, PermissionResponseDecision::Allowed) {
            let scope = Self::infer_scope_from_updates(&response.rule_updates);
            self.cache.record_approval(tool_name, tool_input, scope);
        }

        // 处理规则更新
        if !response.rule_updates.is_empty() {
            self.apply_updates(response.rule_updates).await;
        }

        Ok(match response.decision {
            PermissionResponseDecision::Allowed => PermissionDecision::Allow,
            PermissionResponseDecision::Denied { reason } => PermissionDecision::Deny {
                reason: reason.unwrap_or_else(|| "用户拒绝".to_string()),
            },
            PermissionResponseDecision::NeedMoreInfo { question } => PermissionDecision::Ask {
                suggestions: vec![question],
            },
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

    /// 从 rule_updates 推断审批缓存粒度
    ///
    /// - `SessionAllTools`: 存在 session 级 allow 规则
    /// - `Session`: 存在 session 级规则但非 "all tools" 语义
    /// - `Once`: 无 session 规则，仅批准本次
    fn infer_scope_from_updates(updates: &[PermissionUpdate]) -> ApprovalScope {
        for update in updates {
            if let PermissionUpdate::AddRule {
                source, behavior, ..
            } = update
                && source == "session"
            {
                // session 级 allow 规则 → SessionAllTools
                if behavior == "allow" {
                    return ApprovalScope::SessionAllTools;
                }
                // session 级 deny 规则 → Session（仅针对当前工具）
                return ApprovalScope::Session;
            }
        }
        ApprovalScope::Once
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
        use super::{HumanLoopRequest, HumanLoopResponse};

        let req = HumanLoopRequest::approval(&request.tool_name, request.tool_input.clone());

        match self.provider.request(req).await? {
            HumanLoopResponse::Approved => Ok(PermissionResponse::allowed()),
            HumanLoopResponse::ApprovedWithScope { scope } => {
                Ok(response_with_scope(&request.tool_name, scope))
            }
            HumanLoopResponse::ModifiedArgs { args, scope } => {
                // 保留用户修改的参数，传递给调用方
                let mut response = response_with_scope(&request.tool_name, scope);
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

fn response_with_scope(tool_name: &str, scope: ApprovalScope) -> PermissionResponse {
    match scope {
        ApprovalScope::Once => PermissionResponse::allowed(),
        ApprovalScope::Session => PermissionResponse {
            decision: PermissionResponseDecision::Allowed,
            rule_updates: vec![PermissionUpdate::add_session_rule(tool_name.to_string())],
            feedback: None,
            updated_input: None,
        },
        ApprovalScope::SessionAllTools => PermissionResponse {
            decision: PermissionResponseDecision::Allowed,
            rule_updates: vec![PermissionUpdate::add_session_rule("*".to_string())],
            feedback: None,
            updated_input: None,
        },
    }
}

// ── 权限服务构建器 ──────────────────────────────────────────────────────────────

/// 权限服务构建器
#[allow(dead_code)]
pub struct PermissionServiceBuilder {
    config: PermissionServiceConfig,
    rules: RuleRegistry,
    classifier: Option<Arc<dyn Classifier>>,
    request_handler: Option<Arc<dyn PermissionRequestHandler>>,
    protected_paths: ProtectedPathChecker,
}

#[allow(dead_code)]
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

    pub fn enable_classifier(mut self, enable: bool) -> Self {
        self.config.enable_classifier = enable;
        self
    }

    pub fn timeout_strategy(mut self, strategy: TimeoutStrategy) -> Self {
        self.config.timeout_strategy = strategy;
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
            last_modified_args: RwLock::new(None),
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

    struct CountingAllowHandler {
        count: Arc<AtomicUsize>,
        response: PermissionResponse,
    }

    #[async_trait::async_trait]
    impl PermissionRequestHandler for CountingAllowHandler {
        async fn handle(&self, _request: PermissionRequest) -> EchoResult<PermissionResponse> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
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
        let service = PermissionService::new().with_request_handler(Arc::new(
            CountingAllowHandler {
                count: count.clone(),
                response: PermissionResponse {
                    decision: PermissionResponseDecision::Allowed,
                    rule_updates: vec![PermissionUpdate::add_session_rule("Write".to_string())],
                    feedback: None,
                    updated_input: None,
                },
            },
        ));
        service.set_mode(PermissionMode::StrictConfirm).await;

        let decision = service
            .check_with_permissions("Write", &serde_json::json!({}), &[ToolPermission::Write])
            .await
            .unwrap();
        assert!(decision.is_allowed());
        assert_eq!(count.load(Ordering::SeqCst), 1);

        let decision = service
            .check_with_permissions("Write", &serde_json::json!({}), &[ToolPermission::Write])
            .await
            .unwrap();
        assert!(decision.is_allowed());
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "session approval should install a rule and skip the handler next time"
        );
    }

    #[tokio::test]
    async fn test_provider_transport_swap_preserves_session_approval_cache() {
        let count = Arc::new(AtomicUsize::new(0));
        let service = PermissionService::new().with_request_handler(Arc::new(
            CountingAllowHandler {
                count: count.clone(),
                response: PermissionResponse {
                    decision: PermissionResponseDecision::Allowed,
                    rule_updates: vec![PermissionUpdate::add_session_rule("Bash".to_string())],
                    feedback: None,
                    updated_input: None,
                },
            },
        ));
        service.set_mode(PermissionMode::StrictConfirm).await;

        let decision = service
            .check_with_permissions("Bash", &serde_json::json!({}), &[ToolPermission::Execute])
            .await
            .unwrap();
        assert!(decision.is_allowed());
        assert_eq!(count.load(Ordering::SeqCst), 1);

        service.replace_provider_preserving_cache(Arc::new(TestHumanLoopProvider));

        let decision = service
            .check_with_permissions(
                "Bash",
                &serde_json::json!({"command": "pwd"}),
                &[ToolPermission::Execute],
            )
            .await
            .unwrap();
        assert!(decision.is_allowed());
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "GUI provider transport swaps must not erase session approvals"
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
