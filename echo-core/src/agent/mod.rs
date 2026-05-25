//! Agent core trait, events, and callback interfaces

pub mod builder;
mod critic;
mod executor;
mod plan;
mod reflection;
mod types;

pub use critic::{CompositeCritic, CompositeStrategy, Critic, StaticCritic, ThresholdCritic};
pub use executor::{Executor, ReactExecutor, SimpleExecutor};
pub use plan::{
    IssueSeverity, Plan, PlanOutput, PlanStep, PlanStepOutput, PlanStore, PlanSummary,
    PlanValidationIssue, Planner, StaticPlanner, StepResult, StepStatus, plan_output_schema,
};
pub use reflection::{
    InMemoryReflectionStore, ReflectionExperience, ReflectionRecord, ReflectionStore,
    default_refinement_prompt, default_reflection_prompt,
};
pub use types::{Critique, CritiqueOutput, critique_output_schema};

use crate::error::{ReactError, Result};
use crate::llm::ToolDefinition;
use crate::llm::types::Message;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::stream::StreamExt as _;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
pub use tokio_util::sync::CancellationToken;

/// Events produced during Agent execution
///
/// Cover each phase of the Agent lifecycle for progress bars, logs, UI updates, etc.
#[derive(Debug)]
#[non_exhaustive]
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

    // ── Tool Invocation ──────────────────────────────────────────────────────────
    /// Preparing to invoke a tool
    ToolCall {
        /// Tool name
        name: String,
        /// Tool arguments (JSON format)
        args: Value,
    },
    /// Tool execution completed
    ToolResult {
        /// Tool name
        name: String,
        /// Tool execution result (string format)
        output: String,
    },
    /// Tool execution error
    ToolError {
        /// Tool name
        name: String,
        /// Error message
        error: String,
    },

    // ── Step-level Events ────────────────────────────────────────────────────────
    /// Plan-and-Execute engine generated a plan
    PlanGenerated {
        /// List of plan step descriptions
        steps: Vec<String>,
    },
    /// Plan step execution started
    StepStart {
        /// Step index (0-based)
        step_index: usize,
        /// Step description
        description: String,
    },
    /// Plan step execution ended
    StepEnd {
        /// Step index (0-based)
        step_index: usize,
        /// Whether the step executed successfully
        success: bool,
    },

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
    /// Agent-to-agent handoff started
    HandoffStart {
        /// Source agent name
        from: String,
        /// Target agent name
        to: String,
    },
    /// Agent-to-agent handoff ended
    HandoffEnd {
        /// Target agent name
        to: String,
    },

    // ── Introspection / Reflection ──────────────────────────────────────────────────────────
    /// Reflection iteration started
    ReflectionStart {
        /// Current iteration number (starting from 1)
        iteration: usize,
    },
    /// Reflection iteration ended
    ReflectionEnd {
        /// Iteration number (starting from 1)
        iteration: usize,
        /// Reflection score (0.0-1.0)
        score: f64,
        /// Whether reflection passed
        passed: bool,
    },
    /// Evaluator produced a critique
    CritiqueGenerated {
        /// Critique score (0.0-1.0)
        score: f64,
        /// Whether the evaluation passed
        passed: bool,
        /// Evaluation feedback text
        feedback: String,
    },
    /// Refining answer based on reflection
    Refining {
        /// Current iteration number (starting from 1)
        iteration: usize,
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
    /// Tool execution in progress (ToolCall → ToolResult / ToolError)
    Acting,
    /// Plan formulation and step execution (PlanGenerated / StepStart / StepEnd)
    Planning,
    /// Reflection and refinement (ReflectionStart / CritiqueGenerated / Refining / ReflectionEnd)
    Reflecting,
    /// Agent-to-agent switching (HandoffStart → HandoffEnd)
    HandingOff,
    /// Final result produced or cancelled
    Terminal,
}

impl AgentEvent {
    /// Return prompt token count for `ThinkEnd`.
    pub fn prompt_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd { prompt_tokens, .. } => Some(*prompt_tokens),
            _ => None,
        }
    }

    /// Return completion token count for `ThinkEnd`.
    pub fn completion_tokens(&self) -> Option<usize> {
        match self {
            AgentEvent::ThinkEnd {
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
            } => Some(prompt_tokens + completion_tokens),
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
            | AgentEvent::MemoryRecalled { .. }
            | AgentEvent::ContextCompressed { .. }
            | AgentEvent::Chart { .. } => AgentPhase::Thinking,

            AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::ToolError { .. }
            | AgentEvent::GuardTriggered { .. }
            | AgentEvent::SafetyNotice { .. }
            | AgentEvent::ParameterError { .. } => AgentPhase::Acting,

            AgentEvent::PlanGenerated { .. }
            | AgentEvent::StepStart { .. }
            | AgentEvent::StepEnd { .. } => AgentPhase::Planning,

            AgentEvent::ReflectionStart { .. }
            | AgentEvent::ReflectionEnd { .. }
            | AgentEvent::CritiqueGenerated { .. }
            | AgentEvent::Refining { .. } => AgentPhase::Reflecting,

            AgentEvent::HandoffStart { .. } | AgentEvent::HandoffEnd { .. } => {
                AgentPhase::HandingOff
            }

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
                | AgentEvent::ToolError { .. }
                | AgentEvent::ParameterError { .. }
                | AgentEvent::PlanGenerated { .. }
                | AgentEvent::StepEnd { .. }
                | AgentEvent::ReflectionEnd { .. }
                | AgentEvent::HandoffEnd { .. }
                | AgentEvent::ContextCompressed { .. }
                | AgentEvent::FinalAnswer(_)
                | AgentEvent::Cancelled
                | AgentEvent::Error { .. }
        )
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
    /// (if `tasks` feature is enabled), then enters the ReAct loop.
    /// Use this for standalone, single-round tasks where the agent starts fresh
    /// or resumes from a checkpoint.
    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>>;

    /// Execute a task and stream lifecycle events.
    ///
    /// Same semantics as [`Self::execute`] but returns a stream of
    /// [`AgentEvent`] for real-time observability.
    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>>;

    /// Execute a task with cooperative cancellation support.
    ///
    /// The default implementation wraps [`Self::execute_stream`] with a
    /// cancellation-aware wrapper. When `cancel` is triggered, the stream
    /// yields [`AgentEvent::Cancelled`] and terminates.
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

    /// Chat with the agent in a multi-turn conversation.
    ///
    /// **Conversation-oriented**: preserves existing context (no reset),
    /// appends the user message, and runs the ReAct loop.
    /// Use this for interactive, multi-turn dialogue where the agent
    /// accumulates state across calls.
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

    /// Reset in-memory conversational state.
    ///
    /// Implementations should clear context and fire `SessionStart("clear")` hooks
    /// so that registered hooks can react to the reset event.
    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }

    /// Get the current run ID, if the agent is tracking one.
    /// Default: None. ReactAgent overrides this.
    fn current_run_id(&self) -> Option<String> { None }
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
    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        self.as_ref().chat(message)
    }
    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        self.as_ref().chat_stream(message)
    }
    fn current_run_id(&self) -> Option<String> {
        self.as_ref().current_run_id()
    }
    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.as_ref().reset()
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

/// Agent lifecycle callback interface
pub trait AgentCallback: Send + Sync {
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
