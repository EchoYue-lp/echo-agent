//! Agent module
//!
//! Defines the core [`Agent`] trait, event enum [`AgentEvent`], and callback interface [`AgentCallback`].
//!
//! ## Architecture
//!
//! The framework provides a single, robust [`ReactAgent`] that implements the ReAct
//! (Think-Act-Observe) pattern. Different execution paradigms are expressed through
//! composable tools and configurations rather than separate agent types:
//!
//! | Capability | Mechanism |
//! |------------|-----------|
//! | ReAct reasoning | [`react`] — the default loop |
//! | Task planning | Tools (plan, create_task, update_task) |
//! | Self-reflection | [`critic`] — evaluation and feedback tools |
//! | Subagent coordination | [`subagent`] — multi-agent orchestration |
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
pub use echo_core::agent::{
    AGENT_EVENT_SCHEMA_VERSION, Agent, AgentCallback, AgentEvent, AgentInvocationContext,
    CancellationToken, EventEnvelope, EventIdentity, InterventionCallback, InterventionResult,
    StepType, envelope_event_stream, envelope_event_stream_after, validate_event_trajectory,
};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// SubAgent registry type alias
#[allow(dead_code)]
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<dyn Agent>>>>;

// ── Core sub-modules ───────────────────────────────────────────────────────

pub mod callbacks;
pub mod config;
pub mod critic;
pub mod default_factory;
pub mod handle;
pub mod react;
pub mod snapshot;
mod steer;

#[cfg(feature = "subagent")]
pub mod subagent;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use crate::agent::handle::AgentHandle;
pub use crate::agent::react::ReactAgent;
pub use crate::agent::react::builder::ReactAgentBuilder;
pub use crate::agent::react::structured::StructuredAgent;
pub use crate::agent::steer::TurnSteerError;
pub use config::{AgentConfig, AgentRole};

/// Agent factory types — re-exported from echo-core with facade-level overrides.
///
/// This module provides [`factory::AgentFactory`], [`factory::AgentFactoryConfig`],
/// and [`factory::DefaultAgentFactory`] (the concrete facade implementation that uses
/// `ReactAgentBuilder`).
pub mod factory {
    pub use crate::agent::default_factory::DefaultAgentFactory;
    pub use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig};
}

/// Alias for backward compatibility with macros and minimal API.
pub type AgentBuilder = ReactAgentBuilder;
