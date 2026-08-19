//! Subagent event system — lifecycle notifications for subagent operations

use echo_core::agent::ToolInvocation;
use echo_core::tools::ToolResult;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

use super::types::{ExecutionMode, ObservedIsolation, SubagentOutcome, SubagentStatus};

const DEFAULT_CHANNEL_CAPACITY: usize = 128;

/// Lifecycle events emitted by the subagent system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentEvent {
    /// A subagent was registered.
    Registered {
        /// Name of the subagent that was registered.
        name: String,
    },
    /// A subagent was unregistered.
    Unregistered {
        /// Name of the subagent that was unregistered.
        name: String,
    },
    /// Dispatch started.
    DispatchStarted {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent being dispatched to.
        agent: String,
        /// Execution mode (e.g., `ExecutionMode::Parallel`).
        mode: ExecutionMode,
        /// Task description being dispatched.
        task: String,
        /// Stable execution id from the caller's `ExternalRunContext`
        /// (format `{task_id}:{attempt}` in EKO). `None` = legacy caller that
        /// has not opted in; bridges fall back to temp id allocation.
        /// Frontends should use this as the canonical `subagent_run_id`.
        execution_id: Option<String>,
        /// Parent run id from the caller's `ExternalRunContext`. `None` =
        /// legacy caller.
        run_id: Option<String>,
        /// Conversation id from the caller's `ExternalRunContext`. This is
        /// retained even for ad-hoc dispatches that have no formal run id.
        conversation_id: Option<String>,
        /// Message id that triggered the run (chat `message_key`). Lets the
        /// frontend pin the subagent stream to the right chat message block.
        /// `None` = non-chat path (cron, etc).
        message_id: Option<String>,
        /// True when this dispatch was started via `dispatch_background`
        /// (non-blocking); UI shows a background card and injects a finished
        /// note into the parent chat on completion.
        background: bool,
    },
    /// Isolation boundary established after setup and before model execution.
    DispatchIsolationObserved {
        parent: String,
        agent: String,
        isolation: ObservedIsolation,
        execution_id: Option<String>,
        run_id: Option<String>,
    },
    /// Dispatch completed successfully.
    DispatchCompleted {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that completed the task.
        agent: String,
        /// Duration of the dispatch in milliseconds.
        duration_ms: u64,
        /// Total tokens consumed (input + output), if available.
        tokens_used: Option<u64>,
        /// Number of ReAct iterations executed.
        iterations: Option<u64>,
        /// Final output text produced by the subagent.
        output: String,
        /// Structured terminal result consumed by the parent/application.
        result: SubagentOutcome,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Dispatch failed.
    DispatchFailed {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that failed.
        agent: String,
        /// Error message describing the failure.
        error: String,
        /// Failed or timed-out terminal status.
        status: SubagentStatus,
        /// Structured terminal result, including remaining work.
        result: SubagentOutcome,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Dispatch was cancelled.
    DispatchCancelled {
        /// Name of the parent agent that cancelled the dispatch.
        parent: String,
        /// Name of the subagent whose dispatch was cancelled.
        agent: String,
        /// Structured cancelled result.
        result: SubagentOutcome,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Subagent reasoning started.
    DispatchThinkingStarted {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that is reasoning.
        agent: String,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Subagent reasoning emitted incremental content.
    DispatchThinkingDelta {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that is reasoning.
        agent: String,
        /// Incremental reasoning text.
        content: String,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Subagent reasoning ended.
    DispatchThinkingEnded {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that finished reasoning.
        agent: String,
        /// Number of prompt tokens consumed.
        prompt_tokens: usize,
        /// Number of completion tokens consumed.
        completion_tokens: usize,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Subagent emitted final-answer text.
    DispatchTokenDelta {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent producing output.
        agent: String,
        /// Incremental final-answer text.
        content: String,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// LLM usage reported by the subagent's underlying model call (carries the
    /// full cache-diagnostic breakdown). Emitted once per model call so the
    /// frontend can render token / cache-hit metrics without peeking at the
    /// legacy `subagent://trace` channel.
    DispatchLlmUsage {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that made the model call.
        agent: String,
        /// Model name (provider-specific).
        model: String,
        /// Prompt (input) tokens for this call.
        prompt_tokens: usize,
        /// Completion (output) tokens for this call.
        completion_tokens: usize,
        /// Total tokens (input + output), as reported by the provider.
        total_tokens: usize,
        /// Prompt tokens served from the prefix cache (cache hit).
        cached_prompt_tokens: usize,
        /// Prompt tokens written into the cache (cache write).
        cache_creation_prompt_tokens: usize,
        /// Whether the provider actually returned a usage report for this call.
        usage_reported: bool,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Subagent started a tool call.
    DispatchToolStarted {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent invoking a tool.
        agent: String,
        /// Stable tool-call identity emitted by the model.
        call_id: String,
        /// Canonical requested/effective invocation after all runtime rewrites.
        invocation: ToolInvocation,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// Subagent completed a tool call.
    DispatchToolCompleted {
        /// Name of the parent agent that initiated the dispatch.
        parent: String,
        /// Name of the subagent that invoked a tool.
        agent: String,
        /// Stable tool-call identity matching [`Self::DispatchToolStarted`].
        call_id: String,
        /// Effective tool name matching the invocation event.
        name: String,
        /// Canonical rich terminal result emitted by the ReAct runner.
        result: ToolResult,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
}

/// Sync event listener trait.
pub trait SubagentEventListener: Send + Sync {
    /// Handle a subagent lifecycle event.
    ///
    /// # Parameters
    /// * `event` - The event to handle.
    fn on_event(&self, event: &SubagentEvent);
}

/// Logging listener — emits tracing events.
///
/// Implements `SubagentEventListener` to log events via `tracing::info!`.
pub struct LoggingSubagentListener;

impl SubagentEventListener for LoggingSubagentListener {
    fn on_event(&self, event: &SubagentEvent) {
        match event {
            SubagentEvent::Registered { name } => {
                info!(subagent = %name, "subagent_registered");
            }
            SubagentEvent::DispatchStarted {
                parent,
                agent,
                mode,
                ..
            } => {
                info!(
                    parent = %parent,
                    agent = %agent,
                    mode = %mode,
                    "subagent_dispatch_started"
                );
            }
            SubagentEvent::DispatchCompleted {
                parent,
                agent,
                duration_ms,
                ..
            } => {
                info!(
                    parent = %parent,
                    agent = %agent,
                    duration_ms = duration_ms,
                    "subagent_dispatch_completed"
                );
            }
            SubagentEvent::DispatchIsolationObserved {
                parent,
                agent,
                isolation,
                ..
            } => {
                info!(
                    parent = %parent,
                    agent = %agent,
                    isolation = isolation.as_str(),
                    "subagent_dispatch_isolation_observed"
                );
            }
            SubagentEvent::DispatchFailed {
                parent,
                agent,
                error,
                ..
            } => {
                info!(
                    parent = %parent,
                    agent = %agent,
                    error = %error,
                    "subagent_dispatch_failed"
                );
            }
            _ => {}
        }
    }
}

/// Async event bus for subagent lifecycle events.
///
/// Uses `tokio::sync::broadcast` for efficient fan-out.
pub struct SubagentEventBus {
    tx: broadcast::Sender<Arc<SubagentEvent>>,
    sync_listeners: Vec<Box<dyn SubagentEventListener>>,
}

impl SubagentEventBus {
    /// Create a new event bus with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    /// Create a new event bus with the specified channel capacity.
    ///
    /// # Parameters
    /// * `capacity` - Maximum number of events to buffer before dropping old ones.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            sync_listeners: Vec::new(),
        }
    }

    /// Register a sync listener (called immediately on emit).
    pub fn register(&mut self, listener: Box<dyn SubagentEventListener>) {
        self.sync_listeners.push(listener);
    }

    /// Subscribe to the async event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<SubagentEvent>> {
        self.tx.subscribe()
    }

    /// Emit an event to all listeners.
    pub fn emit(&self, event: SubagentEvent) {
        for listener in &self.sync_listeners {
            listener.on_event(&event);
        }
        let _ = self.tx.send(Arc::new(event));
    }

    /// Get the current number of active subscribers to the async event stream.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Clone for SubagentEventBus {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            sync_listeners: Vec::new(),
        }
    }
}

impl Default for SubagentEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::ToolInvocationRewrite;
    use echo_core::tools::{ToolFailure, ToolFailureCategory, ToolResultKind};
    use std::collections::HashMap;

    #[test]
    fn tool_events_round_trip_without_losing_invocation_or_result_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let invocation = ToolInvocation {
            requested_name: "requested_shell".to_string(),
            requested_args: serde_json::json!({"command": "echo requested"}),
            name: "shell".to_string(),
            args: serde_json::json!({"command": "echo effective"}),
            rewrites: vec![ToolInvocationRewrite::Approval],
        };
        let started = SubagentEvent::DispatchToolStarted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            call_id: "call-1".to_string(),
            invocation: invocation.clone(),
            execution_id: Some("task-1:1".to_string()),
            run_id: Some("run-1".to_string()),
        };
        let started = serde_json::from_value::<SubagentEvent>(serde_json::to_value(started)?)?;
        let SubagentEvent::DispatchToolStarted {
            invocation: decoded_invocation,
            ..
        } = started
        else {
            return Err(std::io::Error::other("tool-start event changed variant").into());
        };
        assert_eq!(decoded_invocation, invocation);

        let result = ToolResult {
            kind: ToolResultKind::Json,
            success: false,
            output: "preview".to_string(),
            error: Some("partial failure".to_string()),
            failure: Some(ToolFailure::new(ToolFailureCategory::PartialSideEffect)),
            data: Some(serde_json::json!({"partial": true})),
            truncated: true,
            mime_type: Some("application/json".to_string()),
            metadata: HashMap::from([("artifact_path".to_string(), "/tmp/tool.log".to_string())]),
            model_content: Vec::new(),
        };
        let completed = SubagentEvent::DispatchToolCompleted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            result,
            execution_id: Some("task-1:1".to_string()),
            run_id: Some("run-1".to_string()),
        };
        let completed = serde_json::from_value::<SubagentEvent>(serde_json::to_value(completed)?)?;
        let SubagentEvent::DispatchToolCompleted {
            name,
            result: decoded_result,
            ..
        } = completed
        else {
            return Err(std::io::Error::other("tool-result event changed variant").into());
        };
        assert_eq!(name, "shell");
        assert_eq!(decoded_result.kind, ToolResultKind::Json);
        assert!(!decoded_result.success);
        assert_eq!(decoded_result.output, "preview");
        assert_eq!(decoded_result.error.as_deref(), Some("partial failure"));
        assert_eq!(
            decoded_result.data,
            Some(serde_json::json!({"partial": true}))
        );
        assert_eq!(
            decoded_result.mime_type.as_deref(),
            Some("application/json")
        );
        assert!(decoded_result.truncated);
        assert_eq!(
            decoded_result
                .metadata
                .get("artifact_path")
                .map(String::as_str),
            Some("/tmp/tool.log")
        );
        assert_eq!(
            decoded_result.failure.map(|failure| failure.category),
            Some(ToolFailureCategory::PartialSideEffect)
        );
        Ok(())
    }

    #[test]
    fn test_event_bus_emit() {
        let bus = SubagentEventBus::new();
        bus.emit(SubagentEvent::Registered {
            name: "test".into(),
        });
    }

    #[tokio::test]
    async fn test_event_bus_subscribe() -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::new();
        let mut rx = bus.subscribe();

        bus.emit(SubagentEvent::Registered {
            name: "test".into(),
        });

        let event = rx.try_recv()?;
        if let SubagentEvent::Registered { name } = event.as_ref() {
            assert_eq!(name, "test");
            Ok(())
        } else {
            Err(std::io::Error::other("wrong event type").into())
        }
    }

    #[test]
    fn test_logging_listener() {
        let listener = LoggingSubagentListener;
        listener.on_event(&SubagentEvent::Registered {
            name: "test".into(),
        });
    }
}
