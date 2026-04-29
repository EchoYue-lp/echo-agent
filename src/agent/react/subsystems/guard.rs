//! 护栏与安全子系统
//!
//! 集中管理护栏检查、权限策略、审计日志、熔断器等安全相关组件。

use crate::guard::GuardManager;
use echo_core::circuit_breaker::CircuitBreaker;
use std::sync::Arc;

/// 护栏与安全子系统
///
/// 聚合输入/输出护栏、工具权限策略、审计日志记录、LLM 熔断器。
pub(crate) struct GuardSubsystem {
    pub(crate) guard_manager: Option<GuardManager>,
    pub(crate) permission_policy: Option<Arc<dyn crate::tools::permission::PermissionPolicy>>,
    pub(crate) audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
    pub(crate) circuit_breaker: Option<Arc<CircuitBreaker>>,
}
