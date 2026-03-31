//! 工具权限模型
//!
//! 类似 MCP 的权限声明系统，敏感工具需要显式授权才能执行。
//!
//! # 核心类型
//!
//! - [`ToolPermission`]: 工具所需的权限类型
//! - [`PermissionPolicy`]: 权限策略 trait
//! - [`DefaultPermissionPolicy`]: 默认策略实现

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// 工具权限类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolPermission {
    /// 读取文件或数据
    Read,
    /// 写入文件或数据
    Write,
    /// 网络请求
    Network,
    /// 执行系统命令
    Execute,
    /// 涉及敏感数据
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

/// 权限决策
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// 允许执行
    Allow,
    /// 拒绝执行
    Deny { reason: String },
    /// 需要人工审批
    RequireApproval,
}

/// 权限策略 trait
///
/// 根据工具名称和所需权限做出执行决策。
pub trait PermissionPolicy: Send + Sync {
    fn check<'a>(
        &'a self,
        tool_name: &'a str,
        permissions: &'a [ToolPermission],
    ) -> BoxFuture<'a, PermissionDecision>;
}

/// 默认权限策略
///
/// 基于已授权权限集合和需审批权限集合做决策。
/// 未明确授权的 `Execute` 和 `Sensitive` 默认需要审批。
///
/// # 示例
///
/// ```rust
/// use echo_agent::tools::permission::{DefaultPermissionPolicy, ToolPermission};
///
/// let policy = DefaultPermissionPolicy::new()
///     .grant(ToolPermission::Read)
///     .grant(ToolPermission::Network)
///     .require_approval(ToolPermission::Execute);
/// ```
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

    /// 授予权限
    pub fn grant(mut self, perm: ToolPermission) -> Self {
        self.granted.insert(perm);
        self.approval_required.remove(&perm);
        self
    }

    /// 标记权限需要审批
    pub fn require_approval(mut self, perm: ToolPermission) -> Self {
        self.approval_required.insert(perm);
        self.granted.remove(&perm);
        self
    }

    /// 授予所有权限（不再需要任何审批）
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

#[cfg(test)]
mod tests {
    use super::*;

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
