//! Tasks façade
//!
//! 此模块只做 `echo_orchestration::tasks` 的薄重导出。
//! 权威实现位于 `echo_orchestration`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::orchestration::tasks`]。

/// Direct re-exports from `echo_orchestration::tasks`.
pub mod orchestration {
    pub use echo_orchestration::tasks::*;
}

pub use echo_orchestration::tasks::*;
