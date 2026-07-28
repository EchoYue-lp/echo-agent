//! Tasks facade
//!
//! This module is a thin re-export of `echo_orchestration::tasks`.
//! The authoritative implementation is in `echo_orchestration`; if you need
//! to directly depend on the split crate, use
//! [`crate::workspace::orchestration::tasks`].

/// Direct re-exports from `echo_orchestration::tasks`.
pub mod orchestration {
    pub use echo_orchestration::tasks::*;
}

pub use echo_orchestration::tasks::*;

/// Replace the Agent's task relation tools with tools backed by `service`.
/// Tool registration is name-based, so this atomically selects the supplied
/// store/policy adapter without exposing a second task API.
pub fn register_task_tools(
    agent: &mut crate::agent::ReactAgent,
    service: std::sync::Arc<TaskRevisionService>,
) {
    agent.add_tools(build_task_tools(service));
}
