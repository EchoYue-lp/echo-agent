//! Human-loop façade
//!
//! 此模块只做 `echo_orchestration::human_loop` 的薄重导出。
//! 权威实现位于 `echo_orchestration`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::orchestration::human_loop`]。

/// Direct re-exports from `echo_orchestration::human_loop`.
pub mod orchestration {
    pub use echo_orchestration::human_loop::*;
}

pub use echo_orchestration::human_loop::*;
