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
pub use echo_core::agent::mode::{AgentMode, DefaultModeEngine, ModeConfig, ModeEngine};
pub use echo_core::agent::{
    Agent, AgentCallback, AgentEvent, CancellationToken, InterventionCallback, InterventionResult,
    StepType,
};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// SubAgent registry type alias
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<dyn Agent>>>>;

// ── Core sub-modules ───────────────────────────────────────────────────────

pub mod approval_stack;
pub mod config;
pub mod default_factory;
pub mod mode_engine;
pub mod react;
pub mod runner;
pub mod snapshot;
pub mod turn;

#[cfg(feature = "plan-execute")]
pub mod plan_execute;
#[cfg(feature = "self-reflection")]
pub mod self_reflection;
#[cfg(feature = "subagent")]
pub mod subagent;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use crate::agent::mode_engine::LocalizedModeEngine;
pub use crate::agent::react::ReactAgent;
pub use crate::agent::react::builder::ReactAgentBuilder;
pub use crate::agent::react::structured::StructuredAgent;
pub use config::{AgentConfig, AgentRole};
pub use runner::Runner;

/// Agent factory types — re-exported from echo-core with facade-level overrides.
///
/// This module provides [`AgentFactory`], [`AgentFactoryConfig`], [`AgentParadigm`],
/// and [`DefaultAgentFactory`] (the concrete facade implementation that uses
/// `ReactAgentBuilder`).
pub mod factory {
    pub use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig, AgentParadigm};
    pub use crate::agent::default_factory::DefaultAgentFactory;
}

/// Alias for backward compatibility with macros and minimal API.
pub type AgentBuilder = ReactAgentBuilder;
