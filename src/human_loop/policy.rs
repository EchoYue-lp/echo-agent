//! 审批策略引擎
//!
//! 提供可配置的工具审批策略接口。
//!
//! ## 核心概念
//!
//! - **ApprovalScope**: 审批范围（Once/Session/SessionAllTools）
//! - **ApprovalRule**: 匹配工具调用的规则，决定风险等级
//! - **ApprovalPolicy**: 策略 trait，评估工具调用是否需要审批
//! - **PolicyDecision**: 策略评估结果
//!
//! **注意**: `DefaultApprovalPolicy` 和 `LegacyApprovalPolicy` 已移除。
//! 请使用 `PermissionService` 作为统一的权限检查入口。

use futures::future::BoxFuture;
use serde_json::Value;

use crate::human_loop::permission::RiskLevel;

// ── Approval Scope ────────────────────────────────────────────────────────────

/// 审批的范围/持久性
///
/// 参考 Claude Code 的 "Allow once / Allow for session" 语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalScope {
    /// 仅本次调用有效
    Once,
    /// 本次会话内，相同工具 + 相同参数不再请求审批
    Session,
    /// 本次会话内，该工具的所有调用不再请求审批
    SessionAllTools,
}

// ── Approval Rule ─────────────────────────────────────────────────────────────

/// 审批规则：匹配工具名并指定风险等级
///
/// 支持三种匹配方式：
/// - 精确匹配: `"Bash"` 匹配工具名 `Bash`
/// - 前缀匹配: `"Bash(rm"` 匹配 `Bash(rm:*)`
/// - 通配符: `"*"` 匹配所有工具
#[derive(Debug, Clone)]
pub struct ApprovalRule {
    /// 工具名匹配模式
    pub pattern: String,
    /// 风险等级
    pub risk_level: RiskLevel,
    /// 规则描述（可选）
    pub description: Option<String>,
}

impl ApprovalRule {
    /// 创建新的审批规则（默认 Medium 风险）
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            risk_level: RiskLevel::Medium,
            description: None,
        }
    }

    /// 设置风险等级
    pub fn risk(mut self, level: RiskLevel) -> Self {
        self.risk_level = level;
        self
    }

    /// 设置规则描述
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// 检查规则是否匹配给定的工具名
    pub fn matches(&self, tool_name: &str) -> bool {
        crate::human_loop::pattern::matches_tool_pattern(&self.pattern, tool_name)
    }
}

// ── Policy Decision ────────────────────────────────────────────────────────────

/// 策略评估结果
#[derive(Debug, Clone)]
pub enum PolicyDecision {
    /// 自动批准（Low risk 或缓存命中）
    AutoApprove {
        /// 批准原因
        reason: String,
    },
    /// 需要人工审批
    RequireApproval {
        /// 风险等级
        risk_level: RiskLevel,
        /// 给用户的提示信息
        prompt: String,
    },
    /// 自动拒绝
    AutoDeny {
        /// 拒绝原因
        reason: String,
    },
}

// ── Approval Policy Trait ─────────────────────────────────────────────────────

/// 审批策略 trait
///
/// 核心接口：评估工具调用是否需要人工审批。
/// 对于新项目，推荐直接使用 [`crate::human_loop::PermissionService`]。
pub trait ApprovalPolicy: Send + Sync {
    /// 评估工具调用是否需要审批
    fn evaluate<'a>(&'a self, tool_name: &'a str, args: &'a Value)
    -> BoxFuture<'a, PolicyDecision>;

    /// 记录审批决策（用于缓存）
    fn record_decision(&self, tool_name: &str, args: &Value, approved: bool, scope: ApprovalScope);

    /// 检查是否有缓存决策
    fn is_cached(&self, tool_name: &str, args: &Value) -> bool;
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_requires_approval() {
        assert!(!RiskLevel::Low.requires_confirmation());
        assert!(!RiskLevel::Medium.requires_confirmation());
        assert!(RiskLevel::High.requires_confirmation());
        assert!(RiskLevel::Critical.requires_confirmation());
    }

    #[test]
    fn test_approval_rule_exact_match() {
        let rule = ApprovalRule::new("Bash").risk(RiskLevel::High);
        assert!(rule.matches("Bash"));
        assert!(!rule.matches("Read"));
    }

    #[test]
    fn test_approval_rule_wildcard() {
        let rule = ApprovalRule::new("*").risk(RiskLevel::Medium);
        assert!(rule.matches("Bash"));
        assert!(rule.matches("Read"));
        assert!(rule.matches("Anything"));
    }

    #[test]
    fn test_approval_rule_prefix_match() {
        let rule = ApprovalRule::new("Bash").risk(RiskLevel::High);
        assert!(rule.matches("Bash(rm:*)"));
        assert!(!rule.matches("BashExtra"));
    }
}
