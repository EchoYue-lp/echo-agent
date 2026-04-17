//! Retry façade
//!
//! 此模块只做 `echo_core::retry` 的薄重导出。
//! 权威实现位于 `echo_core`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::core::retry`]。

/// Direct re-exports from `echo_core::retry`.
pub mod core {
    pub use echo_core::retry::*;
}

pub use echo_core::retry::*;
