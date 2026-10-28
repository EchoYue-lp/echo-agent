//! Agent core trait, events, and callback interfaces

pub mod admission;
pub mod builder;
mod critic;
mod event_envelope;
pub mod factory;
pub mod intervention;
mod output_projection;
pub mod prompt_template;
mod types;

pub use admission::{
    ExecutionAdmission, KeyedExecutionAdmission, KeyedExecutionAdmissionError, KeyedExecutionLease,
    KeyedExecutionRetirement,
};
pub use factory::{AgentFactory, AgentFactoryConfig};
pub use intervention::{CallbackBridge, InterventionCallback, InterventionResult};
pub use prompt_template::PromptTemplateManager;

pub use critic::{CompositeCritic, CompositeStrategy, Critic, StaticCritic, ThresholdCritic};
pub use event_envelope::{
    AGENT_EVENT_SCHEMA_VERSION, ConversationId, EventEnvelope, EventId, EventIdentity, ExecutionId,
    MessageId, RunId, StreamId, TurnId, envelope_event_stream, envelope_event_stream_after,
    validate_envelope_trajectory, validate_event_trajectory,
};
pub use output_projection::{AgentOutputProjection, AssistantTextDelta};
pub use types::{Critique, CritiqueOutput, critique_output_schema};

use crate::error::{ReactError, Result};
use crate::llm::ToolDefinition;
use crate::llm::types::Message;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::stream::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
pub use tokio_util::sync::CancellationToken;

/// Product-neutral usage facts for one finite Agent execution.
///
/// Provider-specific counters remain on their native responses. This value is
/// the stable result surface shared by primary Agent turns and delegated
/// Subagent executions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionUsage {
    /// Total wall-clock duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Total input plus output tokens when the provider reported usage.
    pub tokens_used: Option<u64>,
    /// Number of ReAct iterations when the execution path reports it.
    pub iterations: Option<u64>,
}

impl ExecutionUsage {
    /// Return the duration in milliseconds, defaulting to zero when absent.
    pub fn duration_millis(&self) -> u64 {
        self.duration_ms.unwrap_or(0)
    }
}

/// Typed failure returned when a caller tries to steer an active agent turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSteerError {
    /// This agent implementation does not expose live steering.
    Unsupported,
    /// No turn is currently active.
    NoActiveTurn,
    /// The active turn does not match the caller's exact expected identity.
    TurnMismatch { expected: String, actual: String },
    /// The turn exists but has not reached a safe injection point.
    NotSteerable { turn_id: String },
    /// Empty instructions are never admitted.
    EmptyInput,
    /// The steering state could not be accessed.
    StateUnavailable,
}

impl std::fmt::Display for AgentSteerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("live steering is not supported by this agent"),
            Self::NoActiveTurn => f.write_str("no active turn to steer"),
            Self::TurnMismatch { expected, actual } => {
                write!(
                    f,
                    "active turn mismatch: expected {expected}, actual {actual}"
                )
            }
            Self::NotSteerable { turn_id } => write!(f, "turn {turn_id} is not steerable"),
            Self::EmptyInput => f.write_str("steer input is empty"),
            Self::StateUnavailable => f.write_str("turn steer state is unavailable"),
        }
    }
}

impl std::error::Error for AgentSteerError {}

/// Observable lifecycle phase of one accepted steering input.
///
/// Acceptance only means that the active turn's mailbox owns the input.
/// [`AgentSteerPhase::Drained`] means the ReAct loop moved it into model
/// context, while [`AgentSteerPhase::TurnSettled`] means the owning root turn
/// reached its real terminal boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerPhase {
    /// The active turn mailbox owns the input, but model context does not.
    Accepted,
    /// The input was inserted into the active turn's model context.
    Drained,
    /// The root turn reached a terminal boundary.
    TurnSettled,
}

/// Terminal outcome of the root turn that owned a steering input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerTurnOutcome {
    /// The root turn completed normally.
    Completed,
    /// Cancellation reached the root turn.
    Cancelled,
    /// Provider, tool, validation, or runtime processing failed.
    Failed,
    /// The turn owner was dropped or aborted before it could publish a typed
    /// terminal. This is terminal, but must not be interpreted as success.
    Dropped,
}

impl AgentSteerTurnOutcome {
    /// Stable lowercase wire identifier for this terminal outcome.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Dropped => "dropped",
        }
    }

    /// Parse a stable lowercase wire identifier.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            "dropped" => Some(Self::Dropped),
            _ => None,
        }
    }
}

/// Producer hook for a generic initial-input lifecycle.
///
/// The turn driver owns acceptance and terminal settlement. An Agent
/// implementation that knows the exact point at which its input entered model
/// context may publish the drain boundary through this hook. Implementations
/// without that knowledge leave the receipt at `drained = false`.
pub trait AgentInputLifecycle: Send + Sync {
    fn mark_drained(&self);
}

/// Current durable-consumption boundary observed for one steering input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSteerState {
    /// The mailbox owns the input.
    Accepted,
    /// The active model context owns the input.
    Drained,
    /// The root turn terminated.
    TurnSettled {
        /// Root-turn terminal outcome.
        outcome: AgentSteerTurnOutcome,
        /// Whether the input reached model context before the turn settled.
        drained: bool,
    },
}

impl AgentSteerState {
    /// Return the coarse lifecycle phase.
    pub fn phase(&self) -> AgentSteerPhase {
        match self {
            Self::Accepted => AgentSteerPhase::Accepted,
            Self::Drained => AgentSteerPhase::Drained,
            Self::TurnSettled { .. } => AgentSteerPhase::TurnSettled,
        }
    }

    /// Whether the input reached model context before the current state.
    pub fn was_drained(&self) -> bool {
        matches!(
            self,
            Self::Drained | Self::TurnSettled { drained: true, .. }
        )
    }
}

/// Tracked receipt for one steering input accepted by an active Agent turn.
///
/// The receipt is driven by the Agent implementation. Product adapters should
/// retain their durable input until `state().was_drained()` is true, and use
/// `wait_for_turn_settled` when terminal root-turn semantics are required.
#[derive(Debug, Clone)]
pub struct AgentSteerReceipt {
    steer_id: String,
    turn_id: String,
    state: tokio::sync::watch::Receiver<AgentSteerState>,
    closed_terminal: std::sync::Arc<std::sync::Mutex<Option<AgentSteerState>>>,
}

impl AgentSteerReceipt {
    #[doc(hidden)]
    pub fn new(
        steer_id: String,
        turn_id: String,
        state: tokio::sync::watch::Receiver<AgentSteerState>,
    ) -> Self {
        Self {
            steer_id,
            turn_id,
            state,
            closed_terminal: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Stable identity of this steering input.
    pub fn steer_id(&self) -> &str {
        &self.steer_id
    }

    /// Identity of the root turn that accepted this input.
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    /// Snapshot the latest observed lifecycle state without waiting.
    pub fn state(&self) -> AgentSteerState {
        if let Some(state) = self.cached_closed_terminal() {
            return state;
        }
        if self.state.has_changed().is_err() {
            return self.synthesize_closed_terminal();
        }
        self.state.borrow().clone()
    }

    fn cached_closed_terminal(&self) -> Option<AgentSteerState> {
        self.closed_terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn synthesize_closed_terminal(&self) -> AgentSteerState {
        let mut cached = self
            .closed_terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(state) = cached.as_ref() {
            return state.clone();
        }
        let current = self.state.borrow().clone();
        let terminal = match current {
            state @ AgentSteerState::TurnSettled { .. } => state,
            state => AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: state.was_drained(),
            },
        };
        *cached = Some(terminal.clone());
        terminal
    }

    /// Wait until the input is drained or its turn settles first.
    pub async fn wait_for_drained(&mut self) -> AgentSteerState {
        loop {
            let state = self.state();
            if !matches!(state, AgentSteerState::Accepted) {
                return state;
            }
            if self.state.changed().await.is_err() {
                return self.synthesize_closed_terminal();
            }
        }
    }

    /// Wait for the owning root turn's real terminal boundary.
    pub async fn wait_for_turn_settled(&mut self) -> AgentSteerState {
        loop {
            let state = self.state();
            if matches!(state, AgentSteerState::TurnSettled { .. }) {
                return state;
            }
            if self.state.changed().await.is_err() {
                return self.synthesize_closed_terminal();
            }
        }
    }
}

#[cfg(test)]
mod steer_receipt_tests {
    use super::*;

    #[test]
    fn turn_outcome_wire_names_round_trip() {
        for outcome in [
            AgentSteerTurnOutcome::Completed,
            AgentSteerTurnOutcome::Cancelled,
            AgentSteerTurnOutcome::Failed,
            AgentSteerTurnOutcome::Dropped,
        ] {
            assert_eq!(
                AgentSteerTurnOutcome::parse(outcome.as_str()),
                Some(outcome)
            );
        }
        assert_eq!(AgentSteerTurnOutcome::parse("unknown"), None);
    }

    #[tokio::test]
    async fn closed_sender_terminal_is_shared_across_receipt_clones() {
        let (sender, receiver) = tokio::sync::watch::channel(AgentSteerState::Accepted);
        let mut first = AgentSteerReceipt::new(
            "steer-closed".to_string(),
            "turn-closed".to_string(),
            receiver,
        );
        let mut second = first.clone();
        drop(sender);

        let (first_state, second_state) = tokio::join!(
            first.wait_for_turn_settled(),
            second.wait_for_turn_settled()
        );
        let expected = AgentSteerState::TurnSettled {
            outcome: AgentSteerTurnOutcome::Dropped,
            drained: false,
        };
        assert_eq!(first_state, expected);
        assert_eq!(second_state, expected);
        assert_eq!(first.state(), expected);
        assert_eq!(second.state(), expected);
    }

    #[tokio::test]
    async fn closed_sender_preserves_prior_drain() {
        let (sender, receiver) = tokio::sync::watch::channel(AgentSteerState::Drained);
        let mut receipt = AgentSteerReceipt::new(
            "steer-drained".to_string(),
            "turn-drained".to_string(),
            receiver,
        );
        drop(sender);

        assert_eq!(
            receipt.wait_for_turn_settled().await,
            AgentSteerState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: true,
            }
        );
    }
}

/// Optional soft budgets applied to one ReAct invocation.
///
/// `None` fields preserve the existing behavior. The hard iteration limit
/// remains `AgentConfig::max_iterations`; this policy only adds an explicit
/// wind-down point and a provider-reported model-token ceiling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunBudgetPolicy {
    /// Inject a one-shot wind-down instruction when this many iterations remain.
    pub iteration_wind_down_remaining: Option<usize>,
    /// Enter final-only mode after this many provider-reported model tokens.
    pub max_model_tokens: Option<usize>,
}

/// Observable decision made by the run budget controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDecision {
    /// Normal ReAct execution may continue.
    Continue,
    /// The run should converge, but tools remain available.
    WindDown,
    /// The next model call must produce text without tools.
    FinalOnly,
    /// A hard budget has been exhausted and the run must fail.
    HardStop,
}

/// A policy stage that changed the model-requested tool invocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolInvocationRewrite {
    InterventionRedirect,
    InterventionArguments,
    PreToolUseHook,
    Approval,
}

/// The requested and effective identity of one tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInvocation {
    pub requested_name: String,
    pub requested_args: Value,
    pub name: String,
    pub args: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rewrites: Vec<ToolInvocationRewrite>,
}

/// Value-scoped metadata for one streaming agent invocation.
#[derive(Clone, Default)]
pub struct AgentInvocationContext {
    /// Run metadata propagated into the invocation's tool context.
    pub runtime: Option<crate::tools::ExternalRunContext>,
    /// Optional runtime checkpoint identity distinct from the product
    /// conversation carried by `runtime`.
    ///
    /// `None` preserves the existing behavior: a runtime conversation override
    /// is also used for checkpoints. Persistent runtimes may set this to an
    /// ephemeral incarnation while keeping product events and transcripts on a
    /// stable conversation ID. When one Agent instance receives a different
    /// runtime-state identity, the framework resets or restores that identity
    /// before model input is prepared; warm context is reused only for the same
    /// identity.
    pub runtime_state_id: Option<String>,
    /// Optional model-context generation for append-only transcript projection.
    ///
    /// When set, the framework tracks the already-projected prefix on the Agent
    /// and appends only new messages from this generation. This prevents a
    /// fresh model context from being content-deduplicated against an older
    /// product transcript that happens to end with identical text.
    /// The value must equal the effective runtime-state identity resolved from
    /// explicit `runtime_state_id`, the runtime conversation, or the configured
    /// conversation. A mismatch fails closed with a runtime-state error before
    /// admission side effects or checkpoint persistence.
    pub transcript_generation_id: Option<String>,
    /// Per-invocation working directory. `None` uses the agent's configured default.
    pub working_dir: Option<std::path::PathBuf>,
    /// Cancellation token captured with the invocation before queueing.
    pub cancel: Option<CancellationToken>,
    /// Tool names hidden only for this invocation.
    ///
    /// These exclusions are combined with agent-level defaults when the run
    /// snapshot is created. They never mutate the shared tool registry.
    pub disabled_tools: Option<std::collections::HashSet<String>>,
    /// Nonempty hard upper bound retained until this invocation takes its
    /// execution snapshot. Intersect the then-current tool catalog with this
    /// set; a previously computed exclusion list cannot cover later registrations.
    /// `None` and an empty set preserve the historical unrestricted behavior.
    /// A nonempty set with zero registered matches still allows no tool.
    pub allowed_tools: Option<std::collections::HashSet<String>>,
    /// 调用级工具能力上界。ReadOnly 只会收窄已有配置，不改变共享 Agent。
    pub tool_access: Option<crate::tools::ToolAccess>,
    /// Initial tool names whose schemas are visible to the model.
    /// `None` keeps the complete eligible tool surface visible.
    pub visible_tools: Option<std::collections::HashSet<String>>,
    /// Per-invocation budget policy. `None` uses the agent default.
    pub run_budget: Option<RunBudgetPolicy>,
    /// Structured conversation turns inserted after the system prompt and
    /// before the current input. This is value-scoped and never mutates the
    /// agent's configured system prompt.
    pub history: Option<Vec<Message>>,
    /// Opaque resources retained through the invocation's spawned Agent,
    /// subagent, and tool work.
    pub resource_guards: Vec<crate::tools::InvocationResourceGuard>,
    /// Optional generic producer for the initial input's drain boundary.
    ///
    /// This is framework-only lifecycle plumbing. Product policy and durable
    /// receipt ownership remain with the caller of the turn driver.
    pub input_lifecycle: Option<std::sync::Arc<dyn AgentInputLifecycle>>,
}

impl std::fmt::Debug for AgentInvocationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentInvocationContext")
            .field("runtime_state_id", &self.runtime_state_id)
            .field("transcript_generation_id", &self.transcript_generation_id)
            .field(
                "run_id",
                &self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.run_id.as_deref()),
            )
            .field(
                "turn_id",
                &self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.turn_id.as_deref()),
            )
            .field(
                "execution_id",
                &self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.execution_id.as_deref()),
            )
            .field("working_dir", &self.working_dir)
            .field(
                "cancel",
                &self.cancel.as_ref().map(|_| "<CancellationToken>"),
            )
            .field(
                "disabled_tools",
                &self
                    .disabled_tools
                    .as_ref()
                    .map(std::collections::HashSet::len),
            )
            .field(
                "visible_tools",
                &self
                    .visible_tools
                    .as_ref()
                    .map(std::collections::HashSet::len),
            )
            .field(
                "allowed_tools",
                &self
                    .allowed_tools
                    .as_ref()
                    .map(std::collections::HashSet::len),
            )
            .field("tool_access", &self.tool_access)
            .field("run_budget", &self.run_budget)
            .field("history_messages", &self.history.as_ref().map(Vec::len))
            .field("resource_guard_count", &self.resource_guards.len())
            .field("resource_guards", &self.resource_guards)
            .field("has_input_lifecycle", &self.input_lifecycle.is_some())
            .finish()
    }
}

/// Events produced during Agent execution
///
/// Cover each phase of the Agent lifecycle for progress bars, logs, UI updates, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
// Boxing ToolStream would break the public event contract for every consumer.
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    // ── LLM Interaction ──────────────────────────────────────────────────────────
    /// Provider-neutral assistant output block.
    ///
    /// This is the lossless message contract for new consumers. The legacy
    /// `Token`/`ThinkStart`/`ThinkEnd` events remain as compatibility
    /// projections until downstream consumers have migrated, but they are not
    /// an execution or completion authority.
    AssistantMessage {
        /// Stable block identity and provider/request ordering metadata.
        block: AssistantMessageBlock,
    },
    /// Runtime execution observation. This is not a TaskRun completion signal.
    ExecutionStatus(ExecutionStatus),
    /// LLM is generating a token (streaming)
    Token(String),
    /// LLM reasoning started
    ThinkStart,
    /// LLM reasoning ended
    ThinkEnd {
        /// Number of prompt tokens consumed
        prompt_tokens: usize,
        /// Number of completion tokens consumed
        completion_tokens: usize,
    },
    /// Provider-reported LLM usage for a single request.
    ///
    /// `ThinkEnd` is kept as a compact, backward-compatible UI event. This
    /// richer event carries cache observability. When `usage_reported` is
    /// false, the provider or gateway did not return usage metadata; callers
    /// must treat cache fields as unknown instead of a zero-percent cache hit.
    LlmUsage {
        /// Model used for the request.
        model: String,
        /// Prompt/input tokens reported by the provider.
        prompt_tokens: usize,
        /// Completion/output tokens reported by the provider.
        completion_tokens: usize,
        /// Total tokens reported by the provider, or prompt + completion when absent.
        total_tokens: usize,
        /// Prompt/input tokens served from provider-side cache.
        cached_prompt_tokens: usize,
        /// Prompt/input tokens written into provider-side cache.
        cache_creation_prompt_tokens: usize,
        /// Whether the provider response contained usage metadata.
        usage_reported: bool,
    },
    /// A run budget crossed a behavior boundary.
    BudgetDecision {
        /// Decision taken by the harness.
        decision: BudgetDecision,
        /// Stable machine-readable reason.
        reason: String,
        /// Current iteration count (one-based).
        iteration: usize,
        /// Provider-reported model tokens accumulated in this invocation.
        reported_model_tokens: usize,
        /// False when at least one provider response omitted usage metadata.
        usage_complete: bool,
    },

    // ── Tool Invocation ──────────────────────────────────────────────────────────
    /// Canonical invocation emitted after policy rewrites and before execution.
    ToolCall {
        /// Stable tool-call identity (model tool_call_id, or generated UUID).
        call_id: String,
        /// Requested/effective invocation and rewrite provenance.
        invocation: ToolInvocation,
    },
    /// Tool execution completed, successfully or unsuccessfully.
    ToolResult {
        /// Stable tool-call identity matching the preceding [`Self::ToolCall`].
        call_id: String,
        /// Effective tool name.
        name: String,
        /// Complete result, including typed failure, artifact metadata, and truncation.
        result: crate::tools::ToolResult,
    },
    /// Streaming tool progress / output event (not a terminal lifecycle event).
    ToolStream {
        /// Stable tool-call identity matching the preceding [`Self::ToolCall`].
        call_id: String,
        /// Tool name
        name: String,
        /// Stream event payload (`Progress` / `Output`; `Complete` is mapped to
        /// [`Self::ToolResult`] by the ReAct runner).
        event: crate::tools::ToolStreamEvent,
    },
    /// Emitted before a batch of tools starts executing.
    /// All tools between ToolBatchStart and ToolBatchEnd are concurrent.
    ToolBatchStart {
        /// Number of tools in this batch
        tool_count: usize,
    },
    /// Emitted after all tools in the batch have completed.
    ToolBatchEnd,

    // ── Guard & Safety ──────────────────────────────────────────────────────
    /// A guard was triggered
    GuardTriggered {
        /// Guard name
        guard: String,
        /// Whether the action was blocked
        blocked: bool,
    },

    // ── Memory & Orchestration ──────────────────────────────────────────────────────
    /// Long-term memory was recalled
    MemoryRecalled {
        /// Number of recalled memory entries
        count: usize,
    },
    /// Context was auto-compressed to fit within token limits
    ContextCompressed {
        /// Message count before compression
        before_count: usize,
        /// Message count after compression
        after_count: usize,
        /// Estimated token count before compression
        before_tokens: usize,
        /// Estimated token count after compression
        after_tokens: usize,
    },
    /// Transcript projection reached a typed persistence settlement before terminal publication.
    TranscriptProjectionSettlement(crate::memory::TranscriptProjectionSettlement),

    // ── Visualization ────────────────────────────────────────────────────────────
    /// Chart generation (vega-lite JSON spec)
    Chart { spec: Value },

    // ── Errors ────────────────────────────────────────────────────────────
    /// Generic Agent error (non-tool errors, e.g. LLM call failure, guard rejection, etc.)
    Error {
        /// Error source (e.g. "llm", "guard", "config")
        source: String,
        /// Error message
        message: String,
        /// Stable classification used by retry, lifecycle, and UI adapters.
        failure: crate::error::AgentFailure,
    },

    /// Safety notice: the agent is about to perform an action that needs user awareness.
    /// Includes what action, why, risk level, and required permission.
    SafetyNotice {
        /// What the agent is about to do.
        action: String,
        /// Why this action is needed.
        reason: String,
        /// Risk description.
        risk: String,
        /// Permission level required.
        permission: String,
    },
    /// A tool parameter validation error occurred.
    ParameterError {
        /// Tool name.
        tool: String,
        /// Parameter name.
        parameter: String,
        /// Expected type.
        expected: String,
        /// Actual type received.
        got: String,
    },

    // ── Terminal States ──────────────────────────────────────────────────────────────
    /// Final answer
    FinalAnswer(String),
    /// Cancelled
    Cancelled,
}

/// Semantic phase of an assistant output block.
///
/// `Unknown` is intentional: adapters must not infer a phase from wording
/// when the provider did not supply one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantPhase {
    Reasoning,
    Commentary,
    PartialAnswer,
    FinalAnswer,
    Unknown,
}

/// Lossless provider-neutral assistant block used by Framework, providers,
/// EKO and SDK consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageBlock {
    /// Native source coordinates and content kind. Runtime-confirmed blocks
    /// do not fabricate provider provenance and leave this field None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_source: Option<crate::llm::types::ProviderOutputSource>,
    /// Provider item/block id, or a Framework-generated stable fallback.
    pub block_id: String,
    /// Model request/response identity owning this block.
    pub request_id: String,
    /// Monotonic order within one model response.
    pub sequence: u64,
    /// Provider-neutral semantic phase.
    pub phase: AssistantPhase,
    /// Incremental text for this observation.
    pub delta: String,
    /// Whether the provider/runtime has completed this block.
    pub complete: bool,
    /// Provider identity, when known.
    pub provider: Option<String>,
    /// Native signed/redacted/opaque reasoning payload, if any.
    pub reasoning: Option<crate::llm::types::ReasoningBlock>,
}

/// Runtime execution observation. This is deliberately separate from
/// TaskRun/Agent completion and cannot publish a successful task on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatusKind {
    Started,
    Waiting,
    ToolPending,
    Candidate,
    Settled,
    Cancelled,
    Failed,
}

/// Stable execution status envelope shared by all consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionStatus {
    /// 默认legacy适配器合成的状态，仅在进程内区分投影与原source事实。
    /// 不进入JSON/Message/Checkpoint合同，嵌套wrapper不能把合成身份当原owner。
    #[serde(skip)]
    pub adapter_generated: bool,
    pub execution_id: String,
    pub request_id: Option<String>,
    pub sequence: u64,
    pub phase: AgentPhase,
    pub status: ExecutionStatusKind,
    pub reason: Option<String>,
}

/// The lifecycle phase an Agent event belongs to
///
/// Maps each variant of `AgentEvent` into a unified phase model for:
/// - **State persistence**: checkpoints are only created at phase boundaries
/// - **Frontend rendering**: route to the corresponding UI component per phase, without matching every variant
/// - **Developer understanding**: newcomers first understand phases, then specific events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AgentPhase {
    /// LLM reasoning in progress (Token → ThinkStart → ThinkEnd)
    Thinking,
    /// Tool execution in progress (ToolCall -> typed ToolResult)
    Acting,
    /// Final result produced or cancelled
    Terminal,
}

impl AgentEvent {
    /// Build a terminal event while preserving the typed framework failure.
    pub fn from_error(source: impl Into<String>, error: &ReactError) -> crate::error::Result<Self> {
        let failure = crate::error::AgentFailure::try_from(error)?;
        Ok(Self::Error {
            source: source.into(),
            message: failure.message.clone(),
            failure,
        })
    }

    /// Build an unclassified terminal failure for boundaries that have no typed error.
    pub fn error_message(source: impl Into<String>, message: impl Into<String>) -> Self {
        let source = source.into();
        let failure = crate::error::AgentFailure::message(&source, message);
        Self::Error {
            source,
            message: failure.message.clone(),
            failure,
        }
    }

    /// Whether this event ends the invocation event stream.
    pub fn is_terminal(&self) -> bool {
        self.is_execution_terminal()
            || matches!(
                self,
                AgentEvent::FinalAnswer(_) | AgentEvent::Cancelled | AgentEvent::Error { .. }
            )
    }

    /// 原生执行最后的物理结算事实。模型块和 FinalAnswer 确认发布仍不能代替它。
    pub fn is_execution_terminal(&self) -> bool {
        matches!(self, Self::ExecutionStatus(status) if matches!(status.status,
            ExecutionStatusKind::Settled | ExecutionStatusKind::Failed | ExecutionStatusKind::Cancelled))
    }

    /// Return prompt token count for `ThinkEnd`.
    pub fn prompt_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd { prompt_tokens, .. } => Some(*prompt_tokens),
            AgentEvent::LlmUsage { prompt_tokens, .. } => Some(*prompt_tokens),
            _ => None,
        }
    }

    /// Return completion token count for `ThinkEnd`.
    pub fn completion_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd {
                completion_tokens, ..
            } => Some(*completion_tokens),
            AgentEvent::LlmUsage {
                completion_tokens, ..
            } => Some(*completion_tokens),
            _ => None,
        }
    }

    /// Return total token usage for `ThinkEnd`.
    pub fn total_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd {
                prompt_tokens,
                completion_tokens,
            } => Some(prompt_tokens.saturating_add(*completion_tokens)),
            AgentEvent::LlmUsage { total_tokens, .. } => Some(*total_tokens),
            _ => None,
        }
    }

    /// Compatibility helper for older call sites that tracked a single token count.
    pub fn tokens_used(&self) -> Option<usize> {
        self.total_tokens()
    }

    /// Return the lifecycle phase of this event
    ///
    /// Used for frontend phase-routed rendering, state machine derivation, and similar scenarios.
    ///
    /// # Example
    ///
    /// ```
    /// use echo_core::agent::{AgentEvent, AgentPhase};
    ///
    /// let event = AgentEvent::ThinkStart;
    /// assert_eq!(event.phase(), AgentPhase::Thinking);
    ///
    /// let event = AgentEvent::FinalAnswer("done".into());
    /// assert_eq!(event.phase(), AgentPhase::Terminal);
    /// ```
    pub fn phase(&self) -> AgentPhase {
        match self {
            // provider阶段描述正文类型，不能充当执行生命周期终态。
            AgentEvent::AssistantMessage { .. } => AgentPhase::Thinking,
            AgentEvent::ExecutionStatus(status) => status.phase,
            AgentEvent::Token(_)
            | AgentEvent::ThinkStart
            | AgentEvent::ThinkEnd { .. }
            | AgentEvent::LlmUsage { .. }
            | AgentEvent::BudgetDecision { .. }
            | AgentEvent::MemoryRecalled { .. }
            | AgentEvent::ContextCompressed { .. }
            | AgentEvent::Chart { .. } => AgentPhase::Thinking,

            AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::ToolStream { .. }
            | AgentEvent::ToolBatchStart { .. }
            | AgentEvent::ToolBatchEnd
            | AgentEvent::GuardTriggered { .. }
            | AgentEvent::TranscriptProjectionSettlement(_)
            | AgentEvent::SafetyNotice { .. }
            | AgentEvent::ParameterError { .. } => AgentPhase::Acting,

            AgentEvent::FinalAnswer(_) | AgentEvent::Cancelled | AgentEvent::Error { .. } => {
                AgentPhase::Terminal
            }
        }
    }

    /// Whether this is a persistable snapshot point (phase boundary event)
    ///
    /// When these events occur, the Agent state is at a "stable point" — no in-flight LLM calls
    /// or tool executions, suitable for checkpoint save, resume-from-checkpoint, or Time Travel debugging.
    ///
    /// # Example
    ///
    /// ```
    /// use echo_core::agent::AgentEvent;
    ///
    /// assert!(AgentEvent::ThinkEnd { prompt_tokens: 100, completion_tokens: 50 }.is_checkpoint());
    /// assert!(AgentEvent::FinalAnswer("done".into()).is_checkpoint());
    /// assert!(!AgentEvent::Token("hello".into()).is_checkpoint());
    /// ```
    pub fn is_checkpoint(&self) -> bool {
        match self {
            // 单块结束时整个模型响应/工具批仍可能在途，不是运行安全点。
            AgentEvent::AssistantMessage { .. } => false,
            AgentEvent::ExecutionStatus(status) => matches!(
                status.status,
                ExecutionStatusKind::Settled
                    | ExecutionStatusKind::Cancelled
                    | ExecutionStatusKind::Failed
            ),
            AgentEvent::ThinkEnd { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::ParameterError { .. }
            | AgentEvent::ContextCompressed { .. }
            | AgentEvent::TranscriptProjectionSettlement(_)
            | AgentEvent::FinalAnswer(_)
            | AgentEvent::Cancelled
            | AgentEvent::Error { .. } => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod accounting_tests {
    use super::AgentEvent;

    #[test]
    fn native_model_final_block_is_not_execution_terminal_or_safe_point() {
        let event = AgentEvent::AssistantMessage {
            block: super::AssistantMessageBlock {
                provider_source: None,
                block_id: "final-with-pending-tools".to_string(),
                request_id: "response".to_string(),
                sequence: 0,
                phase: super::AssistantPhase::FinalAnswer,
                delta: "尚有工具待执行".to_string(),
                complete: true,
                provider: Some("fixture".to_string()),
                reasoning: None,
            },
        };
        assert!(!event.is_terminal());
        assert!(!event.is_execution_terminal());
        assert!(!event.is_checkpoint());
        assert_eq!(event.phase(), super::AgentPhase::Thinking);
    }

    #[test]
    fn total_tokens_saturates_without_panicking() {
        let event = AgentEvent::ThinkEnd {
            prompt_tokens: usize::MAX,
            completion_tokens: 1,
        };
        assert_eq!(event.total_tokens(), Some(usize::MAX));
        assert_eq!(event.tokens_used(), Some(usize::MAX));
    }

    #[test]
    fn transcript_projection_settlement_is_observable_checkpoint_not_terminal() {
        let event = AgentEvent::TranscriptProjectionSettlement(
            crate::memory::TranscriptProjectionSettlement {
                status: crate::memory::TranscriptProjectionSettlementStatus::Settled,
                operation_id: Some("operation".to_string()),
                conversation_id: Some("conversation".to_string()),
                generation_id: Some("generation".to_string()),
                attempt: 1,
                error_class: None,
                detail: None,
            },
        );

        assert!(!event.is_terminal());
        assert!(event.is_checkpoint());
        assert_eq!(event.phase(), super::AgentPhase::Acting);
    }
}

/// Parsed step type from LLM response
#[derive(Debug)]
pub enum StepType {
    /// Thought step (internal reasoning)
    Thought(String),
    /// Tool invocation step
    Call {
        /// Tool call ID (unique identifier)
        tool_call_id: String,
        /// Function name
        function_name: String,
        /// Function arguments (JSON format)
        arguments: Value,
    },
}

/// Unified Agent execution interface
///
/// All methods accept `&self` so that an `Agent` may be shared via `Arc`.
/// **However**, the underlying mutable state (dialogue history, context,
/// tool caches) is *not* safe for concurrent `execute` / `chat_stream` calls
/// on the same instance.  Callers **must** serialize access — typically
/// through `Arc<RwLock<Agent>>` or `Arc<tokio::sync::Mutex<Agent>>`.
///
/// The workflow layer already enforces this serialization when driving
/// agents through plan-execute and multi-agent topologies.
/// Shareable agent-wide tool visibility policy.
///
/// The policy is read when an invocation snapshot is created. Sharing it with
/// lazily built Subagents keeps their registered capability surface aligned
/// without coupling the framework to any product-specific tool-control store.
#[derive(Debug, Clone, Default)]
pub struct ToolVisibilityPolicy {
    disabled: std::sync::Arc<std::sync::RwLock<Option<std::collections::HashSet<String>>>>,
}

impl ToolVisibilityPolicy {
    pub fn set_disabled(&self, names: Option<std::collections::HashSet<String>>) {
        if let Ok(mut guard) = self.disabled.write() {
            *guard = names.filter(|names| !names.is_empty());
        }
    }

    pub fn disabled_names(&self) -> std::collections::HashSet<String> {
        self.disabled
            .read()
            .map(|guard| guard.clone().unwrap_or_default())
            .unwrap_or_default()
    }
}

pub trait Agent: Send + Sync {
    /// Human-readable agent name used in logs, events, and orchestration.
    fn name(&self) -> &str;
    /// Model identifier currently bound to the agent.
    fn model_name(&self) -> &str;
    /// System prompt that seeds the agent's behavior.
    fn system_prompt(&self) -> &str;

    /// Names of tools currently exposed to the model.
    fn tool_names(&self) -> Vec<String> {
        vec![]
    }

    /// Tool definitions serialized into LLM requests.
    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    /// Registered tools hidden from the model by the agent-wide capability
    /// policy. Invocation-specific exclusions remain in
    /// [`AgentInvocationContext::disabled_tools`].
    fn disabled_tool_names(&self) -> std::collections::HashSet<String> {
        self.tool_visibility_policy().disabled_names()
    }

    /// Agent-wide visibility policy used for future invocation snapshots.
    fn tool_visibility_policy(&self) -> ToolVisibilityPolicy {
        ToolVisibilityPolicy::default()
    }

    /// Effective working directory configured for subsequent invocations.
    fn working_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Human-readable skill identifiers available to this agent.
    fn skill_names(&self) -> Vec<String> {
        vec![]
    }

    /// Configured MCP server identifiers available to this agent.
    fn mcp_server_names(&self) -> Vec<String> {
        vec![]
    }

    /// Release external resources before dropping the agent.
    ///
    /// Returns `Err` if cleanup fails (e.g., MCP server disconnect error,
    /// flush failure), allowing callers to log or handle resource leaks.
    fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Execute a task and return the final answer.
    ///
    /// **Task-oriented**: resets/restores context, may run a planning phase
    /// then enters the ReAct loop.
    /// Use this for standalone, single-round tasks where the agent starts fresh
    /// or resumes from a checkpoint.
    ///
    /// # ⚠️ Warning: Do NOT use for multi-turn chat UIs
    ///
    /// `execute()` **clears conversation history** on every call (only the
    /// system prompt survives). Calling it in a REPL / terminal UI / chatbot loop
    /// will make the agent "forget" all previous turns.
    ///
    /// **Use [`chat()`](Agent::chat) instead** for any scenario where the
    /// agent must remember prior messages across calls.
    ///
    /// | Scenario | Correct method |
    /// |---|---|
    /// | CLI one-shot command | `execute()` ✓ |
    /// | Workflow / pipeline node | `execute()` ✓ |
    /// | Batch processing (independent tasks) | `execute()` ✓ |
    /// | REPL / terminal UI / chatbot | `chat()` ✓ |
    /// | Multi-turn dialogue | `chat()` ✓ |
    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>>;

    /// Execute a task and stream lifecycle events.
    ///
    /// Same semantics as [`Self::execute`] but returns a stream of
    /// [`AgentEvent`] for real-time observability.
    ///
    /// # ⚠️ Warning
    ///
    /// Clears conversation history on every call — see [`execute()`](Agent::execute)
    /// for details. Use [`chat_stream()`](Agent::chat_stream) for multi-turn UIs.
    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>>;

    /// Execute a task with cooperative cancellation support.
    ///
    /// The default implementation wraps [`Self::execute_stream`] with a
    /// cancellation-aware wrapper. Cancellation retains the original source
    /// through physical EOF before publishing [`AgentEvent::Cancelled`]. An
    /// unresolved source observation is propagated without a fake terminal.
    ///
    /// # ⚠️ Warning
    ///
    /// Clears conversation history on every call — see [`execute()`](Agent::execute)
    /// for details. Use [`chat_stream_with_cancel()`](Agent::chat_stream_with_cancel)
    /// for multi-turn UIs.
    fn execute_stream_with_cancel<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let stream = self.execute_stream(task).await?;
            Ok(cancel_aware_stream(stream, cancel))
        })
    }

    /// Execute with invocation metadata carried as one immutable value.
    ///
    /// 默认实现保留旧元数据调用行为；若请求 ReadOnly，则在旧执行入口前
    /// 明确拒绝，不能声称已落实一个实际被忽略的能力上界。
    fn execute_stream_with_invocation_context<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            if invocation.tool_access == Some(crate::tools::ToolAccess::ReadOnly) {
                // 本地复用方也不能静默丢弃只读调用，否则旧实现可能写入用户文件。
                return Err(ReactError::Other(
                    "this agent does not support read-only invocation constraints".to_string(),
                ));
            }
            let stream = self.execute_stream_with_cancel(task, cancel).await?;
            Ok(invocation_retaining_stream(stream, invocation))
        })
    }

    /// Chat with the agent in a multi-turn conversation.
    ///
    /// **Conversation-oriented**: preserves existing context (no reset),
    /// appends the user message, and runs the ReAct loop.
    /// Use this for interactive, multi-turn dialogue where the agent
    /// accumulates state across calls.
    ///
    /// **Prefer this over [`execute()`](Agent::execute)** for any UI that
    /// sends multiple messages in sequence (REPL, terminal UI, chatbot, web chat).
    ///
    /// By default this delegates to [`Self::execute`]; concrete implementations
    /// (like `ReactAgent`) override it to avoid resetting context.
    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        self.execute(message)
    }

    /// Chat with the agent and stream lifecycle events.
    ///
    /// Same semantics as [`Self::chat`] but returns a stream of
    /// [`AgentEvent`] for real-time observability.
    ///
    /// **Prefer this over [`execute_stream()`](Agent::execute_stream)** for
    /// any UI that sends multiple messages in sequence.
    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream(message)
    }

    /// Chat streaming variant with cooperative cancellation support.
    ///
    /// The default implementation wraps [`Self::chat_stream`] with a
    /// cancellation-aware wrapper. Cancellation retains the original source
    /// through physical EOF before publishing [`AgentEvent::Cancelled`]. An
    /// unresolved source observation is propagated without a fake terminal.
    fn chat_stream_with_cancel<'a>(
        &'a self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let stream = self.chat_stream(message).await?;
            Ok(cancel_aware_stream(stream, cancel))
        })
    }

    /// Multi-turn chat streaming with a structured multimodal message.
    ///
    /// Implementations must preserve the same conversational semantics as
    /// [`Self::chat_stream_with_cancel`]: append to the existing context rather
    /// than resetting it. The default adapts only a metadata-free plain User
    /// text message to the legacy text method. Other message forms remain
    /// unsupported instead of losing role, multimodal content or metadata.
    fn chat_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            if let Message {
                role: crate::llm::types::Role::User,
                content: crate::llm::types::MessageContent::Text(text),
                tool_calls: None,
                name: None,
                tool_call_id: None,
                reasoning_content: None,
                reasoning_blocks: None,
                assistant_blocks: None,
                execution_status: None,
                approved_assistant_blocks: None,
            } = message
            {
                // The adapter owns text for as long as the legacy stream can
                // borrow it. Legacy setup starts on first poll; its original
                // setup error is emitted as a stream error, never a success.
                let stream = async_stream::stream! {
                    match self.chat_stream_with_cancel(&text, cancel).await {
                        Ok(legacy) => {
                            // Retain the same Result source through cleanup.
                            // The existing wrapper preserves the first business
                            // error and necessary later observation/confirmation.
                            let mut legacy = retaining_source_stream(legacy, None, None);
                            while let Some(event) = legacy.next().await { yield event; }
                        }
                        Err(error) => yield Err(error),
                    }
                };
                return Ok(Box::pin(stream) as BoxStream<'a, Result<AgentEvent>>);
            }
            Err(crate::error::ReactError::Other(
                "this agent does not implement multimodal streaming (chat_stream_message_with_cancel)"
                    .to_string(),
            ))
        })
    }

    /// Structured multi-turn chat with value-scoped invocation metadata.
    ///
    /// None 沿用原结构化聊天；ReadOnly 必须由实际支持该约束的实现承接，
    /// 默认回退在旧方法开始前拒绝。
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            if invocation.tool_access == Some(crate::tools::ToolAccess::ReadOnly) {
                // 本地复用方也不能静默丢弃只读调用，否则旧实现可能写入用户文件。
                return Err(ReactError::Other(
                    "this agent does not support read-only invocation constraints".to_string(),
                ));
            }
            let stream = self
                .chat_stream_message_with_cancel(message, cancel)
                .await?;
            Ok(invocation_retaining_stream(stream, invocation))
        })
    }

    /// Streaming task execution with cancellation (multimodal version).
    ///
    /// Accepts a pre-built [`Message`] so subagents dispatched via subagent
    /// delegation can see images/files attached by the user. `ReactAgent`
    /// overrides this to route through its real multimodal pipeline.
    ///
    /// The default implementation is **not supported** — agents that don't
    /// implement multimodal streaming return an error if a multimodal task is
    /// dispatched to them. This keeps the trait signature lifetime-safe (the
    /// text-extracted fallback would borrow a local). Callers that need a
    /// text fallback should extract the text themselves and use
    /// [`execute_stream_with_cancel`](Self::execute_stream_with_cancel).
    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        _message: Message,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            Err(crate::error::ReactError::Other(
                "this agent does not implement multimodal streaming (execute_stream_message_with_cancel)".to_string(),
            ))
        })
    }

    /// Multimodal streaming with value-scoped invocation metadata.
    ///
    /// None 沿用原多模态方法；ReadOnly 由实际支持该约束的实现承接，
    /// 默认回退不能静默退化成不限能力的旧调用。
    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            if invocation.tool_access == Some(crate::tools::ToolAccess::ReadOnly) {
                // 本地复用方也不能静默丢弃只读调用，否则旧实现可能写入用户文件。
                return Err(ReactError::Other(
                    "this agent does not support read-only invocation constraints".to_string(),
                ));
            }
            let stream = self
                .execute_stream_message_with_cancel(message, cancel)
                .await?;
            Ok(invocation_retaining_stream(stream, invocation))
        })
    }

    /// Reset in-memory conversational state.
    ///
    /// Implementations should clear context and fire `SessionStart("clear")` hooks
    /// so that registered hooks can react to the reset event.
    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }

    /// Get the current run ID, if the agent is tracking one.
    /// Default: None. ReactAgent overrides this.
    fn current_run_id(&self) -> Option<String> {
        None
    }

    /// Return a cumulative token-usage snapshot for this agent instance.
    ///
    /// Lightweight and third-party agents may keep the empty default. Team
    /// orchestration compares snapshots before and after execution to collect
    /// usage without depending on a concrete agent implementation.
    fn token_usage_summary(&self) -> crate::tokenizer::UsageSummary {
        crate::tokenizer::UsageSummary::default()
    }

    /// Inject a message into the current turn at the implementation's existing
    /// safe point. Implementations without live steering keep the typed default.
    fn steer_input(
        &self,
        _expected_turn_id: Option<&str>,
        _message: Message,
    ) -> std::result::Result<String, AgentSteerError> {
        Err(AgentSteerError::Unsupported)
    }

    /// Inject and track one message through acceptance, model-context drain,
    /// and root-turn settlement.
    ///
    /// Implementations that only support the legacy [`Self::steer_input`]
    /// remain source compatible and return [`AgentSteerError::Unsupported`]
    /// here until they can provide real lifecycle signals.
    fn steer_input_tracked(
        &self,
        _expected_turn_id: Option<&str>,
        _message: Message,
    ) -> std::result::Result<AgentSteerReceipt, AgentSteerError> {
        Err(AgentSteerError::Unsupported)
    }

    // ── External run context (跨 spawn 安全的值传递, 见 ExternalRunContext) ──

    /// Legacy agent-wide run context setter.
    ///
    /// **背景**：`tokio::task_local!` 不会跨 `tokio::spawn` 继承。subagent instance
    /// 在框架层的 `tokio::spawn`（subagent_executor.rs 的 dispatch_fork）里执行，
    /// 应用层经 task_local 注入的 run_id / cancel / trace_sink 全部丢失。
    /// 新代码应使用 `AgentInvocationContext` 的 value-scoped streaming methods；
    /// 此 setter 仅为外部兼容保留。
    fn set_external_context(&self, _ctx: &crate::tools::ExternalRunContext) {}

    /// Clear context installed through the legacy setter.
    fn clear_external_context(&self) {}

    /// Bind a working directory for this agent's tool calls (Sprint 8 worktree
    /// isolation). When set, every shell/file/git tool runs inside `path`
    /// (via `ToolContext.working_dir`). `None` clears it (restore default cwd).
    ///
    /// Default: noop. New invocation paths should use
    /// `AgentInvocationContext::working_dir`; this setter remains compatible
    /// with callers that intentionally configure an agent-wide default.
    fn set_working_dir(&self, _path: Option<std::path::PathBuf>) {}

    /// Clear the bound working directory (alias for `set_working_dir(None)`).
    /// Default: noop. ReactAgent override.
    fn clear_working_dir(&self) {}

    // ── Dynamic capability methods (default noop) ────────────────────

    /// Dynamically register a tool at runtime.
    ///
    /// Default: noop (returns without action). ReactAgent overrides this
    /// to add the tool to its ToolManager.
    fn register_tool(&self, _tool: Box<dyn crate::tools::Tool>) {}

    /// Dynamically remove a tool by name at runtime.
    ///
    /// Returns `true` if the tool was found and removed, `false` otherwise.
    /// Default: returns `false` (no action). ReactAgent overrides this.
    fn remove_tool(&self, _name: &str) -> bool {
        false
    }

    /// Get the current conversation history.
    ///
    /// Default: empty list. ReactAgent overrides this to return
    /// messages from its ContextManager.
    fn messages(&self) -> Vec<Message> {
        vec![]
    }

    /// Update the system prompt at runtime.
    ///
    /// Default: noop. ReactAgent overrides this to update its config
    /// and re-inject the prompt into the context.
    fn set_system_prompt(&self, _prompt: &str) {}

    /// Delegate a task to a named subagent or team member.
    ///
    /// Default: returns error ("delegation not supported").
    /// ReactAgent overrides this when Subagent feature is enabled.
    fn delegate_to<'a>(
        &'a self,
        _target: &'a str,
        _task: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async {
            Err(ReactError::Other(
                "delegation not supported by this agent".into(),
            ))
        })
    }
}

// ── Blanket impl for Box<dyn Agent> ──────────────────────────────────────

/// Allow `Box<dyn Agent>` to be used as an `Agent` directly.
///
/// This enables `Arc<Box<dyn Agent>>` to coerce to `Arc<dyn Agent>`,
/// which is essential for removing the outer `Mutex` from `SharedAgent`.
impl Agent for Box<dyn Agent> {
    fn name(&self) -> &str {
        self.as_ref().name()
    }
    fn model_name(&self) -> &str {
        self.as_ref().model_name()
    }
    fn system_prompt(&self) -> &str {
        self.as_ref().system_prompt()
    }
    fn tool_names(&self) -> Vec<String> {
        self.as_ref().tool_names()
    }
    fn tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        self.as_ref().tool_definitions()
    }
    fn skill_names(&self) -> Vec<String> {
        self.as_ref().skill_names()
    }
    fn mcp_server_names(&self) -> Vec<String> {
        self.as_ref().mcp_server_names()
    }
    fn token_usage_summary(&self) -> crate::tokenizer::UsageSummary {
        self.as_ref().token_usage_summary()
    }
    fn steer_input(
        &self,
        expected_turn_id: Option<&str>,
        message: Message,
    ) -> std::result::Result<String, AgentSteerError> {
        self.as_ref().steer_input(expected_turn_id, message)
    }
    fn steer_input_tracked(
        &self,
        expected_turn_id: Option<&str>,
        message: Message,
    ) -> std::result::Result<AgentSteerReceipt, AgentSteerError> {
        self.as_ref().steer_input_tracked(expected_turn_id, message)
    }
    fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
        self.as_ref().close()
    }
    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        self.as_ref().execute(task)
    }
    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref().execute_stream(task)
    }
    fn execute_stream_with_cancel<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref().execute_stream_with_cancel(task, cancel)
    }
    fn execute_stream_with_invocation_context<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref()
            .execute_stream_with_invocation_context(task, cancel, invocation)
    }
    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        self.as_ref().chat(message)
    }
    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref().chat_stream(message)
    }
    fn chat_stream_with_cancel<'a>(
        &'a self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref().chat_stream_with_cancel(message, cancel)
    }
    fn chat_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref()
            .chat_stream_message_with_cancel(message, cancel)
    }
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref()
            .chat_stream_message_with_invocation_context(message, cancel, invocation)
    }
    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref()
            .execute_stream_message_with_cancel(message, cancel)
    }
    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref()
            .execute_stream_message_with_invocation_context(message, cancel, invocation)
    }
    fn current_run_id(&self) -> Option<String> {
        self.as_ref().current_run_id()
    }
    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.as_ref().reset()
    }
    fn set_external_context(&self, ctx: &crate::tools::ExternalRunContext) {
        self.as_ref().set_external_context(ctx);
    }
    fn clear_external_context(&self) {
        self.as_ref().clear_external_context();
    }
    fn set_working_dir(&self, path: Option<std::path::PathBuf>) {
        self.as_ref().set_working_dir(path);
    }
    fn clear_working_dir(&self) {
        self.as_ref().clear_working_dir();
    }
    fn register_tool(&self, tool: Box<dyn crate::tools::Tool>) {
        self.as_ref().register_tool(tool)
    }
    fn remove_tool(&self, name: &str) -> bool {
        self.as_ref().remove_tool(name)
    }
    fn messages(&self) -> Vec<Message> {
        self.as_ref().messages()
    }
    fn set_system_prompt(&self, prompt: &str) {
        self.as_ref().set_system_prompt(prompt)
    }
    fn delegate_to<'a>(&'a self, target: &'a str, task: &'a str) -> BoxFuture<'a, Result<String>> {
        self.as_ref().delegate_to(target, task)
    }
}

/// Wrap an agent event stream with cooperative cancellation support.
///
/// A cancellation event is a delivery cause, not the producer's closure
/// receipt. Publish cancellation only after the original source's EOF.
fn cancel_aware_stream<'a>(
    stream: BoxStream<'a, Result<AgentEvent>>,
    cancel: CancellationToken,
) -> BoxStream<'a, Result<AgentEvent>> {
    retaining_source_stream(stream, Some(cancel), None)
}

fn invocation_retaining_stream<'a>(
    stream: BoxStream<'a, Result<AgentEvent>>,
    invocation: AgentInvocationContext,
) -> BoxStream<'a, Result<AgentEvent>> {
    retaining_source_stream(stream, None, Some(invocation))
}

// Both existing wrappers retain the same source. A cancellation terminal is
// delayed until physical EOF; if the source reported uncertainty, a later
// original terminal must confirm its closure first. No timer or owner is added.
fn retaining_source_stream<'a>(
    mut stream: BoxStream<'a, Result<AgentEvent>>,
    cancel: Option<CancellationToken>,
    invocation: Option<AgentInvocationContext>,
) -> BoxStream<'a, Result<AgentEvent>> {
    let wrapped = async_stream::stream! {
        let _invocation = invocation;
        let execution_id = _invocation.as_ref().and_then(|value| value.runtime.as_ref())
            .and_then(|runtime| runtime.execution_id.clone());
        let invocation_run_id = _invocation.as_ref().and_then(|value| value.runtime.as_ref())
            .and_then(|runtime| runtime.run_id.clone());
        let mut native_source_execution_id: Option<String> = None;
        let fallback_id = format!("execution:{}",uuid::Uuid::new_v4());
        let mut cancel_requested = false;
        let mut business_observed = false;
        let mut failure_observed = false;
        let mut cancelled_observed = false;
        let mut source_observation_pending = false;
        let mut foreign_observation_pending = false;
        let mut pending_success = None;
        let mut pending_status: Option<ExecutionStatus> = None;
        let mut publication = Vec::new();
        let mut next_sequence = 0_u64;
        let mut first_item = true;
        loop {
            let next = if cancel_requested { stream.next().await }
            else if let Some(cancel) = cancel.as_ref() {
                tokio::select! { biased;
                    next = stream.next() => next,
                    _ = cancel.cancelled() => { cancel_requested=true; pending_success=None; publication.clear(); continue; }
                }
            } else { stream.next().await };
            let Some(mut item) = next else { break; };
            if let Ok(AgentEvent::ExecutionStatus(status)) = &mut item
                && status.adapter_generated
                && let Some(id) = native_source_execution_id.as_ref().or(execution_id.as_ref())
            {
                // 只绑定合成投影，且Started/末尾一致；真实原生身份绝不覆盖。
                status.execution_id = id.clone();
            }
            if first_item {
                first_item = false;
                if let Ok(AgentEvent::ExecutionStatus(status)) = &item
                    && !status.adapter_generated
                {
                    native_source_execution_id = Some(status.execution_id.clone());
                }
                if !matches!(&item, Ok(AgentEvent::ExecutionStatus(_))) {
                    // 旧custom Agent的原source由此既有adapter持有；标记运输模式，
                    // 让外层envelope继续读取至同source的最终结算而非早停于正文。
                    yield Ok(AgentEvent::ExecutionStatus(ExecutionStatus {
                        adapter_generated: true,
                        execution_id: execution_id.clone().unwrap_or_else(|| fallback_id.clone()),
                        request_id: None, sequence: next_sequence, phase: AgentPhase::Thinking,
                        status: ExecutionStatusKind::Started, reason: None,
                    }));
                    next_sequence = next_sequence.saturating_add(1);
                }
            }
            match item {
                Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                    if let ReactError::ExecutionNotSettled { run_id, execution_id: observed } = &error {
                        // 已知owner的foreign观察不能被本owner的final/状态清除。
                        // 未绑定的legacy原source仍可在真正闭合后给晚到final。
                        foreign_observation_pending |= invocation_run_id.as_ref()
                            .is_some_and(|owner| owner != run_id)
                            || native_source_execution_id.as_ref().or(execution_id.as_ref())
                                .is_some_and(|owner| observed.as_ref() != Some(owner));
                    }
                    source_observation_pending=true;
                    pending_success=None; publication.clear(); business_observed=false;
                    // 保原source和原错误身份；只观察到EOF不能把unknown补成终态。
                    yield Err(error);
                }
                Ok(AgentEvent::ExecutionStatus(status)) if matches!(status.status,
                    ExecutionStatusKind::Settled | ExecutionStatusKind::Failed | ExecutionStatusKind::Cancelled) => {
                    source_observation_pending=foreign_observation_pending;
                    pending_status=Some(status);
                }
                Ok(AgentEvent::ExecutionStatus(mut status)) => {
                    status.sequence=status.sequence.max(next_sequence);
                    next_sequence=status.sequence.saturating_add(1);
                    yield Ok(AgentEvent::ExecutionStatus(status));
                }
                Ok(event @ AgentEvent::FinalAnswer(_)) => {
                    source_observation_pending=foreign_observation_pending;
                    if !cancel_requested && !failure_observed && pending_success.is_none() {
                        business_observed=true; pending_success=Some(event);
                    }
                }
                Ok(event) if event.is_terminal() => {
                    source_observation_pending=foreign_observation_pending; pending_success=None; publication.clear();
                    if !cancel_requested && !failure_observed {
                        business_observed=true; failure_observed=true;
                        cancelled_observed=matches!(event,AgentEvent::Cancelled);
                        yield Ok(event);
                    }
                }
                Err(error) if !cancel_requested && !failure_observed => {
                    pending_success=None; publication.clear();
                    cancelled_observed |= crate::error::AgentFailure::try_from(&error)
                        .is_ok_and(|failure| failure.terminal_kind == crate::error::AgentTerminalKind::Cancelled);
                    business_observed=true; failure_observed=true; yield Err(error);
                }
                Ok(event) if !cancel_requested && !failure_observed && pending_status.is_some() => publication.push(event),
                Ok(event) if !cancel_requested && !business_observed => yield Ok(event),
                _ => {}
            }
        }
        // 既有source物理EOF之后才发布单次terminal status；嵌套wrapper保同一原状态，不复制终态。
        if !source_observation_pending {
            let kind = if cancel_requested || cancelled_observed { Some(ExecutionStatusKind::Cancelled) }
                else if failure_observed { Some(ExecutionStatusKind::Failed) }
                else if pending_success.is_some() { Some(ExecutionStatusKind::Settled) }
                else { pending_status.as_ref().map(|status|status.status.clone()) };
            if let Some(kind)=kind {
                if kind==ExecutionStatusKind::Settled {
                    for event in publication { yield Ok(event); }
                    if let Some(success)=pending_success { yield Ok(success); }
                } else if cancel_requested && !failure_observed { yield Ok(AgentEvent::Cancelled); }
                let mut status=pending_status.unwrap_or_else(||ExecutionStatus {
                    adapter_generated: true,
                    execution_id:native_source_execution_id.clone().or_else(|| execution_id.clone()).unwrap_or(fallback_id),request_id:None,
                    sequence:next_sequence,phase:AgentPhase::Terminal,status:kind.clone(),reason:None,
                });
                // 原生source已有身份就保持它；仅legacy合成投影使用调用上下文身份。
                if status.adapter_generated
                    && let Some(id)=native_source_execution_id.as_ref().or(execution_id.as_ref()) { status.execution_id=id.clone(); }
                status.status=kind; status.sequence=status.sequence.max(next_sequence);
                yield Ok(AgentEvent::ExecutionStatus(status));
            }
        }
    };
    Box::pin(wrapped)
}

#[cfg(test)]
mod native_default_wrapper_identity_tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn adapter_provenance_is_runtime_only_and_does_not_change_serialized_status()
    -> std::result::Result<(), serde_json::Error> {
        let mut status = ExecutionStatus {
            adapter_generated: true,
            execution_id: "legacy-projection".into(),
            request_id: None,
            sequence: 0,
            phase: AgentPhase::Thinking,
            status: ExecutionStatusKind::Started,
            reason: None,
        };
        let projected = serde_json::to_value(&status)?;
        assert!(projected.get("adapter_generated").is_none());
        status.adapter_generated = false;
        assert_eq!(projected, serde_json::to_value(&status)?);
        assert!(!serde_json::from_value::<ExecutionStatus>(projected)?.adapter_generated);
        Ok(())
    }

    struct NativeSource {
        foreign_observation: Option<bool>,
        emit_terminal: bool,
    }

    impl Agent for NativeSource {
        fn name(&self) -> &str {
            "native-default-source"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            "fixture"
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("unused".into()) })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                let status = |kind, phase, sequence| {
                    AgentEvent::ExecutionStatus(ExecutionStatus {
                        adapter_generated: false,
                        execution_id: "source-execution".into(),
                        request_id: None,
                        sequence,
                        phase,
                        status: kind,
                        reason: None,
                    })
                };
                let mut events = vec![Ok(status(
                    ExecutionStatusKind::Started,
                    AgentPhase::Thinking,
                    0,
                ))];
                if let Some(foreign) = self.foreign_observation {
                    events.push(Err(ReactError::ExecutionNotSettled {
                        run_id: if foreign { "foreign-run" } else { "source-run" }.into(),
                        execution_id: Some(
                            if foreign {
                                "foreign-execution"
                            } else {
                                "source-execution"
                            }
                            .into(),
                        ),
                    }));
                }
                events.push(Ok(AgentEvent::FinalAnswer("source answer".into())));
                if self.emit_terminal {
                    events.push(Ok(status(
                        ExecutionStatusKind::Settled,
                        AgentPhase::Terminal,
                        1,
                    )));
                }
                Ok(futures::stream::iter(events).boxed())
            })
        }
        fn execute_stream_message_with_cancel<'a>(
            &'a self,
            _: Message,
            _: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            // 实现原结构化source，公共invocation方法仍用被测的默认adapter。
            self.execute_stream("native source fixture")
        }
    }

    async fn collect(
        source: NativeSource,
        caller_execution: &str,
    ) -> Result<Vec<Result<AgentEvent>>> {
        let invocation = AgentInvocationContext {
            runtime: Some(crate::tools::ExternalRunContext {
                run_id: Some("source-run".into()),
                execution_id: Some(caller_execution.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        Ok(source
            .execute_stream_message_with_invocation_context(
                Message::user("request".into()),
                CancellationToken::new(),
                invocation,
            )
            .await?
            .collect()
            .await)
    }

    #[tokio::test]
    async fn native_default_wrapper_keeps_original_execution_identity() -> Result<()> {
        let events = collect(
            NativeSource {
                foreign_observation: None,
                emit_terminal: true,
            },
            "caller-execution",
        )
        .await?;
        let ids = events
            .iter()
            .filter_map(|event| match event {
                Ok(AgentEvent::ExecutionStatus(status)) => Some(status.execution_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["source-execution", "source-execution"]);
        Ok(())
    }

    #[tokio::test]
    async fn native_default_wrapper_keeps_native_owner_for_synthesized_terminal() -> Result<()> {
        let events = collect(
            NativeSource {
                foreign_observation: None,
                emit_terminal: false,
            },
            "caller-execution",
        )
        .await?;
        let statuses = events
            .iter()
            .filter_map(|event| match event {
                Ok(AgentEvent::ExecutionStatus(status)) => Some(status),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            statuses
                .iter()
                .map(|status| status.execution_id.as_str())
                .collect::<Vec<_>>(),
            vec!["source-execution", "source-execution"]
        );
        assert!(
            statuses
                .last()
                .is_some_and(|status| status.status == ExecutionStatusKind::Settled)
        );
        assert!(events.iter().any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(answer)) if answer == "source answer")));
        Ok(())
    }

    #[tokio::test]
    async fn native_default_wrapper_foreign_unknown_cannot_be_cleared_by_own_final() -> Result<()> {
        for (foreign, caller) in [
            (true, "source-execution"),
            (false, "source-execution"),
            (true, "caller-execution"),
            (false, "caller-execution"),
        ] {
            let events = collect(
                NativeSource {
                    foreign_observation: Some(foreign),
                    emit_terminal: true,
                },
                caller,
            )
            .await?;
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, Err(ReactError::ExecutionNotSettled { .. })))
            );
            let final_answer = events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))));
            let settled = events.iter().any(|event| matches!(event, Ok(AgentEvent::ExecutionStatus(status)) if status.status == ExecutionStatusKind::Settled));
            assert_eq!(final_answer, !foreign);
            assert_eq!(settled, !foreign);
        }
        Ok(())
    }

    struct LegacySource;
    impl Agent for LegacySource {
        fn name(&self) -> &str {
            "legacy default source"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            "fixture"
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("unused".into()) })
        }
        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.chat_stream(task)
        }
        fn chat_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Ok(futures::stream::iter([
                    Err(ReactError::ExecutionNotSettled {
                        run_id: "source-run".into(),
                        execution_id: Some("source-execution".into()),
                    }),
                    Ok(AgentEvent::FinalAnswer("legacy closed answer".into())),
                ])
                .boxed())
            })
        }
    }

    #[tokio::test]
    async fn native_default_wrapper_binds_nested_legacy_projection_to_caller() -> Result<()> {
        let invocation = AgentInvocationContext {
            runtime: Some(crate::tools::ExternalRunContext {
                run_id: Some("source-run".into()),
                execution_id: Some("source-execution".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let events = LegacySource
            .chat_stream_message_with_invocation_context(
                Message::user("pure text".into()),
                CancellationToken::new(),
                invocation,
            )
            .await?
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(answer)) if answer == "legacy closed answer")));
        let statuses = events
            .iter()
            .filter_map(|event| match event {
                Ok(AgentEvent::ExecutionStatus(status)) => Some(status),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(statuses.len(), 2);
        assert!(
            statuses
                .iter()
                .all(|status| status.execution_id == "source-execution")
        );
        Ok(())
    }
}

#[cfg(test)]
mod invocation_retaining_stream_tests {
    #[tokio::test]
    async fn structured_legacy_chat_preserves_observation_and_original_live_source() -> Result<()> {
        use futures::FutureExt;
        use tokio::sync::{Mutex, Notify};
        struct SourceDrop(Arc<AtomicUsize>);
        impl Drop for SourceDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        type Backend = futures::future::Shared<BoxFuture<'static, std::result::Result<(), String>>>;
        struct LegacyChat {
            release: Arc<Notify>,
            join: Arc<Mutex<Option<Backend>>>,
            drops: Arc<AtomicUsize>,
            path: std::path::PathBuf,
        }
        impl Agent for LegacyChat {
            fn name(&self) -> &str {
                "legacy-live-chat"
            }
            fn model_name(&self) -> &str {
                "test"
            }
            fn system_prompt(&self) -> &str {
                "test"
            }
            fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
                Box::pin(async { Ok("unused".into()) })
            }
            fn execute_stream<'a>(
                &'a self,
                _: &'a str,
            ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
                Box::pin(async move {
                    let release = self.release.clone();
                    let path = self.path.clone();
                    let slot = self.join.clone();
                    let drops = self.drops.clone();
                    let source = async_stream::stream! {
                        let _scope = SourceDrop(drops);
                        let entered = Arc::new(Notify::new());
                        let producer_entered = entered.clone();
                        let backend = tokio::spawn(async move {
                            std::fs::write(&path, "entered").map_err(|error| error.to_string())?;
                            producer_entered.notify_one();
                            release.notified().await;
                            std::fs::write(&path, "closed").map_err(|error| error.to_string())?;
                            Ok::<(), String>(())
                        }).map(|result| result.map_err(|error| error.to_string()).and_then(|result| result)).boxed().shared();
                        *slot.lock().await = Some(backend.clone());
                        entered.notified().await;
                        yield Err(ReactError::ExecutionNotSettled { run_id: "original-chat-run".into(), execution_id: Some("original-chat-attempt".into()) });
                        if let Err(error) = backend.await { yield Err(ReactError::Other(error)); return; }
                        yield Ok(AgentEvent::FinalAnswer("same original source".into()));
                    };
                    Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
                })
            }
        }
        let directory =
            std::env::temp_dir().join(format!("echo_legacy_chat_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory)?;
        let path = directory.join("effect.txt");
        let release = Arc::new(Notify::new());
        let join = Arc::new(Mutex::new(None));
        let drops = Arc::new(AtomicUsize::new(0));
        let agent = LegacyChat {
            release: release.clone(),
            join: join.clone(),
            drops: drops.clone(),
            path: path.clone(),
        };
        let mut source = agent
            .chat_stream_message_with_invocation_context(
                Message::user("plain user".to_string()),
                CancellationToken::new(),
                AgentInvocationContext::default(),
            )
            .await?;
        let started = source.next().await;
        assert!(
            matches!(started, Some(Ok(AgentEvent::ExecutionStatus(status))) if status.status == ExecutionStatusKind::Started)
        );
        let first = source.next().await;
        let early = tokio::time::timeout(std::time::Duration::from_millis(20), source.next()).await;
        let held = std::fs::read_to_string(&path).ok();
        let early_drops = drops.load(Ordering::SeqCst);
        release.notify_one();
        let cleanup = match join.lock().await.take() {
            Some(backend) => backend.await.map_err(ReactError::Other),
            None => Err(ReactError::Other("missing original backend".into())),
        };
        let mut tail = Vec::new();
        while let Some(event) = source.next().await {
            tail.push(event);
        }
        let closed = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_dir_all(&directory);
        cleanup?;
        assert!(matches!(
            first,
            Some(Err(ReactError::ExecutionNotSettled { .. }))
        ));
        assert_eq!(held.as_deref(), Some("entered"));
        assert_eq!(closed.as_deref(), Some("closed"));
        assert!(
            early.is_err(),
            "legacy source ended instead of retaining its physical continuation: {early:?}"
        );
        assert_eq!(early_drops, 0);
        assert!(
            matches!(tail.as_slice(), [Ok(AgentEvent::FinalAnswer(answer)), Ok(AgentEvent::ExecutionStatus(status))] if answer == "same original source" && status.status==ExecutionStatusKind::Settled)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }

    use super::*;
    use crate::tools::InvocationResourceGuard;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct DefaultInvocationAgent;

    impl Agent for DefaultInvocationAgent {
        fn name(&self) -> &str {
            "default-invocation"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("done".to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Ok(Box::pin(futures::stream::pending()) as BoxStream<'a, Result<AgentEvent>>)
            })
        }

        fn chat_stream_message_with_cancel<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Ok(Box::pin(futures::stream::pending()) as BoxStream<'a, Result<AgentEvent>>)
            })
        }

        fn execute_stream_message_with_cancel<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Ok(Box::pin(futures::stream::pending()) as BoxStream<'a, Result<AgentEvent>>)
            })
        }
    }

    fn invocation(drops: &Arc<AtomicUsize>) -> AgentInvocationContext {
        AgentInvocationContext {
            resource_guards: vec![InvocationResourceGuard::new(DropCounter(Arc::clone(drops)))],
            ..AgentInvocationContext::default()
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum DefaultStreamEntry {
        Text,
        ChatMessage,
        ExecuteMessage,
    }

    struct DefaultTailAgent {
        items: std::sync::Mutex<Vec<Result<AgentEvent>>>,
    }

    impl Agent for DefaultTailAgent {
        fn name(&self) -> &str {
            "default-tail"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("unused".into()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                let items = std::mem::take(
                    &mut *self
                        .items
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                );
                Ok(futures::stream::iter(items).boxed())
            })
        }

        fn execute_stream_message_with_cancel<'a>(
            &'a self,
            _: Message,
            cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            // Supply only the supported legacy producer. All invocation methods,
            // including structured chat's text fallback, remain trait defaults.
            self.execute_stream_with_cancel("structured execute", cancel)
        }
    }

    async fn default_tail_events(
        entry: DefaultStreamEntry,
        items: Vec<Result<AgentEvent>>,
    ) -> Result<Vec<Result<AgentEvent>>> {
        let agent = DefaultTailAgent {
            items: std::sync::Mutex::new(items),
        };
        let cancel = CancellationToken::new();
        let stream = match entry {
            DefaultStreamEntry::Text => {
                agent
                    .execute_stream_with_invocation_context(
                        "text",
                        cancel,
                        AgentInvocationContext::default(),
                    )
                    .await?
            }
            DefaultStreamEntry::ChatMessage => {
                agent
                    .chat_stream_message_with_invocation_context(
                        Message::user("chat".to_string()),
                        cancel,
                        AgentInvocationContext::default(),
                    )
                    .await?
            }
            DefaultStreamEntry::ExecuteMessage => {
                agent
                    .execute_stream_message_with_invocation_context(
                        Message::user("execute".to_string()),
                        cancel,
                        AgentInvocationContext::default(),
                    )
                    .await?
            }
        };
        Ok(stream.collect().await)
    }

    #[tokio::test]
    async fn default_invocation_entries_preserve_first_tail_cleanup_failure() -> Result<()> {
        for entry in [
            DefaultStreamEntry::Text,
            DefaultStreamEntry::ChatMessage,
            DefaultStreamEntry::ExecuteMessage,
        ] {
            let events = default_tail_events(
                entry,
                vec![
                    Ok(AgentEvent::FinalAnswer("done".into())),
                    Err(ReactError::Other("first cleanup failure".into())),
                    Err(ReactError::Other("later cleanup failure".into())),
                ],
            )
            .await?;
            assert!(
                matches!(events.as_slice(), [Ok(AgentEvent::ExecutionStatus(started)), Err(ReactError::Other(error)), Ok(AgentEvent::ExecutionStatus(status))]
                    if error == "first cleanup failure" && started.status == ExecutionStatusKind::Started && status.status==ExecutionStatusKind::Failed && started.execution_id == status.execution_id),
                "{entry:?} must suppress premature success and retain the first cleanup failure: {events:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn default_invocation_entries_keep_prior_failure_and_cancellation() -> Result<()> {
        for entry in [
            DefaultStreamEntry::Text,
            DefaultStreamEntry::ChatMessage,
            DefaultStreamEntry::ExecuteMessage,
        ] {
            for first in [
                Err(ReactError::Other("original failure".into())),
                Ok(AgentEvent::error_message("producer", "original failure")),
                Ok(AgentEvent::Cancelled),
            ] {
                let expected = format!("{first:?}");
                let events = default_tail_events(
                    entry,
                    vec![
                        first,
                        Err(ReactError::Other("later cleanup failure".into())),
                    ],
                )
                .await?;
                assert_eq!(events.len(), 3, "{entry:?}: {events:?}");
                assert!(
                    matches!(events.first(), Some(Ok(AgentEvent::ExecutionStatus(status))) if status.status == ExecutionStatusKind::Started)
                );
                assert_eq!(format!("{:?}", events.get(1)), format!("Some({expected})"));
                assert!(
                    matches!(events.last(),Some(Ok(AgentEvent::ExecutionStatus(status))) if matches!(status.status,ExecutionStatusKind::Failed|ExecutionStatusKind::Cancelled))
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn default_invocation_entries_keep_unsettled_identity_and_tail_failure() -> Result<()> {
        for entry in [
            DefaultStreamEntry::Text,
            DefaultStreamEntry::ChatMessage,
            DefaultStreamEntry::ExecuteMessage,
        ] {
            let events = default_tail_events(
                entry,
                vec![
                    Ok(AgentEvent::FinalAnswer("done".into())),
                    Err(ReactError::ExecutionNotSettled {
                        run_id: "original-run".into(),
                        execution_id: Some("original-attempt".into()),
                    }),
                    Err(ReactError::Other("cleanup failure".into())),
                ],
            )
            .await?;
            assert!(
                matches!(events.as_slice(), [Ok(AgentEvent::ExecutionStatus(started)), Err(ReactError::ExecutionNotSettled { run_id, execution_id: Some(execution_id) }), Err(ReactError::Other(error))]
                    if started.status == ExecutionStatusKind::Started && run_id == "original-run" && execution_id == "original-attempt" && error == "cleanup failure"),
                "{entry:?} lost original observation or cleanup cause: {events:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn default_invocation_entries_publish_only_one_success_after_clean_original_eof()
    -> Result<()> {
        for entry in [
            DefaultStreamEntry::Text,
            DefaultStreamEntry::ChatMessage,
            DefaultStreamEntry::ExecuteMessage,
        ] {
            let events = default_tail_events(
                entry,
                vec![
                    Ok(AgentEvent::FinalAnswer("done".into())),
                    Ok(AgentEvent::FinalAnswer("duplicate".into())),
                ],
            )
            .await?;
            assert!(
                matches!(events.as_slice(), [Ok(AgentEvent::ExecutionStatus(started)), Ok(AgentEvent::FinalAnswer(answer)), Ok(AgentEvent::ExecutionStatus(status))] if answer == "done" && started.status == ExecutionStatusKind::Started && status.status==ExecutionStatusKind::Settled && started.execution_id == status.execution_id),
                "{entry:?}: {events:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn default_text_invocation_retains_guards_until_stream_drop() -> Result<()> {
        let drops = Arc::new(AtomicUsize::new(0));
        let stream = DefaultInvocationAgent
            .execute_stream_with_invocation_context(
                "task",
                CancellationToken::new(),
                invocation(&drops),
            )
            .await?;
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(stream);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn default_multimodal_chat_retains_guards_until_stream_drop() -> Result<()> {
        let drops = Arc::new(AtomicUsize::new(0));
        let stream = DefaultInvocationAgent
            .chat_stream_message_with_invocation_context(
                Message::user("chat".to_string()),
                CancellationToken::new(),
                invocation(&drops),
            )
            .await?;
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(stream);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn default_multimodal_execute_retains_guards_until_stream_drop() -> Result<()> {
        let drops = Arc::new(AtomicUsize::new(0));
        let stream = DefaultInvocationAgent
            .execute_stream_message_with_invocation_context(
                Message::user("execute".to_string()),
                CancellationToken::new(),
                invocation(&drops),
            )
            .await?;
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(stream);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }
}

/// Agent lifecycle callback interface
pub trait AgentCallback: Send + Sync {
    /// Optional stable callback kind for targeted removal.
    fn callback_kind(&self) -> Option<&'static str> {
        None
    }

    /// Optional stable identifier for removing a specific callback instance.
    fn callback_id(&self) -> Option<&str> {
        None
    }

    /// Called before the model starts a reasoning step.
    fn on_think_start<'a>(
        &'a self,
        _agent: &'a str,
        _messages: &'a [Message],
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after the model reasoning step with token usage information.
    fn on_think_end<'a>(
        &'a self,
        _agent: &'a str,
        _steps: &'a [StepType],
        _prompt_tokens: usize,
        _completion_tokens: usize,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called before a tool invocation begins.
    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called before a tool invocation begins with its canonical call identity.
    ///
    /// The default preserves source compatibility for callbacks that only need
    /// tool-level lifecycle notifications.
    fn on_tool_start_with_id<'a>(
        &'a self,
        agent: &'a str,
        _call_id: &'a str,
        tool: &'a str,
        args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        self.on_tool_start(agent, tool, args)
    }

    /// Called after a tool invocation succeeds.
    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after a tool invocation succeeds with its canonical call identity.
    fn on_tool_end_with_id<'a>(
        &'a self,
        agent: &'a str,
        _call_id: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.on_tool_end(agent, tool, result)
    }

    /// Called when a tool invocation fails.
    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _err: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called after a tool invocation fails with its canonical call identity.
    fn on_tool_error_with_id<'a>(
        &'a self,
        agent: &'a str,
        _call_id: &'a str,
        tool: &'a str,
        error: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        self.on_tool_error(agent, tool, error)
    }

    /// Settle an invocation that the runtime interrupted without a captured result.
    ///
    /// The full input lets persistence callbacks retain the admitted call even
    /// when interruption happened before their normal start callback. The
    /// default emits one error terminal and deliberately does not fabricate a
    /// duplicate start notification.
    fn on_tool_interrupted_with_id<'a>(
        &'a self,
        agent: &'a str,
        call_id: &'a str,
        tool: &'a str,
        _input: &'a Value,
        error: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        self.on_tool_error_with_id(agent, call_id, tool, error)
    }

    /// Called when the agent emits its final answer.
    fn on_final_answer<'a>(&'a self, _agent: &'a str, _answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called at the end of each outer control-loop iteration.
    fn on_iteration<'a>(&'a self, _agent: &'a str, _iteration: usize) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod default_pure_text_chat_tests {
    use super::*;
    use crate::llm::types::{ContentPart, MessageContent, Role};
    use crate::tools::InvocationResourceGuard;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct DropProof(Arc<AtomicUsize>);
    impl Drop for DropProof {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct LegacyTextAgent {
        calls: AtomicUsize,
        fail_setup: bool,
        release_cleanup: tokio::sync::Notify,
    }
    impl Agent for LegacyTextAgent {
        fn name(&self) -> &str {
            "legacy-text"
        }
        fn model_name(&self) -> &str {
            "none"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(text.to_string()) })
        }
        fn execute_stream<'a>(
            &'a self,
            text: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.chat_stream(text)
        }
        fn chat_stream<'a>(
            &'a self,
            text: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.fail_setup {
                    return Err(ReactError::Other(
                        "original legacy setup failure".to_string(),
                    ));
                }
                // Keep the borrowed original text across the caller's later poll.
                Ok(async_stream::stream! {
                    yield Ok(AgentEvent::Token(text.to_string()));
                    self.release_cleanup.notified().await;
                }
                .boxed())
            })
        }
    }

    #[tokio::test]
    async fn default_pure_text_chat_owns_input_until_poll_and_retains_cancel_and_guard()
    -> Result<()> {
        let agent = LegacyTextAgent::default();
        let drops = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let context = AgentInvocationContext {
            resource_guards: vec![InvocationResourceGuard::new(DropProof(drops.clone()))],
            ..Default::default()
        };
        let mut events = agent
            .chat_stream_message_with_invocation_context(
                Message::user("owned 中文🌊".to_string()),
                cancel.clone(),
                context,
            )
            .await?;
        assert_eq!(agent.calls.load(Ordering::SeqCst), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let Some(Ok(AgentEvent::ExecutionStatus(started))) = events.next().await else {
            return Err(ReactError::Other(
                "missing original execution Started".into(),
            ));
        };
        assert_eq!(started.status, ExecutionStatusKind::Started);
        assert!(
            matches!(events.next().await, Some(Ok(AgentEvent::Token(text))) if text == "owned 中文🌊")
        );
        assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
        cancel.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), events.next())
                .await
                .is_err(),
            "cancel terminal waits for the original producer's EOF"
        );
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "original invocation guard remains owned while cleanup is held"
        );
        agent.release_cleanup.notify_one();
        assert!(matches!(
            events.next().await,
            Some(Ok(AgentEvent::Cancelled))
        ));
        let Some(Ok(AgentEvent::ExecutionStatus(cancelled))) = events.next().await else {
            return Err(ReactError::Other(
                "missing original execution Cancelled".into(),
            ));
        };
        assert_eq!(cancelled.status, ExecutionStatusKind::Cancelled);
        assert_eq!(cancelled.execution_id, started.execution_id);
        assert!(events.next().await.is_none());
        drop(events);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn default_pure_text_chat_setup_failure_remains_original_stream_error() -> Result<()> {
        let agent = LegacyTextAgent {
            fail_setup: true,
            ..Default::default()
        };
        let mut events = agent
            .chat_stream_message_with_cancel(
                Message::user("owned".to_string()),
                CancellationToken::new(),
            )
            .await?;
        assert!(
            matches!(events.next().await, Some(Err(ReactError::Other(message))) if message == "original legacy setup failure")
        );
        assert!(events.next().await.is_none());
        assert_eq!(agent.calls.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn default_pure_text_chat_rejects_every_nonlossless_message_without_legacy_call() {
        let agent = LegacyTextAgent::default();
        let mut messages = Vec::new();
        for role in [
            Role::System,
            Role::Assistant,
            Role::Tool,
            Role::Custom("provider-role".to_string()),
        ] {
            messages.push(Message {
                role,
                ..Message::user("text".to_string())
            });
        }
        messages.push(Message {
            content: MessageContent::Empty,
            ..Message::user("text".to_string())
        });
        messages.push(Message::user_multimodal(vec![ContentPart::Text {
            text: "must not flatten".to_string(),
        }]));
        let mut value = Message::user("text".to_string());
        value.name = Some(String::new());
        messages.push(value);
        let mut value = Message::user("text".to_string());
        value.tool_call_id = Some(String::new());
        messages.push(value);
        let mut value = Message::user("text".to_string());
        value.tool_calls = Some(Vec::new());
        messages.push(value);
        let mut value = Message::user("text".to_string());
        value.reasoning_content = Some(String::new());
        messages.push(value);
        let mut value = Message::user("text".to_string());
        value.reasoning_blocks = Some(Vec::new());
        messages.push(value);
        for message in messages {
            let result = agent
                .chat_stream_message_with_cancel(message, CancellationToken::new())
                .await;
            assert!(
                matches!(result, Err(ReactError::Other(message)) if message.contains("does not implement multimodal streaming"))
            );
        }
        assert_eq!(agent.calls.load(Ordering::SeqCst), 0);
    }

    #[derive(Default)]
    struct StructuredOverrideAgent {
        legacy: AtomicUsize,
        structured: AtomicUsize,
    }
    impl Agent for StructuredOverrideAgent {
        fn name(&self) -> &str {
            "structured-wins"
        }
        fn model_name(&self) -> &str {
            "none"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(text.to_string()) })
        }
        fn execute_stream<'a>(
            &'a self,
            _text: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.legacy.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ReactError::Other("legacy must not be selected".to_string())) })
        }
        fn chat_stream_message_with_cancel<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.structured.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(futures::stream::iter([Ok(AgentEvent::FinalAnswer(
                    "structured-original".to_string(),
                ))])
                .boxed())
            })
        }
    }
    #[tokio::test]
    async fn default_pure_text_chat_invocation_keeps_existing_structured_override() -> Result<()> {
        let agent = StructuredOverrideAgent::default();
        let mut events = agent
            .chat_stream_message_with_invocation_context(
                Message::user("plain user".to_string()),
                CancellationToken::new(),
                AgentInvocationContext::default(),
            )
            .await?;
        let Some(Ok(AgentEvent::ExecutionStatus(started))) = events.next().await else {
            return Err(ReactError::Other(
                "missing structured execution Started".into(),
            ));
        };
        assert_eq!(started.status, ExecutionStatusKind::Started);
        assert!(
            matches!(events.next().await, Some(Ok(AgentEvent::FinalAnswer(answer))) if answer == "structured-original")
        );
        let Some(Ok(AgentEvent::ExecutionStatus(settled))) = events.next().await else {
            return Err(ReactError::Other(
                "missing structured execution Settled".into(),
            ));
        };
        assert_eq!(settled.status, ExecutionStatusKind::Settled);
        assert_eq!(settled.execution_id, started.execution_id);
        assert!(events.next().await.is_none());
        assert_eq!(agent.structured.load(Ordering::SeqCst), 1);
        assert_eq!(agent.legacy.load(Ordering::SeqCst), 0);
        Ok(())
    }
}

#[cfg(test)]
mod readonly_invocation_fallback_tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct LegacyAgent {
        calls: AtomicUsize,
    }

    impl Agent for LegacyAgent {
        fn name(&self) -> &str {
            "legacy"
        }
        fn model_name(&self) -> &str {
            "test"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok("legacy".into())
            })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(futures::stream::empty().boxed())
            })
        }
        fn chat_stream_message_with_cancel<'a>(
            &'a self,
            _: Message,
            _: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(futures::stream::empty().boxed())
            })
        }
        fn execute_stream_message_with_cancel<'a>(
            &'a self,
            _: Message,
            _: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(futures::stream::empty().boxed())
            })
        }
    }

    #[tokio::test]
    async fn readonly_default_invocation_fallbacks_reject_before_legacy_calls() -> Result<()> {
        let agent = std::sync::Arc::new(LegacyAgent::default());
        for mode in 0..3 {
            let context = AgentInvocationContext {
                tool_access: Some(crate::tools::ToolAccess::ReadOnly),
                ..Default::default()
            };
            let result = match mode {
                0 => {
                    agent
                        .execute_stream_with_invocation_context(
                            "task",
                            CancellationToken::new(),
                            context,
                        )
                        .await
                }
                1 => {
                    agent
                        .chat_stream_message_with_invocation_context(
                            Message::user("task".to_string()),
                            CancellationToken::new(),
                            context,
                        )
                        .await
                }
                _ => {
                    agent
                        .execute_stream_message_with_invocation_context(
                            Message::user("task".to_string()),
                            CancellationToken::new(),
                            context,
                        )
                        .await
                }
            };
            assert!(
                matches!(result, Err(error) if error.to_string().contains("read-only invocation constraints"))
            );
        }
        assert_eq!(agent.calls.load(Ordering::SeqCst), 0);
        for mode in 0..3 {
            match mode {
                0 => {
                    drop(
                        agent
                            .execute_stream_with_invocation_context(
                                "task",
                                CancellationToken::new(),
                                AgentInvocationContext::default(),
                            )
                            .await?,
                    );
                }
                1 => {
                    drop(
                        agent
                            .chat_stream_message_with_invocation_context(
                                Message::user("task".to_string()),
                                CancellationToken::new(),
                                AgentInvocationContext::default(),
                            )
                            .await?,
                    );
                }
                _ => {
                    drop(
                        agent
                            .execute_stream_message_with_invocation_context(
                                Message::user("task".to_string()),
                                CancellationToken::new(),
                                AgentInvocationContext::default(),
                            )
                            .await?,
                    );
                }
            }
        }
        assert_eq!(agent.calls.load(Ordering::SeqCst), 3);
        Ok(())
    }
}
