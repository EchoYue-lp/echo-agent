//! Generic turn driver for one finite Agent invocation.
//!
//! # Purpose
//!
//! This is the framework-side extraction of the driver pattern that EKO
//! currently implements per-surface (`drive_chat` for interactive turns and
//! `drive_run_async` for headless task runs). It drives exactly one finite
//! Agent invocation: wrap the raw event stream with the versioned
//! [`EventEnvelope`] transport, forward every envelope to one [`EventSink`],
//! enforce the exactly-one-terminal contract, and return a typed
//! [`TurnOutcome`] receipt. Chat, execute/headless, and task-driven callers
//! differ only in the request fields and the sink; the loop, terminal
//! mapping, and accounting are shared.
//!
//! # Industry basis
//!
//! - OpenAI Codex `Thread -> Turn -> Item`: a turn is a finite unit with one
//!   typed terminal; progress is delivered as stable-identity events and the
//!   final `turn/completed` carries the converged state.
//! - Claude Code subagents: the caller receives a structured result rather
//!   than inferring success from "the stream returned".
//!
//! # Layering decision
//!
//! Driving an Agent stream, envelope sequencing, terminal mapping, and usage
//! accounting are framework concerns that any consumer needs; product policy
//! (chat journals, TaskRuntime runs, foreground admission, continuation)
//! stays in the application and composes this driver. The application's own
//! `TurnOutcome`/driver loop migrates onto this primitive in planned
//! follow-up work, which deletes the duplicated loop at that point.
//!
//! # Example
//!
//! ```
//! use echo_core::agent::{Agent, EventIdentity};
//! use echo_orchestration::runtime::turn_driver::{
//!     AgentTurnDriver, EventSink, SinkControl, TurnMode, TurnRequest,
//! };
//!
//! # struct MyAgent;
//! # impl Agent for MyAgent {
//! #     fn name(&self) -> &str { "my-agent" }
//! #     fn model_name(&self) -> &str { "test-model" }
//! #     fn system_prompt(&self) -> &str { "" }
//! #     fn execute<'a>(&'a self, _task: &'a str)
//! #         -> futures::future::BoxFuture<'a, echo_core::error::Result<String>> {
//! #         Box::pin(async { Ok("done".to_string()) })
//! #     }
//! #     fn execute_stream<'a>(&'a self, _task: &'a str) -> futures::future::BoxFuture<
//! #         'a,
//! #         echo_core::error::Result<
//! #             futures::stream::BoxStream<'a, echo_core::error::Result<echo_core::agent::AgentEvent>>,
//! #         >,
//! #     > {
//! #         Box::pin(async {
//! #             use futures::StreamExt;
//! #             Ok(futures::stream::iter([Ok(echo_core::agent::AgentEvent::FinalAnswer(
//! #                 "done".to_string(),
//! #             ))])
//! #             .boxed())
//! #         })
//! #     }
//! # }
//! # struct PrintSink;
//! # #[async_trait::async_trait]
//! # impl EventSink for PrintSink {
//! #     async fn on_event(&self, _envelope: echo_core::agent::EventEnvelope)
//! #         -> echo_core::error::Result<SinkControl> {
//! #         Ok(SinkControl::Continue)
//! #     }
//! # }
//! # async fn run() -> echo_core::error::Result<()> {
//! let agent = MyAgent;
//! let identity = EventIdentity::new("stream-1", "turn-1")?;
//! let request = TurnRequest::new(identity, "hello").mode(TurnMode::Execute);
//! let receipt = AgentTurnDriver.drive(&agent, request, &PrintSink).await;
//! assert_eq!(receipt.outcome.status(), "completed");
//! assert_eq!(receipt.final_answer.as_deref(), Some("done"));
//! # Ok(())
//! # }
//! ```

use async_trait::async_trait;
use echo_core::agent::{
    Agent, AgentEvent, AgentInvocationContext, AgentSteerTurnOutcome, CancellationToken,
    EventEnvelope, EventIdentity, ExecutionUsage, MessageId, TurnId, envelope_event_stream_after,
};
use echo_core::error::{AgentFailure, ReactError};
use echo_core::llm::Message;
use futures::StreamExt;
use std::time::{Duration, Instant};

/// Which Agent stream flavor the turn drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnMode {
    /// Multi-turn conversation stream (`chat_stream_with_cancel`).
    #[default]
    Chat,
    /// Fresh-context execution stream (`execute_stream_with_cancel`).
    Execute,
}

/// One finite Agent invocation to drive.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// Text or structured multimodal input.
    pub input: TurnInput,
    /// Transport identity stamped onto every emitted envelope.
    pub identity: EventIdentity,
    /// Chat vs execute stream flavor.
    pub mode: TurnMode,
    /// Cooperative cancellation for the invocation. `None` starts an
    /// independent scope that the sink can still close by returning `false`.
    pub cancel: Option<CancellationToken>,
    /// Last durably persisted envelope sequence; the wrapped stream resumes
    /// sequencing at `last_persisted_sequence + 1`.
    pub last_persisted_sequence: u64,
    /// Value-scoped run, tool visibility, working-directory, and history
    /// metadata. Structured message callers use the invocation-aware Agent API.
    pub invocation: Option<AgentInvocationContext>,
    input_publisher: Option<TurnInputLifecycle>,
}

/// Lifecycle state for one initial input accepted by [`AgentTurnDriver`].
///
/// `Accepted` is published by the driver after request/identity validation and
/// immediately before it calls the Agent stream API. A concrete Agent publishes
/// `Drained` after its input has entered model context. It is never inferred
/// from an output envelope, EOF, or a terminal event. The terminal state
/// records the owning turn outcome and whether the input reached the drain
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnInputState {
    Pending,
    Accepted,
    Drained,
    TurnSettled {
        outcome: AgentSteerTurnOutcome,
        drained: bool,
    },
}

struct TurnInputLifecycleInner {
    state: tokio::sync::watch::Sender<TurnInputState>,
    settled: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for TurnInputLifecycleInner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnInputLifecycleInner")
            .field(
                "settled",
                &self.settled.load(std::sync::atomic::Ordering::Acquire),
            )
            .finish()
    }
}

#[derive(Clone)]
struct TurnInputLifecycle {
    inner: std::sync::Arc<TurnInputLifecycleInner>,
}

impl std::fmt::Debug for TurnInputLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnInputLifecycle")
            .field("inner", &self.inner)
            .finish()
    }
}

/// Lossless receipt for one initial turn input.
#[derive(Debug, Clone)]
pub struct TurnInputReceipt {
    turn_id: String,
    state: tokio::sync::watch::Receiver<TurnInputState>,
}

impl TurnInputReceipt {
    fn new(turn_id: String) -> (Self, TurnInputLifecycle) {
        let (state, receiver) = tokio::sync::watch::channel(TurnInputState::Pending);
        let inner = std::sync::Arc::new(TurnInputLifecycleInner {
            state,
            settled: std::sync::atomic::AtomicBool::new(false),
        });
        (
            Self {
                turn_id,
                state: receiver,
            },
            TurnInputLifecycle { inner },
        )
    }

    /// Stable identity of the turn that owns this input.
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    /// Snapshot the latest lifecycle state without waiting.
    pub fn state(&self) -> TurnInputState {
        if self.state.has_changed().is_err() {
            return self.synthesize_closed_terminal();
        }
        self.state.borrow().clone()
    }

    fn synthesize_closed_terminal(&self) -> TurnInputState {
        let current = self.state.borrow().clone();
        match current {
            state @ TurnInputState::TurnSettled { .. } => state,
            state => TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: matches!(state, TurnInputState::Drained),
            },
        }
    }

    /// Wait until the Agent accepts the input or settles the turn first.
    pub async fn wait_for_accepted(&mut self) -> TurnInputState {
        loop {
            let state = self.state();
            if !matches!(state, TurnInputState::Pending) {
                return state;
            }
            if self.state.changed().await.is_err() {
                return self.synthesize_closed_terminal();
            }
        }
    }

    /// Wait until the input reaches the driver's real drain boundary.
    pub async fn wait_for_drained(&mut self) -> TurnInputState {
        loop {
            let state = self.state();
            if !matches!(state, TurnInputState::Pending | TurnInputState::Accepted) {
                return state;
            }
            if self.state.changed().await.is_err() {
                return self.synthesize_closed_terminal();
            }
        }
    }

    /// Wait for the owning turn's typed terminal outcome.
    pub async fn wait_for_turn_settled(&mut self) -> TurnInputState {
        loop {
            let state = self.state();
            if matches!(state, TurnInputState::TurnSettled { .. }) {
                return state;
            }
            if self.state.changed().await.is_err() {
                return self.synthesize_closed_terminal();
            }
        }
    }
}

impl TurnInputLifecycle {
    fn mark_accepted(&self) {
        self.inner.state.send_if_modified(|state| {
            if matches!(state, TurnInputState::Pending) {
                *state = TurnInputState::Accepted;
                true
            } else {
                false
            }
        });
    }

    fn mark_drained(&self) {
        self.inner.state.send_if_modified(|state| {
            if matches!(state, TurnInputState::Pending | TurnInputState::Accepted) {
                *state = TurnInputState::Drained;
                true
            } else {
                false
            }
        });
    }

    fn settle(&self, outcome: AgentSteerTurnOutcome) {
        if self
            .inner
            .settled
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let drained = matches!(*self.inner.state.borrow(), TurnInputState::Drained);
        let terminal = TurnInputState::TurnSettled { outcome, drained };
        self.inner.state.send_replace(terminal);
    }
}

impl Drop for TurnInputLifecycle {
    fn drop(&mut self) {
        if std::sync::Arc::strong_count(&self.inner) == 1 {
            self.settle(AgentSteerTurnOutcome::Dropped);
        }
    }
}

impl echo_core::agent::AgentInputLifecycle for TurnInputLifecycle {
    fn mark_drained(&self) {
        TurnInputLifecycle::mark_drained(self);
    }
}

/// Input accepted by one driven turn.
#[derive(Debug, Clone)]
pub enum TurnInput {
    Text(String),
    Message(Message),
}

impl TurnRequest {
    pub fn new(identity: EventIdentity, message: impl Into<String>) -> Self {
        Self {
            input: TurnInput::Text(message.into()),
            identity,
            mode: TurnMode::Chat,
            cancel: None,
            last_persisted_sequence: 0,
            invocation: None,
            input_publisher: None,
        }
    }

    pub fn from_message(identity: EventIdentity, message: Message) -> Self {
        Self {
            input: TurnInput::Message(message),
            identity,
            mode: TurnMode::Chat,
            cancel: None,
            last_persisted_sequence: 0,
            invocation: None,
            input_publisher: None,
        }
    }

    pub fn mode(mut self, mode: TurnMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn last_persisted_sequence(mut self, sequence: u64) -> Self {
        self.last_persisted_sequence = sequence;
        self
    }

    pub fn invocation(mut self, invocation: AgentInvocationContext) -> Self {
        self.invocation = Some(invocation);
        self
    }

    /// Attach a lossless lifecycle receipt to this initial input.
    ///
    /// The returned receipt starts in `Pending`; the same driver publishes
    /// `Accepted`, `Drained`, and `TurnSettled` from the real execution path.
    pub fn with_input_receipt(mut self) -> (Self, TurnInputReceipt) {
        let (receipt, lifecycle) = TurnInputReceipt::new(self.identity.turn_id.to_string());
        self.input_publisher = Some(lifecycle);
        (self, receipt)
    }
}

/// Typed terminal for one driven turn.
///
/// The envelope transport guarantees exactly one terminal Agent event; this
/// value carries that fact back to the caller so success is never inferred
/// from "the stream returned".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed(AgentFailure),
}

impl TurnOutcome {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }

    /// Map an [`AgentEvent`] payload to its typed outcome, if it is terminal.
    ///
    /// Non-terminal events return `None`. An `Error` whose terminal kind is
    /// cancellation maps to [`Self::Cancelled`], mirroring the framework's
    /// typed failure contract. Custom sinks can use this to project terminal
    /// envelopes the same way the driver does.
    pub fn classify(event: &AgentEvent) -> Option<Self> {
        match event {
            AgentEvent::FinalAnswer(_) => Some(Self::Completed),
            AgentEvent::Cancelled => Some(Self::Cancelled),
            AgentEvent::Error { failure, .. } => {
                if failure.terminal_kind == echo_core::error::AgentTerminalKind::Cancelled {
                    Some(Self::Cancelled)
                } else {
                    Some(Self::Failed(failure.clone()))
                }
            }
            _ => None,
        }
    }
}

/// Bounded control-plane receipt for one driven turn.
#[derive(Debug, Clone)]
pub struct TurnReceipt {
    /// Turn identity from the request identity.
    pub turn_id: TurnId,
    /// Typed terminal outcome; always set by the driver.
    pub outcome: TurnOutcome,
    /// Final answer text when the turn completed with one.
    pub final_answer: Option<String>,
    /// Message identity carried by the accepted final-answer envelope.
    pub final_message_id: Option<MessageId>,
    /// Provider-reported prompt tokens accumulated over the turn.
    pub prompt_tokens: u64,
    /// Provider-reported completion tokens accumulated over the turn.
    pub completion_tokens: u64,
    /// Number of provider calls that reported usage.
    pub llm_calls: u64,
    /// Number of explicit context-compaction boundaries emitted by the Agent.
    pub compaction_count: u64,
    /// Last envelope sequence emitted for the turn.
    pub last_event_sequence: u64,
    /// Wall-clock turn duration.
    pub elapsed: Duration,
}

impl TurnReceipt {
    /// Return the stable usage facts carried by this completed turn.
    pub fn usage(&self) -> ExecutionUsage {
        ExecutionUsage {
            duration_ms: Some(u64::try_from(self.elapsed.as_millis()).unwrap_or(u64::MAX)),
            tokens_used: (self.llm_calls > 0)
                .then(|| self.prompt_tokens.saturating_add(self.completion_tokens)),
            iterations: None,
        }
    }

    /// Stable status string for projections (`completed`/`cancelled`/`failed`).
    pub fn status(&self) -> &'static str {
        self.outcome.status()
    }

    /// Construct a typed failure receipt when execution cannot start or a
    /// caller must report a failure before a stream exists.
    pub fn failed(
        turn_id: impl Into<String>,
        failure: AgentFailure,
    ) -> echo_core::error::Result<Self> {
        Ok(Self {
            turn_id: TurnId::new(turn_id)?,
            outcome: TurnOutcome::Failed(failure),
            final_answer: None,
            final_message_id: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            llm_calls: 0,
            compaction_count: 0,
            last_event_sequence: 0,
            elapsed: Duration::ZERO,
        })
    }

    /// Construct a typed cancellation receipt when execution is stopped
    /// before a stream can produce its normal terminal event.
    pub fn cancelled(turn_id: impl Into<String>) -> echo_core::error::Result<Self> {
        Ok(Self {
            turn_id: TurnId::new(turn_id)?,
            outcome: TurnOutcome::Cancelled,
            final_answer: None,
            final_message_id: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            llm_calls: 0,
            compaction_count: 0,
            last_event_sequence: 0,
            elapsed: Duration::ZERO,
        })
    }

    fn failure_receipt(
        turn_id: TurnId,
        failure: AgentFailure,
        last_event_sequence: u64,
        started: Instant,
    ) -> Self {
        Self {
            turn_id,
            outcome: TurnOutcome::Failed(failure),
            final_answer: None,
            final_message_id: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            llm_calls: 0,
            compaction_count: 0,
            last_event_sequence,
            elapsed: started.elapsed(),
        }
    }
}

/// Per-consumer event handler for one driven turn.
///
/// `on_event` receives every envelope in order, including the terminal one.
/// `Closed` is an intentional consumer disconnect; `Err` is a delivery or
/// persistence failure and produces a failed receipt rather than a cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkControl {
    Continue,
    Closed,
}

#[async_trait]
pub trait EventSink: Send + Sync {
    /// Take ownership of one envelope after the driver has completed its
    /// accounting, allowing product adapters to retain or forward it losslessly.
    async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl>;
}

fn invocation_with_cancel(
    invocation: Option<AgentInvocationContext>,
    token: &CancellationToken,
) -> AgentInvocationContext {
    let mut invocation = invocation.unwrap_or_default();
    invocation.cancel = Some(token.clone());
    invocation
}

/// Drives one finite Agent invocation through one sink.
///
/// See the module docs for the contract; the driver is stateless and can be
/// reused for any number of turns.
#[derive(Debug, Default)]
pub struct AgentTurnDriver;

impl AgentTurnDriver {
    /// Drive `request` on `agent`, forwarding envelopes to `sink`.
    pub async fn drive(
        &self,
        agent: &dyn Agent,
        mut request: TurnRequest,
        sink: &dyn EventSink,
    ) -> TurnReceipt {
        let started = Instant::now();
        let turn_id = request.identity.turn_id.clone();
        let input_lifecycle = request.input_publisher.take();
        if let Err(error) = request.identity.validate() {
            if let Some(lifecycle) = input_lifecycle.as_ref() {
                lifecycle.settle(AgentSteerTurnOutcome::Failed);
            }
            return TurnReceipt::failure_receipt(
                turn_id,
                AgentFailure::from(&error),
                request.last_persisted_sequence,
                started,
            );
        }
        if let Some(lifecycle) = input_lifecycle.as_ref() {
            lifecycle.mark_accepted();
        }
        let token = request
            .cancel
            .clone()
            .or_else(|| {
                request
                    .invocation
                    .as_ref()
                    .and_then(|invocation| invocation.cancel.clone())
            })
            .unwrap_or_default();
        let input_lifecycle_for_agent = input_lifecycle.as_ref().map(|lifecycle| {
            std::sync::Arc::new(lifecycle.clone())
                as std::sync::Arc<dyn echo_core::agent::AgentInputLifecycle>
        });
        let invocation = request
            .invocation
            .map(|mut invocation| {
                invocation.cancel = Some(token.clone());
                invocation.input_lifecycle = input_lifecycle_for_agent.clone();
                invocation
            })
            .or_else(|| {
                input_lifecycle_for_agent.map(|input_lifecycle| {
                    echo_core::agent::AgentInvocationContext {
                        cancel: Some(token.clone()),
                        input_lifecycle: Some(input_lifecycle),
                        ..echo_core::agent::AgentInvocationContext::default()
                    }
                })
            });

        let input = request.input;
        let raw = match (&input, request.mode, invocation) {
            (TurnInput::Text(message), TurnMode::Chat, None) => {
                agent.chat_stream_with_cancel(message, token.clone()).await
            }
            (TurnInput::Text(message), TurnMode::Execute, None) => {
                agent
                    .execute_stream_with_cancel(message, token.clone())
                    .await
            }
            (TurnInput::Text(message), TurnMode::Execute, Some(invocation)) => {
                agent
                    .execute_stream_with_invocation_context(message, token.clone(), invocation)
                    .await
            }
            (TurnInput::Text(message), TurnMode::Chat, Some(invocation)) => {
                agent
                    .chat_stream_message_with_invocation_context(
                        Message::user(message.clone()),
                        token.clone(),
                        invocation,
                    )
                    .await
            }
            (TurnInput::Message(message), TurnMode::Chat, invocation) => {
                agent
                    .chat_stream_message_with_invocation_context(
                        message.clone(),
                        token.clone(),
                        invocation_with_cancel(invocation, &token),
                    )
                    .await
            }
            (TurnInput::Message(message), TurnMode::Execute, invocation) => {
                agent
                    .execute_stream_message_with_invocation_context(
                        message.clone(),
                        token.clone(),
                        invocation_with_cancel(invocation, &token),
                    )
                    .await
            }
        };
        let raw = match raw {
            Ok(stream) => stream,
            Err(error) => {
                if let Some(lifecycle) = input_lifecycle.as_ref() {
                    lifecycle.settle(AgentSteerTurnOutcome::Failed);
                }
                let failure = AgentFailure::from(&error);
                let next_sequence = request.last_persisted_sequence.checked_add(1);
                let mut last_event_sequence = request.last_persisted_sequence;
                if let Some(sequence) = next_sequence
                    && let Ok(envelope) = EventEnvelope::new(
                        &request.identity,
                        sequence,
                        request.identity.parent_event_id.clone(),
                        AgentEvent::from_error("turn_driver", &error),
                    )
                {
                    last_event_sequence = envelope.sequence;
                    if let Err(sink_error) = sink.on_event(envelope).await {
                        return TurnReceipt::failure_receipt(
                            turn_id,
                            AgentFailure::from(&sink_error),
                            last_event_sequence,
                            started,
                        );
                    }
                }
                return TurnReceipt::failure_receipt(
                    turn_id,
                    failure,
                    last_event_sequence,
                    started,
                );
            }
        };
        let mut outcome: Option<TurnOutcome> = None;
        let mut final_answer: Option<String> = None;
        let mut final_message_id: Option<MessageId> = None;
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut llm_calls: u64 = 0;
        let mut compaction_count: u64 = 0;
        let mut last_event_sequence = request.last_persisted_sequence;
        let mut stream =
            envelope_event_stream_after(raw, request.identity, request.last_persisted_sequence);
        while let Some(item) = stream.next().await {
            let envelope = match item {
                Ok(envelope) => envelope,
                Err(error) => {
                    // The transport only errors before the first event (an
                    // invalid identity); map it to a typed failure.
                    outcome = Some(TurnOutcome::Failed(AgentFailure::from(&error)));
                    break;
                }
            };
            last_event_sequence = envelope.sequence;
            match &envelope.payload {
                AgentEvent::LlmUsage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    usage_reported,
                    ..
                } if *usage_reported => {
                    llm_calls = llm_calls.saturating_add(1);
                    prompt_tokens =
                        prompt_tokens.saturating_add(u64::try_from(*prompt).unwrap_or(u64::MAX));
                    completion_tokens = completion_tokens
                        .saturating_add(u64::try_from(*completion).unwrap_or(u64::MAX));
                }
                AgentEvent::FinalAnswer(answer) => {
                    if outcome.is_none() {
                        outcome = Some(TurnOutcome::Completed);
                        final_answer = Some(answer.clone());
                        final_message_id = envelope.message_id.clone();
                    }
                }
                AgentEvent::ContextCompressed { .. } => {
                    compaction_count = compaction_count.saturating_add(1);
                }
                AgentEvent::Cancelled => {
                    outcome.get_or_insert(TurnOutcome::Cancelled);
                }
                AgentEvent::Error { failure, .. } => {
                    outcome.get_or_insert(
                        if failure.terminal_kind == echo_core::error::AgentTerminalKind::Cancelled {
                            TurnOutcome::Cancelled
                        } else {
                            TurnOutcome::Failed(failure.clone())
                        },
                    );
                }
                _ => {}
            }
            match sink.on_event(envelope).await {
                Ok(SinkControl::Continue) => {}
                Ok(SinkControl::Closed) => {
                    token.cancel();
                    if outcome.is_none() {
                        outcome = Some(TurnOutcome::Cancelled);
                    }
                    break;
                }
                Err(error) => {
                    token.cancel();
                    outcome = Some(TurnOutcome::Failed(AgentFailure::from(&error)));
                    final_answer = None;
                    final_message_id = None;
                    break;
                }
            }
        }

        let outcome = outcome.unwrap_or_else(|| {
            TurnOutcome::Failed(AgentFailure::from(&ReactError::Other(
                "turn stream ended without a terminal event".to_string(),
            )))
        });
        if let Some(lifecycle) = input_lifecycle.as_ref() {
            lifecycle.settle(match &outcome {
                TurnOutcome::Completed => AgentSteerTurnOutcome::Completed,
                TurnOutcome::Cancelled => AgentSteerTurnOutcome::Cancelled,
                TurnOutcome::Failed(_) => AgentSteerTurnOutcome::Failed,
            });
        }
        TurnReceipt {
            turn_id,
            outcome,
            final_answer,
            final_message_id,
            prompt_tokens,
            completion_tokens,
            llm_calls,
            compaction_count,
            last_event_sequence,
            elapsed: started.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::AgentEvent;
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::Duration as StdDuration;

    /// Agent that replays scripted events, optionally with a delay so
    /// cancellation can be observed mid-stream. Events are built per stream
    /// because `AgentEvent` is not `Clone`.
    struct ScriptedAgent {
        name: &'static str,
        script: fn() -> Vec<AgentEvent>,
        delay: Option<StdDuration>,
    }

    impl ScriptedAgent {
        fn new(script: fn() -> Vec<AgentEvent>) -> Self {
            Self {
                name: "scripted",
                script,
                delay: None,
            }
        }

        fn with_delay(mut self, delay: StdDuration) -> Self {
            self.delay = Some(delay);
            self
        }
    }

    impl Agent for ScriptedAgent {
        fn name(&self) -> &str {
            self.name
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<String>> {
            Box::pin(async { Ok(String::new()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            let script = self.script;
            let delay = self.delay;
            Box::pin(async move {
                let stream = futures::stream::iter((script)().into_iter().map(Ok));
                match delay {
                    None => Ok(stream.boxed()),
                    Some(delay) => Ok(stream
                        .then(move |item| async move {
                            tokio::time::sleep(delay).await;
                            item
                        })
                        .boxed()),
                }
            })
        }
    }

    struct ImmediateDrainAgent;

    impl Agent for ImmediateDrainAgent {
        fn name(&self) -> &str {
            "immediate-drain"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<String>> {
            Box::pin(async { Ok(String::new()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async {
                Ok(
                    futures::stream::iter([Ok(AgentEvent::FinalAnswer("done".to_string()))])
                        .boxed(),
                )
            })
        }

        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            if let Some(lifecycle) = invocation.input_lifecycle {
                lifecycle.mark_drained();
            }
            Box::pin(async {
                Ok(
                    futures::stream::iter([Ok(AgentEvent::FinalAnswer("done".to_string()))])
                        .boxed(),
                )
            })
        }
    }

    struct PendingAgent;

    impl Agent for PendingAgent {
        fn name(&self) -> &str {
            "pending"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<String>> {
            Box::pin(std::future::pending())
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(std::future::pending())
        }

        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(std::future::pending())
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<u64>>,
        close_after: Option<usize>,
    }

    #[async_trait]
    impl EventSink for RecordingSink {
        async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            let mut events = self
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            events.push(envelope.sequence);
            Ok(match self.close_after {
                Some(limit) if events.len() >= limit => SinkControl::Closed,
                _ => SinkControl::Continue,
            })
        }
    }

    fn identity(label: &str) -> EventIdentity {
        EventIdentity::new(format!("stream-{label}"), format!("turn-{label}"))
            .expect("valid identity")
    }

    fn usage_event(prompt: usize, completion: usize) -> AgentEvent {
        AgentEvent::LlmUsage {
            model: "test-model".to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_prompt_tokens: 0,
            cache_creation_prompt_tokens: 0,
            usage_reported: true,
        }
    }

    #[tokio::test]
    async fn tracked_initial_input_uses_immediate_agent_context_drain() {
        let (request, mut input) =
            TurnRequest::new(identity("initial-immediate"), "hello").with_input_receipt();
        assert_eq!(input.state(), TurnInputState::Pending);
        let receipt = AgentTurnDriver
            .drive(&ImmediateDrainAgent, request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            input.wait_for_turn_settled().await,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: true,
            }
        );
    }

    #[tokio::test]
    async fn tracked_initial_input_without_agent_publisher_keeps_no_drain() {
        let (request, mut input) = TurnRequest::new(identity("initial-generic"), "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(
                &ScriptedAgent::new(|| vec![AgentEvent::FinalAnswer("done".to_string())]),
                request,
                &RecordingSink::default(),
            )
            .await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            input.wait_for_turn_settled().await,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: false,
            }
        );
    }

    #[tokio::test]
    async fn tracked_initial_input_publishes_accepted_before_stream_call() {
        let (request, mut input) =
            TurnRequest::new(identity("initial-accepted"), "hello").with_input_receipt();
        let sink = RecordingSink::default();
        let mut drive = Box::pin(AgentTurnDriver.drive(&PendingAgent, request, &sink));
        assert!(matches!(
            futures::poll!(drive.as_mut()),
            std::task::Poll::Pending
        ));
        assert_eq!(input.state(), TurnInputState::Accepted);
        drop(drive);
        assert_eq!(
            input.wait_for_turn_settled().await,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Dropped,
                drained: false,
            }
        );
    }

    #[tokio::test]
    async fn tracked_initial_input_cancelled_terminal_is_typed_without_drain() {
        let (request, mut input) = TurnRequest::new(identity("initial-cancelled"), "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(
                &ScriptedAgent::new(|| vec![AgentEvent::Cancelled]),
                request,
                &RecordingSink::default(),
            )
            .await;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(
            input.wait_for_turn_settled().await,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Cancelled,
                drained: false,
            }
        );
    }

    #[tokio::test]
    async fn tracked_initial_input_start_failure_is_typed_without_drain() {
        let (request, mut input) = TurnRequest::new(identity("initial-failed"), "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(&FailingAgent, request, &RecordingSink::default())
            .await;
        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert_eq!(
            input.wait_for_turn_settled().await,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Failed,
                drained: false,
            }
        );
    }

    #[tokio::test]
    async fn tracked_initial_input_sink_failure_is_typed_without_drain() {
        let (request, mut input) = TurnRequest::new(identity("initial-sink-failed"), "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(
                &ScriptedAgent::new(|| vec![AgentEvent::FinalAnswer("done".to_string())]),
                request,
                &FailingSink,
            )
            .await;
        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert_eq!(
            input.wait_for_turn_settled().await,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Failed,
                drained: false,
            }
        );
    }

    #[tokio::test]
    async fn completed_turn_maps_final_answer_and_sequences_from_one() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                usage_event(10, 5),
                AgentEvent::FinalAnswer("done".to_string()),
            ]
        }));
        let sink = RecordingSink::default();
        let request = TurnRequest::new(identity("ok"), "hello");
        let receipt = AgentTurnDriver.drive(agent.as_ref(), request, &sink).await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.final_answer.as_deref(), Some("done"));
        assert_eq!(receipt.prompt_tokens, 10);
        assert_eq!(receipt.completion_tokens, 5);
        assert_eq!(receipt.llm_calls, 1);
        assert_eq!(receipt.last_event_sequence, 2);
        let events = sink
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(*events, vec![1, 2]);
    }

    #[tokio::test]
    async fn execute_mode_maps_cancelled_terminal() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| vec![AgentEvent::Cancelled]));
        let sink = RecordingSink::default();
        let request = TurnRequest::new(identity("cancel"), "task").mode(TurnMode::Execute);
        let receipt = AgentTurnDriver.drive(agent.as_ref(), request, &sink).await;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(receipt.status(), "cancelled");
        assert!(receipt.final_answer.is_none());
    }

    #[tokio::test]
    async fn error_terminal_maps_typed_failure() {
        let failure = AgentFailure::from(&ReactError::Other("boom".to_string()));
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![AgentEvent::Error {
                source: "test".to_string(),
                message: "boom".to_string(),
                failure: AgentFailure::from(&ReactError::Other("boom".to_string())),
            }]
        }));
        let request = TurnRequest::new(identity("fail"), "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome, TurnOutcome::Failed(failure));
        assert_eq!(receipt.status(), "failed");
    }

    #[tokio::test]
    async fn usage_accumulates_across_provider_calls() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                usage_event(100, 40),
                usage_event(7, 3),
                AgentEvent::FinalAnswer("summed".to_string()),
            ]
        }));
        let request = TurnRequest::new(identity("usage"), "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.prompt_tokens, 107);
        assert_eq!(receipt.completion_tokens, 43);
        assert_eq!(receipt.llm_calls, 2);
    }

    #[tokio::test]
    async fn receipt_owns_final_message_and_compaction_facts() -> Result<(), String> {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                AgentEvent::ContextCompressed {
                    before_count: 12,
                    after_count: 5,
                    before_tokens: 1_200,
                    after_tokens: 500,
                },
                AgentEvent::ContextCompressed {
                    before_count: 8,
                    after_count: 4,
                    before_tokens: 800,
                    after_tokens: 400,
                },
                AgentEvent::FinalAnswer("finished".to_string()),
            ]
        }));
        let identity = EventIdentity::for_chat(
            Some("conversation-receipt".to_string()),
            "turn-receipt",
            "message-receipt",
            None,
        )
        .map_err(|error| error.to_string())?;
        let receipt = AgentTurnDriver
            .drive(
                agent.as_ref(),
                TurnRequest::new(identity, "hello"),
                &RecordingSink::default(),
            )
            .await;

        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.compaction_count, 2);
        assert_eq!(
            receipt
                .final_message_id
                .as_ref()
                .map(echo_core::agent::MessageId::as_str),
            Some("message-receipt")
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_terminal_delivery_clears_final_completion_facts() -> Result<(), String> {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![AgentEvent::FinalAnswer("undelivered".to_string())]
        }));
        let identity = EventIdentity::for_chat(
            Some("conversation-delivery-failure".to_string()),
            "turn-delivery-failure",
            "message-delivery-failure",
            None,
        )
        .map_err(|error| error.to_string())?;
        let receipt = AgentTurnDriver
            .drive(
                agent.as_ref(),
                TurnRequest::new(identity, "hello"),
                &FailingSink,
            )
            .await;

        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert!(receipt.final_answer.is_none());
        assert!(receipt.final_message_id.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn resumed_sequencing_continues_after_persisted_sequence() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| vec![AgentEvent::Cancelled]));
        let sink = RecordingSink::default();
        let request = TurnRequest::new(identity("resume"), "hello").last_persisted_sequence(41);
        let receipt = AgentTurnDriver.drive(agent.as_ref(), request, &sink).await;
        assert_eq!(receipt.last_event_sequence, 42);
        let events = sink
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(*events, vec![42]);
    }

    #[tokio::test]
    async fn closed_sink_cancels_and_yields_cancelled_terminal() {
        let agent: Arc<dyn Agent> = Arc::new(
            ScriptedAgent::new(|| {
                (0..5)
                    .map(|index| AgentEvent::Token(format!("t{index}")))
                    .chain(std::iter::once(AgentEvent::FinalAnswer("late".to_string())))
                    .collect()
            })
            .with_delay(StdDuration::from_millis(5)),
        );
        let sink = RecordingSink {
            close_after: Some(1),
            ..RecordingSink::default()
        };
        let request = TurnRequest::new(identity("closed"), "hello");
        let receipt = AgentTurnDriver.drive(agent.as_ref(), request, &sink).await;
        // The default cancellation wrapper yields Cancelled once the token
        // fires and the stream stops delivering later events.
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
    }

    struct FailingAgent;

    impl Agent for FailingAgent {
        fn name(&self) -> &str {
            "failing"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<String>> {
            Box::pin(async { Ok(String::new()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async {
                Err(echo_core::error::ReactError::Other(
                    "stream could not start".to_string(),
                ))
            })
        }
    }

    #[tokio::test]
    async fn stream_start_failure_maps_to_failed_receipt() {
        let agent: Arc<dyn Agent> = Arc::new(FailingAgent);
        let request = TurnRequest::new(identity("startfail"), "hello");
        let sink = RecordingSink::default();
        let receipt = AgentTurnDriver.drive(agent.as_ref(), request, &sink).await;
        assert_eq!(receipt.outcome.status(), "failed");
        assert_eq!(receipt.last_event_sequence, 1);
        assert_eq!(
            *sink
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![1]
        );
    }

    struct FailingSink;

    #[async_trait]
    impl EventSink for FailingSink {
        async fn on_event(
            &self,
            _envelope: EventEnvelope,
        ) -> echo_core::error::Result<SinkControl> {
            Err(ReactError::Other(
                "injected durable sink failure".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn sink_failure_is_failed_not_cancelled() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                AgentEvent::Token("partial".to_string()),
                AgentEvent::FinalAnswer("must not win".to_string()),
            ]
        }));
        let receipt = AgentTurnDriver
            .drive(
                agent.as_ref(),
                TurnRequest::new(identity("sink-failure"), "hello"),
                &FailingSink,
            )
            .await;
        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert!(receipt.final_answer.is_none());
        assert_eq!(receipt.last_event_sequence, 1);
    }

    #[derive(Default)]
    struct InvocationRecordingAgent {
        working_dir: Mutex<Option<std::path::PathBuf>>,
        message_text: Mutex<Option<String>>,
        cancel_states: Mutex<Vec<(bool, bool)>>,
    }

    impl Agent for InvocationRecordingAgent {
        fn name(&self) -> &str {
            "invocation-recording"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<String>> {
            Box::pin(async { Ok(String::new()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async {
                Err(ReactError::Other(
                    "text path should not be selected".to_string(),
                ))
            })
        }

        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                *self
                    .message_text
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = message.content.as_text();
                let invocation_cancelled = invocation
                    .cancel
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled);
                self.cancel_states
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push((cancel.is_cancelled(), invocation_cancelled));
                *self
                    .working_dir
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = invocation.working_dir;
                Ok(
                    futures::stream::iter([Ok(AgentEvent::FinalAnswer("message-ok".to_string()))])
                        .boxed(),
                )
            })
        }
    }

    #[tokio::test]
    async fn structured_chat_message_preserves_invocation_context() {
        let agent = Arc::new(InvocationRecordingAgent::default());
        let expected = std::path::PathBuf::from("/tmp/eko-turn-driver-test");
        let request = TurnRequest::from_message(
            identity("message-invocation"),
            Message::user("hello".to_string()),
        )
        .invocation(AgentInvocationContext {
            working_dir: Some(expected.clone()),
            ..AgentInvocationContext::default()
        });
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.final_answer.as_deref(), Some("message-ok"));
        assert_eq!(
            *agent
                .working_dir
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn text_chat_with_invocation_uses_structured_chat_contract() {
        let agent = Arc::new(InvocationRecordingAgent::default());
        let expected = std::path::PathBuf::from("/tmp/eko-text-chat-test");
        let request = TurnRequest::new(identity("text-invocation"), "text hello").invocation(
            AgentInvocationContext {
                working_dir: Some(expected.clone()),
                ..AgentInvocationContext::default()
            },
        );

        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            agent
                .message_text
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_deref(),
            Some("text hello")
        );
        assert_eq!(
            *agent
                .working_dir
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn request_cancel_precedes_invocation_cancel_and_invocation_only_is_retained() {
        let agent = Arc::new(InvocationRecordingAgent::default());
        let invocation_only = CancellationToken::new();
        invocation_only.cancel();
        let first = TurnRequest::new(identity("invocation-cancel"), "first").invocation(
            AgentInvocationContext {
                cancel: Some(invocation_only),
                ..AgentInvocationContext::default()
            },
        );
        AgentTurnDriver
            .drive(agent.as_ref(), first, &RecordingSink::default())
            .await;

        let request_cancel = CancellationToken::new();
        let shadowed_invocation_cancel = CancellationToken::new();
        shadowed_invocation_cancel.cancel();
        let second = TurnRequest::new(identity("request-cancel"), "second")
            .invocation(AgentInvocationContext {
                cancel: Some(shadowed_invocation_cancel),
                ..AgentInvocationContext::default()
            })
            .cancel(request_cancel);
        AgentTurnDriver
            .drive(agent.as_ref(), second, &RecordingSink::default())
            .await;

        assert_eq!(
            *agent
                .cancel_states
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![(true, true), (false, false)]
        );
    }

    struct DelayedSink {
        events: tokio::sync::Mutex<Vec<(u64, bool)>>,
    }

    #[async_trait]
    impl EventSink for DelayedSink {
        async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            tokio::time::sleep(StdDuration::from_millis(5)).await;
            self.events.lock().await.push((
                envelope.sequence,
                TurnOutcome::classify(&envelope.payload).is_some(),
            ));
            Ok(SinkControl::Continue)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_sink_preserves_order_and_allows_runtime_progress() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                AgentEvent::Token("one".to_string()),
                AgentEvent::Token("two".to_string()),
                AgentEvent::FinalAnswer("done".to_string()),
            ]
        }));
        let sink = DelayedSink {
            events: tokio::sync::Mutex::new(Vec::new()),
        };
        let runtime_progressed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let progressed = Arc::clone(&runtime_progressed);
        let heartbeat = tokio::spawn(async move {
            tokio::time::sleep(StdDuration::from_millis(1)).await;
            progressed.store(true, Ordering::SeqCst);
        });

        let receipt = AgentTurnDriver
            .drive(
                agent.as_ref(),
                TurnRequest::new(identity("async-sink"), "hello"),
                &sink,
            )
            .await;
        assert!(heartbeat.await.is_ok());
        assert!(runtime_progressed.load(Ordering::SeqCst));
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            *sink.events.lock().await,
            vec![(1, false), (2, false), (3, true)]
        );
    }

    #[tokio::test]
    async fn missing_terminal_is_a_failed_receipt() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![AgentEvent::Token("partial".to_string())]
        }));
        let request = TurnRequest::new(identity("noterminal"), "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome.status(), "failed");
    }

    #[tokio::test]
    async fn sink_takes_terminal_envelope_after_receipt_accounting() {
        #[derive(Default)]
        struct OwningSink {
            envelopes: Mutex<Vec<EventEnvelope>>,
        }
        #[async_trait]
        impl EventSink for OwningSink {
            async fn on_event(
                &self,
                envelope: EventEnvelope,
            ) -> echo_core::error::Result<SinkControl> {
                self.envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(envelope);
                Ok(SinkControl::Continue)
            }
        }
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                usage_event(7, 3),
                AgentEvent::Token("t".to_string()),
                AgentEvent::FinalAnswer("fin".to_string()),
            ]
        }));
        let sink = OwningSink::default();
        let request = TurnRequest::new(identity("exactly"), "hello");
        let receipt = AgentTurnDriver.drive(agent.as_ref(), request, &sink).await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.final_answer.as_deref(), Some("fin"));
        assert_eq!(receipt.prompt_tokens, 7);
        assert_eq!(receipt.completion_tokens, 3);
        assert_eq!(receipt.llm_calls, 1);

        let envelopes = sink
            .envelopes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let terminals: Vec<&EventEnvelope> = envelopes
            .iter()
            .filter(|envelope| TurnOutcome::classify(&envelope.payload).is_some())
            .collect();
        assert_eq!(terminals.len(), 1);
        assert!(matches!(
            terminals.first().map(|envelope| &envelope.payload),
            Some(AgentEvent::FinalAnswer(answer)) if answer == "fin"
        ));
    }
}
