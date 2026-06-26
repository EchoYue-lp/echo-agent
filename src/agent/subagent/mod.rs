//! Subagent system — multi-level agent delegation and team coordination.
//!
//! Provides three execution modes:
//! - **Sync**: parent blocks until subagent returns (current `AgentDispatchTool` behavior)
//! - **Fork**: inherits parent context, runs independently with timeout
//! - **Teammate**: parallel independent agent with mailbox communication

pub mod builder;
pub mod context;
pub mod context_builder;
pub mod events;
pub mod executor;
pub mod hooks;
pub mod isolated;
pub mod lightweight;
pub mod pool;
pub mod registry;
pub mod team;
pub mod types;
pub mod usage;

// Re-export the most commonly used types
pub use builder::SubagentBuilder;
pub use context::{ContextInheritance, MemoryScope, OutputSchema, SubagentContext};
pub use context_builder::{ContextBuilder, SubagentOutput};
pub use events::{SubagentEvent, SubagentEventBus};
pub use executor::{DispatchRequest, SubagentExecutor, SubagentExecutorConfig, TeammateHandle};
pub use hooks::{SubagentHookContext, SubagentHookRegistry, SubagentHooks, SubagentRetryDecision};
pub use registry::SubagentRegistry;
pub use types::{
    ExecutionMode, RegisteredSubagent, SubagentDefinition, SubagentKind, SubagentResult,
};
