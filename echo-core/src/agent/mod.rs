//! Agent core trait, events, and callback interfaces

pub mod admission;
pub mod builder;
mod critic;
mod event_envelope;
pub mod factory;
pub mod intervention;
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
    validate_event_trajectory,
};
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
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
// Boxing ToolStream would break the public event contract for every consumer.
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    // ── LLM Interaction ──────────────────────────────────────────────────────────
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

/// The lifecycle phase an Agent event belongs to
///
/// Maps each variant of `AgentEvent` into a unified phase model for:
/// - **State persistence**: checkpoints are only created at phase boundaries
/// - **Frontend rendering**: route to the corresponding UI component per phase, without matching every variant
/// - **Developer understanding**: newcomers first understand phases, then specific events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn from_error(source: impl Into<String>, error: &ReactError) -> Self {
        let failure = crate::error::AgentFailure::from(error);
        Self::Error {
            source: source.into(),
            message: failure.message.clone(),
            failure,
        }
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
        matches!(
            self,
            AgentEvent::FinalAnswer(_) | AgentEvent::Cancelled | AgentEvent::Error { .. }
        )
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
        matches!(
            self,
            AgentEvent::ThinkEnd { .. }
                | AgentEvent::ToolResult { .. }
                | AgentEvent::ParameterError { .. }
                | AgentEvent::ContextCompressed { .. }
                | AgentEvent::FinalAnswer(_)
                | AgentEvent::Cancelled
                | AgentEvent::Error { .. }
        )
    }
}

#[cfg(test)]
mod accounting_tests {
    use super::AgentEvent;

    #[test]
    fn total_tokens_saturates_without_panicking() {
        let event = AgentEvent::ThinkEnd {
            prompt_tokens: usize::MAX,
            completion_tokens: 1,
        };
        assert_eq!(event.total_tokens(), Some(usize::MAX));
        assert_eq!(event.tokens_used(), Some(usize::MAX));
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
    /// cancellation-aware wrapper. When `cancel` is triggered, the stream
    /// yields [`AgentEvent::Cancelled`] and terminates.
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
    /// The default preserves compatibility for agents that do not consume
    /// invocation metadata.
    fn execute_stream_with_invocation_context<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
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
    /// cancellation-aware wrapper. When `cancel` is triggered, the stream
    /// yields [`AgentEvent::Cancelled`] and terminates.
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
    /// than resetting it. The default is unsupported because extracting a
    /// borrowed text fallback from an owned message would not be lifetime-safe.
    fn chat_stream_message_with_cancel<'a>(
        &'a self,
        _message: Message,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            Err(crate::error::ReactError::Other(
                "this agent does not implement multimodal streaming (chat_stream_message_with_cancel)"
                    .to_string(),
            ))
        })
    }

    /// Structured multi-turn chat with value-scoped invocation metadata.
    ///
    /// The default delegates to the existing structured chat method so
    /// third-party agents can opt into metadata without changing semantics.
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
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
    /// The default delegates to the existing multimodal method so third-party
    /// agents remain source compatible.
    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
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
/// When `cancel` is triggered, the stream yields [`AgentEvent::Cancelled`]
/// and terminates. Shared by both `execute_stream_with_cancel` and
/// `chat_stream_with_cancel` default implementations.
fn cancel_aware_stream<'a>(
    mut stream: BoxStream<'a, Result<AgentEvent>>,
    cancel: CancellationToken,
) -> BoxStream<'a, Result<AgentEvent>> {
    let wrapped = async_stream::try_stream! {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    yield AgentEvent::Cancelled;
                    break;
                }
                next = stream.next() => {
                    match next {
                        Some(event) => yield event?,
                        None => break,
                    }
                }
            }
        }
    };
    Box::pin(wrapped)
}

fn invocation_retaining_stream<'a>(
    mut stream: BoxStream<'a, Result<AgentEvent>>,
    invocation: AgentInvocationContext,
) -> BoxStream<'a, Result<AgentEvent>> {
    let wrapped = async_stream::try_stream! {
        let _invocation = invocation;
        while let Some(event) = stream.next().await {
            yield event?;
        }
    };
    Box::pin(wrapped)
}

#[cfg(test)]
mod invocation_retaining_stream_tests {
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

    /// Called after a tool invocation succeeds.
    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
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

    /// Called when the agent emits its final answer.
    fn on_final_answer<'a>(&'a self, _agent: &'a str, _answer: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called at the end of each outer control-loop iteration.
    fn on_iteration<'a>(&'a self, _agent: &'a str, _iteration: usize) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}
