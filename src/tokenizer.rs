//! Tokenizer façade
//!
//! 此模块只做 `echo_core::tokenizer` 的薄重导出。
//! 权威实现位于 `echo_core`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::core::tokenizer`]。

/// Direct re-exports from `echo_core::tokenizer`.
pub mod core {
    pub use echo_core::tokenizer::*;
}

pub use echo_core::tokenizer::*;
