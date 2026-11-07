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

/// 原模型调用的观察结果，不授予 Agent/TaskRun 终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// 与原 provider usage 一起发布的可选客户端计时。
///
/// call_id 与 attempt_id 是本地调用/流身份，不能回填 ProviderOutputSource.request_id。
/// 所有时刻为同一请求单调时钟的相对毫秒，不使用 UI 收到批准块的时刻。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCallObservation {
    pub call_id: String,
    pub attempt_id: String,
    pub outcome: ModelCallOutcome,
    pub request_duration_ms: u64,
    pub first_output_ms: Option<u64>,
    pub last_output_ms: Option<u64>,
    pub output_samples: u64,
    pub timing_comparable: bool,
}

impl ModelCallObservation {
    /// 只核算同一已完成尝试的可比较输出；总请求耗时不是解码分母。
    pub fn observed_output_tokens_per_second(&self, completion_tokens: u64) -> Option<f64> {
        if self.outcome != ModelCallOutcome::Completed
            || !self.timing_comparable
            || self.output_samples < 2
            || completion_tokens == 0
        {
            return None;
        }
        let last = self.last_output_ms?;
        if last > self.request_duration_ms {
            return None;
        }
        let duration = last.checked_sub(self.first_output_ms?)?;
        (duration > 0).then_some(completion_tokens as f64 * 1000.0 / duration as f64)
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
    /// The current lossless provider-neutral output contract. Block completion
    /// is distinct from runtime settlement and task/goal completion.
    AssistantMessage {
        /// Stable block identity and provider/request ordering metadata.
        block: AssistantMessageBlock,
    },
    /// Runtime execution observation. This is not a TaskRun completion signal.
    ExecutionStatus(ExecutionStatus),
    /// Provider-reported LLM usage for a single request.
    ///
    /// This event carries provider usage and cache observability. When `usage_reported` is
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
        /// 缺失表示该 producer 没有可证明的调用关联/计时，不得按零耗时处理。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<ModelCallObservation>,
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
    /// 非原source投影的进程内标志；当前canonical消费者明确拒绝此类状态。
    /// 不进入JSON/Message/Checkpoint合同，也不授予执行owner或物理结算资格。
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
    /// Current model step; typed assistant blocks/status carry actual progress
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

    /// Return current provider-reported prompt tokens; absent usage remains unknown.
    pub fn prompt_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::LlmUsage {
                prompt_tokens,
                usage_reported: true,
                ..
            } => Some(*prompt_tokens),
            _ => None,
        }
    }

    /// Return current provider-reported completion tokens.
    pub fn completion_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::LlmUsage {
                completion_tokens,
                usage_reported: true,
                ..
            } => Some(*completion_tokens),
            _ => None,
        }
    }

    /// Return current provider-reported total usage.
    pub fn total_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::LlmUsage {
                total_tokens,
                usage_reported: true,
                ..
            } => Some(*total_tokens),
            _ => None,
        }
    }

    /// Total provider-reported tokens for current compact usage consumers.
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
    /// let event = AgentEvent::ToolBatchEnd;
    /// assert_eq!(event.phase(), AgentPhase::Acting);
    ///
    /// let event = AgentEvent::FinalAnswer("done".into());
    /// assert_eq!(event.phase(), AgentPhase::Terminal);
    /// ```
    pub fn phase(&self) -> AgentPhase {
        match self {
            // provider阶段描述正文类型，不能充当执行生命周期终态。
            AgentEvent::AssistantMessage { .. } => AgentPhase::Thinking,
            AgentEvent::ExecutionStatus(status) => status.phase,
            AgentEvent::LlmUsage { .. }
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
    /// assert!(AgentEvent::Cancelled.is_checkpoint());
    /// assert!(AgentEvent::FinalAnswer("done".into()).is_checkpoint());
    /// assert!(!AgentEvent::FinalAnswer("body".into()).is_execution_terminal());
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
            AgentEvent::ToolResult { .. }
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
    use super::{AgentEvent, ModelCallObservation, ModelCallOutcome};

    #[test]
    fn missing_model_observation_round_trips_without_inventing_timing()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = serde_json::json!({"type":"llm_usage","data":{
            "model":"fixture","prompt_tokens":100,"completion_tokens":50,"total_tokens":150,
            "cached_prompt_tokens":80,"cache_creation_prompt_tokens":0,"usage_reported":true
        }});
        let event: AgentEvent = serde_json::from_value(original.clone())?;
        assert!(matches!(
            &event,
            AgentEvent::LlmUsage {
                observation: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(&event)?, original);
        assert_eq!(event.tokens_used(), Some(150));
        assert!(!event.is_execution_terminal());
        Ok(())
    }

    #[test]
    fn observed_speed_requires_complete_comparable_multi_sample_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let observation = ModelCallObservation {
            call_id: "logical-call".into(),
            attempt_id: "stream-attempt".into(),
            outcome: ModelCallOutcome::Completed,
            request_duration_ms: 2200,
            first_output_ms: Some(200),
            last_output_ms: Some(1200),
            output_samples: 2,
            timing_comparable: true,
        };
        assert_eq!(
            observation.observed_output_tokens_per_second(100),
            Some(100.0)
        );
        let restored: ModelCallObservation =
            serde_json::from_value(serde_json::to_value(&observation)?)?;
        assert_eq!(restored, observation);
        for invalid in [
            ModelCallObservation {
                outcome: ModelCallOutcome::Failed,
                ..observation.clone()
            },
            ModelCallObservation {
                timing_comparable: false,
                ..observation.clone()
            },
            ModelCallObservation {
                output_samples: 1,
                ..observation.clone()
            },
            ModelCallObservation {
                first_output_ms: None,
                ..observation.clone()
            },
            ModelCallObservation {
                last_output_ms: Some(200),
                ..observation.clone()
            },
            ModelCallObservation {
                last_output_ms: Some(100),
                ..observation.clone()
            },
            ModelCallObservation {
                last_output_ms: Some(2300),
                ..observation.clone()
            },
        ] {
            assert_eq!(invalid.observed_output_tokens_per_second(100), None);
        }
        assert_eq!(observation.observed_output_tokens_per_second(0), None);
        Ok(())
    }

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
        let event = AgentEvent::LlmUsage {
            model: "fixture".into(),
            prompt_tokens: usize::MAX,
            completion_tokens: 1,
            total_tokens: usize::MAX,
            cached_prompt_tokens: 0,
            cache_creation_prompt_tokens: 0,
            usage_reported: true,
            observation: None,
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

    /// String convenience for a fresh execution. The current typed producer owns
    /// input admission, context reset/restore, cancellation and physical settlement.
    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let source = self
                .execute_stream_message_with_invocation_context(
                    Message::user(task.to_string()),
                    CancellationToken::new(),
                    AgentInvocationContext::default(),
                )
                .await?;
            collect_canonical_agent_output(source, self.current_run_id()).await
        })
    }

    /// Current raw-event convenience, backed by the same typed execution kernel.
    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream_with_cancel(task, CancellationToken::new())
    }

    fn execute_stream_with_cancel<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream_message_with_invocation_context(
            Message::user(task.to_string()),
            cancel.clone(),
            AgentInvocationContext {
                cancel: Some(cancel),
                ..Default::default()
            },
        )
    }

    /// Convert only the caller's explicit String; every invocation field stays intact.
    fn execute_stream_with_invocation_context<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
        mut invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        invocation.cancel = Some(cancel.clone());
        self.execute_stream_message_with_invocation_context(
            Message::user(task.to_string()),
            cancel,
            invocation,
        )
    }

    /// Multi-turn String convenience. It never falls back to fresh execute semantics.
    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let source = self
                .chat_stream_message_with_invocation_context(
                    Message::user(message.to_string()),
                    CancellationToken::new(),
                    AgentInvocationContext::default(),
                )
                .await?;
            collect_canonical_agent_output(source, self.current_run_id()).await
        })
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.chat_stream_with_cancel(message, CancellationToken::new())
    }

    fn chat_stream_with_cancel<'a>(
        &'a self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.chat_stream_message_with_invocation_context(
            Message::user(message.to_string()),
            cancel.clone(),
            AgentInvocationContext {
                cancel: Some(cancel),
                ..Default::default()
            },
        )
    }

    fn chat_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.chat_stream_message_with_invocation_context(
            message,
            cancel.clone(),
            AgentInvocationContext {
                cancel: Some(cancel),
                ..Default::default()
            },
        )
    }

    /// Canonical multi-turn producer. Implementations accept the whole current
    /// Message and invocation or refuse before source admission; no text/metadata fallback.
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        _message: Message,
        _cancel: CancellationToken,
        _invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async { Err(ReactError::Other("canonical typed chat invocation".into())) })
    }

    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.execute_stream_message_with_invocation_context(
            message,
            cancel.clone(),
            AgentInvocationContext {
                cancel: Some(cancel),
                ..Default::default()
            },
        )
    }

    /// Canonical fresh-execution producer. The source retains its original resources,
    /// reports its own native status and closes physically; this default invents none.
    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        _message: Message,
        _cancel: CancellationToken,
        _invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async {
            Err(ReactError::Other(
                "canonical typed execute invocation".into(),
            ))
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
    /// 只支持即时steer而不提供真实接纳/排空/结算信号的复用方，在此明确返回
    /// [`AgentSteerError::Unsupported`]，不把普通消息投递冒作完整生命周期。
    fn steer_input_tracked(
        &self,
        _expected_turn_id: Option<&str>,
        _message: Message,
    ) -> std::result::Result<AgentSteerReceipt, AgentSteerError> {
        Err(AgentSteerError::Unsupported)
    }

    /// Bind a working directory for this agent's tool calls (Sprint 8 worktree
    /// isolation). When set, every shell/file/git tool runs inside `path`
    /// (via `ToolContext.working_dir`). `None` clears it (restore default cwd).
    ///
    /// 配置级默认工作目录仍是当前能力；本次调用的工作目录使用
    /// `AgentInvocationContext::working_dir`，不借它注入共享运行身份。
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

/// Thin nonstream projection of the same producer; always drain its physical EOF.
/// A publication or error does not authorize dropping callbacks/effects still owned by it.
async fn collect_canonical_agent_output(
    mut source: BoxStream<'_, Result<AgentEvent>>,
    owner_run: Option<String>,
) -> Result<String> {
    let mut output = AgentOutputProjection::default();
    let mut first_error = None;
    let mut unresolved = None::<ReactError>;
    while let Some(item) = source.next().await {
        let matches_owner = |error: &ReactError, output: &AgentOutputProjection| {
            matches!(error, ReactError::ExecutionNotSettled { run_id, execution_id }
                if owner_run.as_deref() == Some(run_id.as_str())
                    && execution_id.as_deref().is_some()
                    && execution_id.as_deref() == output.execution_id())
        };
        let observed = match item {
            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                // Foreign observation remains sticky; own real terminal can clear only its exact pair.
                if unresolved
                    .as_ref()
                    .is_none_or(|prior| matches_owner(prior, &output))
                {
                    unresolved = Some(error);
                }
                Ok(())
            }
            Ok(AgentEvent::ExecutionStatus(status)) => {
                let checked = output.observe_status(&status);
                match checked {
                    Err(error) => Err(error),
                    Ok(changed) => {
                        if changed
                            && matches!(
                                status.status,
                                ExecutionStatusKind::Settled
                                    | ExecutionStatusKind::Failed
                                    | ExecutionStatusKind::Cancelled
                            )
                            && unresolved
                                .as_ref()
                                .is_some_and(|prior| matches_owner(prior, &output))
                        {
                            unresolved = None;
                        }
                        if let Some(error) = output.terminal_failure() {
                            first_error.get_or_insert(error);
                        }
                        Ok(())
                    }
                }
            }
            Ok(AgentEvent::AssistantMessage { block }) => output.observe_block(&block).map(|_| ()),
            Ok(AgentEvent::FinalAnswer(answer)) => output.confirm_answer(&answer),
            Ok(AgentEvent::Error { failure, .. }) => Err(match failure.terminal_kind {
                crate::error::AgentTerminalKind::Cancelled => {
                    crate::error::AgentError::Cancelled(failure.message).into()
                }
                crate::error::AgentTerminalKind::TimedOut => {
                    crate::error::AgentError::Timeout(failure.message).into()
                }
                crate::error::AgentTerminalKind::PermissionDenied => {
                    crate::error::AgentError::PermissionDenied(failure.message).into()
                }
                crate::error::AgentTerminalKind::Failed => ReactError::Other(failure.message),
            }),
            Ok(AgentEvent::Cancelled) => {
                Err(crate::error::AgentError::Cancelled("source cancelled".into()).into())
            }
            Err(error) => Err(error),
            Ok(_) => Ok(()),
        };
        if let Err(error) = observed {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = unresolved {
        return Err(error);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    output.finish()
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

    /// Optional current observer of the exact source tool invocation identity.
    fn on_tool_start_with_id<'a>(
        &'a self,
        _agent: &'a str,
        _call_id: &'a str,
        _tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_end_with_id<'a>(
        &'a self,
        _agent: &'a str,
        _call_id: &'a str,
        _tool: &'a str,
        _result: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_tool_error_with_id<'a>(
        &'a self,
        _agent: &'a str,
        _call_id: &'a str,
        _tool: &'a str,
        _error: &'a ReactError,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {})
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
mod canonical_agent_runtime_contract_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn status(kind: ExecutionStatusKind, ordinal: u64) -> AgentEvent {
        AgentEvent::ExecutionStatus(ExecutionStatus {
            adapter_generated: false,
            execution_id: "native-source".into(),
            request_id: Some("native-request".into()),
            sequence: ordinal,
            phase: AgentPhase::Terminal,
            status: kind,
            reason: None,
        })
    }

    struct Call {
        chat: bool,
        message: Message,
        invocation: AgentInvocationContext,
    }

    #[derive(Default)]
    struct NativeAgent {
        calls: Arc<Mutex<Vec<Call>>>,
        text_calls: AtomicUsize,
        tail_failure: bool,
        terminal_missing: bool,
    }

    impl NativeAgent {
        fn source<'a>(
            &'a self,
            chat: bool,
            message: Message,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(Call {
                    chat,
                    message,
                    invocation,
                });
            Box::pin(async move {
                let mut events = vec![
                    Ok(status(ExecutionStatusKind::Started, 0)),
                    Ok(AgentEvent::FinalAnswer("安全最终中文🙂".into())),
                ];
                if !self.terminal_missing {
                    events.push(Ok(status(ExecutionStatusKind::Settled, 1)));
                }
                if self.tail_failure {
                    events.push(Err(ReactError::Other("original tail failure".into())));
                }
                Ok(futures::stream::iter(events).boxed())
            })
        }
    }

    impl Agent for NativeAgent {
        fn name(&self) -> &str {
            "native-current-custom"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        // 旧required方法只为tests-first编译；新默认便利方法不得回到它们。
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            self.text_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ReactError::Other("text-only entry was used".into())) })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.text_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ReactError::Other("text-only source was used".into())) })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.source(false, message, invocation)
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.source(true, message, invocation)
        }
    }

    #[tokio::test]
    async fn canonical_default_chat_and_raw_cancel_route_to_their_original_typed_kernels()
    -> Result<()> {
        let agent = NativeAgent::default();
        assert_eq!(agent.chat("会话").await?, "安全最终中文🙂");
        let events = agent
            .execute_stream_with_cancel("执行", CancellationToken::new())
            .await?
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 3);
        let calls = agent
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(calls.len(), 2);
        assert!(calls.first().is_some_and(
            |call| call.chat && call.message.text_content().as_deref() == Some("会话")
        ));
        assert!(calls.get(1).is_some_and(
            |call| !call.chat && call.message.text_content().as_deref() == Some("执行")
        ));
        assert_eq!(agent.text_calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn canonical_nonstream_waits_for_real_status_and_preserves_original_tail_error() {
        let missing = NativeAgent {
            terminal_missing: true,
            ..Default::default()
        };
        assert!(missing.chat("no terminal").await.is_err());
        let failed = NativeAgent {
            tail_failure: true,
            ..Default::default()
        };
        assert!(
            matches!(failed.chat("tail error").await, Err(error) if error.to_string().contains("original tail failure"))
        );
    }

    #[tokio::test]
    async fn canonical_box_forwards_rich_message_and_every_invocation_field() -> Result<()> {
        let mut message = Message::user("输入".into());
        message.content = crate::llm::types::MessageContent::Parts(vec![
            crate::llm::types::ContentPart::Text {
                text: "中文🙂".into(),
            },
            crate::llm::types::ContentPart::File {
                name: "附件.txt".into(),
                content: "binary-safe-text".into(),
            },
        ]);
        message.name = Some("native-user".into());
        message.reasoning_blocks = Some(vec![crate::llm::types::ReasoningBlock::Signed {
            thinking: "思考原文".into(),
            signature: "sig".into(),
        }]);
        let expected =
            serde_json::to_value(&message).map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = NativeAgent::default();
        let invocation = AgentInvocationContext {
            runtime: Some(crate::tools::ExternalRunContext {
                run_id: Some("run".into()),
                execution_id: Some("native-source".into()),
                ..Default::default()
            }),
            runtime_state_id: Some("runtime-state".into()),
            transcript_generation_id: Some("generation".into()),
            working_dir: Some(std::path::PathBuf::from("/current/workspace")),
            disabled_tools: Some(std::collections::HashSet::from(["shell".into()])),
            allowed_tools: Some(std::collections::HashSet::from(["read_file".into()])),
            visible_tools: Some(std::collections::HashSet::from(["read_file".into()])),
            tool_access: Some(crate::tools::ToolAccess::ReadOnly),
            history: Some(vec![Message::user("原会话".into())]),
            run_budget: Some(RunBudgetPolicy {
                max_model_tokens: Some(12),
                ..Default::default()
            }),
            resource_guards: vec![crate::tools::InvocationResourceGuard::new(
                "original-resource".to_string(),
            )],
            ..Default::default()
        };
        let records = Arc::clone(&agent.calls);
        let boxed: Box<dyn Agent> = Box::new(agent);
        let _events = boxed
            .chat_stream_message_with_invocation_context(
                message,
                CancellationToken::new(),
                invocation,
            )
            .await?
            .collect::<Vec<_>>()
            .await;
        let calls = records.lock().unwrap_or_else(|error| error.into_inner());
        let call = calls
            .first()
            .ok_or_else(|| ReactError::Other("typed source was not invoked".into()))?;
        assert_eq!(
            serde_json::to_value(&call.message)
                .map_err(|error| ReactError::Other(error.to_string()))?,
            expected
        );
        assert_eq!(call.invocation.resource_guards.len(), 1);
        assert_eq!(
            call.invocation
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.execution_id.as_deref()),
            Some("native-source")
        );
        assert_eq!(
            call.invocation.working_dir.as_deref(),
            Some(std::path::Path::new("/current/workspace"))
        );
        assert!(
            call.invocation
                .visible_tools
                .as_ref()
                .is_some_and(|visible| visible.contains("read_file"))
        );
        assert_eq!(
            call.invocation.runtime_state_id.as_deref(),
            Some("runtime-state")
        );
        assert_eq!(
            call.invocation.transcript_generation_id.as_deref(),
            Some("generation")
        );
        assert_eq!(
            call.invocation.tool_access,
            Some(crate::tools::ToolAccess::ReadOnly)
        );
        assert!(call.invocation.history.as_ref().is_some_and(|history| {
            history
                .first()
                .is_some_and(|message| message.text_content().as_deref() == Some("原会话"))
        }));
        assert!(
            call.invocation
                .allowed_tools
                .as_ref()
                .is_some_and(|allowed| allowed.contains("read_file"))
        );
        assert!(
            call.invocation
                .disabled_tools
                .as_ref()
                .is_some_and(|disabled| disabled.contains("shell"))
        );
        assert_eq!(
            call.invocation
                .run_budget
                .as_ref()
                .and_then(|budget| budget.max_model_tokens),
            Some(12)
        );
        Ok(())
    }

    struct UnscopedOnly(Arc<AtomicUsize>);
    impl Agent for UnscopedOnly {
        fn name(&self) -> &str {
            "unscoped"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("unscoped".into()) })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(futures::stream::iter([Ok(AgentEvent::FinalAnswer(
                    "must not synthesize native status".into(),
                ))])
                .boxed())
            })
        }
    }
    #[tokio::test]
    async fn canonical_default_refuses_unsupported_invocation_before_starting_unscoped_source() {
        let calls = Arc::new(AtomicUsize::new(0));
        let agent = UnscopedOnly(calls.clone());
        let invocation = AgentInvocationContext {
            history: Some(vec![Message::user("must retain".into())]),
            ..Default::default()
        };
        let result = agent
            .execute_stream_with_invocation_context("current", CancellationToken::new(), invocation)
            .await;
        assert!(
            matches!(result, Err(ReactError::Other(message)) if message.contains("canonical typed"))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod canonical_physical_source_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct OwnerGuard(Arc<AtomicUsize>);
    impl Drop for OwnerGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Source {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        drops: Arc<AtomicUsize>,
    }
    impl Agent for Source {
        fn name(&self) -> &str {
            "physical-source"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Err(ReactError::Other("unscoped".into())) })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async { Err(ReactError::Other("unscoped".into())) })
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _: Message,
            cancel: CancellationToken,
            _: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                let source = async_stream::stream! {
                    let _guard=OwnerGuard(self.drops.clone());
                    yield Ok(AgentEvent::ExecutionStatus(ExecutionStatus{adapter_generated:false,execution_id:"original".into(),request_id:Some("request".into()),sequence:0,phase:AgentPhase::Thinking,status:ExecutionStatusKind::Started,reason:None}));
                    if !cancel.is_cancelled(){yield Ok(AgentEvent::FinalAnswer("安全回答".into()));}
                    let kind=if cancel.is_cancelled(){ExecutionStatusKind::Cancelled}else{ExecutionStatusKind::Settled};
                    yield Ok(AgentEvent::ExecutionStatus(ExecutionStatus{adapter_generated:false,execution_id:"original".into(),request_id:Some("request".into()),sequence:1,phase:AgentPhase::Terminal,status:kind,reason:None}));
                    self.entered.notify_one();
                    self.release.notified().await;
                };
                Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
            })
        }
    }
    #[tokio::test]
    async fn canonical_nonstream_does_not_release_original_owner_before_physical_eof() -> Result<()>
    {
        let drops = Arc::new(AtomicUsize::new(0));
        let agent = Source {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            drops: drops.clone(),
        };
        let mut result = Box::pin(agent.chat("current"));
        assert!(futures::poll!(result.as_mut()).is_pending());
        agent.entered.notified().await;
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        agent.release.notify_one();
        assert_eq!(result.await?, "安全回答");
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }
    #[tokio::test]
    async fn canonical_cancel_stream_keeps_original_status_and_eof_instead_of_inventing_completion()
    -> Result<()> {
        let drops = Arc::new(AtomicUsize::new(0));
        let agent = Source {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            drops: drops.clone(),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut source = agent.chat_stream_with_cancel("cancel", cancel).await?;
        let started = source
            .next()
            .await
            .transpose()?
            .ok_or_else(|| ReactError::Other("missing start".into()))?;
        let terminal = source
            .next()
            .await
            .transpose()?
            .ok_or_else(|| ReactError::Other("missing terminal".into()))?;
        assert!(
            matches!(started,AgentEvent::ExecutionStatus(status) if status.execution_id=="original"&&!status.adapter_generated)
        );
        assert!(
            matches!(terminal,AgentEvent::ExecutionStatus(status) if status.execution_id=="original"&&status.status==ExecutionStatusKind::Cancelled&&!status.adapter_generated)
        );
        let mut eof = Box::pin(source.next());
        assert!(futures::poll!(eof.as_mut()).is_pending());
        agent.entered.notified().await;
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        agent.release.notify_one();
        assert!(eof.await.is_none());
        drop(source);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }
}

#[cfg(test)]
mod canonical_source_observation_tests {
    use super::*;
    struct ObservedSource {
        run: &'static str,
        execution: &'static str,
        kind: ExecutionStatusKind,
        tail_error: bool,
    }
    impl Agent for ObservedSource {
        fn name(&self) -> &str {
            "current-observed-source"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn current_run_id(&self) -> Option<String> {
            Some("original-run".into())
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Err(ReactError::Other("unscoped".into())) })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async { Err(ReactError::Other("unscoped".into())) })
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _: Message,
            _: CancellationToken,
            _: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                let status = |kind, sequence| {
                    AgentEvent::ExecutionStatus(ExecutionStatus {
                        adapter_generated: false,
                        execution_id: "original-source".into(),
                        request_id: Some("request".into()),
                        sequence,
                        phase: AgentPhase::Terminal,
                        status: kind,
                        reason: Some("original first cause".into()),
                    })
                };
                let mut items = vec![
                    Ok(status(ExecutionStatusKind::Started, 0)),
                    Err(ReactError::ExecutionNotSettled {
                        run_id: self.run.into(),
                        execution_id: Some(self.execution.into()),
                    }),
                ];
                if self.kind == ExecutionStatusKind::Settled {
                    items.push(Ok(AgentEvent::FinalAnswer("safe current".into())));
                }
                items.push(Ok(status(self.kind.clone(), 1)));
                if self.tail_error {
                    items.push(Err(ReactError::Other("later tail cause".into())));
                }
                Ok(futures::stream::iter(items).boxed())
            })
        }
    }
    #[tokio::test]
    async fn canonical_matching_source_observation_resolves_only_at_its_real_terminal_and_eof()
    -> Result<()> {
        let agent = ObservedSource {
            run: "original-run",
            execution: "original-source",
            kind: ExecutionStatusKind::Settled,
            tail_error: false,
        };
        assert_eq!(agent.chat("request").await?, "safe current");
        Ok(())
    }
    #[tokio::test]
    async fn canonical_foreign_execution_or_run_observation_is_not_cleared_by_own_settlement() {
        for (run, execution) in [
            ("foreign-run", "original-source"),
            ("original-run", "foreign-source"),
        ] {
            let agent = ObservedSource {
                run,
                execution,
                kind: ExecutionStatusKind::Settled,
                tail_error: false,
            };
            assert!(
                matches!(agent.chat("request").await,Err(ReactError::ExecutionNotSettled{run_id,execution_id:Some(id)}) if run_id==run&&id==execution)
            );
        }
    }
    #[tokio::test]
    async fn canonical_original_failed_or_cancelled_status_precedes_late_tail_error() {
        for kind in [ExecutionStatusKind::Failed, ExecutionStatusKind::Cancelled] {
            let agent = ObservedSource {
                run: "original-run",
                execution: "original-source",
                kind: kind.clone(),
                tail_error: true,
            };
            let result = agent.chat("request").await;
            match kind {
                ExecutionStatusKind::Cancelled => assert!(
                    matches!(result,Err(ReactError::Agent(error)) if matches!(*error,crate::error::AgentError::Cancelled(ref text) if text=="original first cause"))
                ),
                _ => assert!(
                    matches!(result,Err(error) if error.to_string().contains("original first cause")&&!error.to_string().contains("later tail cause"))
                ),
            }
        }
    }
}

#[cfg(test)]
mod canonical_declared_execute_contract_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // 只在新trait默认方法声明存在后编译；不把它冒作旧required接口下的tests-first。
    struct TypedExecuteOnly(AtomicUsize);
    impl Agent for TypedExecuteOnly {
        fn name(&self) -> &str {
            "current-execute-only"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                assert_eq!(message.text_content().as_deref(), Some("当前执行"));
                assert!(invocation.history.is_none());
                let status = |kind, sequence| {
                    AgentEvent::ExecutionStatus(ExecutionStatus {
                        adapter_generated: false,
                        execution_id: "execute-source".into(),
                        request_id: Some("execute-request".into()),
                        sequence,
                        phase: AgentPhase::Terminal,
                        status: kind,
                        reason: None,
                    })
                };
                Ok(futures::stream::iter([
                    Ok(status(ExecutionStatusKind::Started, 0)),
                    Ok(AgentEvent::FinalAnswer("当前完成".into())),
                    Ok(status(ExecutionStatusKind::Settled, 1)),
                ])
                .boxed())
            })
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async { Err(ReactError::Other("execute crossed into chat".into())) })
        }
    }
    #[tokio::test]
    async fn canonical_default_execute_uses_current_execute_source_without_string_override()
    -> Result<()> {
        let agent = TypedExecuteOnly(AtomicUsize::new(0));
        assert_eq!(agent.execute("当前执行").await?, "当前完成");
        assert_eq!(agent.0.load(Ordering::SeqCst), 1);
        Ok(())
    }
}

#[cfg(test)]
mod canonical_typed_error_projection_tests {
    use super::*;
    use crate::error::{AgentError, AgentFailure, AgentTerminalKind};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Source {
        failure: AgentFailure,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        drops: Arc<AtomicUsize>,
    }
    impl Agent for Source {
        fn name(&self) -> &str {
            "typed-error-source"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                let failure = self.failure.clone();
                let status = |kind, sequence| {
                    AgentEvent::ExecutionStatus(ExecutionStatus {
                        adapter_generated: false,
                        execution_id: "typed-error-source".into(),
                        request_id: Some("typed-error-request".into()),
                        sequence,
                        phase: AgentPhase::Terminal,
                        status: kind,
                        reason: Some("later native failure".into()),
                    })
                };
                let source = async_stream::stream! {
                    let _guard = Guard(self.drops.clone());
                    yield Ok(status(ExecutionStatusKind::Started, 0));
                    yield Ok(AgentEvent::Error { source: "fixture".into(), message: failure.message.clone(), failure });
                    yield Ok(status(ExecutionStatusKind::Failed, 1));
                    yield Err(ReactError::Other("later raw cleanup failure".into()));
                    self.entered.notify_one();
                    self.release.notified().await;
                };
                Ok(Box::pin(source) as BoxStream<'a, Result<AgentEvent>>)
            })
        }
    }
    #[tokio::test]
    async fn canonical_default_string_keeps_typed_error_reason_through_tail_and_physical_eof()
    -> Result<()> {
        for kind in [
            AgentTerminalKind::TimedOut,
            AgentTerminalKind::PermissionDenied,
        ] {
            let original = match kind {
                AgentTerminalKind::TimedOut => AgentError::Timeout("original typed cause".into()),
                AgentTerminalKind::PermissionDenied => {
                    AgentError::PermissionDenied("original typed cause".into())
                }
                _ => return Err(ReactError::Other("invalid typed-error fixture".into())),
            };
            let failure = AgentFailure::try_from(&ReactError::Agent(Box::new(original)))?;
            let expected_source_message = failure.message.clone();
            let drops = Arc::new(AtomicUsize::new(0));
            let agent = Source {
                failure,
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
                drops: drops.clone(),
            };
            let mut result = Box::pin(agent.chat("current"));
            assert!(futures::poll!(result.as_mut()).is_pending());
            agent.entered.notified().await;
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            agent.release.notify_one();
            let result = result.await;
            let kept_original_kind_and_reason = match (kind, result) {
                (AgentTerminalKind::TimedOut, Err(ReactError::Agent(error))) => {
                    matches!(error.as_ref(), AgentError::Timeout(reason) if reason == &expected_source_message)
                }
                (AgentTerminalKind::PermissionDenied, Err(ReactError::Agent(error))) => {
                    matches!(error.as_ref(), AgentError::PermissionDenied(reason) if reason == &expected_source_message)
                }
                _ => false,
            };
            assert!(kept_original_kind_and_reason);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }
}
