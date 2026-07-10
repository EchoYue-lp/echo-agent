//! Subagent event system — lifecycle notifications for subagent operations

use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

use super::types::{ExecutionMode, ObservedIsolation};

const DEFAULT_CHANNEL_CAPACITY: usize = 128;

/// Lifecycle events emitted by the subagent system.
#[derive(Debug, Clone, Serialize)]
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
        /// Stable execution id from the caller's [`ExternalRunContext`]
        /// (format `{task_id}:{attempt}` in EKO). `None` = legacy caller that
        /// has not opted in; bridges fall back to temp id allocation.
        /// Frontends should use this as the canonical `subagent_run_id`.
        execution_id: Option<String>,
        /// Parent run id from the caller's [`ExternalRunContext`]. `None` =
        /// legacy caller.
        run_id: Option<String>,
        /// Message id that triggered the run (chat `message_key`). Lets the
        /// frontend pin the subagent stream to the right chat message block.
        /// `None` = non-chat path (cron, etc).
        message_id: Option<String>,
        /// True when this dispatch was started via [`dispatch_background`]
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
    /// legacy `worker://trace` channel.
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
        /// Tool name.
        name: String,
        /// Tool arguments.
        args: serde_json::Value,
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
        /// Tool name.
        name: String,
        /// Tool result or error text.
        result: String,
        /// Whether the tool call succeeded.
        success: bool,
        /// Stable execution id (see [`Self::DispatchStarted::execution_id`]).
        execution_id: Option<String>,
        /// Parent run id (see [`Self::DispatchStarted::run_id`]).
        run_id: Option<String>,
    },
    /// A team was created.
    TeamCreated {
        /// Unique identifier for the team.
        team_id: String,
        /// Names of subagents assigned to the team.
        members: Vec<String>,
    },
    /// A team was dissolved.
    TeamDissolved {
        /// Unique identifier for the team that was dissolved.
        team_id: String,
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
            SubagentEvent::TeamCreated { team_id, members } => {
                info!(
                    team_id = %team_id,
                    members = ?members,
                    "team_created"
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

    #[test]
    fn test_event_bus_emit() {
        let bus = SubagentEventBus::new();
        bus.emit(SubagentEvent::Registered {
            name: "test".into(),
        });
    }

    #[tokio::test]
    async fn test_event_bus_subscribe() {
        let bus = SubagentEventBus::new();
        let mut rx = bus.subscribe();

        bus.emit(SubagentEvent::Registered {
            name: "test".into(),
        });

        let event = rx.try_recv().unwrap();
        match event.as_ref() {
            SubagentEvent::Registered { name } => assert_eq!(name, "test"),
            _ => panic!("Wrong event type"),
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
