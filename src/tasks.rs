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
