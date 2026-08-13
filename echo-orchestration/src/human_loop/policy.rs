//! 审批策略引擎
//!
//! 提供可配置的工具审批策略类型。
//!
//! ## 核心概念
//!
//! - **ApprovalScope**: 审批缓存范围（Once/Session/SessionTool）
//!
//! **注意**: 审批策略逻辑已统一到 `PermissionService`。
//! 请使用 `PermissionService` 作为统一的权限检查入口。

// ── Approval Scope ────────────────────────────────────────────────────────────

/// 审批的范围/持久性
///
/// 参考 Claude Code 的 "Allow once / Allow for session" 语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ApprovalScope {
    /// 仅本次调用有效
    Once,
    /// 本次会话内，相同工具 + 相同参数不再请求审批
    Session,
    /// 本次会话内，该工具的所有调用不再请求审批
    SessionTool,
}
