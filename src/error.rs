//! Unified error types for the echo-agent framework.
//!
//! All errors are collected under the `Error` enum with variants for LLM
//! failures, tool errors, MCP issues, and more.
//!
//! The `Result<T>` type alias is `std::result::Result<T, Error>`. Use
//! `use echo_agent::prelude::*` to bring it into scope.

/// Direct re-exports from `echo_core::error`.
pub mod core {
    pub use echo_core::error::*;
}

pub use echo_core::error::*;
