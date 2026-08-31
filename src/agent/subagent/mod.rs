//! Subagent system — multi-level agent delegation.
//!
//! Provides three execution modes:
//! - **Sync**: parent blocks until subagent returns (current `AgentDispatchTool` behavior)
//! - **Fork**: runs independently; context is fresh unless filtered history is explicitly requested
//! - **Teammate**: parallel independent agent controlled through a join/cancel handle

pub mod builder;
pub mod context;
pub mod control;
pub mod events;
pub mod executor;
pub mod hooks;
pub mod isolation;
pub mod prompt;
pub mod registry;
pub mod team;
pub mod types;
pub mod usage;

// Re-export the most commonly used types
pub use builder::SubagentBuilder;
pub use context::{ContextInheritance, SubagentContext};
pub use control::{
    SubagentAttemptIdentity, SubagentCommandIdentity, SubagentCommandPhase, SubagentControlError,
    SubagentControlPhase, SubagentGuidanceQueueReceipt, SubagentInterruptOutcome,
    SubagentMessageReceipt,
};
pub use events::{SubagentEvent, SubagentEventBus};
pub use executor::{
    BackgroundSubagentHandle, DispatchRequest, SubagentExecutor, SubagentExecutorConfig,
    TeammateHandle, merge_observed_evidence, subagent_status_from_error,
};
pub use hooks::{SubagentHookContext, SubagentHookRegistry, SubagentHooks, SubagentRetryDecision};
pub use isolation::{
    IsolationError, IsolationHandle, IsolationOutcome, IsolationProvider, IsolationRequest,
    SharedIsolationProvider,
};
pub use prompt::{
    CompiledSubagentInvocation, CompiledSubagentSystemPrompt, ContextTransferPolicy,
    DefaultSubagentPromptCompiler, PromptActor, PromptDiagnostics, PromptSectionDiagnostic,
    SubagentExecutionBoundary, SubagentInvocation, SubagentPromptCompiler,
    SubagentSystemPromptInput, SubagentTaskContext, ToolCapability, ToolCapabilitySnapshot,
    compiled_current_message, filter_history, remove_duplicate_current_message, with_compiled_task,
};
pub use registry::{AgentFactory, FnAgentFactory, SubagentRegistry};
pub use team::{
    Team, TeamAgent, TeamAgentBuilder, TeamConfig, TeamExecutionResult, TeamMember, TeamRole,
    TeamRuntime, TeamSpec, TeamStrategy, execute_team, execute_team_on_runtime,
};
pub use types::{
    ExecutionMode, ObservedIsolation, RegisteredSubagent, SubagentAccessMode, SubagentArtifact,
    SubagentDefinition, SubagentEvidence, SubagentEvidenceSource, SubagentKind, SubagentOutcome,
    SubagentResult, SubagentStatus, SubagentTouchedFiles, SubagentVerification,
    SubagentVerificationStatus, parse_json_objects, parse_subagent_outcome, render_result_contract,
    split_subagent_output,
};
