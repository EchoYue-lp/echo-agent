//! Human-loop facade
//!
//! This module is a thin re-export of `echo_orchestration::human_loop`.
//! The authoritative implementation is in `echo_orchestration`; if you need
//! to directly depend on the split crate, use
//! [`crate::workspace::orchestration::human_loop`].

/// Direct re-exports from `echo_orchestration::human_loop`.
pub mod orchestration {
    pub use echo_orchestration::human_loop::*;
}

pub use echo_orchestration::human_loop::*;
