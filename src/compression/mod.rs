//! Compression façade
//!
//! 此模块只做 `echo_state::compression` 的薄重导出。
//! 权威实现位于 `echo_state`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::state::compression`]。

/// Direct re-exports from `echo_state::compression`.
pub mod state {
    pub use echo_state::compression::*;
}

pub use echo_state::compression::*;
