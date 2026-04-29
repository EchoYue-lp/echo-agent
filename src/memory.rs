//! Memory façade
//!
//! 此模块只做 `echo_state::memory` 的薄重导出。
//! 权威实现位于 `echo_state`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::state::memory`]。

/// Direct re-exports from `echo_state::memory`.
pub mod state {
    pub use echo_state::memory::*;
}

pub use echo_state::memory::*;
