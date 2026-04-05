//! Subagent system — multi-level agent delegation and team coordination.
//!
//! Provides three execution modes:
//! - **Sync**: parent blocks until subagent returns (current `AgentDispatchTool` behavior)
//! - **Fork**: inherits parent context, runs independently with timeout
//! - **Teammate**: parallel independent agent with mailbox communication

pub mod builder;
pub mod context;
pub mod events;
pub mod executor;
pub mod hooks;
pub mod registry;
pub mod team;
pub mod types;

// Re-export the most commonly used types
pub use builder::SubagentBuilder;
pub use context::{ContextInheritance, SubagentContext};
pub use events::SubagentEventBus;
pub use executor::{DispatchRequest, SubagentExecutor, SubagentExecutorConfig, TeammateHandle};
pub use hooks::{SubagentHookContext, SubagentHookRegistry, SubagentHooks, SubagentRetryDecision};
pub use registry::SubagentRegistry;
pub use types::{
    ExecutionMode, RegisteredSubagent, SubagentDefinition, SubagentKind, SubagentResult,
};
