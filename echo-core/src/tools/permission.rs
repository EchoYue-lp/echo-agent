//! 工具权限模型
//!
//! 提供多层级的权限控制系统：
//! - PermissionMode: 权限模式（default/plan/auto/bypass 等）
//! - PermissionRule: 规则系统（allow/deny/ask）
//! - RuleSource: 规则来源优先级
//! - RuleRegistry: 规则注册表
//!
//! 参考 Claude Code 的权限架构设计

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── 工具权限类型 ───────────────────────────────────────────────────────────────

/// 工具权限类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolPermission {
    /// 读取文件/目录权限
    Read,
    /// 写入文件/目录权限
    Write,
    /// 网络访问权限
    Network,
    /// 执行命令/代码权限
    Execute,
    /// 敏感操作权限（如访问密钥、环境变量等）
    Sensitive,
}

impl std::fmt::Display for ToolPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolPermission::Read => write!(f, "read"),
            ToolPermission::Write => write!(f, "write"),
            ToolPermission::Network => write!(f, "network"),
            ToolPermission::Execute => write!(f, "execute"),
            ToolPermission::Sensitive => write!(f, "sensitive"),
        }
    }
}

// ── 权限模式 ───────────────────────────────────────────────────────────────────

/// 权限模式 - 控制权限检查的行为
///
/// 参考 Claude Code 的 PermissionMode 设计：
/// - Default: 需要用户确认危险操作
/// - Plan: 只读模式
/// - AcceptEdits: 自动接受编辑
/// - BypassPermissions: 绕过所有检查（可被 bypass_disabled 禁用）
/// - Auto: AI 分类器自动决策
/// - Bubble: 子代理权限上浮
/// - DontAsk: 未匹配 allow 规则的工具静默拒绝（不提示用户）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// 默认模式：需要用户确认危险操作
    #[default]
    Default,
    /// 计划模式：只读，不允许写入和执行
    Plan,
    /// 自动接受文件编辑操作
    AcceptEdits,
    /// 绕过所有权限检查（谨慎使用）
    BypassPermissions,
    /// AI 分类器自动决策（需要 Classifier 实现）
    Auto,
    /// 子代理权限上浮到父进程
    Bubble,
    /// 静默模式：匹配 allow 规则的自动通过，未匹配的静默拒绝
    ///
    /// 介于 Default 和 BypassPermissions 之间的中间模式，
    /// 适合 CI/CD 等需要无人值守运行的场景。
    DontAsk,
}

impl PermissionMode {
    /// 检查当前模式是否允许写入操作
    pub fn allows_write(&self) -> bool {
        match self {
            PermissionMode::BypassPermissions => true,
            PermissionMode::AcceptEdits => true,
            PermissionMode::DontAsk => true, // 允许规则中的写入操作
            PermissionMode::Plan => false,
            _ => false,
        }
    }

    /// 检查当前模式是否需要用户交互
    pub fn requires_interaction(&self) -> bool {
        match self {
            PermissionMode::BypassPermissions => false,
            PermissionMode::Auto => false,
            PermissionMode::DontAsk => false, // 静默拒绝，不交互
            PermissionMode::AcceptEdits => false, // 编辑自动接受，其他仍需确认
            _ => true,
        }
    }

    /// 检查当前模式是否使用分类器
    pub fn uses_classifier(&self) -> bool {
        matches!(self, PermissionMode::Auto)
    }
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PermissionMode::Default => write!(f, "default"),
            PermissionMode::Plan => write!(f, "plan"),
            PermissionMode::AcceptEdits => write!(f, "acceptEdits"),
            PermissionMode::BypassPermissions => write!(f, "bypassPermissions"),
            PermissionMode::Auto => write!(f, "auto"),
            PermissionMode::Bubble => write!(f, "bubble"),
            PermissionMode::DontAsk => write!(f, "dontAsk"),
        }
    }
}

// ── 权限决策 ───────────────────────────────────────────────────────────────────

/// 权限决策
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    /// 允许执行
    Allow,
    /// 拒绝执行
    Deny {
        /// 拒绝原因
        reason: String,
    },
    /// 需要用户审批
    RequireApproval,
    /// 需要用户审批并提供建议
    Ask {
        /// 建议列表
        suggestions: Vec<String>,
    },
}

impl PermissionDecision {
    /// 检查是否为允许决策
    pub fn is_allowed(&self) -> bool {
        matches!(self, PermissionDecision::Allow)
    }

    /// 检查是否为拒绝决策
    pub fn is_denied(&self) -> bool {
        matches!(self, PermissionDecision::Deny { .. })
    }

    /// 检查是否需要用户审批
    pub fn requires_approval(&self) -> bool {
        matches!(
            self,
            PermissionDecision::RequireApproval | PermissionDecision::Ask { .. }
        )
    }
}

// ── 规则来源优先级 ─────────────────────────────────────────────────────────────

/// 规则来源优先级（数值越大优先级越高）
///
/// 参考 Claude Code 的 PERMISSION_RULE_SOURCES 设计。
/// 在 deny-first 评估中，来源优先级只影响同类型规则（deny/ask/allow）之间的顺序，
/// deny 规则始终优先于 ask 和 allow 规则。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum RuleSource {
    /// 默认规则（最低优先级）
    #[default]
    Default = 0,
    /// 本地设置（.echo/settings.local.json）
    LocalSettings = 1,
    /// 项目设置（.echo/settings.json）
    ProjectSettings = 2,
    /// 用户设置（~/.echo/settings.json）
    UserSettings = 3,
    /// 管理员策略（不可被用户覆盖，企业部署用）
    Managed = 4,
    /// 命令行参数
    CliArg = 5,
    /// 会话临时规则（最高优先级）
    Session = 6,
}

impl std::fmt::Display for RuleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleSource::Default => write!(f, "default"),
            RuleSource::LocalSettings => write!(f, "localSettings"),
            RuleSource::ProjectSettings => write!(f, "projectSettings"),
            RuleSource::UserSettings => write!(f, "userSettings"),
            RuleSource::Managed => write!(f, "managed"),
            RuleSource::CliArg => write!(f, "cliArg"),
            RuleSource::Session => write!(f, "session"),
        }
    }
}

// ── 规则匹配器 ─────────────────────────────────────────────────────────────────

/// 规则匹配器 - 定义规则如何匹配工具调用
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RuleMatcher {
    /// 精确工具名匹配
    Tool { name: String },
    /// 通配符模式匹配（支持 "Bash(git:*)" 等格式）
    Pattern { pattern: String },
    /// 按权限类型匹配
    Permission { permission: ToolPermission },
    /// 匹配所有工具
    All,
}

impl RuleMatcher {
    /// 检查 matcher 字符串是否匹配此 matcher（用于规则移除）
    ///
    /// 与 `parse_rule()` 中的 `RuleMatcher::Pattern` 构造语义保持一致：
    /// 移除时使用与添加时相同的 matcher 字符串来定位规则。
    pub fn matches_matcher_str(&self, matcher_str: &str) -> bool {
        match self {
            RuleMatcher::Tool { name } => name == matcher_str,
            RuleMatcher::Pattern { pattern } => pattern == matcher_str,
            RuleMatcher::Permission { .. } => false,
            RuleMatcher::All => matcher_str == "*" || matcher_str == "all",
        }
    }

    /// 检查是否匹配指定的工具
    pub fn matches(&self, tool_name: &str, permissions: &[ToolPermission]) -> bool {
        match self {
            RuleMatcher::Tool { name } => tool_name == name,
            RuleMatcher::Pattern { pattern } => {
                if pattern == "*" {
                    return true;
                }
                // Exact match first
                if tool_name == pattern {
                    return true;
                }
                // Use glob matching for patterns like "Bash(rm:*)"
                // Only return true on glob match; fall through to prefix check on non-match.
                #[cfg(feature = "permission")]
                {
                    if let Ok(glob) = globset::Glob::new(pattern) {
                        let matcher = glob.compile_matcher();
                        if matcher.is_match(tool_name) {
                            return true;
                        }
                    }
                }
                // Fallback: handle "prefix*)" patterns without globset.
                // E.g., "Bash(rm:*)" matches "Bash(rm:rf)".
                if pattern.ends_with("*)") {
                    let prefix = &pattern[..pattern.len() - 2];
                    if tool_name.starts_with(prefix) {
                        return true;
                    }
                }
                // Fallback: prefix match for "Bash" matching "Bash(git:*)"
                if tool_name.starts_with(pattern)
                    && tool_name.len() > pattern.len()
                    && tool_name.as_bytes()[pattern.len()] == b'('
                {
                    return true;
                }
                false
            }
            RuleMatcher::Permission { permission } => permissions.contains(permission),
            RuleMatcher::All => true,
        }
    }
}

impl std::fmt::Display for RuleMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleMatcher::Tool { name } => write!(f, "tool:{}", name),
            RuleMatcher::Pattern { pattern } => write!(f, "pattern:{}", pattern),
            RuleMatcher::Permission { permission } => write!(f, "permission:{}", permission),
            RuleMatcher::All => write!(f, "all"),
        }
    }
}

// ── 规则行为 ───────────────────────────────────────────────────────────────────

/// 规则行为 - 匹配后采取的动作
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RuleBehavior {
    /// 允许执行
    Allow,
    /// 拒绝执行
    Deny { reason: String },
    /// 需要用户确认
    Ask { suggestions: Vec<String> },
}

impl RuleBehavior {
    /// 转换为 PermissionDecision
    pub fn to_decision(&self) -> PermissionDecision {
        match self {
            RuleBehavior::Allow => PermissionDecision::Allow,
            RuleBehavior::Deny { reason } => PermissionDecision::Deny {
                reason: reason.clone(),
            },
            RuleBehavior::Ask { suggestions } => PermissionDecision::Ask {
                suggestions: suggestions.clone(),
            },
        }
    }
}

// ── 权限规则 ───────────────────────────────────────────────────────────────────

/// 权限规则 - 单条规则的完整定义
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// 规则匹配器
    pub matcher: RuleMatcher,
    /// 规则行为
    pub behavior: RuleBehavior,
    /// 规则来源
    pub source: RuleSource,
    /// 规则描述（可选）
    #[serde(default)]
    pub description: Option<String>,
}

impl PermissionRule {
    /// 创建允许规则
    pub fn allow(matcher: RuleMatcher, source: RuleSource) -> Self {
        Self {
            matcher,
            behavior: RuleBehavior::Allow,
            source,
            description: None,
        }
    }

    /// 创建拒绝规则
    pub fn deny(matcher: RuleMatcher, reason: String, source: RuleSource) -> Self {
        Self {
            matcher,
            behavior: RuleBehavior::Deny { reason },
            source,
            description: None,
        }
    }

    /// 创建询问规则
    pub fn ask(matcher: RuleMatcher, suggestions: Vec<String>, source: RuleSource) -> Self {
        Self {
            matcher,
            behavior: RuleBehavior::Ask { suggestions },
            source,
            description: None,
        }
    }

    /// 检查是否匹配指定的工具调用
    pub fn matches(&self, tool_name: &str, permissions: &[ToolPermission]) -> bool {
        self.matcher.matches(tool_name, permissions)
    }
}

// ── 规则注册表 ─────────────────────────────────────────────────────────────────

/// 规则注册表 - 管理所有权限规则
///
/// 规则按来源优先级排序，高优先级规则优先匹配。
/// 匹配顺序：
/// 1. 按来源优先级从高到低
/// 2. 同一来源内按添加顺序
#[derive(Debug, Clone, Default)]
pub struct RuleRegistry {
    rules: Vec<PermissionRule>,
    /// Index for fast exact tool name lookups: tool_name -> list of rule indices
    tool_index: HashMap<String, Vec<usize>>,
}

impl RuleRegistry {
    /// 创建空的规则注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加规则（自动按优先级排序）
    pub fn add_rule(&mut self, rule: PermissionRule) {
        // 按来源优先级插入排序
        let pos = self
            .rules
            .iter()
            .position(|r| r.source < rule.source)
            .unwrap_or(self.rules.len());

        // Build index entry for exact tool name matches
        if let RuleMatcher::Tool { name } = &rule.matcher {
            let entry = self.tool_index.entry(name.clone()).or_default();
            entry.push(pos);
        }

        self.rules.insert(pos, rule);
    }

    /// 批量添加规则
    pub fn add_rules(&mut self, rules: Vec<PermissionRule>) {
        for rule in rules {
            self.add_rule(rule);
        }
    }

    /// 检查工具调用，返回匹配的规则行为
    ///
    /// 评估顺序遵循 deny-first 原则（参考 Claude Code）：
    /// 1. Deny 规则 — 任何来源的 deny 都优先于所有 allow
    /// 2. Ask 规则 — 按来源优先级
    /// 3. Allow 规则 — 按来源优先级
    ///
    /// 这确保了一个低优先级的 deny 规则永远不会被高优先级的 allow 规则覆盖。
    pub fn check(&self, tool_name: &str, permissions: &[ToolPermission]) -> Option<RuleBehavior> {
        // Pass 1: Deny — any deny anywhere wins (full scan)
        for rule in &self.rules {
            if matches!(rule.behavior, RuleBehavior::Deny { .. })
                && rule.matches(tool_name, permissions)
            {
                return Some(rule.behavior.clone());
            }
        }
        // Pass 2: Ask — by source priority (rules are already sorted by source)
        for rule in &self.rules {
            if matches!(rule.behavior, RuleBehavior::Ask { .. })
                && rule.matches(tool_name, permissions)
            {
                return Some(rule.behavior.clone());
            }
        }
        // Pass 3: Allow — by source priority
        for rule in &self.rules {
            if matches!(rule.behavior, RuleBehavior::Allow) && rule.matches(tool_name, permissions)
            {
                return Some(rule.behavior.clone());
            }
        }
        None
    }

    /// 获取指定来源的所有规则
    pub fn rules_by_source(&self, source: RuleSource) -> Vec<&PermissionRule> {
        self.rules.iter().filter(|r| r.source == source).collect()
    }

    /// 移除指定来源的所有规则
    pub fn remove_by_source(&mut self, source: RuleSource) {
        self.rules.retain(|r| r.source != source);
        self.rebuild_tool_index();
    }

    /// 移除匹配指定 matcher 字符串的所有规则
    ///
    /// 返回被移除的规则数量。匹配方式与 `parse_rule()` 的 `AddRule` 语义一致。
    pub fn remove_by_matcher(&mut self, matcher_str: &str) -> usize {
        let before = self.rules.len();
        self.rules
            .retain(|r| !r.matcher.matches_matcher_str(matcher_str));
        let removed = before - self.rules.len();
        if removed > 0 {
            self.rebuild_tool_index();
        }
        removed
    }

    /// 重建 tool_index（在移除规则后调用）
    fn rebuild_tool_index(&mut self) {
        self.tool_index.clear();
        for (i, rule) in self.rules.iter().enumerate() {
            if let RuleMatcher::Tool { name } = &rule.matcher {
                self.tool_index.entry(name.clone()).or_default().push(i);
            }
        }
    }

    /// 清空所有规则
    pub fn clear(&mut self) {
        self.rules.clear();
        self.tool_index.clear();
    }

    /// 获取规则数量
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// 获取所有规则
    pub fn all_rules(&self) -> &[PermissionRule] {
        &self.rules
    }
}

// ── 权限策略 trait ───────────────────────────────────────────────────────────────

/// 权限策略 trait
pub trait PermissionPolicy: Send + Sync {
    fn check<'a>(
        &'a self,
        tool_name: &'a str,
        permissions: &'a [ToolPermission],
    ) -> BoxFuture<'a, PermissionDecision>;
}

/// 默认权限策略（保留向后兼容）
pub struct DefaultPermissionPolicy {
    granted: HashSet<ToolPermission>,
    approval_required: HashSet<ToolPermission>,
}

impl Default for DefaultPermissionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultPermissionPolicy {
    pub fn new() -> Self {
        let mut approval_required = HashSet::new();
        approval_required.insert(ToolPermission::Execute);
        approval_required.insert(ToolPermission::Sensitive);

        Self {
            granted: HashSet::new(),
            approval_required,
        }
    }

    pub fn grant(mut self, perm: ToolPermission) -> Self {
        self.granted.insert(perm);
        self.approval_required.remove(&perm);
        self
    }

    pub fn require_approval(mut self, perm: ToolPermission) -> Self {
        self.approval_required.insert(perm);
        self.granted.remove(&perm);
        self
    }

    pub fn grant_all(mut self) -> Self {
        self.granted.insert(ToolPermission::Read);
        self.granted.insert(ToolPermission::Write);
        self.granted.insert(ToolPermission::Network);
        self.granted.insert(ToolPermission::Execute);
        self.granted.insert(ToolPermission::Sensitive);
        self.approval_required.clear();
        self
    }
}

impl PermissionPolicy for DefaultPermissionPolicy {
    fn check<'a>(
        &'a self,
        _tool_name: &'a str,
        permissions: &'a [ToolPermission],
    ) -> BoxFuture<'a, PermissionDecision> {
        Box::pin(async move {
            if permissions.is_empty() {
                return PermissionDecision::Allow;
            }

            let mut need_approval = Vec::new();
            let mut denied = Vec::new();

            for perm in permissions {
                if self.granted.contains(perm) {
                    continue;
                }
                if self.approval_required.contains(perm) {
                    need_approval.push(*perm);
                } else {
                    denied.push(*perm);
                }
            }

            if !denied.is_empty() {
                let names: Vec<String> = denied.iter().map(|p| p.to_string()).collect();
                return PermissionDecision::Deny {
                    reason: format!("未授权的权限: {}", names.join(", ")),
                };
            }

            if !need_approval.is_empty() {
                return PermissionDecision::RequireApproval;
            }

            PermissionDecision::Allow
        })
    }
}

// ── 单元测试 ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_mode_default() {
        let mode = PermissionMode::default();
        assert_eq!(mode, PermissionMode::Default);
        assert!(mode.requires_interaction());
        assert!(!mode.uses_classifier());
    }

    #[test]
    fn test_permission_mode_bypass() {
        let mode = PermissionMode::BypassPermissions;
        assert!(mode.allows_write());
        assert!(!mode.requires_interaction());
    }

    #[test]
    fn test_permission_mode_plan() {
        let mode = PermissionMode::Plan;
        assert!(!mode.allows_write());
        assert!(mode.requires_interaction());
    }

    #[test]
    fn test_permission_mode_auto() {
        let mode = PermissionMode::Auto;
        assert!(!mode.requires_interaction());
        assert!(mode.uses_classifier());
    }

    #[test]
    fn test_rule_source_ordering() {
        assert!(RuleSource::Session > RuleSource::CliArg);
        assert!(RuleSource::CliArg > RuleSource::UserSettings);
        assert!(RuleSource::UserSettings > RuleSource::ProjectSettings);
        assert!(RuleSource::ProjectSettings > RuleSource::LocalSettings);
        assert!(RuleSource::LocalSettings > RuleSource::Default);
    }

    #[test]
    fn test_rule_matcher_tool() {
        let matcher = RuleMatcher::Tool {
            name: "Bash".to_string(),
        };
        assert!(matcher.matches("Bash", &[]));
        assert!(!matcher.matches("Read", &[]));
    }

    #[test]
    fn test_rule_matcher_pattern() {
        let matcher = RuleMatcher::Pattern {
            pattern: "Bash".to_string(),
        };
        assert!(matcher.matches("Bash", &[]));
        assert!(matcher.matches("Bash(git:*)", &[]));
        assert!(!matcher.matches("BashExtra", &[]));
    }

    #[test]
    fn test_rule_matcher_wildcard() {
        let matcher = RuleMatcher::Pattern {
            pattern: "*".to_string(),
        };
        assert!(matcher.matches("Bash", &[]));
        assert!(matcher.matches("Read", &[]));
        assert!(matcher.matches("Write", &[]));
    }

    #[test]
    fn test_rule_matcher_permission() {
        let matcher = RuleMatcher::Permission {
            permission: ToolPermission::Execute,
        };
        assert!(matcher.matches("shell", &[ToolPermission::Execute]));
        assert!(!matcher.matches("read", &[ToolPermission::Read]));
    }

    #[test]
    fn test_permission_rule_create() {
        let rule = PermissionRule::allow(
            RuleMatcher::Tool {
                name: "Read".to_string(),
            },
            RuleSource::UserSettings,
        );
        assert_eq!(rule.behavior, RuleBehavior::Allow);
        assert_eq!(rule.source, RuleSource::UserSettings);
    }

    #[test]
    fn test_rule_registry_add() {
        let mut registry = RuleRegistry::new();

        // 添加低优先级规则
        registry.add_rule(PermissionRule::deny(
            RuleMatcher::All,
            "default deny".to_string(),
            RuleSource::Default,
        ));

        // 添加高优先级规则
        registry.add_rule(PermissionRule::allow(
            RuleMatcher::Tool {
                name: "Read".to_string(),
            },
            RuleSource::UserSettings,
        ));

        // 高优先级规则应该在前面
        assert_eq!(registry.rules[0].source, RuleSource::UserSettings);
        assert_eq!(registry.rules[1].source, RuleSource::Default);
    }

    #[test]
    fn test_rule_registry_check() {
        let mut registry = RuleRegistry::new();

        // 默认拒绝所有
        registry.add_rule(PermissionRule::deny(
            RuleMatcher::All,
            "default deny".to_string(),
            RuleSource::Default,
        ));

        // 用户设置允许 Read
        registry.add_rule(PermissionRule::allow(
            RuleMatcher::Tool {
                name: "Read".to_string(),
            },
            RuleSource::UserSettings,
        ));

        // deny-first: 即使有高优先级 Allow，Deny All 仍然匹配
        // Read 匹配 Deny(All) → 被拒绝
        let result = registry.check("Read", &[]);
        assert_eq!(
            result,
            Some(RuleBehavior::Deny {
                reason: "default deny".to_string()
            })
        );

        // Bash 也匹配 Deny(All) → 被拒绝
        let result = registry.check("Bash", &[]);
        assert!(matches!(result, Some(RuleBehavior::Deny { .. })));
    }

    #[test]
    fn test_rule_registry_allow_without_deny() {
        let mut registry = RuleRegistry::new();

        // 只有 allow 规则
        registry.add_rule(PermissionRule::allow(
            RuleMatcher::Tool {
                name: "Read".to_string(),
            },
            RuleSource::UserSettings,
        ));

        // Read 匹配 Allow → 允许
        let result = registry.check("Read", &[]);
        assert_eq!(result, Some(RuleBehavior::Allow));

        // Bash 无匹配规则
        let result = registry.check("Bash", &[]);
        assert_eq!(result, None);
    }

    #[test]
    fn test_rule_registry_deny_first_ordering() {
        let mut registry = RuleRegistry::new();

        // 高优先级 allow
        registry.add_rule(PermissionRule::allow(
            RuleMatcher::Pattern {
                pattern: "Bash".to_string(),
            },
            RuleSource::UserSettings,
        ));
        // 低优先级 deny
        registry.add_rule(PermissionRule::deny(
            RuleMatcher::Pattern {
                pattern: "Bash(rm:*)".to_string(),
            },
            "dangerous".to_string(),
            RuleSource::Default,
        ));

        // Bash(rm:rf) — deny 匹配，deny 优先于 allow
        let result = registry.check("Bash(rm:rf)", &[]);
        assert!(matches!(result, Some(RuleBehavior::Deny { .. })));

        // Bash(ls) — 只有 allow 匹配
        let result = registry.check("Bash(ls)", &[]);
        assert_eq!(result, Some(RuleBehavior::Allow));
    }

    #[test]
    fn test_rule_registry_ask_between_deny_and_allow() {
        let mut registry = RuleRegistry::new();

        registry.add_rule(PermissionRule::allow(
            RuleMatcher::Pattern {
                pattern: "Bash".to_string(),
            },
            RuleSource::UserSettings,
        ));
        registry.add_rule(PermissionRule::ask(
            RuleMatcher::Pattern {
                pattern: "Bash(rm:*)".to_string(),
            },
            vec!["确认".to_string()],
            RuleSource::Default,
        ));

        // Bash(rm:rf) — Ask 规则匹配，优先于 Allow
        let result = registry.check("Bash(rm:rf)", &[]);
        assert!(matches!(result, Some(RuleBehavior::Ask { .. })));

        // Bash(git:status) — 只有 Allow 规则匹配
        let result = registry.check("Bash(git:status)", &[]);
        assert_eq!(result, Some(RuleBehavior::Allow));
    }

    #[test]
    fn test_permission_decision_is_allowed() {
        assert!(PermissionDecision::Allow.is_allowed());
        assert!(
            !PermissionDecision::Deny {
                reason: "test".to_string()
            }
            .is_allowed()
        );
    }

    #[test]
    fn test_permission_decision_requires_approval() {
        assert!(PermissionDecision::RequireApproval.requires_approval());
        assert!(
            PermissionDecision::Ask {
                suggestions: vec!["yes".to_string()]
            }
            .requires_approval()
        );
        assert!(!PermissionDecision::Allow.requires_approval());
    }

    #[tokio::test]
    async fn test_empty_permissions_allowed() {
        let policy = DefaultPermissionPolicy::new();
        let decision = policy.check("tool", &[]).await;
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[tokio::test]
    async fn test_granted_permission() {
        let policy = DefaultPermissionPolicy::new().grant(ToolPermission::Read);
        let decision = policy.check("tool", &[ToolPermission::Read]).await;
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[tokio::test]
    async fn test_execute_requires_approval() {
        let policy = DefaultPermissionPolicy::new();
        let decision = policy.check("tool", &[ToolPermission::Execute]).await;
        assert!(matches!(decision, PermissionDecision::RequireApproval));
    }

    #[tokio::test]
    async fn test_ungranted_denied() {
        let policy = DefaultPermissionPolicy::new();
        let decision = policy.check("tool", &[ToolPermission::Write]).await;
        assert!(matches!(decision, PermissionDecision::Deny { .. }));
    }

    #[tokio::test]
    async fn test_grant_all() {
        let policy = DefaultPermissionPolicy::new().grant_all();
        let decision = policy
            .check(
                "tool",
                &[
                    ToolPermission::Read,
                    ToolPermission::Write,
                    ToolPermission::Execute,
                    ToolPermission::Sensitive,
                ],
            )
            .await;
        assert!(matches!(decision, PermissionDecision::Allow));
    }
}
