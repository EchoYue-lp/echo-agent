//! Subagent system — multi-level agent delegation and team coordination.
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
pub mod prompt;
pub mod registry;
pub mod team;
pub mod types;
pub mod usage;
pub mod workspace;
pub mod worktree;

// Re-export the most commonly used types
pub use builder::SubagentBuilder;
pub use context::{ContextInheritance, SubagentContext};
pub use control::{
    SubagentAttemptIdentity, SubagentControlError, SubagentControlPhase,
    SubagentGuidanceQueueReceipt, SubagentInterruptOutcome, SubagentMessageDelivery,
};
pub use events::{SubagentEvent, SubagentEventBus};
pub use executor::{
    BackgroundSubagentHandle, DispatchRequest, SubagentExecutor, SubagentExecutorConfig,
    TeammateHandle, merge_observed_evidence, subagent_status_from_error,
};
pub use hooks::{SubagentHookContext, SubagentHookRegistry, SubagentHooks, SubagentRetryDecision};
pub use prompt::{
    CompiledSubagentInvocation, CompiledSubagentSystemPrompt, ContextTransferPolicy,
    DefaultSubagentPromptCompiler, PromptDiagnostics, PromptSectionDiagnostic,
    SubagentPromptCompiler, SubagentPromptInput, SubagentSystemPromptInput, filter_history,
    with_compiled_task,
};
pub use registry::{AgentFactory, FnAgentFactory, SubagentRegistry};
pub use types::{
    ExecutionMode, ObservedIsolation, RegisteredSubagent, SubagentArtifact, SubagentDefinition,
    SubagentKind, SubagentOutcome, SubagentResult, SubagentStatus, SubagentTouchedFiles,
    SubagentVerification, SubagentVerificationSource, SubagentVerificationStatus,
    parse_subagent_outcome, render_result_contract, split_subagent_output,
};
pub use workspace::{
    DataWorkspaceFactory, DataWorkspaceHandle, NoWorkspaceFactory, SharedDataWorkspaceFactory,
    WorkspaceError,
};
pub use worktree::{
    NoWorktreeFactory, SharedWorktreeFactory, WorktreeError, WorktreeFactory, WorktreeHandle,
};
