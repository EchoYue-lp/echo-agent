//! Structured audit logging for agent actions and decisions.
//!
//! Every tool call, guard check, and agent decision can be recorded as an
//! [`AuditEvent`] and persisted via pluggable backends.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .audit_logger(Arc::new(InMemoryAuditLogger::new()))
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Key Types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`AuditEvent`] | A single structured audit record |
//! | [`AuditLogger`] | Trait for audit backends |
//! | [`InMemoryAuditLogger`] | In-memory logger for testing |
//! | [`FileAuditLogger`] | File-based logger for production |

/// Direct re-exports from `echo_state::audit`.
pub mod state {
    pub use echo_state::audit::*;
}

pub use echo_state::audit::file;
pub use echo_state::audit::file::FileAuditLogger;
pub use echo_state::audit::memory;
pub use echo_state::audit::memory::InMemoryAuditLogger;
pub use echo_state::audit::*;
