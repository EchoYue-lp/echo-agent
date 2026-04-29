//! Audit façade
//!
//! 此模块只做 `echo_state::audit` 的薄重导出。
//! 核心事件类型仍来自 `echo_core::audit`，而 logger / callback 实现的权威版本
//! 位于 `echo_state::audit`。
//!
//! 如需直接依赖拆分后的 crate，可使用 [`crate::workspace::state::audit`]。

/// Direct re-exports from `echo_state::audit`.
pub mod state {
    pub use echo_state::audit::*;
}

pub use echo_state::audit::file;
pub use echo_state::audit::file::FileAuditLogger;
pub use echo_state::audit::memory;
pub use echo_state::audit::memory::InMemoryAuditLogger;
pub use echo_state::audit::*;
