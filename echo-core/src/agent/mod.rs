//! Agent core trait, events, and callback interfaces

pub mod builder;
mod critic;
pub mod factory;
pub mod intervention;
pub mod prompt_template;
mod types;

pub use factory::{AgentFactory, AgentFactoryConfig, DefaultAgentFactory};
pub use intervention::{CallbackBridge, InterventionCallback, InterventionResult};
pub use prompt_template::PromptTemplateManager;

pub use critic::{CompositeCritic, CompositeStrategy, Critic, StaticCritic, ThresholdCritic};
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
    /// Streaming tool progress event
    ToolStream {
        /// Tool name
        name: String,
        /// Stream event payload
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
    /// Final result produced or cancelled
    Terminal,
}

impl AgentEvent {
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
            } => Some(prompt_tokens + completion_tokens),
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
            | AgentEvent::MemoryRecalled { .. }
            | AgentEvent::ContextCompressed { .. }
            | AgentEvent::Chart { .. } => AgentPhase::Thinking,

            AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::ToolError { .. }
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
                | AgentEvent::ToolError { .. }
                | AgentEvent::ParameterError { .. }
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
    ///
    /// # ⚠️ Warning: Do NOT use for multi-turn chat UIs
    ///
    /// `execute()` **clears conversation history** on every call (only the
    /// system prompt survives). Calling it in a REPL / TUI / chatbot loop
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
    /// | REPL / TUI / chatbot | `chat()` ✓ |
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

    /// Chat with the agent in a multi-turn conversation.
    ///
    /// **Conversation-oriented**: preserves existing context (no reset),
    /// appends the user message, and runs the ReAct loop.
    /// Use this for interactive, multi-turn dialogue where the agent
    /// accumulates state across calls.
    ///
    /// **Prefer this over [`execute()`](Agent::execute)** for any UI that
    /// sends multiple messages in sequence (REPL, TUI, chatbot, web chat).
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

    /// Streaming task execution with cancellation (multimodal version).
    ///
    /// Accepts a pre-built [`Message`] so workers dispatched via subagent
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

    // ── External run context (跨 spawn 安全的值传递, 见 ExternalRunContext) ──

    /// 注入应用层 run 级上下文（worker 场景）。
    ///
    /// **背景**：`tokio::task_local!` 不会跨 `tokio::spawn` 继承。worker agent
    /// 在框架层的 `tokio::spawn`（subagent_executor.rs 的 dispatch_fork）里执行，
    /// 应用层经 task_local 注入的 run_id / cancel / trace_sink 全部丢失。本方法
    /// 让应用层经值传递（`ExternalRunContext`，跨 spawn 安全）把 context 注入到
    /// worker agent 实例——dispatch_fork 在 worker 执行前调用，pipeline 构造
    /// `ToolContext` 时读取，工具 override `execute_with_context` 即可拿到。
    ///
    /// 默认 noop。ReactAgent override：把 ctx 写入自己的 external_* Mutex 字段。
    fn set_external_context(&self, _ctx: &crate::tools::ExternalRunContext) {}

    /// 清除外部上下文（worker 执行后调用，防止泄漏到下一个 run）。
    /// 默认 noop。ReactAgent override。
    fn clear_external_context(&self) {}

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

    /// Delegate a task to a named sub-agent or team member.
    ///
    /// Default: returns error ("delegation not supported").
    /// ReactAgent overrides this when SubAgent feature is enabled.
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
