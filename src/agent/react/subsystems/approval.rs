//! 人工介入审批子系统
//!
//! 集中管理 human-in-the-loop 审批流程所需的 Provider、权限服务、待刷新规则。

#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopProvider, PermissionService};
#[cfg(feature = "human-loop")]
use echo_core::tools::permission::PermissionRule;
use std::sync::Arc;

/// 人工介入审批子系统
///
/// 聚合审批 Provider、统一权限服务、待刷新权限规则。
/// 仅在 `human-loop` feature 启用时有实际字段。
pub(crate) struct ApprovalSubsystem {
    #[cfg(feature = "human-loop")]
    pub(crate) approval_provider: Arc<dyn HumanLoopProvider>,
    #[cfg(feature = "human-loop")]
    pub(crate) permission_service: Option<Arc<PermissionService>>,
    #[cfg(feature = "human-loop")]
    pub(crate) pending_permission_rules: std::sync::Mutex<Vec<PermissionRule>>,
}
