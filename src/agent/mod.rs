//! Agent module
//!
//! Defines the core [`Agent`] trait, event enum [`AgentEvent`], and callback interface [`AgentCallback`].
//!
//! ## Built-in Agent Paradigms
//!
//! | Module | Paradigm | Feature |
//! |--------|----------|---------|
//! | [`react`] | ReAct (Think-Act-Observe) | always available |
//! | [`plan_execute`] | Plan-and-Execute | `plan-execute` |
//! | [`self_reflection`] | Self-Reflection | `self-reflection` |
//! | [`subagent`] | Subagent system | `subagent` |
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .system_prompt("You are a helpful assistant")
//!     .enable_tools()
//!     .build()?;
//!
//! println!("Agent name: {}", agent.name());
//! println!("Model: {}", agent.model_name());
//! # Ok(())
//! # }
//! ```

pub use echo_core::agent::builder::AgentBuilder as AgentBuilderTrait;
pub use echo_core::agent::{Agent, AgentCallback, AgentEvent, CancellationToken, StepType};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// SubAgent registry type alias
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<dyn Agent>>>>;

// ── Core sub-modules ───────────────────────────────────────────────────────

pub mod config;
pub mod react;
pub mod runner;

#[cfg(feature = "plan-execute")]
pub mod plan_execute;
#[cfg(feature = "self-reflection")]
pub mod self_reflection;
#[cfg(feature = "subagent")]
pub mod subagent;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use crate::agent::react::ReactAgent;
pub use crate::agent::react::builder::ReactAgentBuilder;
pub use crate::agent::react::structured::StructuredAgent;
pub use config::{AgentConfig, AgentRole};
pub use runner::Runner;

/// Alias for backward compatibility with macros and minimal API.
pub type AgentBuilder = ReactAgentBuilder;
