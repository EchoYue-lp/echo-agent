//! Subagent event system — lifecycle notifications for subagent operations

use echo_core::agent::{EventEnvelope, EventId, EventIdentity, StreamId, ToolInvocation};
use echo_core::tools::{SubagentLineage, ToolResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock, Weak};
use tokio::sync::broadcast;
use tracing::info;

use super::types::{ExecutionMode, ObservedIsolation, SubagentOutcome, SubagentStatus};

const DEFAULT_CHANNEL_CAPACITY: usize = 128;

/// Lifecycle events emitted by the subagent system.
#[derive(Debug, Clone, Serialize, Deserialize)]
// Boxing ToolResult would change the public event contract for every consumer.
#[allow(clippy::large_enum_variant)]
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
    /// A running Subagent sent a parent/sibling message through the default
    /// uplink sink. Emitted for observability; the delivery disposition is
    /// carried in `status` (e.g. `parent_steered`, `event_emitted`,
    /// `delivered_to_sibling`).
    UplinkReceived {
        /// Name of the dispatching parent agent.
        parent: String,
        /// Name of the sending Subagent.
        agent: String,
        /// `parent` or `sibling`.
        direction: String,
        /// Delivery disposition reported to the sender.
        status: String,
        /// Bounded message preview (first 200 chars).
        summary: String,
        /// Sending attempt's execution id.
        execution_id: Option<String>,
        /// Sending attempt's run id.
        run_id: Option<String>,
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
        /// (format `{task_id}:{attempt}` in embedding application). `None` = legacy caller that
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
        /// Structured terminal outcome consumed by the parent/application.
        outcome: SubagentOutcome,
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
        /// Structured terminal outcome, including remaining work.
        outcome: SubagentOutcome,
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
        /// Structured cancelled outcome.
        outcome: SubagentOutcome,
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

/// Complete dispatch-attempt identity hashed into every execution envelope.
///
/// Transport addressing stays in [`EventEnvelope`]. These Subagent-specific
/// fields preserve task and lineage facts without parsing an execution id or
/// adding application-owned workspace metadata to the framework.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentInvocationIdentity {
    pub parent_agent: String,
    pub agent_name: String,
    pub parent_execution_id: Option<String>,
    pub agent_path: Option<String>,
    pub task_id: Option<String>,
    pub attempt: Option<u32>,
    pub plan_revision: Option<u64>,
}

impl SubagentInvocationIdentity {
    pub fn from_lineage(
        parent_agent: impl Into<String>,
        agent_name: impl Into<String>,
        transport_identity: &EventIdentity,
        lineage: Option<&SubagentLineage>,
    ) -> crate::error::Result<Self> {
        let parent_agent = parent_agent.into();
        let agent_name = agent_name.into();
        if parent_agent.trim().is_empty() || agent_name.trim().is_empty() {
            return Err(crate::error::ReactError::Other(
                "Subagent invocation agent identity must not be empty".to_string(),
            ));
        }
        if let Some(lineage) = lineage {
            for (field, actual, expected) in [
                (
                    "agent_name",
                    lineage.agent_name.as_deref(),
                    Some(agent_name.as_str()),
                ),
                (
                    "parent_agent",
                    lineage.parent_agent.as_deref(),
                    Some(parent_agent.as_str()),
                ),
                (
                    "execution_id",
                    lineage.execution_id.as_deref(),
                    transport_identity
                        .execution_id
                        .as_ref()
                        .map(|value| value.as_str()),
                ),
                (
                    "run_id",
                    lineage.run_id.as_deref(),
                    transport_identity
                        .run_id
                        .as_ref()
                        .map(|value| value.as_str()),
                ),
            ] {
                if actual.is_some() && actual != expected {
                    return Err(crate::error::ReactError::Other(format!(
                        "Subagent lineage {field} conflicts with transport identity"
                    )));
                }
            }
        }
        Ok(Self {
            parent_execution_id: lineage.and_then(|value| value.parent_execution_id.clone()),
            agent_path: lineage.and_then(|value| value.agent_path.clone()),
            task_id: lineage.and_then(|value| value.task_id.clone()),
            attempt: lineage.and_then(|value| value.attempt),
            plan_revision: lineage.and_then(|value| value.plan_revision),
            parent_agent,
            agent_name,
        })
    }
}

/// Hashed payload carried by the generic framework [`EventEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentEventPayload {
    pub invocation: SubagentInvocationIdentity,
    pub event: SubagentEvent,
}

/// Authoritative, versioned event transport for one Subagent dispatch attempt.
pub type SubagentEventEnvelope = EventEnvelope<SubagentEventPayload>;

/// Explicit indication that an envelope replay cannot provide a contiguous
/// suffix from the requested sequence because bounded retention has advanced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentEventGap {
    pub stream_id: StreamId,
    pub requested_after: u64,
    /// First retained sequence after the gap. `None` means the known stream
    /// has advanced but its replayable suffix is no longer retained.
    pub available_from: Option<u64>,
    pub latest_sequence: u64,
}

/// Bounded replay result for one exact dispatch-attempt stream.
#[derive(Debug)]
pub struct SubagentEventReplay {
    pub events: Vec<Arc<SubagentEventEnvelope>>,
    pub gap: Option<SubagentEventGap>,
    /// Latest retained terminal is returned independently so a consumer can
    /// reconcile final output even when high-volume deltas fell out of history.
    pub terminal: Option<Arc<SubagentEventEnvelope>>,
}

#[derive(Default)]
struct SubagentPublisherState {
    sequence: u64,
    dispatch_started: Option<EventId>,
    tool_started: HashMap<String, EventId>,
    terminal_emitted: bool,
}

/// Shared state behind one attempt-scoped publisher.
struct SubagentEventPublisherInner {
    bus: SubagentEventBus,
    transport_identity: EventIdentity,
    invocation_identity: RwLock<SubagentInvocationIdentity>,
    state: Mutex<SubagentPublisherState>,
}

/// Cloneable producer that owns ordering for exactly one dispatch attempt.
#[derive(Clone)]
pub struct SubagentEventPublisher {
    inner: Arc<SubagentEventPublisherInner>,
}

impl SubagentEventPublisher {
    fn new(
        bus: SubagentEventBus,
        transport_identity: EventIdentity,
        invocation_identity: SubagentInvocationIdentity,
    ) -> crate::error::Result<Self> {
        transport_identity.validate()?;
        Ok(Self {
            inner: Arc::new(SubagentEventPublisherInner {
                bus,
                transport_identity,
                invocation_identity: RwLock::new(invocation_identity),
                state: Mutex::new(SubagentPublisherState::default()),
            }),
        })
    }

    /// Update the actual execution segment after a framework hook delegates
    /// the same logical attempt to another registered Subagent.
    pub(super) fn retarget(
        &self,
        invocation_identity: SubagentInvocationIdentity,
    ) -> crate::error::Result<()> {
        let mut current = self
            .inner
            .invocation_identity
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.parent_agent != invocation_identity.parent_agent
            || current.parent_execution_id != invocation_identity.parent_execution_id
            || current.task_id != invocation_identity.task_id
            || current.attempt != invocation_identity.attempt
            || current.plan_revision != invocation_identity.plan_revision
        {
            return Err(crate::error::ReactError::Other(
                "Subagent publisher retarget changed stable attempt identity".to_string(),
            ));
        }
        *current = invocation_identity;
        Ok(())
    }

    pub(super) fn retarget_from_lineage(
        &self,
        parent_agent: impl Into<String>,
        agent_name: impl Into<String>,
        lineage: Option<&SubagentLineage>,
    ) -> crate::error::Result<()> {
        let identity = SubagentInvocationIdentity::from_lineage(
            parent_agent,
            agent_name,
            &self.inner.transport_identity,
            lineage,
        )?;
        self.retarget(identity)
    }

    /// Emit one event in the attempt's monotonic sequence.
    pub fn emit(&self, event: SubagentEvent) -> crate::error::Result<Arc<SubagentEventEnvelope>> {
        let invocation_identity = self
            .inner
            .invocation_identity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.terminal_emitted {
            return Err(crate::error::ReactError::Other(
                "Subagent event emitted after terminal settlement".to_string(),
            ));
        }
        if !is_execution_event(&event) {
            return Err(crate::error::ReactError::Other(
                "registry events cannot be emitted through a Subagent attempt publisher"
                    .to_string(),
            ));
        }
        if let SubagentEvent::DispatchStarted {
            conversation_id,
            message_id,
            ..
        } = &event
        {
            let expected_conversation = self
                .inner
                .transport_identity
                .conversation_id
                .as_ref()
                .map(|value| value.as_str());
            let expected_message = self
                .inner
                .transport_identity
                .message_id
                .as_ref()
                .map(|value| value.as_str());
            if conversation_id.as_deref() != expected_conversation
                || message_id.as_deref() != expected_message
            {
                return Err(crate::error::ReactError::Other(
                    "Subagent dispatch start conversation or message identity changed".to_string(),
                ));
            }
        }
        match &event {
            SubagentEvent::DispatchStarted { .. } if state.dispatch_started.is_some() => {
                return Err(crate::error::ReactError::Other(
                    "Subagent dispatch start was emitted more than once".to_string(),
                ));
            }
            SubagentEvent::DispatchStarted { .. } => {}
            _ if state.dispatch_started.is_none() => {
                return Err(crate::error::ReactError::Other(
                    "Subagent execution event was emitted before dispatch start".to_string(),
                ));
            }
            _ => {}
        }
        if let SubagentEvent::DispatchToolStarted { call_id, .. } = &event
            && state.tool_started.contains_key(call_id)
        {
            return Err(crate::error::ReactError::Other(format!(
                "duplicate in-flight Subagent tool call: {call_id}"
            )));
        }
        let route = event_route(&event).ok_or_else(|| {
            crate::error::ReactError::Other(
                "Subagent execution event has no routing identity".to_string(),
            )
        })?;
        validate_event_route(route, &self.inner.transport_identity, &invocation_identity)?;
        let sequence = state.sequence.checked_add(1).ok_or_else(|| {
            crate::error::ReactError::Other(
                "Subagent event sequence exhausted before terminal event".to_string(),
            )
        })?;
        let parent_event_id = match &event {
            SubagentEvent::DispatchStarted { .. } if state.dispatch_started.is_none() => {
                self.inner.transport_identity.parent_event_id.clone()
            }
            SubagentEvent::DispatchToolCompleted { call_id, .. } => {
                Some(state.tool_started.get(call_id).cloned().ok_or_else(|| {
                    crate::error::ReactError::Other(format!(
                        "orphan Subagent tool completion: {call_id}"
                    ))
                })?)
            }
            _ => state
                .dispatch_started
                .clone()
                .or_else(|| self.inner.transport_identity.parent_event_id.clone()),
        };
        let envelope = EventEnvelope::new(
            &self.inner.transport_identity,
            sequence,
            parent_event_id,
            SubagentEventPayload {
                invocation: invocation_identity,
                event: event.clone(),
            },
        )?;
        state.sequence = sequence;
        match &event {
            SubagentEvent::DispatchStarted { .. } if state.dispatch_started.is_none() => {
                state.dispatch_started = Some(envelope.event_id.clone());
            }
            SubagentEvent::DispatchToolStarted { call_id, .. } => {
                state
                    .tool_started
                    .insert(call_id.clone(), envelope.event_id.clone());
            }
            SubagentEvent::DispatchToolCompleted { call_id, .. } => {
                state.tool_started.remove(call_id);
            }
            _ => {}
        }
        if is_terminal_event(&event) {
            state.terminal_emitted = true;
        }
        drop(state);

        let envelope = Arc::new(envelope);
        self.inner.bus.emit_envelope(Arc::clone(&envelope));
        if is_terminal_event(&event) {
            self.inner.bus.retire_publisher(self);
        }
        Ok(envelope)
    }

    pub fn stream_id(&self) -> &StreamId {
        &self.inner.transport_identity.stream_id
    }

    pub fn dispatch_started_event_id(&self) -> Option<EventId> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .dispatch_started
            .clone()
    }
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
/// Raw subscriptions remain available for compatibility. Execution producers
/// should create a [`SubagentEventPublisher`] and consumers that need identity,
/// order, or recovery should subscribe to envelopes.
pub struct SubagentEventBus {
    raw_tx: broadcast::Sender<Arc<SubagentEvent>>,
    envelope_tx: broadcast::Sender<Arc<SubagentEventEnvelope>>,
    sync_listeners: Arc<RwLock<Vec<Arc<dyn SubagentEventListener>>>>,
    replay: Arc<Mutex<SubagentReplayState>>,
    active_publishers: Arc<Mutex<HashMap<String, Weak<SubagentEventPublisherInner>>>>,
    active_streams: Arc<Mutex<HashMap<StreamId, Weak<SubagentEventPublisherInner>>>>,
}

struct SubagentReplayState {
    capacity: usize,
    history: VecDeque<Arc<SubagentEventEnvelope>>,
    boundaries: VecDeque<Arc<SubagentEventEnvelope>>,
    terminals: VecDeque<Arc<SubagentEventEnvelope>>,
}

impl SubagentReplayState {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            history: VecDeque::with_capacity(capacity),
            boundaries: VecDeque::with_capacity(capacity),
            terminals: VecDeque::with_capacity(capacity),
        }
    }

    fn retain(&mut self, envelope: Arc<SubagentEventEnvelope>) {
        self.history.push_back(Arc::clone(&envelope));
        while self.history.len() > self.capacity {
            self.history.pop_front();
        }
        if is_replay_boundary(&envelope.payload.event) {
            self.boundaries.push_back(Arc::clone(&envelope));
            while self.boundaries.len() > self.capacity {
                self.boundaries.pop_front();
            }
        }
        if is_terminal_event(&envelope.payload.event) {
            self.terminals
                .retain(|candidate| candidate.stream_id != envelope.stream_id);
            self.terminals.push_back(envelope);
            while self.terminals.len() > self.capacity {
                self.terminals.pop_front();
            }
        }
    }
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
        let capacity = capacity.max(1);
        let (raw_tx, _) = broadcast::channel(capacity);
        let (envelope_tx, _) = broadcast::channel(capacity);
        Self {
            raw_tx,
            envelope_tx,
            sync_listeners: Arc::new(RwLock::new(Vec::new())),
            replay: Arc::new(Mutex::new(SubagentReplayState::new(capacity))),
            active_publishers: Arc::new(Mutex::new(HashMap::new())),
            active_streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a sync listener (called immediately on emit).
    pub fn register(&mut self, listener: Box<dyn SubagentEventListener>) {
        self.sync_listeners
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::from(listener));
    }

    /// Subscribe to the async event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<SubagentEvent>> {
        self.raw_tx.subscribe()
    }

    /// Subscribe to authoritative execution envelopes.
    pub fn subscribe_envelopes(&self) -> broadcast::Receiver<Arc<SubagentEventEnvelope>> {
        self.envelope_tx.subscribe()
    }

    /// Create one ordering authority for an exact dispatch attempt.
    pub fn publisher(
        &self,
        transport_identity: EventIdentity,
        invocation_identity: SubagentInvocationIdentity,
    ) -> crate::error::Result<SubagentEventPublisher> {
        if invocation_identity.parent_agent.trim().is_empty()
            || invocation_identity.agent_name.trim().is_empty()
        {
            return Err(crate::error::ReactError::Other(
                "Subagent publisher agent identity must not be empty".to_string(),
            ));
        }
        if invocation_identity.attempt == Some(0) {
            return Err(crate::error::ReactError::Other(
                "Subagent publisher attempt must be one-based".to_string(),
            ));
        }
        let execution_id = transport_identity
            .execution_id
            .as_ref()
            .map(|value| value.as_str().to_string());
        let stream_id = transport_identity.stream_id.clone();
        let retained_stream = self.replay_after(&stream_id, 0);
        if !retained_stream.events.is_empty() || retained_stream.terminal.is_some() {
            return Err(crate::error::ReactError::Other(format!(
                "Subagent event stream '{stream_id}' is still retained"
            )));
        }
        if let Some(execution_id) = execution_id.as_deref()
            && self.replay_for_execution(execution_id, 0).is_some()
        {
            return Err(crate::error::ReactError::Other(format!(
                "Subagent execution '{execution_id}' is still retained"
            )));
        }
        let publisher =
            SubagentEventPublisher::new(self.clone(), transport_identity, invocation_identity)?;
        let mut streams = self
            .active_streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        streams.retain(|_, candidate| candidate.strong_count() > 0);
        if streams.get(&stream_id).and_then(Weak::upgrade).is_some() {
            return Err(crate::error::ReactError::Other(format!(
                "Subagent event publisher already exists for stream '{stream_id}'"
            )));
        }
        let mut publishers = self
            .active_publishers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        publishers.retain(|_, candidate| candidate.strong_count() > 0);
        if let Some(execution_id) = execution_id.as_deref()
            && publishers
                .get(execution_id)
                .and_then(Weak::upgrade)
                .is_some()
        {
            return Err(crate::error::ReactError::Other(format!(
                "Subagent event publisher already exists for execution '{execution_id}'"
            )));
        }
        streams.insert(stream_id, Arc::downgrade(&publisher.inner));
        if let Some(execution_id) = execution_id {
            publishers.insert(execution_id, Arc::downgrade(&publisher.inner));
        }
        Ok(publisher)
    }

    /// Emit a non-execution or compatibility-only raw event.
    ///
    /// Dispatch lifecycle producers must use [`Self::publisher`] so envelope
    /// identity and sequencing cannot be bypassed accidentally.
    pub fn emit(&self, event: SubagentEvent) {
        if let Err(error) = self.try_emit(event) {
            tracing::warn!(%error, "rejected raw Subagent execution event");
        }
    }

    /// Emit a registry-only raw event, rejecting execution events that must go
    /// through an attempt publisher.
    pub fn try_emit(&self, event: SubagentEvent) -> crate::error::Result<()> {
        if is_execution_event(&event) {
            return Err(crate::error::ReactError::Other(
                "Subagent execution events require an attempt publisher".to_string(),
            ));
        }
        self.emit_raw(Arc::new(event));
        Ok(())
    }

    fn emit_raw(&self, event: Arc<SubagentEvent>) {
        let listeners = self
            .sync_listeners
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        for listener in &listeners {
            listener.on_event(event.as_ref());
        }
        let _ = self.raw_tx.send(event);
    }

    fn emit_envelope(&self, envelope: Arc<SubagentEventEnvelope>) {
        self.replay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(Arc::clone(&envelope));
        self.emit_raw(Arc::new(envelope.payload.event.clone()));
        let _ = self.envelope_tx.send(envelope);
    }

    pub(crate) fn publisher_for_execution(
        &self,
        execution_id: &str,
    ) -> Option<SubagentEventPublisher> {
        let mut publishers = self
            .active_publishers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let publisher = publishers
            .get(execution_id)
            .and_then(Weak::upgrade)
            .map(|inner| SubagentEventPublisher { inner });
        if publisher.is_none() {
            publishers.remove(execution_id);
        }
        publisher
    }

    fn retire_publisher(&self, publisher: &SubagentEventPublisher) {
        let stream_id = &publisher.inner.transport_identity.stream_id;
        let mut streams = self
            .active_streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let remove_stream = streams
            .get(stream_id)
            .and_then(Weak::upgrade)
            .is_none_or(|inner| Arc::ptr_eq(&inner, &publisher.inner));
        if remove_stream {
            streams.remove(stream_id);
        }
        let Some(execution_id) = publisher.inner.transport_identity.execution_id.as_ref() else {
            return;
        };
        let mut publishers = self
            .active_publishers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let should_remove = publishers
            .get(execution_id.as_str())
            .and_then(Weak::upgrade)
            .is_none_or(|inner| Arc::ptr_eq(&inner, &publisher.inner));
        if should_remove {
            publishers.remove(execution_id.as_str());
        }
    }

    pub(crate) fn parent_event_id_for_tool(
        &self,
        execution_id: &str,
        call_id: &str,
    ) -> Option<EventId> {
        self.publisher_for_execution(execution_id)
            .and_then(|publisher| {
                publisher
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .tool_started
                    .get(call_id)
                    .cloned()
            })
    }

    /// Replay the retained suffix for one exact stream.
    pub fn replay_after(&self, stream_id: &StreamId, after_sequence: u64) -> SubagentEventReplay {
        let (mut events, terminal) = {
            let replay = self
                .replay
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let events = replay
                .history
                .iter()
                .chain(replay.boundaries.iter())
                .chain(replay.terminals.iter())
                .filter(|event| event.stream_id == *stream_id && event.sequence > after_sequence)
                .cloned()
                .collect::<Vec<_>>();
            let terminal = replay
                .terminals
                .iter()
                .rev()
                .find(|event| event.stream_id == *stream_id)
                .cloned();
            (events, terminal)
        };
        events.sort_by_key(|event| event.sequence);
        events.dedup_by_key(|event| event.sequence);
        let retained_latest = events
            .iter()
            .map(|event| event.sequence)
            .max()
            .unwrap_or(after_sequence);
        let active_latest = self.active_latest_for_stream(stream_id);
        let latest_sequence = active_latest
            .map(|sequence| sequence.max(retained_latest))
            .unwrap_or(retained_latest);
        let mut contiguous_through = after_sequence;
        let mut gap = None;
        for event in &events {
            let Some(expected) = contiguous_through.checked_add(1) else {
                break;
            };
            if event.sequence > expected {
                gap = Some(SubagentEventGap {
                    stream_id: stream_id.clone(),
                    requested_after: contiguous_through,
                    available_from: Some(event.sequence),
                    latest_sequence,
                });
                break;
            }
            if event.sequence == expected {
                contiguous_through = event.sequence;
            }
        }
        if gap.is_none() && contiguous_through < latest_sequence {
            gap = Some(SubagentEventGap {
                stream_id: stream_id.clone(),
                requested_after: contiguous_through,
                available_from: None,
                latest_sequence,
            });
        }
        SubagentEventReplay {
            events,
            gap,
            terminal,
        }
    }

    /// Resolve and replay one retained attempt without first knowing its
    /// random transport stream id.
    pub fn replay_for_execution(
        &self,
        execution_id: &str,
        after_sequence: u64,
    ) -> Option<SubagentEventReplay> {
        let retained_stream_id = {
            let replay = self
                .replay
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            replay
                .history
                .iter()
                .chain(replay.boundaries.iter())
                .chain(replay.terminals.iter())
                .filter(|event| {
                    event
                        .execution_id
                        .as_ref()
                        .is_some_and(|candidate| candidate.as_str() == execution_id)
                })
                .max_by_key(|event| event.timestamp)
                .map(|event| event.stream_id.clone())
        };
        let stream_id = retained_stream_id.or_else(|| {
            self.publisher_for_execution(execution_id)
                .map(|publisher| publisher.stream_id().clone())
        })?;
        Some(self.replay_after(&stream_id, after_sequence))
    }

    fn active_latest_for_stream(&self, stream_id: &StreamId) -> Option<u64> {
        let mut streams = self
            .active_streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let publisher = streams.get(stream_id).and_then(Weak::upgrade);
        if publisher.is_none() {
            streams.remove(stream_id);
        }
        publisher.map(|publisher| {
            publisher
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .sequence
        })
    }

    /// Get the current number of active subscribers to the async event stream.
    pub fn subscriber_count(&self) -> usize {
        self.raw_tx.receiver_count()
    }

    pub fn envelope_subscriber_count(&self) -> usize {
        self.envelope_tx.receiver_count()
    }
}

impl Clone for SubagentEventBus {
    fn clone(&self) -> Self {
        Self {
            raw_tx: self.raw_tx.clone(),
            envelope_tx: self.envelope_tx.clone(),
            sync_listeners: Arc::clone(&self.sync_listeners),
            replay: Arc::clone(&self.replay),
            active_publishers: Arc::clone(&self.active_publishers),
            active_streams: Arc::clone(&self.active_streams),
        }
    }
}

fn is_terminal_event(event: &SubagentEvent) -> bool {
    matches!(
        event,
        SubagentEvent::DispatchCompleted { .. }
            | SubagentEvent::DispatchFailed { .. }
            | SubagentEvent::DispatchCancelled { .. }
    )
}

fn is_execution_event(event: &SubagentEvent) -> bool {
    !matches!(
        event,
        SubagentEvent::Registered { .. } | SubagentEvent::Unregistered { .. }
    )
}

fn is_replay_boundary(event: &SubagentEvent) -> bool {
    !matches!(
        event,
        SubagentEvent::DispatchThinkingDelta { .. } | SubagentEvent::DispatchTokenDelta { .. }
    )
}

impl Default for SubagentEventBus {
    fn default() -> Self {
        Self::new()
    }
}

struct SubagentEventRoute<'a> {
    parent: &'a str,
    agent: &'a str,
    execution_id: Option<&'a str>,
    run_id: Option<&'a str>,
}

fn event_route(event: &SubagentEvent) -> Option<SubagentEventRoute<'_>> {
    let (parent, agent, execution_id, run_id) = match event {
        SubagentEvent::UplinkReceived {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchStarted {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchIsolationObserved {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchCompleted {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchFailed {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchCancelled {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchThinkingStarted {
            parent,
            agent,
            execution_id,
            run_id,
        }
        | SubagentEvent::DispatchThinkingDelta {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchThinkingEnded {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchTokenDelta {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchLlmUsage {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchToolStarted {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        }
        | SubagentEvent::DispatchToolCompleted {
            parent,
            agent,
            execution_id,
            run_id,
            ..
        } => (parent, agent, execution_id, run_id),
        SubagentEvent::Registered { .. } | SubagentEvent::Unregistered { .. } => return None,
    };
    Some(SubagentEventRoute {
        parent,
        agent,
        execution_id: execution_id.as_deref(),
        run_id: run_id.as_deref(),
    })
}

fn validate_event_route(
    route: SubagentEventRoute<'_>,
    transport: &EventIdentity,
    invocation: &SubagentInvocationIdentity,
) -> crate::error::Result<()> {
    let expected_execution_id = transport.execution_id.as_ref().map(|value| value.as_str());
    let expected_run_id = transport.run_id.as_ref().map(|value| value.as_str());
    if route.parent != invocation.parent_agent {
        return Err(crate::error::ReactError::Other(format!(
            "Subagent event parent changed: expected '{}', got '{}'",
            invocation.parent_agent, route.parent
        )));
    }
    if route.agent != invocation.agent_name {
        return Err(crate::error::ReactError::Other(format!(
            "Subagent event agent changed: expected '{}', got '{}'",
            invocation.agent_name, route.agent
        )));
    }
    if route.execution_id != expected_execution_id {
        return Err(crate::error::ReactError::Other(
            "Subagent event execution identity changed".to_string(),
        ));
    }
    if route.run_id != expected_run_id {
        return Err(crate::error::ReactError::Other(
            "Subagent event run identity changed".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::ToolInvocationRewrite;
    use echo_core::tools::{ToolFailure, ToolFailureCategory, ToolResultKind};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};

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
            artifact: Some(echo_core::tools::artifact::ToolOutputArtifactRef {
                path: std::path::PathBuf::from("/tmp/tool.log"),
                artifact_bytes: 12,
                payload_bytes: 12,
                sha256: "a".repeat(64),
                retention: "test".to_string(),
            }),
            metadata: HashMap::new(),
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
                .artifact
                .as_ref()
                .map(|artifact| artifact.path.as_path()),
            Some(std::path::Path::new("/tmp/tool.log"))
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

    fn publisher(bus: &SubagentEventBus) -> crate::error::Result<SubagentEventPublisher> {
        publisher_for(bus, "subagent-stream-1", "execution-1")
    }

    fn publisher_for(
        bus: &SubagentEventBus,
        stream_id: &str,
        execution_id: &str,
    ) -> crate::error::Result<SubagentEventPublisher> {
        let transport = EventIdentity::new(stream_id, format!("turn-{execution_id}"))?
            .with_conversation_id("conversation-1")?
            .with_run_id("run-1")?
            .with_message_id("message-1")?
            .with_execution_id(execution_id)?;
        bus.publisher(
            transport,
            SubagentInvocationIdentity {
                parent_agent: "root".to_string(),
                agent_name: "explorer".to_string(),
                parent_execution_id: None,
                agent_path: Some("root/explorer".to_string()),
                task_id: Some("task-1".to_string()),
                attempt: Some(1),
                plan_revision: Some(2),
            },
        )
    }

    fn started_event() -> SubagentEvent {
        started_event_for("execution-1")
    }

    fn started_event_for(execution_id: &str) -> SubagentEvent {
        SubagentEvent::DispatchStarted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            mode: ExecutionMode::Sync,
            task: "inspect".to_string(),
            execution_id: Some(execution_id.to_string()),
            run_id: Some("run-1".to_string()),
            conversation_id: Some("conversation-1".to_string()),
            message_id: Some("message-1".to_string()),
            background: false,
        }
    }

    fn completed_event_for(execution_id: &str) -> SubagentEvent {
        SubagentEvent::DispatchCompleted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            duration_ms: 1,
            tokens_used: None,
            iterations: Some(1),
            output: "done".to_string(),
            outcome: SubagentOutcome::terminal(SubagentStatus::Completed, "done", Vec::new()),
            execution_id: Some(execution_id.to_string()),
            run_id: Some("run-1".to_string()),
        }
    }

    #[tokio::test]
    async fn publisher_sequences_envelopes_and_preserves_raw_compatibility()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(16);
        let mut raw = bus.subscribe();
        let mut envelopes = bus.subscribe_envelopes();
        let publisher = publisher(&bus)?;

        let started = publisher.emit(started_event())?;
        publisher.emit(SubagentEvent::DispatchThinkingStarted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            execution_id: Some("execution-1".to_string()),
            run_id: Some("run-1".to_string()),
        })?;
        let tool_started = publisher.emit(SubagentEvent::DispatchToolStarted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            call_id: "call-1".to_string(),
            invocation: ToolInvocation {
                requested_name: "shell".to_string(),
                requested_args: serde_json::json!({}),
                name: "shell".to_string(),
                args: serde_json::json!({}),
                rewrites: Vec::new(),
            },
            execution_id: Some("execution-1".to_string()),
            run_id: Some("run-1".to_string()),
        })?;
        assert_eq!(
            bus.parent_event_id_for_tool("execution-1", "call-1"),
            Some(tool_started.event_id.clone())
        );
        let tool_completed = publisher.emit(SubagentEvent::DispatchToolCompleted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            result: ToolResult::success("done"),
            execution_id: Some("execution-1".to_string()),
            run_id: Some("run-1".to_string()),
        })?;
        assert!(
            bus.parent_event_id_for_tool("execution-1", "call-1")
                .is_none()
        );
        publisher.emit(SubagentEvent::DispatchCompleted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            duration_ms: 1,
            tokens_used: Some(2),
            iterations: Some(1),
            output: "done".to_string(),
            outcome: SubagentOutcome::terminal(SubagentStatus::Completed, "done", Vec::new()),
            execution_id: Some("execution-1".to_string()),
            run_id: Some("run-1".to_string()),
        })?;

        let mut captured = Vec::new();
        for _ in 0..5 {
            captured.push(envelopes.recv().await?);
        }
        assert_eq!(captured.first().map(|event| event.sequence), Some(1));
        assert_eq!(captured.last().map(|event| event.sequence), Some(5));
        assert_eq!(
            tool_completed.parent_event_id,
            Some(tool_started.event_id.clone())
        );
        assert_eq!(
            captured
                .get(1)
                .and_then(|event| event.parent_event_id.as_ref()),
            Some(&started.event_id)
        );
        assert!(matches!(
            raw.recv().await?.as_ref(),
            SubagentEvent::DispatchStarted { .. }
        ));
        Ok(())
    }

    #[test]
    fn publisher_rejects_raw_bypass_lifecycle_drift_and_orphan_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(16);
        let mut raw = bus.subscribe();
        assert!(bus.try_emit(started_event()).is_err());
        assert!(matches!(
            raw.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        let publisher = publisher(&bus)?;
        let mut wrong_conversation = started_event();
        if let SubagentEvent::DispatchStarted {
            conversation_id, ..
        } = &mut wrong_conversation
        {
            *conversation_id = Some("conversation-other".to_string());
        }
        assert!(publisher.emit(wrong_conversation).is_err());
        let mut wrong_message = started_event();
        if let SubagentEvent::DispatchStarted { message_id, .. } = &mut wrong_message {
            *message_id = Some("message-other".to_string());
        }
        assert!(publisher.emit(wrong_message).is_err());
        assert!(
            publisher
                .emit(SubagentEvent::DispatchTokenDelta {
                    parent: "root".to_string(),
                    agent: "explorer".to_string(),
                    content: "before-start".to_string(),
                    execution_id: Some("execution-1".to_string()),
                    run_id: Some("run-1".to_string()),
                })
                .is_err()
        );
        assert_eq!(publisher.emit(started_event())?.sequence, 1);
        assert!(publisher.emit(started_event()).is_err());
        assert!(
            publisher
                .emit(SubagentEvent::DispatchToolCompleted {
                    parent: "root".to_string(),
                    agent: "explorer".to_string(),
                    call_id: "orphan".to_string(),
                    name: "shell".to_string(),
                    result: ToolResult::success("done"),
                    execution_id: Some("execution-1".to_string()),
                    run_id: Some("run-1".to_string()),
                })
                .is_err()
        );
        let tool_started = SubagentEvent::DispatchToolStarted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            call_id: "call-1".to_string(),
            invocation: ToolInvocation {
                requested_name: "shell".to_string(),
                requested_args: serde_json::json!({}),
                name: "shell".to_string(),
                args: serde_json::json!({}),
                rewrites: Vec::new(),
            },
            execution_id: Some("execution-1".to_string()),
            run_id: Some("run-1".to_string()),
        };
        assert_eq!(publisher.emit(tool_started.clone())?.sequence, 2);
        assert!(publisher.emit(tool_started).is_err());
        for (parent, agent) in [("other-parent", "explorer"), ("root", "other-agent")] {
            assert!(
                publisher
                    .emit(SubagentEvent::DispatchTokenDelta {
                        parent: parent.to_string(),
                        agent: agent.to_string(),
                        content: "wrong-route".to_string(),
                        execution_id: Some("execution-1".to_string()),
                        run_id: Some("run-1".to_string()),
                    })
                    .is_err()
            );
        }
        assert!(
            publisher
                .emit(SubagentEvent::DispatchTokenDelta {
                    parent: "root".to_string(),
                    agent: "explorer".to_string(),
                    content: "wrong-run".to_string(),
                    execution_id: Some("execution-1".to_string()),
                    run_id: Some("run-other".to_string()),
                })
                .is_err()
        );
        assert_eq!(
            publisher
                .emit(SubagentEvent::DispatchToolCompleted {
                    parent: "root".to_string(),
                    agent: "explorer".to_string(),
                    call_id: "call-1".to_string(),
                    name: "shell".to_string(),
                    result: ToolResult::success("done"),
                    execution_id: Some("execution-1".to_string()),
                    run_id: Some("run-1".to_string()),
                })?
                .sequence,
            3
        );
        Ok(())
    }

    #[test]
    fn invocation_identity_rejects_conflicting_caller_lineage()
    -> Result<(), Box<dyn std::error::Error>> {
        let transport = EventIdentity::new("lineage-stream", "lineage-turn")?
            .with_run_id("run-1")?
            .with_execution_id("execution-1")?;
        for lineage in [
            SubagentLineage {
                agent_name: Some("other".to_string()),
                ..SubagentLineage::default()
            },
            SubagentLineage {
                parent_agent: Some("other-parent".to_string()),
                ..SubagentLineage::default()
            },
            SubagentLineage {
                execution_id: Some("execution-other".to_string()),
                ..SubagentLineage::default()
            },
            SubagentLineage {
                run_id: Some("run-other".to_string()),
                ..SubagentLineage::default()
            },
        ] {
            assert!(
                SubagentInvocationIdentity::from_lineage(
                    "root",
                    "explorer",
                    &transport,
                    Some(&lineage),
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn bounded_replay_reports_gap_and_retains_terminal()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(2);
        let mut receiver = bus.subscribe_envelopes();
        let publisher = publisher(&bus)?;
        publisher.emit(started_event())?;
        for content in ["a", "b", "c"] {
            publisher.emit(SubagentEvent::DispatchTokenDelta {
                parent: "root".to_string(),
                agent: "explorer".to_string(),
                content: content.to_string(),
                execution_id: Some("execution-1".to_string()),
                run_id: Some("run-1".to_string()),
            })?;
        }
        let terminal = publisher.emit(SubagentEvent::DispatchCompleted {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            duration_ms: 1,
            tokens_used: None,
            iterations: Some(1),
            output: "abc".to_string(),
            outcome: SubagentOutcome::terminal(SubagentStatus::Completed, "abc", Vec::new()),
            execution_id: Some("execution-1".to_string()),
            run_id: Some("run-1".to_string()),
        })?;

        let replay = bus.replay_after(publisher.stream_id(), 0);
        assert_eq!(replay.events.len(), 3);
        assert_eq!(replay.gap.as_ref().map(|gap| gap.requested_after), Some(1));
        assert_eq!(
            replay.gap.as_ref().and_then(|gap| gap.available_from),
            Some(4)
        );
        assert_eq!(replay.gap.as_ref().map(|gap| gap.latest_sequence), Some(5));
        assert_eq!(
            replay
                .terminal
                .as_ref()
                .map(|event| event.event_id.as_str()),
            Some(terminal.event_id.as_str())
        );
        assert!(matches!(
            receiver.recv().await,
            Err(broadcast::error::RecvError::Lagged(3))
        ));
        assert!(
            publisher
                .emit(SubagentEvent::DispatchTokenDelta {
                    parent: "root".to_string(),
                    agent: "explorer".to_string(),
                    content: "late".to_string(),
                    execution_id: Some("execution-1".to_string()),
                    run_id: Some("run-1".to_string()),
                })
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn replay_finds_terminal_by_execution_when_dispatch_start_was_evicted()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(2);
        let first = publisher_for(&bus, "stream-first", "execution-first")?;
        first.emit(started_event_for("execution-first"))?;
        first.emit(completed_event_for("execution-first"))?;
        let second = publisher_for(&bus, "stream-second", "execution-second")?;
        second.emit(started_event_for("execution-second"))?;
        second.emit(completed_event_for("execution-second"))?;

        let replay = bus
            .replay_for_execution("execution-first", 0)
            .ok_or_else(|| std::io::Error::other("retained terminal was not discoverable"))?;
        assert_eq!(replay.events.len(), 1);
        assert_eq!(replay.gap.as_ref().map(|gap| gap.requested_after), Some(0));
        assert_eq!(
            replay.gap.as_ref().and_then(|gap| gap.available_from),
            Some(2)
        );
        assert!(replay.terminal.is_some());
        assert!(bus.publisher_for_execution("execution-first").is_none());
        assert!(
            publisher_for(&bus, "stream-reused", "execution-first")
                .err()
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn active_stream_watermark_reports_fully_evicted_suffix()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(1);
        let first = publisher_for(&bus, "stream-active-first", "execution-active-first")?;
        first.emit(started_event_for("execution-active-first"))?;
        let second = publisher_for(&bus, "stream-active-second", "execution-active-second")?;
        second.emit(started_event_for("execution-active-second"))?;

        let replay = bus
            .replay_for_execution("execution-active-first", 0)
            .ok_or_else(|| std::io::Error::other("active stream was not discoverable"))?;
        assert!(replay.events.is_empty());
        assert_eq!(replay.gap.as_ref().map(|gap| gap.latest_sequence), Some(1));
        assert_eq!(replay.gap.as_ref().and_then(|gap| gap.available_from), None);
        Ok(())
    }

    #[test]
    fn active_stream_watermark_reports_missing_tail_after_retained_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(2);
        let first = publisher_for(&bus, "stream-tail-first", "execution-tail-first")?;
        first.emit(started_event_for("execution-tail-first"))?;
        first.emit(SubagentEvent::DispatchTokenDelta {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            content: "lost-tail".to_string(),
            execution_id: Some("execution-tail-first".to_string()),
            run_id: Some("run-1".to_string()),
        })?;
        let second = publisher_for(&bus, "stream-tail-second", "execution-tail-second")?;
        second.emit(started_event_for("execution-tail-second"))?;
        second.emit(SubagentEvent::DispatchTokenDelta {
            parent: "root".to_string(),
            agent: "explorer".to_string(),
            content: "other-stream".to_string(),
            execution_id: Some("execution-tail-second".to_string()),
            run_id: Some("run-1".to_string()),
        })?;

        let replay = bus.replay_after(first.stream_id(), 0);
        assert_eq!(
            replay
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(replay.gap.as_ref().map(|gap| gap.requested_after), Some(1));
        assert_eq!(replay.gap.as_ref().map(|gap| gap.latest_sequence), Some(2));
        assert_eq!(replay.gap.as_ref().and_then(|gap| gap.available_from), None);
        Ok(())
    }

    #[test]
    fn legacy_stream_without_execution_id_keeps_active_gap_watermark()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::with_capacity(1);
        let legacy = bus.publisher(
            EventIdentity::new("stream-legacy", "turn-legacy")?,
            SubagentInvocationIdentity {
                parent_agent: "root".to_string(),
                agent_name: "legacy".to_string(),
                parent_execution_id: None,
                agent_path: Some("root/legacy".to_string()),
                task_id: None,
                attempt: None,
                plan_revision: None,
            },
        )?;
        legacy.emit(SubagentEvent::DispatchStarted {
            parent: "root".to_string(),
            agent: "legacy".to_string(),
            mode: ExecutionMode::Sync,
            task: "legacy".to_string(),
            execution_id: None,
            run_id: None,
            conversation_id: None,
            message_id: None,
            background: false,
        })?;
        let other = publisher_for(&bus, "stream-with-execution", "execution-other")?;
        other.emit(started_event_for("execution-other"))?;

        let replay = bus.replay_after(legacy.stream_id(), 0);
        assert!(replay.events.is_empty());
        assert_eq!(replay.gap.as_ref().map(|gap| gap.latest_sequence), Some(1));
        assert_eq!(replay.gap.as_ref().and_then(|gap| gap.available_from), None);
        Ok(())
    }

    #[test]
    fn publisher_index_rejects_duplicates_and_cleans_dropped_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        let bus = SubagentEventBus::new();
        let first = publisher_for(&bus, "stream-duplicate", "execution-duplicate")?;
        assert!(
            publisher_for(&bus, "stream-conflict", "execution-duplicate")
                .err()
                .is_some()
        );
        drop(first);

        for index in 0..32 {
            let publisher = publisher_for(
                &bus,
                &format!("stream-drop-{index}"),
                &format!("execution-drop-{index}"),
            )?;
            drop(publisher);
        }
        let final_publisher = publisher_for(&bus, "stream-final", "execution-final")?;
        assert_eq!(
            bus.active_publishers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );
        drop(final_publisher);
        assert!(bus.publisher_for_execution("execution-final").is_none());
        let _ = bus.replay_after(&StreamId::new("stream-final")?, 0);
        assert!(
            bus.active_publishers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert!(
            bus.active_streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        Ok(())
    }

    struct NoopListener;

    impl SubagentEventListener for NoopListener {
        fn on_event(&self, _event: &SubagentEvent) {}
    }

    struct ReentrantListener {
        bus: SubagentEventBus,
        called: Arc<AtomicBool>,
    }

    impl SubagentEventListener for ReentrantListener {
        fn on_event(&self, _event: &SubagentEvent) {
            let mut bus = self.bus.clone();
            bus.register(Box::new(NoopListener));
            self.called.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn sync_listener_may_register_another_listener_without_deadlock() {
        let mut bus = SubagentEventBus::new();
        let called = Arc::new(AtomicBool::new(false));
        bus.register(Box::new(ReentrantListener {
            bus: bus.clone(),
            called: Arc::clone(&called),
        }));
        bus.emit(SubagentEvent::Registered {
            name: "test".to_string(),
        });
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_logging_listener() {
        let listener = LoggingSubagentListener;
        listener.on_event(&SubagentEvent::Registered {
            name: "test".into(),
        });
    }
}
