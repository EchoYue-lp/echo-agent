//! Tool permission model
//!
//! Provides a multi-level permission control system:
//! - PermissionMode: permission mode (default/plan/auto/bypass etc.)
//! - PermissionRule: rule system (allow/deny/ask)
//! - RuleSource: rule source priority
//! - RuleRegistry: rule registry
//!
//! Referenced from Claude Code's permission architecture design

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ── Tool Permission Types ──────────────────────────────────────────────────────

/// Tool permission types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolPermission {
    /// Read files/directories permission
    Read,
    /// Write files/directories permission
    Write,
    /// Network access permission
    Network,
    /// Execute commands/code permission
    Execute,
    /// Sensitive operation permission (e.g. access keys, environment variables, etc.)
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

// ── Permission Mode ────────────────────────────────────────────────────────────

/// Permission mode - controls permission check behavior
///
/// Referenced from Claude Code's PermissionMode design:
/// - Default: require user confirmation for dangerous operations
/// - Plan: internal read-only execution mode (not a user-facing approval mode)
/// - AcceptEdits: automatically accept edits
/// - BypassPermissions: bypass all checks (can be disabled by bypass_disabled)
/// - Auto: AI classifier auto-decides
/// - Bubble: subagent permissions bubble up
/// - DontAsk: silently reject tools not matching an allow rule (no user prompt)
/// - StrictConfirm: ask before write/execute/network/sensitive operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Default mode: require user confirmation for dangerous operations
    #[default]
    Default,
    /// Internal read-only execution mode: disallow writes and executes.
    /// User-facing planning/task routing should use interaction mode instead.
    Plan,
    /// Automatically accept file edit operations
    #[serde(
        rename = "auto-edit",
        alias = "accept-edits",
        alias = "autoedit",
        alias = "acceptedits"
    )]
    AcceptEdits,
    /// Bypass all permission checks (use with caution)
    #[serde(
        rename = "full-auto",
        alias = "fullauto",
        alias = "bypass",
        alias = "bypass-permissions",
        alias = "bypasspermissions"
    )]
    BypassPermissions,
    /// AI classifier auto-decides (requires Classifier implementation)
    Auto,
    /// Sub-agent permissions bubble up to parent process
    Bubble,
    /// Silent mode: auto-allow rules that match, silently reject those that don't
    ///
    /// An intermediate mode between Default and BypassPermissions,
    /// suitable for CI/CD and other unattended execution scenarios.
    #[serde(rename = "dont-ask", alias = "dontask")]
    DontAsk,
    /// Strict interactive mode: reads are allowed, mutating or external operations ask.
    #[serde(
        rename = "strict",
        alias = "strict-confirm",
        alias = "strict-confirmation"
    )]
    StrictConfirm,
}

impl PermissionMode {
    /// Stable kebab-case identifier used by configuration and UI transports.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::AcceptEdits => "auto-edit",
            Self::BypassPermissions => "full-auto",
            Self::Auto => "auto",
            Self::Bubble => "bubble",
            Self::DontAsk => "dont-ask",
            Self::StrictConfirm => "strict",
        }
    }

    /// 检查当前模式是否允许写入操作
    pub fn allows_write(&self) -> bool {
        match self {
            PermissionMode::BypassPermissions => true,
            PermissionMode::AcceptEdits => true,
            // DontAsk defers to rule matching: writes are only allowed
            // if an explicit allow rule matches; otherwise silently rejected.
            PermissionMode::DontAsk => false,
            PermissionMode::Plan => false,
            PermissionMode::StrictConfirm => false,
            _ => false,
        }
    }

    /// 检查当前模式是否需要用户交互
    pub fn requires_interaction(&self) -> bool {
        match self {
            PermissionMode::BypassPermissions => false,
            PermissionMode::Auto => false,
            PermissionMode::DontAsk => false, // silently reject, no interaction
            PermissionMode::AcceptEdits => false, // edits auto-accepted, others still need confirmation
            PermissionMode::StrictConfirm => true,
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
        f.write_str(self.id())
    }
}

impl std::str::FromStr for PermissionMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "ask" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            "auto-edit" | "autoedit" | "accept-edits" | "acceptedits" => Ok(Self::AcceptEdits),
            "full-auto" | "fullauto" | "bypass" | "bypass-permissions" | "bypasspermissions" => {
                Ok(Self::BypassPermissions)
            }
            "auto" => Ok(Self::Auto),
            "bubble" => Ok(Self::Bubble),
            "dont-ask" | "dontask" => Ok(Self::DontAsk),
            "strict" | "strict-confirm" | "strict-confirmation" => Ok(Self::StrictConfirm),
            other => Err(format!(
                "invalid permission mode '{other}'; expected default, plan, auto-edit, full-auto, auto, bubble, dont-ask, or strict"
            )),
        }
    }
}

// ── Permission Decision ────────────────────────────────────────────────────────

/// Permission decision
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionDecision {
    /// Allow execution
    Allow,
    /// Deny execution
    Deny {
        /// Denial reason
        reason: String,
    },
    /// Require user approval
    RequireApproval,
    /// Require user approval with suggestions
    Ask {
        /// Suggestion list
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

// ── Rule Source Priority ───────────────────────────────────────────────────────

/// Rule source priority (higher value = higher priority)
///
/// Referenced from Claude Code's PERMISSION_RULE_SOURCES design.
/// In deny-first evaluation, source priority only affects ordering among
/// rules of the same type (deny/ask/allow); deny rules always take
/// precedence over ask and allow rules.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum RuleSource {
    /// Default rule (lowest priority)
    #[default]
    Default = 0,
    /// Local settings (.echo/settings.local.json)
    LocalSettings = 1,
    /// Project settings (.echo/settings.json)
    ProjectSettings = 2,
    /// User settings (~/.echo/settings.json)
    UserSettings = 3,
    /// Admin policy (cannot be overridden by users, for enterprise deployment)
    Managed = 4,
    /// CLI argument
    CliArg = 5,
    /// Session temporary rule (highest priority)
    Session = 6,
}

impl std::str::FromStr for RuleSource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "localSettings" | "local_settings" => Ok(Self::LocalSettings),
            "projectSettings" | "project_settings" => Ok(Self::ProjectSettings),
            "userSettings" | "user_settings" | "manual" => Ok(Self::UserSettings),
            "managed" => Ok(Self::Managed),
            "cliArg" | "cli_arg" => Ok(Self::CliArg),
            "session" => Ok(Self::Session),
            _ => Err(format!("unknown permission rule source: {value}")),
        }
    }
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

// ── Rule Matcher ───────────────────────────────────────────────────────────────

/// Rule matcher - defines how a rule matches tool invocations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RuleMatcher {
    /// Exact tool name match
    Tool { name: String },
    /// Wildcard pattern match (supports "Bash(git:*)" etc.)
    Pattern { pattern: String },
    /// Match by permission type
    Permission { permission: ToolPermission },
    /// Match all tools
    All,
}

impl std::str::FromStr for RuleMatcher {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "*" || value == "all" {
            return Ok(Self::All);
        }
        if let Some(name) = value.strip_prefix("tool:") {
            if name.is_empty() {
                return Err("tool permission matcher requires a name".to_string());
            }
            return Ok(Self::Tool {
                name: name.to_string(),
            });
        }
        if let Some(pattern) = value.strip_prefix("pattern:") {
            if pattern.is_empty() {
                return Err("pattern permission matcher cannot be empty".to_string());
            }
            return Ok(Self::Pattern {
                pattern: pattern.to_string(),
            });
        }
        let flag = value
            .strip_prefix("perm:")
            .or_else(|| value.strip_prefix("permission:"));
        if let Some(flag) = flag {
            let permission = match flag {
                "read" => ToolPermission::Read,
                "write" => ToolPermission::Write,
                "network" => ToolPermission::Network,
                "execute" => ToolPermission::Execute,
                "sensitive" => ToolPermission::Sensitive,
                _ => return Err(format!("unknown permission matcher: {flag}")),
            };
            return Ok(Self::Permission { permission });
        }
        Err(format!("unsupported permission matcher: {value}"))
    }
}

impl RuleMatcher {
    /// Check whether a matcher string matches this matcher (for rule removal)
    ///
    /// Consistent with the `RuleMatcher::Pattern` construction semantics in
    /// `parse_rule()`: uses the same matcher string for removal as for addition
    /// to locate rules.
    pub fn matches_matcher_str(&self, matcher_str: &str) -> bool {
        match self {
            RuleMatcher::Tool { name } => name == matcher_str,
            RuleMatcher::Pattern { pattern } => pattern == matcher_str,
            RuleMatcher::Permission { .. } => false,
            RuleMatcher::All => matcher_str == "*" || matcher_str == "all",
        }
    }

    /// Check whether this matches the specified tool
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

// ── Rule Behavior ──────────────────────────────────────────────────────────────

/// Rule behavior - the action taken after a match
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RuleBehavior {
    /// Allow execution
    Allow,
    /// Deny execution
    Deny { reason: String },
    /// Require user confirmation
    Ask { suggestions: Vec<String> },
}

impl std::str::FromStr for RuleBehavior {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny {
                reason: "denied by rule".to_string(),
            }),
            "ask" => Ok(Self::Ask {
                suggestions: vec!["allow".to_string(), "deny".to_string()],
            }),
            _ => Err(format!("unknown permission rule behavior: {value}")),
        }
    }
}

impl RuleBehavior {
    /// Convert to PermissionDecision
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

// ── Permission Rule ────────────────────────────────────────────────────────────

/// Permission rule - complete definition of a single rule
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Rule matcher
    pub matcher: RuleMatcher,
    /// Rule behavior
    pub behavior: RuleBehavior,
    /// Rule source
    pub source: RuleSource,
    /// Rule description (optional)
    #[serde(default)]
    pub description: Option<String>,
}

impl PermissionRule {
    /// Create an allow rule
    pub fn allow(matcher: RuleMatcher, source: RuleSource) -> Self {
        Self {
            matcher,
            behavior: RuleBehavior::Allow,
            source,
            description: None,
        }
    }

    /// Create a deny rule
    pub fn deny(matcher: RuleMatcher, reason: String, source: RuleSource) -> Self {
        Self {
            matcher,
            behavior: RuleBehavior::Deny { reason },
            source,
            description: None,
        }
    }

    /// Create an ask rule
    pub fn ask(matcher: RuleMatcher, suggestions: Vec<String>, source: RuleSource) -> Self {
        Self {
            matcher,
            behavior: RuleBehavior::Ask { suggestions },
            source,
            description: None,
        }
    }

    /// Check whether this matches the specified tool invocation
    pub fn matches(&self, tool_name: &str, permissions: &[ToolPermission]) -> bool {
        self.matcher.matches(tool_name, permissions)
    }
}

// ── Rule Registry ──────────────────────────────────────────────────────────────

/// Rule registry - manages all permission rules
///
/// Rules are sorted by source priority; higher priority rules match first.
/// Matching order:
/// 1. By source priority from high to low
/// 2. Within the same source, by insertion order
#[derive(Debug, Clone, Default)]
pub struct RuleRegistry {
    rules: Vec<PermissionRule>,
}

impl RuleRegistry {
    /// Create an empty rule registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a rule (auto-sorted by priority)
    pub fn add_rule(&mut self, rule: PermissionRule) {
        // 按来源优先级插入排序
        let pos = self
            .rules
            .iter()
            .position(|r| r.source < rule.source)
            .unwrap_or(self.rules.len());

        self.rules.insert(pos, rule);
    }

    /// Batch add rules
    pub fn add_rules(&mut self, rules: Vec<PermissionRule>) {
        for rule in rules {
            self.add_rule(rule);
        }
    }

    /// Remove one exact rule without affecting rules owned by other sources.
    pub fn remove_rule(&mut self, rule: &PermissionRule) -> bool {
        let before = self.rules.len();
        self.rules.retain(|candidate| candidate != rule);
        before != self.rules.len()
    }

    /// Check a tool invocation and return the matching rule behavior
    ///
    /// Evaluation order follows deny-first principle (referenced from Claude Code):
    /// 1. Deny rules — any deny from any source takes precedence over all allows
    /// 2. Ask rules — by source priority
    /// 3. Allow rules — by source priority
    ///
    /// This ensures that a low-priority deny rule can never be overridden by a
    /// high-priority allow rule.
    ///
    /// Implemented as a single pass: deny returns immediately, ask and allow
    /// are tracked and the highest-priority match is returned after the loop.
    pub fn check(&self, tool_name: &str, permissions: &[ToolPermission]) -> Option<RuleBehavior> {
        let mut first_ask: Option<RuleBehavior> = None;
        let mut first_allow: Option<RuleBehavior> = None;

        for rule in &self.rules {
            if !rule.matches(tool_name, permissions) {
                continue;
            }
            match &rule.behavior {
                RuleBehavior::Deny { .. } => return Some(rule.behavior.clone()),
                RuleBehavior::Ask { .. } => {
                    if first_ask.is_none() {
                        first_ask = Some(rule.behavior.clone());
                    }
                }
                RuleBehavior::Allow => {
                    if first_allow.is_none() {
                        first_allow = Some(rule.behavior.clone());
                    }
                }
            }
        }

        first_ask.or(first_allow)
    }

    /// Get all rules from the specified source
    pub fn rules_by_source(&self, source: RuleSource) -> Vec<&PermissionRule> {
        self.rules.iter().filter(|r| r.source == source).collect()
    }

    /// Remove all rules from the specified source
    pub fn remove_by_source(&mut self, source: RuleSource) {
        self.rules.retain(|r| r.source != source);
    }

    /// Remove all rules matching the specified matcher string
    ///
    /// Returns the number of removed rules. Matches with the same semantics as
    /// `parse_rule()`'s `AddRule`.
    pub fn remove_by_matcher(&mut self, matcher_str: &str) -> usize {
        let before = self.rules.len();
        self.rules
            .retain(|r| !r.matcher.matches_matcher_str(matcher_str));
        before - self.rules.len()
    }

    /// Clear all rules
    pub fn clear(&mut self) {
        self.rules.clear();
    }

    /// Get the number of rules
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Check whether it is empty
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Get all rules
    pub fn all_rules(&self) -> &[PermissionRule] {
        &self.rules
    }
}

// ── Permission Policy Trait ────────────────────────────────────────────────────

/// Permission policy trait
pub trait PermissionPolicy: Send + Sync {
    fn check<'a>(
        &'a self,
        tool_name: &'a str,
        permissions: &'a [ToolPermission],
    ) -> BoxFuture<'a, PermissionDecision>;
}

/// Default permission policy (retains backward compatibility)
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
                    reason: format!("Unauthorized permissions: {}", names.join(", ")),
                };
            }

            if !need_approval.is_empty() {
                return PermissionDecision::RequireApproval;
            }

            PermissionDecision::Allow
        })
    }
}

// ── Unit Tests ─────────────────────────────────────────────────────────────────

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
    fn permission_mode_ids_and_aliases_are_framework_owned() {
        let cases = [
            ("default", PermissionMode::Default),
            ("ask", PermissionMode::Default),
            ("auto-edit", PermissionMode::AcceptEdits),
            ("acceptEdits", PermissionMode::AcceptEdits),
            ("full-auto", PermissionMode::BypassPermissions),
            ("bypassPermissions", PermissionMode::BypassPermissions),
            ("dont-ask", PermissionMode::DontAsk),
            ("dontAsk", PermissionMode::DontAsk),
            ("strict", PermissionMode::StrictConfirm),
        ];
        for (wire, expected) in cases {
            assert_eq!(wire.parse::<PermissionMode>(), Ok(expected));
            assert_eq!(expected.id(), expected.to_string());
        }
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
    fn remove_rule_keeps_other_sources_and_matchers() {
        let mut registry = RuleRegistry::new();
        let removable = PermissionRule::allow(
            RuleMatcher::Tool {
                name: "Read".to_string(),
            },
            RuleSource::UserSettings,
        );
        registry.add_rule(removable.clone());
        registry.add_rule(PermissionRule::deny(
            RuleMatcher::All,
            "default deny".to_string(),
            RuleSource::Default,
        ));

        assert!(registry.remove_rule(&removable));
        assert!(!registry.remove_rule(&removable));
        assert_eq!(registry.len(), 1);
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
            vec!["Confirm".to_string()],
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

    #[test]
    fn rule_wire_values_parse_into_framework_types() {
        assert_eq!(
            "tool:read_file".parse::<RuleMatcher>(),
            Ok(RuleMatcher::Tool {
                name: "read_file".to_string()
            })
        );
        assert_eq!(
            "deny".parse::<RuleBehavior>(),
            Ok(RuleBehavior::Deny {
                reason: "denied by rule".to_string(),
            })
        );
        assert_eq!("session".parse::<RuleSource>(), Ok(RuleSource::Session));
        assert!("tool:".parse::<RuleMatcher>().is_err());
    }
}
