//! Guardrails and security subsystem
//!
//! Centralized management of guardrail checks, permission policies, audit logs,
//! circuit breaker, and other security-related components.

use crate::guard::GuardManager;
use echo_core::circuit_breaker::CircuitBreaker;
use std::sync::Arc;

/// Guardrails and security subsystem
///
/// Aggregates input/output guardrails, tool permission policies, audit logging,
/// and LLM circuit breaker.
pub(crate) struct GuardSubsystem {
    pub(crate) guard_manager: Option<GuardManager>,
    pub(crate) permission_policy: Option<Arc<dyn crate::tools::permission::PermissionPolicy>>,
    pub(crate) audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
    pub(crate) circuit_breaker: Option<Arc<CircuitBreaker>>,
}
