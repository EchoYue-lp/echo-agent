//! Human-in-the-loop approval subsystem
//!
//! Centralized management of Provider, permission service, and pending refresh rules
//! required by the human-in-the-loop approval workflow.

#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopProvider, PermissionService};
#[cfg(feature = "human-loop")]
use echo_core::tools::permission::PermissionRule;
use std::sync::Arc;

/// Human-in-the-loop approval subsystem
///
/// Aggregates approval Provider, unified permission service, and pending permission rules.
/// Only has actual fields when the `human-loop` feature is enabled.
pub(crate) struct ApprovalSubsystem {
    #[cfg(feature = "human-loop")]
    pub(crate) approval_provider: Arc<dyn HumanLoopProvider>,
    #[cfg(feature = "human-loop")]
    pub(crate) permission_service: Option<Arc<PermissionService>>,
    #[cfg(feature = "human-loop")]
    pub(crate) pending_permission_rules: std::sync::Mutex<Vec<PermissionRule>>,
}
