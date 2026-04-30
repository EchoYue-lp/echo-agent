//! Content filtering and safety guardrails.
//!
//! Guards inspect user input, LLM output, and tool calls, blocking or modifying
//! unsafe content. Two guard types are supported:
//!
//! - **Rule-based** (`RuleGuard`, `RuleGuardBuilder`) — Pattern matching, length limits, regex
//! - **LLM-based** (`LlmGuard`) — AI-powered content classification
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let guard = RuleGuardBuilder::new("safety")
//!     .max_input_length(10000)
//!     .build();
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .with_guard(Box::new(guard))
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Key Types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Guard`] | Trait — implement to create custom guards |
//! | [`GuardManager`] | Runs multiple guards in sequence, short-circuits on block |
//! | [`GuardResult`] | `Pass` / `Block { reason }` / `Warn { message }` |
//! | [`GuardDirection`] | `Input` / `Output` / `ToolInput` / `ToolOutput` |

pub mod llm;
pub mod rule;

/// Direct re-exports from `echo_core::guard`.
pub mod core {
    pub use echo_core::guard::*;
}

pub use echo_core::guard::*;
