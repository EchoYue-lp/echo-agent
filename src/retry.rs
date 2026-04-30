//! Retry façade.
//!
//! Thin re-exports of `echo_core::retry`.
//! The authoritative implementation lives in `echo_core`.

/// Direct re-exports from `echo_core::retry`.
pub mod core {
    pub use echo_core::retry::*;
}

pub use echo_core::retry::*;
