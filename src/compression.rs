//! Context compression strategies to manage token budget.
//!
//! When conversations grow beyond the model's context window, compressors reduce
//! history while preserving the most relevant information.
//!
//! # Strategies
//!
//! | Compressor | Strategy | Best For |
//! |------------|----------|----------|
//! | `SlidingWindowCompressor` | Keeps the N most recent messages | Simple, fast |
//! | `SummaryCompressor` | Uses an LLM to summarize older messages | High quality (default) |
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .compressor(Box::new(SlidingWindowCompressor::new(4096)))
//!     .build()?;
//! # Ok(())
//! # }
//! ```

/// Direct re-exports from `echo_state::compression`.
pub mod state {
    pub use echo_state::compression::*;
}

pub use echo_state::compression::*;
