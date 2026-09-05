//! Stable ACP v1 Agent adapter for the `echo_agent` framework.
//!
//! The adapter composes the official ACP Rust runtime with the framework's
//! existing [`crate::runtime::AgentTurnDriver`]. It is transport-neutral: a
//! caller can connect it to stdio, an in-process channel, or another official
//! ACP transport without creating another JSON-RPC parser or Agent loop.
//!
//! Each ACP Session owns an independent framework Agent. Session history stays
//! inside that Agent; this module only owns protocol addressing and the active
//! turn cancellation token.

mod adapter;
mod projection;
mod prompt;
mod session;

pub use adapter::{AcpAdapterConfig, AcpAgentAdapter};
pub use session::{AcpSessionContext, AcpSessionFactory};
