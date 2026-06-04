//! # echo-agents
//!
//! Agent implementations for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! This crate defines the public API for agent types:
//!
//! | Agent | Description | Feature |
//! |-------|-------------|---------|
//! | `ReactAgent` | ReAct loop (think-act-observe) | default |
//! | `SubagentExecutor` | Multi-agent subagent dispatch | `subagent` |
//!
//! ## Architecture Note
//!
//! echo-agent uses a single Agent engine (ReactAgent) design. Different execution
//! strategies (planning, self-review) are implemented as tools and configurations
//! rather than separate Agent types. This aligns with industry best practices
//! (Hermes, Claude Code, LangGraph).
//!
//! ## Usage
//!
//! Most users should depend on `echo_agent` (the facade crate) which
//! re-exports everything from this crate.

pub mod prelude;
pub mod traits;

pub use prelude::*;
pub use traits::*;
