//! Prelude — commonly used types for working with agents.
//!
//! ```rust,ignore
//! use echo_agents::prelude::*;
//! ```

pub use crate::traits::{AgentBuilder, AgentBuildConfig, AgentRunResult, ExecutionMode, HasConfig};

// Re-export core agent trait
pub use echo_core::agent::Agent;
