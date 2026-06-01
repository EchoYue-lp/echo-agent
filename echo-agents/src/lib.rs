//! # echo-agents
//!
//! Agent implementations for the [echo-agent](https://crates.io/crates/echo_agent) framework.
//!
//! This crate defines the public API for agent types:
//!
//! | Agent | Description | Feature |
//! |-------|-------------|---------|
//! | `ReactAgent` | ReAct loop (think-act-observe) | default |
//! | `PlanExecuteAgent` | Plan-and-Execute paradigm | `plan-execute` |
//! | `SelfReflectionAgent` | Self-reflection paradigm | `self-reflection` |
//! | `SubagentExecutor` | Multi-agent subagent dispatch | `subagent` |
//!
//! ## Architecture Note
//!
//! This crate establishes the API boundary for agent types. The current
//! implementation lives in the facade crate (`echo_agent`) due to deep
//! coupling with tools, memory, skills, and other subsystems. Future
//! refactoring will move implementations here as coupling is reduced.
//!
//! ## Usage
//!
//! Most users should depend on `echo_agent` (the facade crate) which
//! re-exports everything from this crate.

pub mod prelude;
pub mod traits;

pub use prelude::*;
pub use traits::*;
