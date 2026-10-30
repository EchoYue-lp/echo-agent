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
//! [`TurnReceipt`] with separate execution and delivery results. Chat,
//! execute/headless, and task-driven callers
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
//!     AgentTurnDriver, EventSink, SinkControl, TurnDeliveryOutcome, TurnMode, TurnRequest,
//! };
//!
//! # struct MyAgent;
//! # impl Agent for MyAgent {
//! #     fn name(&self) -> &str { "my-agent" }
//! #     fn model_name(&self) -> &str { "test-model" }
//! #     fn system_prompt(&self) -> &str { "" }
//! #     fn execute_stream_message_with_invocation_context<'a>(
//! #         &'a self, message: echo_core::llm::types::Message,
//! #         _cancel: tokio_util::sync::CancellationToken, invocation: echo_core::agent::AgentInvocationContext,
//! #     ) -> futures::future::BoxFuture<'a, echo_core::error::Result<futures::stream::BoxStream<'a, echo_core::error::Result<echo_core::agent::AgentEvent>>>> {
//! #         Box::pin(async move {
//! #             use echo_core::agent::{AgentEvent, AgentPhase, ExecutionStatus, ExecutionStatusKind};
//! #             let status = |kind, sequence| AgentEvent::ExecutionStatus(ExecutionStatus {
//! #                 adapter_generated: false, execution_id: "example-execution".into(), request_id: Some("example-response".into()), sequence,
//! #                 phase: AgentPhase::Terminal, status: kind, reason: None,
//! #             });
//! #             let events = vec![Ok(status(ExecutionStatusKind::Started, 0)), Ok(AgentEvent::FinalAnswer("done".into())), Ok(status(ExecutionStatusKind::Settled, 1))];
//! #             Ok(Box::pin(async_stream::stream! {
//! #                 let _retained = (message, invocation);
//! #                 for event in events { yield event; }
//! #             }) as futures::stream::BoxStream<'a, echo_core::error::Result<AgentEvent>>)
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
//! let receipt = AgentTurnDriver.drive(&agent, request, &PrintSink).await?;
//! assert_eq!(receipt.outcome.status(), "completed");
//! assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
//! assert_eq!(receipt.final_answer.as_deref(), Some("done"));
//! # Ok(())
//! # }
//! ```

use async_trait::async_trait;
use echo_core::agent::{
    Agent, AgentEvent, AgentInvocationContext, AgentOutputProjection, AgentSteerTurnOutcome,
    CancellationToken, EventEnvelope, EventIdentity, ExecutionUsage, MessageId, TurnId,
    envelope_event_stream_after,
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

#[derive(Debug, Clone)]
struct TurnInputObservation {
    run_id: String,
    execution_id: Option<String>,
}

impl TurnInputObservation {
    fn error(&self) -> ReactError {
        ReactError::ExecutionNotSettled {
            run_id: self.run_id.clone(),
            execution_id: self.execution_id.clone(),
        }
    }
}

/// Private knowledge carried on the original input watch. An observation
/// leaves the public lifecycle state untouched and wakes existing waiters.
#[derive(Debug, Clone)]
struct TurnInputProgress {
    state: TurnInputState,
    owner_identity: TurnInputObservation,
    observation: Option<TurnInputObservation>,
}

struct TurnInputLifecycleInner {
    state: tokio::sync::watch::Sender<TurnInputProgress>,
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
    state: tokio::sync::watch::Receiver<TurnInputProgress>,
}

impl TurnInputReceipt {
    fn new(identity: &EventIdentity) -> (Self, TurnInputLifecycle) {
        let turn_id = identity.turn_id.to_string();
        let (state, receiver) = tokio::sync::watch::channel(TurnInputProgress {
            state: TurnInputState::Pending,
            owner_identity: TurnInputObservation {
                run_id: identity
                    .run_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| turn_id.clone()),
                execution_id: identity.execution_id.as_ref().map(ToString::to_string),
            },
            observation: None,
        });
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

    /// Snapshot the latest real lifecycle state without waiting.
    /// An observation or closed publisher cannot synthesize a terminal state.
    pub fn state(&self) -> TurnInputState {
        self.state.borrow().state.clone()
    }

    fn observed_state(&self) -> echo_core::error::Result<TurnInputState> {
        let current = self.state.borrow();
        if matches!(current.state, TurnInputState::TurnSettled { .. }) {
            return Ok(current.state.clone());
        }
        if let Some(observation) = current.observation.as_ref() {
            return Err(observation.error());
        }
        if self.state.has_changed().is_err() {
            return Err(current.owner_identity.error());
        }
        Ok(current.state.clone())
    }

    /// Wait until acceptance or a real terminal. Unsettled observation and
    /// publisher closure without a receipt return a typed observation error.
    pub async fn wait_for_accepted(&mut self) -> echo_core::error::Result<TurnInputState> {
        loop {
            let state = self.observed_state()?;
            if !matches!(state, TurnInputState::Pending) {
                return Ok(state);
            }
            // Closing the original sender wakes this same receiver; the next
            // read distinguishes a real terminal from missing physical proof.
            let _ = self.state.changed().await;
        }
    }

    /// Wait for the real drain boundary or terminal, preserving observation
    /// errors instead of manufacturing Drained or Dropped.
    pub async fn wait_for_drained(&mut self) -> echo_core::error::Result<TurnInputState> {
        loop {
            let state = self.observed_state()?;
            if !matches!(state, TurnInputState::Pending | TurnInputState::Accepted) {
                return Ok(state);
            }
            let _ = self.state.changed().await;
        }
    }

    /// Wait for the owning turn's real terminal, or report typed uncertainty.
    pub async fn wait_for_turn_settled(&mut self) -> echo_core::error::Result<TurnInputState> {
        loop {
            let state = self.observed_state()?;
            if matches!(state, TurnInputState::TurnSettled { .. }) {
                return Ok(state);
            }
            let _ = self.state.changed().await;
        }
    }
}

impl TurnInputLifecycle {
    fn mark_accepted(&self) {
        self.inner.state.send_if_modified(|progress| {
            if matches!(progress.state, TurnInputState::Pending) {
                progress.state = TurnInputState::Accepted;
                true
            } else {
                false
            }
        });
    }

    fn mark_drained(&self) {
        self.inner.state.send_if_modified(|progress| {
            if matches!(
                progress.state,
                TurnInputState::Pending | TurnInputState::Accepted
            ) {
                progress.state = TurnInputState::Drained;
                true
            } else {
                false
            }
        });
    }

    fn observe_not_settled(&self, error: &ReactError) {
        if let ReactError::ExecutionNotSettled {
            run_id,
            execution_id,
        } = error
        {
            self.inner.state.send_modify(|progress| {
                if !matches!(progress.state, TurnInputState::TurnSettled { .. })
                    && progress.observation.is_none()
                {
                    progress.observation = Some(TurnInputObservation {
                        run_id: run_id.clone(),
                        execution_id: execution_id.clone(),
                    });
                }
            });
        }
    }

    fn settle(&self, outcome: AgentSteerTurnOutcome) {
        if self
            .inner
            .settled
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        self.inner.state.send_modify(|progress| {
            let drained = matches!(progress.state, TurnInputState::Drained);
            progress.state = TurnInputState::TurnSettled { outcome, drained };
            progress.observation = None;
        });
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
    // 原生消息的可选回放数据较大；间接存放避免每个纯文本输入也占用整条消息大小。
    Message(Box<Message>),
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
            input: TurnInput::Message(Box::new(message)),
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
        let (receipt, lifecycle) = TurnInputReceipt::new(&self.identity);
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

/// Delivery result for envelopes emitted by one driven turn.
///
/// This is deliberately orthogonal to [`TurnOutcome`]. The Agent producer
/// owns execution settlement, while the sink owns whether the observed
/// envelopes were accepted by a journal, projection, or observer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnDeliveryOutcome {
    /// No envelope reached the sink.
    NotAttempted,
    /// Every envelope observed so far was accepted by the sink.
    Delivered,
    /// The sink explicitly stopped accepting envelopes.
    Closed,
    /// The sink rejected or failed while delivering an envelope.
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
            AgentEvent::ExecutionStatus(status) => match status.status {
                echo_core::agent::ExecutionStatusKind::Settled => Some(Self::Completed),
                echo_core::agent::ExecutionStatusKind::Cancelled => Some(Self::Cancelled),
                echo_core::agent::ExecutionStatusKind::Failed => {
                    Some(Self::Failed(AgentFailure::message(
                        "runtime",
                        status
                            .reason
                            .clone()
                            .unwrap_or_else(|| "execution failed".to_string()),
                    )))
                }
                _ => None,
            },
            AgentEvent::FinalAnswer(_) => None,
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
    /// Whether the observed envelopes were accepted by the event sink.
    pub delivery: TurnDeliveryOutcome,
    /// Final answer text when the turn completed with one.
    pub final_answer: Option<String>,
    /// Message identity carried by the final-answer envelope observed by the driver.
    pub final_message_id: Option<MessageId>,
    /// Provider-reported prompt tokens accumulated over the turn.
    pub prompt_tokens: u64,
    /// Provider-reported completion tokens accumulated over the turn.
    pub completion_tokens: u64,
    /// Number of provider calls that reported usage.
    pub llm_calls: u64,
    /// Number of explicit context-compaction boundaries emitted by the Agent.
    pub compaction_count: u64,
    /// Last envelope sequence observed by the driver. This does not imply a
    /// failing sink committed the same sequence.
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
            delivery: TurnDeliveryOutcome::NotAttempted,
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
            delivery: TurnDeliveryOutcome::NotAttempted,
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
        delivery: TurnDeliveryOutcome,
        last_event_sequence: u64,
        started: Instant,
    ) -> Self {
        Self::terminal_receipt(
            turn_id,
            TurnOutcome::Failed(failure),
            delivery,
            last_event_sequence,
            started,
        )
    }

    fn terminal_receipt(
        turn_id: TurnId,
        outcome: TurnOutcome,
        delivery: TurnDeliveryOutcome,
        last_event_sequence: u64,
        started: Instant,
    ) -> Self {
        Self {
            turn_id,
            outcome,
            delivery,
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
/// persistence failure recorded separately from a producer terminal already
/// observed by the driver.
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
    ///
    /// A terminal payload fixes the result cause, but only the original raw
    /// producer's EOF permits returning a receipt or an observation error.
    /// Observation-only errors have no terminal receipt; setup observations
    /// propagate directly because no raw stream was opened.
    pub async fn drive(
        &self,
        agent: &dyn Agent,
        mut request: TurnRequest,
        sink: &dyn EventSink,
    ) -> echo_core::error::Result<TurnReceipt> {
        let started = Instant::now();
        let turn_id = request.identity.turn_id.clone();
        let input_lifecycle = request.input_publisher.take();
        if let Err(error) = request.identity.validate() {
            if let Some(lifecycle) = input_lifecycle.as_ref() {
                lifecycle.settle(AgentSteerTurnOutcome::Failed);
            }
            return Ok(TurnReceipt::failure_receipt(
                turn_id,
                AgentFailure::try_from(&error)?,
                TurnDeliveryOutcome::NotAttempted,
                request.last_persisted_sequence,
                started,
            ));
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

        // Observe the original source before envelope normalization. Only a
        // genuine later terminal on this same bound source can supersede its
        // matching observation; synthetic delivery errors are not receipts.
        let source_observation = std::sync::Arc::new(std::sync::Mutex::new(None::<ReactError>));
        let source_progress = source_observation.clone();
        let source_run = request
            .identity
            .run_id
            .as_ref()
            .map(|value| value.as_str())
            .unwrap_or(request.identity.turn_id.as_str())
            .to_string();
        let source_execution = request
            .identity
            .execution_id
            .as_ref()
            .map(|value| value.as_str().to_string());
        let input = request.input;
        let message = match input {
            TurnInput::Text(text) => Message::user(text),
            TurnInput::Message(message) => *message,
        };
        let invocation = invocation_with_cancel(invocation, &token);
        let raw = match request.mode {
            TurnMode::Chat => {
                agent
                    .chat_stream_message_with_invocation_context(message, token.clone(), invocation)
                    .await
            }
            TurnMode::Execute => {
                agent
                    .execute_stream_message_with_invocation_context(
                        message,
                        token.clone(),
                        invocation,
                    )
                    .await
            }
        };
        // Reuse the existing pure canonical projection, not another completion validator/source.
        let source_projection = std::sync::Arc::new(std::sync::Mutex::new((
            AgentOutputProjection::default(),
            None::<ReactError>,
        )));
        let raw_projection = source_projection.clone();
        let mut raw = match raw {
            Ok(stream) => stream,
            Err(error) => {
                if let Some(lifecycle) = input_lifecycle.as_ref() {
                    lifecycle.observe_not_settled(&error);
                }
                let failure = AgentFailure::try_from(&error)?;
                let cancelled =
                    failure.terminal_kind == echo_core::error::AgentTerminalKind::Cancelled;
                if let Some(lifecycle) = input_lifecycle.as_ref() {
                    lifecycle.settle(if cancelled {
                        AgentSteerTurnOutcome::Cancelled
                    } else {
                        AgentSteerTurnOutcome::Failed
                    });
                }
                let mut delivery = TurnDeliveryOutcome::NotAttempted;
                let next_sequence = request.last_persisted_sequence.checked_add(1);
                let mut last_event_sequence = request.last_persisted_sequence;
                if let Some(sequence) = next_sequence
                    && let Ok(envelope) = EventEnvelope::new(
                        &request.identity,
                        sequence,
                        request.identity.parent_event_id.clone(),
                        AgentEvent::from_error("turn_driver", &error)?,
                    )
                {
                    last_event_sequence = envelope.sequence;
                    match sink.on_event(envelope).await {
                        Ok(SinkControl::Continue) => {
                            delivery = TurnDeliveryOutcome::Delivered;
                        }
                        Ok(SinkControl::Closed) => {
                            delivery = TurnDeliveryOutcome::Closed;
                        }
                        Err(sink_error) => {
                            delivery =
                                TurnDeliveryOutcome::Failed(AgentFailure::try_from(&sink_error)?);
                        }
                    }
                }
                return Ok(TurnReceipt::terminal_receipt(
                    turn_id,
                    if cancelled {
                        TurnOutcome::Cancelled
                    } else {
                        TurnOutcome::Failed(failure)
                    },
                    delivery,
                    last_event_sequence,
                    started,
                ));
            }
        }
        .inspect(move |item| {
            let mut newly_validated_terminal = false;
            {
                let mut state = raw_projection
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let checked = match item {
                    Ok(AgentEvent::ExecutionStatus(status)) => {
                        let checked = state.0.observe_status(status);
                        newly_validated_terminal = checked.as_ref().is_ok_and(|changed| *changed)
                            && matches!(
                                status.status,
                                echo_core::agent::ExecutionStatusKind::Settled
                                    | echo_core::agent::ExecutionStatusKind::Failed
                                    | echo_core::agent::ExecutionStatusKind::Cancelled
                            );
                        checked.map(|_| ())
                    }
                    Ok(AgentEvent::AssistantMessage { block }) => {
                        state.0.observe_block(block).map(|_| ())
                    }
                    Ok(AgentEvent::FinalAnswer(answer)) => state.0.confirm_answer(answer),
                    _ => Ok(()),
                };
                if let Err(error) = checked {
                    state.1.get_or_insert(error);
                }
            }
            // Borrow the validated source identity from the one existing projection.
            // This is not another mutable owner cache, nor a guess from final text.
            let checked_source_execution = raw_projection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .0
                .execution_id()
                .map(str::to_string);
            let matched_execution = source_execution
                .as_deref()
                .or(checked_source_execution.as_deref());
            let mut pending = source_progress.lock().unwrap_or_else(|p| p.into_inner());
            if let Ok(AgentEvent::ExecutionStatus(status)) = item
                && source_execution
                    .as_deref()
                    .is_some_and(|owner| owner != status.execution_id)
            {
                let mut projection = raw_projection
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                projection.1.get_or_insert_with(|| {
                    ReactError::Other(
                        "source execution does not match the admitted turn identity".into(),
                    )
                });
            }
            let matches_owner = |error: &ReactError| {
                matches!(error,
                ReactError::ExecutionNotSettled { run_id, execution_id }
                    if run_id == &source_run && execution_id.as_deref() == matched_execution)
            };
            match item {
                Err(ReactError::ExecutionNotSettled {
                    run_id,
                    execution_id,
                }) => {
                    // A foreign observation cannot be cleared by this owner's
                    // terminal, even when a later matching observation arrives.
                    if pending.as_ref().is_none_or(matches_owner) {
                        *pending = Some(ReactError::ExecutionNotSettled {
                            run_id: run_id.clone(),
                            execution_id: execution_id.clone(),
                        });
                    }
                }
                Ok(event @ AgentEvent::ExecutionStatus(status))
                    if newly_validated_terminal
                        && event.is_execution_terminal()
                        && Some(status.execution_id.as_str()) == matched_execution
                        && pending.as_ref().is_some_and(matches_owner) =>
                {
                    *pending = None;
                }
                _ => {}
            }
        })
        .fuse();
        // 复用已核验的原 source projection；主投递与 raw drain 固定同一首次原因。
        // delivery 只描述 sink，不能覆盖真实 native 取消/失败或更早 producer 原因。
        let source_terminal_cause = || -> echo_core::error::Result<Option<TurnOutcome>> {
            source_projection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .0
                .terminal_failure()
                .as_ref()
                .map(|error| {
                    AgentFailure::try_from(error).map(|failure| {
                        if failure.terminal_kind == echo_core::error::AgentTerminalKind::Cancelled {
                            TurnOutcome::Cancelled
                        } else {
                            TurnOutcome::Failed(failure)
                        }
                    })
                })
                .transpose()
        };
        let mut outcome: Option<TurnOutcome> = None;
        let mut native_failure: Option<TurnOutcome> = None;
        let mut delivery = TurnDeliveryOutcome::NotAttempted;
        let mut final_answer: Option<String> = None;
        let mut final_message_id: Option<MessageId> = None;
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut llm_calls: u64 = 0;
        let mut compaction_count: u64 = 0;
        let mut last_event_sequence = request.last_persisted_sequence;
        // The envelope stream is a delivery projection, not the physical owner.
        // Keep the original stream so an observation refusal cannot drop it.
        let mut observation = None;
        let mut stream = envelope_event_stream_after(
            raw.by_ref().boxed(),
            request.identity,
            request.last_persisted_sequence,
        );
        while let Some(item) = stream.next().await {
            let envelope = match item {
                Ok(envelope) => envelope,
                Err(error) => {
                    match AgentFailure::try_from(&error) {
                        Ok(failure) => {
                            let cause = if failure.terminal_kind
                                == echo_core::error::AgentTerminalKind::Cancelled
                            {
                                TurnOutcome::Cancelled
                            } else {
                                TurnOutcome::Failed(failure)
                            };
                            outcome = Some(native_failure.get_or_insert(cause).clone());
                        }
                        Err(error) => {
                            if let Some(lifecycle) = input_lifecycle.as_ref() {
                                lifecycle.observe_not_settled(&error);
                            }
                            // Keep consuming this original source: a later
                            // real terminal and EOF may resolve its observation.
                            continue;
                        }
                    }
                    break;
                }
            };
            last_event_sequence = envelope.sequence;
            match &envelope.payload {
                AgentEvent::ExecutionStatus(_) => {
                    if let Some(terminal) = TurnOutcome::classify(&envelope.payload) {
                        if let Some(cause) = source_terminal_cause()? {
                            native_failure.get_or_insert(cause);
                        }
                        outcome = Some(native_failure.as_ref().cloned().unwrap_or(terminal));
                    }
                }
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
                        final_answer = Some(answer.clone());
                        final_message_id = envelope.message_id.clone();
                    }
                }
                AgentEvent::ContextCompressed { .. } => {
                    compaction_count = compaction_count.saturating_add(1);
                }
                AgentEvent::Cancelled => {
                    let cause = native_failure.get_or_insert(TurnOutcome::Cancelled).clone();
                    outcome = Some(cause);
                }
                AgentEvent::Error { failure, .. } => {
                    let failure_outcome = if failure.terminal_kind
                        == echo_core::error::AgentTerminalKind::Cancelled
                    {
                        TurnOutcome::Cancelled
                    } else {
                        TurnOutcome::Failed(failure.clone())
                    };
                    let cause = native_failure.get_or_insert(failure_outcome).clone();
                    outcome = Some(cause);
                }
                _ => {}
            }
            match sink.on_event(envelope).await {
                Ok(SinkControl::Continue) => {
                    delivery = TurnDeliveryOutcome::Delivered;
                }
                Ok(SinkControl::Closed) => {
                    delivery = TurnDeliveryOutcome::Closed;
                    token.cancel();
                    if outcome.is_none() {
                        outcome = Some(TurnOutcome::Cancelled);
                    }
                    break;
                }
                Err(error) => {
                    let failure = match AgentFailure::try_from(&error) {
                        Ok(failure) => failure,
                        Err(error) => {
                            if let Some(lifecycle) = input_lifecycle.as_ref() {
                                lifecycle.observe_not_settled(&error);
                            }
                            observation = Some(error);
                            break;
                        }
                    };
                    delivery = TurnDeliveryOutcome::Failed(failure.clone());
                    token.cancel();
                    if outcome.is_none() {
                        // 已观察正文仍等待原末尾status裁决；非完成时统一清除，不能在投递失败处丢事实。
                        outcome = Some(TurnOutcome::Failed(failure));
                    }
                    break;
                }
            }
        }

        drop(stream);
        // Terminal payloads and sink closure fix delivery/result causes, not
        // physical settlement. Retain every opened original source through
        // actual EOF, preserving the cause and accounting already observed.
        while let Some(item) = raw.next().await {
            // 仍读同一原source，保首个producer原因；尾部不是新的投递，不增加序号。
            match &item {
                Ok(event @ AgentEvent::ExecutionStatus(_)) => {
                    if let Some(terminal) = TurnOutcome::classify(event) {
                        if let Some(cause) = source_terminal_cause()? {
                            native_failure.get_or_insert(cause);
                        }
                        let preserve_delivery_cause =
                            matches!(delivery, TurnDeliveryOutcome::Failed(_))
                                && final_answer.is_none()
                                && matches!(outcome, Some(TurnOutcome::Failed(_)));
                        if let Some(cause) = native_failure.as_ref() {
                            outcome = Some(cause.clone());
                        } else if !preserve_delivery_cause {
                            outcome = Some(terminal);
                        }
                    }
                }
                Ok(AgentEvent::Error { failure, .. }) => {
                    let cause = if failure.terminal_kind
                        == echo_core::error::AgentTerminalKind::Cancelled
                    {
                        TurnOutcome::Cancelled
                    } else {
                        TurnOutcome::Failed(failure.clone())
                    };
                    // sink失败已触发cancel；晚到的诱发取消不能覆盖更早的投递失败原因。
                    if !(matches!(delivery, TurnDeliveryOutcome::Failed(_))
                        && matches!(cause, TurnOutcome::Cancelled))
                    {
                        outcome = Some(native_failure.get_or_insert(cause).clone());
                    }
                }
                Err(error) => match AgentFailure::try_from(error) {
                    Ok(failure) => {
                        let cause = if failure.terminal_kind
                            == echo_core::error::AgentTerminalKind::Cancelled
                        {
                            TurnOutcome::Cancelled
                        } else {
                            TurnOutcome::Failed(failure)
                        };
                        if !(matches!(delivery, TurnDeliveryOutcome::Failed(_))
                            && matches!(cause, TurnOutcome::Cancelled))
                        {
                            outcome = Some(native_failure.get_or_insert(cause).clone());
                        }
                    }
                    Err(observation) => {
                        if let Some(lifecycle) = input_lifecycle.as_ref() {
                            lifecycle.observe_not_settled(&observation);
                        }
                    }
                },
                _ => {}
            }
        }
        if let Some(error) = source_observation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            return Err(error);
        }
        if let Some(error) = observation {
            return Err(error);
        }

        {
            let mut projection = source_projection
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let contract_error = projection.1.take();
            // 缺 native settlement 不得成功；只有 sink 首因且没有原生终态失败时，
            // 普通 typed cancellation 或 generic missing-terminal 不改写该先前失败。
            let preserve_delivery_cause = contract_error.is_none()
                && native_failure.is_none()
                && matches!(delivery, TurnDeliveryOutcome::Failed(_))
                && matches!(outcome, Some(TurnOutcome::Failed(_)));
            let contract_error = contract_error.or_else(|| projection.0.finish().err());
            if let Some(error) = contract_error {
                let failure = AgentFailure::try_from(&error)?;
                let refusal =
                    if failure.terminal_kind == echo_core::error::AgentTerminalKind::Cancelled {
                        TurnOutcome::Cancelled
                    } else {
                        TurnOutcome::Failed(failure)
                    };
                if let Some(cause) = native_failure.take() {
                    outcome = Some(cause);
                } else if !preserve_delivery_cause {
                    outcome = Some(refusal);
                }
            }
        }
        let outcome = outcome.unwrap_or_else(|| {
            TurnOutcome::Failed(AgentFailure::message(
                "other",
                "turn stream ended without a terminal event",
            ))
        });
        if !matches!(outcome, TurnOutcome::Completed) {
            final_answer = None;
            final_message_id = None;
        }
        if let Some(lifecycle) = input_lifecycle.as_ref() {
            lifecycle.settle(match &outcome {
                TurnOutcome::Completed => AgentSteerTurnOutcome::Completed,
                TurnOutcome::Cancelled => AgentSteerTurnOutcome::Cancelled,
                TurnOutcome::Failed(_) => AgentSteerTurnOutcome::Failed,
            });
        }
        Ok(TurnReceipt {
            turn_id,
            outcome,
            delivery,
            final_answer,
            final_message_id,
            prompt_tokens,
            completion_tokens,
            llm_calls,
            compaction_count,
            last_event_sequence,
            elapsed: started.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::agent::{AgentEvent, ExecutionStatusKind};
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::Duration as StdDuration;

    /// 每个脚本显式声明原生状态；source 不从正文或 EOF 推断成功。
    struct ScriptedAgent {
        name: &'static str,
        script: fn() -> Vec<AgentEvent>,
        delay: Option<StdDuration>,
        cancelled_tail: bool,
    }

    impl ScriptedAgent {
        fn new(script: fn() -> Vec<AgentEvent>) -> Self {
            Self {
                name: "scripted",
                script,
                delay: None,
                cancelled_tail: false,
            }
        }
        fn with_delay(mut self, delay: StdDuration) -> Self {
            self.delay = Some(delay);
            self
        }
        fn with_cancelled_tail(mut self) -> Self {
            self.cancelled_tail = true;
            self
        }
        fn source<'a>(
            &'a self,
            message: Message,
            cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                let source = async_stream::stream! {
                    let _retained_input_and_resources = (message, invocation);
                    for event in (self.script)() {
                        if let Some(delay) = self.delay { tokio::time::sleep(delay).await; }
                        yield Ok(event);
                    }
                    if self.cancelled_tail {
                        cancel.cancelled().await;
                        yield Ok(native_status("scripted", ExecutionStatusKind::Cancelled, 1));
                    }
                };
                Ok(source.boxed())
            })
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
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            self.source(message, cancel, invocation)
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
            self.source(message, cancel, invocation)
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
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            if let Some(lifecycle) = invocation.input_lifecycle.as_ref() {
                lifecycle.mark_drained();
            }
            Box::pin(async move {
                Ok(async_stream::stream! {
                    let _retained_input_and_resources = (message, invocation);
                    yield Ok(native_status("immediate-drain", ExecutionStatusKind::Started, 0));
                    yield Ok(AgentEvent::FinalAnswer("done".to_string()));
                    yield Ok(native_status("immediate-drain", ExecutionStatusKind::Settled, 1));
                }
                .boxed())
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
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                let _retained_input_and_resources = (_message, _cancel, _invocation);
                std::future::pending().await
            })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                let _retained_input_and_resources = (_message, _cancel, _invocation);
                std::future::pending().await
            })
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<u64>>,
        payloads: Mutex<Vec<AgentEvent>>,
        envelopes: Mutex<Vec<EventEnvelope>>,
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
            self.envelopes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(envelope.clone());
            self.payloads
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(envelope.payload);
            Ok(match self.close_after {
                Some(limit) if events.len() >= limit => SinkControl::Closed,
                _ => SinkControl::Continue,
            })
        }
    }

    // 只观察真实执行状态；FinalAnswer 是正文投影，不是物理终态。
    fn delivery_kind(event: &AgentEvent) -> &'static str {
        use echo_core::agent::ExecutionStatusKind;
        match event {
            AgentEvent::ExecutionStatus(status) => match status.status {
                ExecutionStatusKind::Started => "started",
                ExecutionStatusKind::Settled => "settled",
                ExecutionStatusKind::Failed => "runtime_failed",
                ExecutionStatusKind::Cancelled => "runtime_cancelled",
                ExecutionStatusKind::Waiting
                | ExecutionStatusKind::ToolPending
                | ExecutionStatusKind::Candidate => "runtime_progress",
            },
            AgentEvent::LlmUsage { .. } => "usage",
            AgentEvent::AssistantMessage { .. } => "assistant",
            AgentEvent::FinalAnswer(_) => "final_answer",
            AgentEvent::Error { .. } => "error",
            AgentEvent::Cancelled => "cancelled_projection",
            _ => "other",
        }
    }

    fn assert_delivery_trace(
        envelopes: &[EventEnvelope],
        expected_identity: &EventIdentity,
        persisted: u64,
        expected: &[&str],
    ) -> echo_core::error::Result<()> {
        let kinds: Vec<_> = envelopes
            .iter()
            .map(|event| delivery_kind(&event.payload))
            .collect();
        assert_eq!(
            kinds, expected,
            "必须核对完整投递轨迹，不能过滤新增状态后只改计数"
        );
        let violations = echo_core::agent::validate_envelope_trajectory(envelopes);
        assert!(violations.is_empty(), "{violations:?}");
        for (offset, envelope) in envelopes.iter().enumerate() {
            let sequence = u64::try_from(offset)
                .ok()
                .and_then(|offset| persisted.checked_add(offset))
                .and_then(|sequence| sequence.checked_add(1));
            assert_eq!(Some(envelope.sequence), sequence);
            let rebuilt = EventEnvelope::new(
                expected_identity,
                envelope.sequence,
                expected_identity.parent_event_id.clone(),
                envelope.payload.clone(),
            )?;
            assert_eq!(envelope.event_id, rebuilt.event_id);
            assert_eq!(envelope.content_hash, rebuilt.content_hash);
            assert_eq!(envelope.stream_id, expected_identity.stream_id);
            assert_eq!(envelope.turn_id, expected_identity.turn_id);
            assert_eq!(envelope.run_id, expected_identity.run_id);
            assert_eq!(envelope.execution_id, expected_identity.execution_id);
            assert_eq!(envelope.message_id, expected_identity.message_id);
            assert_eq!(envelope.conversation_id, expected_identity.conversation_id);
        }
        let statuses: Vec<_> = envelopes
            .iter()
            .filter_map(|event| match &event.payload {
                AgentEvent::ExecutionStatus(status) => Some(status),
                _ => None,
            })
            .collect();
        if let Some(first) = statuses.first() {
            assert!(!first.execution_id.is_empty());
            assert_eq!(first.status, echo_core::agent::ExecutionStatusKind::Started);
            assert!(
                statuses
                    .iter()
                    .all(|status| status.execution_id == first.execution_id)
            );
            assert!(statuses.windows(2).all(|pair| match pair {
                [left, right] => left.sequence < right.sequence,
                _ => false,
            }));
        }
        let physical: Vec<_> = envelopes
            .iter()
            .filter(|event| event.payload.is_execution_terminal())
            .collect();
        assert!(physical.len() <= 1);
        if let Some(terminal) = physical.first() {
            assert_eq!(
                Some(terminal.sequence),
                envelopes.last().map(|event| event.sequence)
            );
        }
        assert!(
            envelopes
                .iter()
                .filter(|event| matches!(event.payload, AgentEvent::FinalAnswer(_)))
                .count()
                <= 1
        );
        Ok(())
    }

    // 故障落在具名语义事件，不依赖 Started/Status 加入前的序号布局。
    struct SemanticSink {
        recording: RecordingSink,
        target: fn(&AgentEvent) -> bool,
        close: bool,
        hits: std::sync::atomic::AtomicUsize,
    }

    impl SemanticSink {
        fn new(target: fn(&AgentEvent) -> bool, close: bool) -> Self {
            Self {
                recording: RecordingSink::default(),
                target,
                close,
                hits: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl EventSink for SemanticSink {
        async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            let hit = (self.target)(&envelope.payload);
            self.recording.on_event(envelope).await?;
            if !hit {
                return Ok(SinkControl::Continue);
            }
            self.hits.fetch_add(1, Ordering::SeqCst);
            if self.close {
                Ok(SinkControl::Closed)
            } else {
                Err(ReactError::Other(
                    "injected durable sink failure".to_string(),
                ))
            }
        }
    }

    /// fixture 的独立 commentary block；不携带完成或取消含义。
    fn commentary(text: String) -> AgentEvent {
        AgentEvent::AssistantMessage {
            block: echo_core::agent::AssistantMessageBlock {
                provider_source: None,
                block_id: format!("commentary:{text}"),
                request_id: "fixture-response".into(),
                sequence: 0,
                phase: echo_core::agent::AssistantPhase::Commentary,
                delta: text,
                complete: true,
                provider: None,
                reasoning: None,
            },
        }
    }

    fn native_status(
        execution: &str,
        kind: echo_core::agent::ExecutionStatusKind,
        sequence: u64,
    ) -> AgentEvent {
        AgentEvent::ExecutionStatus(echo_core::agent::ExecutionStatus {
            adapter_generated: false,
            execution_id: execution.to_string(),
            request_id: None,
            sequence,
            phase: if kind == echo_core::agent::ExecutionStatusKind::Started {
                echo_core::agent::AgentPhase::Thinking
            } else {
                echo_core::agent::AgentPhase::Terminal
            },
            status: kind,
            reason: None,
        })
    }

    async fn guarded_invocation() -> echo_core::error::Result<(
        AgentInvocationContext,
        Arc<tokio::sync::Semaphore>,
        Arc<std::sync::atomic::AtomicUsize>,
    )> {
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = Arc::clone(&capacity)
            .acquire_owned()
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let invocation = AgentInvocationContext {
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                ObservationResource {
                    _permit: permit,
                    drops: Arc::clone(&drops),
                },
            )],
            ..Default::default()
        };
        Ok((invocation, capacity, drops))
    }

    fn identity(label: &str) -> echo_core::error::Result<EventIdentity> {
        EventIdentity::new(format!("stream-{label}"), format!("turn-{label}"))
    }

    fn usage_event(prompt: usize, completion: usize) -> AgentEvent {
        AgentEvent::LlmUsage {
            model: "test-model".to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt.saturating_add(completion),
            cached_prompt_tokens: 0,
            cache_creation_prompt_tokens: 0,
            usage_reported: true,
        }
    }

    #[tokio::test]
    async fn tracked_initial_input_uses_immediate_agent_context_drain()
    -> echo_core::error::Result<()> {
        let (request, mut input) =
            TurnRequest::new(identity("initial-immediate")?, "hello").with_input_receipt();
        assert_eq!(input.state(), TurnInputState::Pending);
        let receipt = AgentTurnDriver
            .drive(&ImmediateDrainAgent, request, &RecordingSink::default())
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_initial_input_without_agent_publisher_keeps_no_drain()
    -> echo_core::error::Result<()> {
        let (request, mut input) = TurnRequest::new(identity("initial-generic")?, "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(
                &ScriptedAgent::new(|| {
                    let mut events =
                        vec![native_status("scripted", ExecutionStatusKind::Started, 0)];
                    events.extend(vec![AgentEvent::FinalAnswer("done".to_string())]);
                    events.push(native_status("scripted", ExecutionStatusKind::Settled, 1));
                    events
                }),
                request,
                &RecordingSink::default(),
            )
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_initial_input_publishes_accepted_before_stream_call()
    -> echo_core::error::Result<()> {
        let (request, input) =
            TurnRequest::new(identity("initial-accepted")?, "hello").with_input_receipt();
        let sink = RecordingSink::default();
        let mut drive = Box::pin(AgentTurnDriver.drive(&PendingAgent, request, &sink));
        assert!(matches!(
            futures::poll!(drive.as_mut()),
            std::task::Poll::Pending
        ));
        assert_eq!(input.state(), TurnInputState::Accepted);
        drop(drive);
        assert_eq!(input.state(), TurnInputState::Accepted);
        assert_input_observation(&input, "turn-initial-accepted", None).await;

        Ok(())
    }

    #[tokio::test]
    async fn tracked_initial_input_cancelled_terminal_is_typed_without_drain()
    -> echo_core::error::Result<()> {
        let (request, mut input) = TurnRequest::new(identity("initial-cancelled")?, "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(
                &ScriptedAgent::new(|| {
                    let mut events =
                        vec![native_status("scripted", ExecutionStatusKind::Started, 0)];
                    events.extend(vec![AgentEvent::Cancelled]);
                    events.push(native_status("scripted", ExecutionStatusKind::Cancelled, 1));
                    events
                }),
                request,
                &RecordingSink::default(),
            )
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Cancelled,
                drained: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_initial_input_start_failure_is_typed_without_drain()
    -> echo_core::error::Result<()> {
        let (request, mut input) = TurnRequest::new(identity("initial-failed")?, "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(&FailingAgent, request, &RecordingSink::default())
            .await?;
        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert_eq!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Failed,
                drained: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_initial_input_terminal_sink_failure_keeps_execution_terminal()
    -> echo_core::error::Result<()> {
        let (request, mut input) = TurnRequest::new(identity("initial-sink-failed")?, "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let receipt = AgentTurnDriver
            .drive(
                &ScriptedAgent::new(|| {
                    let mut events =
                        vec![native_status("scripted", ExecutionStatusKind::Started, 0)];
                    events.extend(vec![AgentEvent::FinalAnswer("done".to_string())]);
                    events.push(native_status("scripted", ExecutionStatusKind::Settled, 1));
                    events
                }),
                request,
                &FailingSink,
            )
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert!(matches!(receipt.delivery, TurnDeliveryOutcome::Failed(_)));
        assert_eq!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Completed,
                drained: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn completed_turn_maps_final_answer_and_sequences_from_one()
    -> echo_core::error::Result<()> {
        let agent = ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                usage_event(10, 5),
                AgentEvent::FinalAnswer("done".into()),
                native_status("scripted", ExecutionStatusKind::Settled, 1),
            ]
        });
        let sink = RecordingSink::default();
        let id = identity("ok")?;
        let receipt = AgentTurnDriver
            .drive(&agent, TurnRequest::new(id.clone(), "hello"), &sink)
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.final_answer.as_deref(), Some("done"));
        assert_eq!(receipt.prompt_tokens, 10);
        assert_eq!(receipt.completion_tokens, 5);
        assert_eq!(receipt.llm_calls, 1);
        let envelopes = sink
            .envelopes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // 同一脚本 source 显式发布 Started / Settled，正文不决定完成。
        assert_delivery_trace(
            &envelopes,
            &id,
            0,
            &["started", "usage", "final_answer", "settled"],
        )?;
        assert_eq!(receipt.last_event_sequence, 4);
        assert!(
            matches!(envelopes.iter().find(|event| delivery_kind(&event.payload) == "final_answer").map(|event| &event.payload), Some(AgentEvent::FinalAnswer(text)) if text == "done")
        );
        Ok(())
    }

    #[tokio::test]
    async fn native_message_turn_driver_delivers_final_runtime_status()
    -> echo_core::error::Result<()> {
        fn status(kind: echo_core::agent::ExecutionStatusKind, sequence: u64) -> AgentEvent {
            AgentEvent::ExecutionStatus(echo_core::agent::ExecutionStatus {
                adapter_generated: false,
                execution_id: "execution-native".to_string(),
                request_id: Some("response-native".to_string()),
                sequence,
                phase: echo_core::agent::AgentPhase::Terminal,
                status: kind,
                reason: None,
            })
        }
        let agent = ScriptedAgent::new(|| {
            vec![
                status(echo_core::agent::ExecutionStatusKind::Started, 0),
                AgentEvent::FinalAnswer("native done".to_string()),
                status(echo_core::agent::ExecutionStatusKind::Settled, 1),
            ]
        });
        let sink = RecordingSink::default();
        let receipt = AgentTurnDriver
            .drive(&agent, TurnRequest::new(identity("native")?, "run"), &sink)
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.final_answer.as_deref(), Some("native done"));
        assert_eq!(receipt.last_event_sequence, 3);
        let payloads = sink
            .payloads
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(
            matches!(payloads.last(), Some(AgentEvent::ExecutionStatus(status))
            if status.status == echo_core::agent::ExecutionStatusKind::Settled)
        );
        assert_eq!(
            payloads
                .iter()
                .filter(|event| event.is_execution_terminal())
                .count(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn native_message_turn_driver_keeps_execution_settlement_separate_from_sink_closure()
    -> echo_core::error::Result<()> {
        let agent = ScriptedAgent::new(|| {
            vec![
                AgentEvent::ExecutionStatus(echo_core::agent::ExecutionStatus {
                    adapter_generated: false,
                    execution_id: "execution-native".into(),
                    request_id: None,
                    sequence: 0,
                    phase: echo_core::agent::AgentPhase::Thinking,
                    status: echo_core::agent::ExecutionStatusKind::Started,
                    reason: None,
                }),
                AgentEvent::FinalAnswer("native done".into()),
                AgentEvent::ExecutionStatus(echo_core::agent::ExecutionStatus {
                    adapter_generated: false,
                    execution_id: "execution-native".into(),
                    request_id: None,
                    sequence: 1,
                    phase: echo_core::agent::AgentPhase::Terminal,
                    status: echo_core::agent::ExecutionStatusKind::Settled,
                    reason: None,
                }),
            ]
        });
        let sink = RecordingSink {
            close_after: Some(2),
            ..Default::default()
        };
        let receipt = AgentTurnDriver
            .drive(
                &agent,
                TurnRequest::new(identity("native-closed")?, "run"),
                &sink,
            )
            .await?;
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Closed);
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            receipt.last_event_sequence, 2,
            "未交付的status不能冒称journal已提交"
        );
        assert_eq!(
            sink.payloads
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            2
        );
        Ok(())
    }

    #[tokio::test]
    async fn execute_mode_maps_cancelled_terminal() -> echo_core::error::Result<()> {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                AgentEvent::Cancelled,
                native_status("scripted", ExecutionStatusKind::Cancelled, 1),
            ]
        }));
        let sink = RecordingSink::default();
        let request = TurnRequest::new(identity("cancel")?, "task").mode(TurnMode::Execute);
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &sink)
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.status(), "cancelled");
        assert!(receipt.final_answer.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn error_terminal_maps_typed_failure() -> echo_core::error::Result<()> {
        let failure = AgentFailure::try_from(&ReactError::Other("boom".to_string()))?;
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                AgentEvent::Error {
                    source: "test".to_string(),
                    message: "boom".to_string(),
                    failure: AgentFailure::message("other", "boom"),
                },
                native_status("scripted", ExecutionStatusKind::Failed, 1),
            ]
        }));
        let request = TurnRequest::new(identity("fail")?, "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Failed(failure));
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.status(), "failed");
        Ok(())
    }

    #[tokio::test]
    async fn usage_accumulates_across_provider_calls() -> echo_core::error::Result<()> {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                usage_event(100, 40),
                usage_event(7, 3),
                AgentEvent::FinalAnswer("summed".to_string()),
                native_status("scripted", ExecutionStatusKind::Settled, 1),
            ]
        }));
        let request = TurnRequest::new(identity("usage")?, "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await?;
        assert_eq!(receipt.prompt_tokens, 107);
        assert_eq!(receipt.completion_tokens, 43);
        assert_eq!(receipt.llm_calls, 2);
        Ok(())
    }

    #[tokio::test]
    async fn receipt_owns_final_message_and_compaction_facts() -> Result<(), String> {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
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
                native_status("scripted", ExecutionStatusKind::Settled, 1),
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
            .await
            .map_err(|error| error.to_string())?;

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
    async fn failed_terminal_delivery_keeps_final_completion_facts() -> Result<(), String> {
        for at_projection in [false, true] {
            let agent = ScriptedAgent::new(|| {
                vec![
                    native_status("scripted", ExecutionStatusKind::Started, 0),
                    AgentEvent::FinalAnswer("undelivered".into()),
                    native_status("scripted", ExecutionStatusKind::Settled, 1),
                ]
            });
            let id = EventIdentity::for_chat(
                Some("conversation-delivery-failure".into()),
                "turn-delivery-failure",
                "message-delivery-failure",
                None,
            )
            .map_err(|error| error.to_string())?;
            let target: fn(&AgentEvent) -> bool = if at_projection {
                |event| matches!(event, AgentEvent::FinalAnswer(_))
            } else {
                AgentEvent::is_execution_terminal
            };
            let sink = SemanticSink::new(target, false);
            let receipt = AgentTurnDriver
                .drive(&agent, TurnRequest::new(id.clone(), "hello"), &sink)
                .await
                .map_err(|error| error.to_string())?;
            assert_eq!(receipt.outcome, TurnOutcome::Completed);
            assert_eq!(
                receipt.delivery,
                TurnDeliveryOutcome::Failed(AgentFailure::message(
                    "other",
                    "injected durable sink failure"
                ))
            );
            assert_eq!(receipt.final_answer.as_deref(), Some("undelivered"));
            assert_eq!(
                receipt
                    .final_message_id
                    .as_ref()
                    .map(echo_core::agent::MessageId::as_str),
                Some("message-delivery-failure")
            );
            assert_eq!(sink.hits.load(Ordering::SeqCst), 1);
            let envelopes = sink
                .recording
                .envelopes
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let expected: &[&str] = if at_projection {
                &["started", "final_answer"]
            } else {
                &["started", "final_answer", "settled"]
            };
            assert_delivery_trace(&envelopes, &id, 0, expected)
                .map_err(|error| error.to_string())?;
            assert_eq!(
                receipt.last_event_sequence,
                if at_projection { 2 } else { 3 },
                "末尾是尝试投递，Failed 不能冒称持久提交"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_terminal_delivery_keeps_execution_terminal() -> echo_core::error::Result<()>
    {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                AgentEvent::Cancelled,
                native_status("scripted", ExecutionStatusKind::Cancelled, 1),
            ]
        }));
        let receipt = AgentTurnDriver
            .drive(
                agent.as_ref(),
                TurnRequest::new(identity("cancel-delivery-failure")?, "hello"),
                &FailingSink,
            )
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert!(matches!(receipt.delivery, TurnDeliveryOutcome::Failed(_)));
        Ok(())
    }

    #[tokio::test]
    async fn producer_error_delivery_failure_keeps_original_failure() -> echo_core::error::Result<()>
    {
        for before_error in [false, true] {
            let producer_failure =
                AgentFailure::try_from(&ReactError::Other("producer boom".to_string()))?;
            let agent = ScriptedAgent::new(|| {
                vec![
                    native_status("scripted", ExecutionStatusKind::Started, 0),
                    AgentEvent::Error {
                        source: "producer".into(),
                        message: "producer boom".into(),
                        failure: AgentFailure::message("other", "producer boom"),
                    },
                    native_status("scripted", ExecutionStatusKind::Failed, 1),
                ]
            });
            let id = identity("error-delivery-failure")?;
            let target: fn(&AgentEvent) -> bool = if before_error {
                |event| {
                    matches!(event, AgentEvent::ExecutionStatus(status)
                if status.status == echo_core::agent::ExecutionStatusKind::Started)
                }
            } else {
                |event| {
                    matches!(event, AgentEvent::Error { source, message, .. }
                if source == "producer" && message == "producer boom")
                }
            };
            let sink = SemanticSink::new(target, false);
            let receipt = AgentTurnDriver
                .drive(&agent, TurnRequest::new(id.clone(), "hello"), &sink)
                .await?;
            assert_eq!(
                receipt.outcome,
                TurnOutcome::Failed(producer_failure.clone())
            );
            assert_eq!(
                receipt.delivery,
                TurnDeliveryOutcome::Failed(AgentFailure::message(
                    "other",
                    "injected durable sink failure"
                ))
            );
            assert!(receipt.final_answer.is_none());
            assert_eq!(sink.hits.load(Ordering::SeqCst), 1);
            let envelopes = sink
                .recording
                .envelopes
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let expected: &[&str] = if before_error {
                &["started"]
            } else {
                &["started", "error"]
            };
            assert_delivery_trace(&envelopes, &id, 0, expected)?;
            if !before_error {
                assert!(
                    matches!(envelopes.last().map(|event| &event.payload), Some(AgentEvent::Error { failure, .. }) if failure == &producer_failure)
                );
            }
            assert_eq!(
                receipt.last_event_sequence,
                if before_error { 1 } else { 2 },
                "raw drain 的末尾状态未试投 sink，不能增加投递序号"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn resumed_sequencing_continues_after_persisted_sequence() -> echo_core::error::Result<()>
    {
        let agent = ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                AgentEvent::Cancelled,
                native_status("scripted", ExecutionStatusKind::Cancelled, 1),
            ]
        });
        let sink = RecordingSink::default();
        let id = identity("resume")?;
        let receipt = AgentTurnDriver
            .drive(
                &agent,
                TurnRequest::new(id.clone(), "hello").last_persisted_sequence(41),
                &sink,
            )
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert!(receipt.final_answer.is_none());
        let envelopes = sink
            .envelopes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_delivery_trace(
            &envelopes,
            &id,
            41,
            &["started", "cancelled_projection", "runtime_cancelled"],
        )?;
        assert_eq!(receipt.last_event_sequence, 44);
        assert_eq!(
            *sink
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![42, 43, 44]
        );
        Ok(())
    }

    #[tokio::test]
    async fn closed_sink_cancels_and_yields_cancelled_terminal() -> echo_core::error::Result<()> {
        let agent: Arc<dyn Agent> = Arc::new(
            ScriptedAgent::new(|| {
                vec![
                    native_status("scripted", ExecutionStatusKind::Started, 0),
                    commentary("before cancellation".to_string()),
                ]
            })
            .with_delay(StdDuration::from_millis(5))
            .with_cancelled_tail(),
        );
        let sink = RecordingSink {
            close_after: Some(1),
            ..RecordingSink::default()
        };
        let request = TurnRequest::new(identity("closed")?, "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &sink)
            .await?;
        // sink 关闭后，同一个脚本 source 观察原 token 并发布自己的 Cancelled。
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Closed);
        Ok(())
    }

    #[tokio::test]
    async fn terminal_closed_sink_keeps_completed_execution() -> echo_core::error::Result<()> {
        let agent = ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                AgentEvent::FinalAnswer("done".into()),
                native_status("scripted", ExecutionStatusKind::Settled, 1),
            ]
        });
        let id = identity("terminal-closed")?;
        let sink = SemanticSink::new(AgentEvent::is_execution_terminal, true);
        let receipt = AgentTurnDriver
            .drive(&agent, TurnRequest::new(id.clone(), "hello"), &sink)
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Closed);
        assert_eq!(receipt.final_answer.as_deref(), Some("done"));
        assert_eq!(sink.hits.load(Ordering::SeqCst), 1);
        let envelopes = sink
            .recording
            .envelopes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_delivery_trace(&envelopes, &id, 0, &["started", "final_answer", "settled"])?;
        assert_eq!(receipt.last_event_sequence, 3);
        Ok(())
    }

    struct ObservationPhysicalSource {
        effect: std::path::PathBuf,
        observation_consumed: tokio::sync::Notify,
        terminal_consumed: tokio::sync::Notify,
        has_terminal: bool,
    }

    struct ObservationResource {
        _permit: tokio::sync::OwnedSemaphorePermit,
        drops: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Drop for ObservationResource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct ObservationAgent {
        physical: Option<Arc<ObservationPhysicalSource>>,
        setup_observation: bool,
        stream_observation: bool,
        release_terminal: Arc<tokio::sync::Notify>,
        release_eof: Arc<tokio::sync::Notify>,
        raw_closed: Arc<std::sync::atomic::AtomicBool>,
    }

    fn pending_observation() -> ReactError {
        ReactError::ExecutionNotSettled {
            run_id: "original-observed-run".to_string(),
            execution_id: Some("original-observed-attempt".to_string()),
        }
    }

    impl Agent for ObservationAgent {
        fn name(&self) -> &str {
            "observation-agent"
        }
        fn model_name(&self) -> &str {
            "test-model"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                if self.setup_observation {
                    return Err(pending_observation());
                }
                let stream = async_stream::stream! {
                    let _retained_input_and_resources = (message, invocation);
                    yield Ok(native_status("original-observed-attempt", ExecutionStatusKind::Started, 0));
                    if let Some(physical) = self.physical.as_ref()
                        && let Err(error) = tokio::fs::write(&physical.effect, b"original-source-entered").await
                    { yield Err(ReactError::Other(error.to_string())); return; }
                    if self.stream_observation { yield Err(pending_observation()); }
                    else { yield Ok(commentary("before observation".to_string())); }
                    if let Some(physical) = self.physical.as_ref() { physical.observation_consumed.notify_one(); }
                    self.release_terminal.notified().await;
                    // has_terminal 是 fixture 声明的真实 source 事实，不由 final 文本推断。
                    if self.physical.as_ref().is_none_or(|physical| physical.has_terminal) {
                        if cancel.is_cancelled() {
                            yield Ok(AgentEvent::Cancelled);
                            yield Ok(native_status("original-observed-attempt", ExecutionStatusKind::Cancelled, 1));
                        } else {
                            yield Ok(AgentEvent::FinalAnswer("later terminal".to_string()));
                            yield Ok(native_status("original-observed-attempt", ExecutionStatusKind::Settled, 1));
                        }
                    }
                    if let Some(physical) = self.physical.as_ref() { physical.terminal_consumed.notify_one(); }
                    self.release_eof.notified().await;
                    if let Some(physical) = self.physical.as_ref()
                        && let Err(error) = tokio::fs::write(&physical.effect, b"original-source-physically-closed").await
                    { yield Err(ReactError::Other(error.to_string())); return; }
                    self.raw_closed.store(true, Ordering::SeqCst);
                };
                Ok(stream.boxed())
            })
        }
    }

    async fn assert_input_observation(
        input: &TurnInputReceipt,
        expected_run: &str,
        expected_execution: Option<&str>,
    ) {
        let mut accepted = input.clone();
        let mut drained = input.clone();
        let mut settled = input.clone();
        for result in [
            futures::poll!(Box::pin(accepted.wait_for_accepted()).as_mut()),
            futures::poll!(Box::pin(drained.wait_for_drained()).as_mut()),
            futures::poll!(Box::pin(settled.wait_for_turn_settled()).as_mut()),
        ] {
            assert!(matches!(result,
                std::task::Poll::Ready(Err(ReactError::ExecutionNotSettled { run_id, execution_id }))
                if run_id == expected_run && execution_id.as_deref() == expected_execution));
        }
    }

    struct ObservationSink;

    #[async_trait]
    impl EventSink for ObservationSink {
        async fn on_event(
            &self,
            _envelope: EventEnvelope,
        ) -> echo_core::error::Result<SinkControl> {
            Err(pending_observation())
        }
    }

    #[tokio::test]
    async fn matching_live_source_terminal_and_eof_settle_the_original_input()
    -> echo_core::error::Result<()> {
        assert_live_source_closure(true, true, false, false).await
    }

    #[tokio::test]
    async fn matching_live_source_after_cancel_preserves_cancel_cause_until_physical_eof()
    -> echo_core::error::Result<()> {
        assert_live_source_closure(true, true, true, false).await
    }

    #[tokio::test]
    async fn matching_live_source_without_terminal_cannot_settle_from_eof_or_cancel()
    -> echo_core::error::Result<()> {
        for cancel in [false, true] {
            assert_live_source_closure(false, true, cancel, false).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn live_source_terminal_cannot_clear_foreign_or_sink_observation()
    -> echo_core::error::Result<()> {
        assert_live_source_closure(true, false, false, false).await?;
        assert_live_source_closure(true, true, false, true).await
    }

    async fn assert_live_source_closure(
        has_terminal: bool,
        matching_identity: bool,
        cancel_after_observation: bool,
        sink_observation: bool,
    ) -> echo_core::error::Result<()> {
        let directory =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let physical = Arc::new(ObservationPhysicalSource {
            effect: directory.path().join("original-source-effect.txt"),
            observation_consumed: tokio::sync::Notify::new(),
            terminal_consumed: tokio::sync::Notify::new(),
            has_terminal,
        });
        let agent = Arc::new(ObservationAgent {
            physical: Some(physical.clone()),
            setup_observation: false,
            stream_observation: true,
            release_terminal: Arc::new(tokio::sync::Notify::new()),
            release_eof: Arc::new(tokio::sync::Notify::new()),
            raw_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let invocation = AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                run_id: Some("original-observed-run".to_string()),
                execution_id: Some("original-observed-attempt".to_string()),
                ..Default::default()
            }),
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                ObservationResource {
                    _permit: permit,
                    drops: drops.clone(),
                },
            )],
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        let (request, mut input) = TurnRequest::new(
            identity("matching-live-source")?
                .with_run_id("original-observed-run")?
                .with_execution_id(if matching_identity {
                    "original-observed-attempt"
                } else {
                    "different-attempt"
                })?,
            "task",
        )
        .mode(TurnMode::Execute)
        .cancel(cancel.clone())
        .invocation(invocation)
        .with_input_receipt();
        let sink = Arc::new(ObservedTerminalSink::default());
        let owned_agent = agent.clone();
        let owned_sink: Arc<dyn EventSink> = if sink_observation {
            Arc::new(ObservationSink)
        } else {
            sink.clone()
        };
        let mut drive = Box::pin(async move {
            AgentTurnDriver
                .drive(owned_agent.as_ref(), request, owned_sink.as_ref())
                .await
        });
        let observed = tokio::time::timeout(
            StdDuration::from_secs(2),
            async {
                tokio::select! {
                    result = drive.as_mut() => Err(ReactError::Other(format!("source completed before observation: {result:?}"))),
                    _ = physical.observation_consumed.notified() => Ok(()),
                }
            },
        )
        .await;
        let held = (
            futures::poll!(drive.as_mut()).is_pending(),
            capacity.available_permits(),
            drops.load(Ordering::SeqCst),
            input.state(),
        );
        let held_effect = tokio::fs::read(&physical.effect).await;
        let observed_input = futures::poll!(Box::pin(input.wait_for_turn_settled()).as_mut());
        if cancel_after_observation {
            cancel.cancel();
            // Poll this exact owner while its source terminal remains held,
            // admitting cancellation before making either branch ready.
            assert!(futures::poll!(drive.as_mut()).is_pending());
        }
        agent.release_terminal.notify_one();
        let terminal = tokio::time::timeout(
            StdDuration::from_secs(2),
            async {
                tokio::select! {
                    result = drive.as_mut() => Err(ReactError::Other(format!("source completed before physical EOF: {result:?}"))),
                    _ = physical.terminal_consumed.notified() => Ok(()),
                }
            },
        )
        .await;
        let after_terminal = (
            futures::poll!(drive.as_mut()).is_pending(),
            capacity.available_permits(),
            drops.load(Ordering::SeqCst),
            input.state(),
        );
        // Always open the original EOF barrier and join this same drive before
        // any assertion. No re-execution substitutes for a physical receipt.
        agent.release_eof.notify_one();
        let result = tokio::time::timeout(StdDuration::from_secs(2), drive)
            .await
            .map_err(|_| {
                ReactError::Other("original live-source drive did not close".to_string())
            })?;
        assert!(matches!(observed, Ok(Ok(()))));
        assert!(matches!(terminal, Ok(Ok(()))));
        assert_eq!(held, (true, 0, 0, TurnInputState::Accepted));
        assert_eq!(after_terminal, (true, 0, 0, TurnInputState::Accepted));
        assert_eq!(
            held_effect.map_err(|error| ReactError::Other(error.to_string()))?,
            b"original-source-entered"
        );
        assert!(
            matches!(observed_input, std::task::Poll::Ready(Err(ReactError::ExecutionNotSettled { run_id, execution_id }))
            if run_id == "original-observed-run" && execution_id.as_deref() == Some("original-observed-attempt"))
        );
        assert!(agent.raw_closed.load(Ordering::SeqCst));
        assert_eq!(
            tokio::fs::read(&physical.effect)
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?,
            b"original-source-physically-closed"
        );
        assert_eq!(capacity.available_permits(), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        eprintln!(
            "LIVE_SOURCE_SETTLEMENT_FACTS {}",
            serde_json::json!({
                "run_id": "original-observed-run", "execution_id": "original-observed-attempt",
                "held_pending": held.0, "held_capacity": held.1, "held_drops": held.2,
                "terminal_pending": after_terminal.0, "terminal_capacity": after_terminal.1,
                "terminal_drops": after_terminal.2,
                "physical_source_closed": agent.raw_closed.load(Ordering::SeqCst),
                "final_capacity": capacity.available_permits(), "final_drops": drops.load(Ordering::SeqCst),
                "driver_result": format!("{result:?}")
            })
        );
        if !has_terminal || !matching_identity || sink_observation {
            assert!(
                matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, ref execution_id })
                if run_id == "original-observed-run" && execution_id.as_deref() == Some("original-observed-attempt"))
            );
            assert_eq!(input.state(), TurnInputState::Accepted);
            return Ok(());
        }
        let expected_outcome = if cancel_after_observation {
            TurnOutcome::Cancelled
        } else {
            TurnOutcome::Completed
        };
        let expected_input = if cancel_after_observation {
            AgentSteerTurnOutcome::Cancelled
        } else {
            AgentSteerTurnOutcome::Completed
        };
        let receipt = result.map_err(|error| {
            ReactError::Other(format!(
                "same-owner real terminal and physical EOF remained sticky: {error:?}"
            ))
        })?;
        assert_eq!(receipt.outcome, expected_outcome);
        assert_eq!(
            receipt.final_answer.as_deref(),
            (!cancel_after_observation).then_some("later terminal")
        );
        assert!(matches!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled { outcome, .. } if outcome == expected_input
        ));
        assert_eq!(
            *sink
                .terminals
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![if cancel_after_observation {
                "cancelled"
            } else {
                "completed"
            }]
        );
        Ok(())
    }

    #[tokio::test]
    async fn setup_observation_has_no_failed_receipt_or_event() -> echo_core::error::Result<()> {
        let agent = ObservationAgent {
            physical: None,
            setup_observation: true,
            stream_observation: false,
            release_terminal: Arc::new(tokio::sync::Notify::new()),
            release_eof: Arc::new(tokio::sync::Notify::new()),
            raw_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let sink = RecordingSink::default();
        let (request, input) = TurnRequest::new(identity("setup-observation")?, "task")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let result = AgentTurnDriver.drive(&agent, request, &sink).await;
        assert!(
            matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, ref execution_id })
            if run_id == "original-observed-run" && execution_id.as_deref() == Some("original-observed-attempt"))
        );
        assert!(
            sink.events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
        assert_eq!(input.state(), TurnInputState::Accepted);
        assert_input_observation(
            &input,
            "original-observed-run",
            Some("original-observed-attempt"),
        )
        .await;
        Ok(())
    }

    #[tokio::test]
    async fn stream_and_sink_observations_retain_raw_owner_through_terminal_until_eof()
    -> echo_core::error::Result<()> {
        for (stream_observation, retain_invocation) in
            [(true, false), (false, false), (true, true), (false, true)]
        {
            let agent = ObservationAgent {
                physical: None,
                setup_observation: false,
                stream_observation,
                release_terminal: Arc::new(tokio::sync::Notify::new()),
                release_eof: Arc::new(tokio::sync::Notify::new()),
                raw_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };
            let sink = ObservationSink;
            let request =
                TurnRequest::new(identity("held-observation")?, "task").mode(TurnMode::Execute);
            // 默认 invocation 与显式 invocation 都保留同一 producer，直到原 EOF。
            let request = if retain_invocation {
                request.invocation(AgentInvocationContext::default())
            } else {
                request
            };
            let mut drive = Box::pin(AgentTurnDriver.drive(&agent, request, &sink));
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert!(!agent.raw_closed.load(Ordering::SeqCst));
            agent.release_terminal.notify_one();
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert!(!agent.raw_closed.load(Ordering::SeqCst));
            agent.release_eof.notify_one();
            let result = drive.await;
            assert!(agent.raw_closed.load(Ordering::SeqCst));
            assert!(
                matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, ref execution_id })
                if run_id == "original-observed-run" && execution_id.as_deref() == Some("original-observed-attempt"))
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn input_observation_wakes_all_waits_while_original_cleanup_is_held()
    -> echo_core::error::Result<()> {
        for stream_observation in [true, false] {
            let agent = ObservationAgent {
                physical: None,
                setup_observation: false,
                stream_observation,
                release_terminal: Arc::new(tokio::sync::Notify::new()),
                release_eof: Arc::new(tokio::sync::Notify::new()),
                raw_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };
            let sink = ObservationSink;
            let (request, input) = TurnRequest::new(identity("held-input-observation")?, "task")
                .mode(TurnMode::Execute)
                .with_input_receipt();
            let mut drive = Box::pin(AgentTurnDriver.drive(&agent, request, &sink));
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert_eq!(input.state(), TurnInputState::Accepted);
            assert_input_observation(
                &input,
                "original-observed-run",
                Some("original-observed-attempt"),
            )
            .await;
            assert!(!agent.raw_closed.load(Ordering::SeqCst));
            agent.release_terminal.notify_one();
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert_eq!(input.state(), TurnInputState::Accepted);
            assert_input_observation(
                &input,
                "original-observed-run",
                Some("original-observed-attempt"),
            )
            .await;
            agent.release_eof.notify_one();
            assert!(matches!(
                drive.await,
                Err(ReactError::ExecutionNotSettled { .. })
            ));
            assert!(agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(input.state(), TurnInputState::Accepted);
            assert_input_observation(
                &input,
                "original-observed-run",
                Some("original-observed-attempt"),
            )
            .await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn closed_input_watch_preserves_pending_and_drained_without_terminal_synthesis()
    -> echo_core::error::Result<()> {
        for drained in [false, true] {
            let identity = identity("closed-input")?
                .with_run_id("original-closed-run")?
                .with_execution_id("original-closed-attempt")?;
            let (input, lifecycle) = TurnInputReceipt::new(&identity);
            if drained {
                lifecycle.mark_drained();
            }
            drop(lifecycle);
            assert_eq!(
                input.state(),
                if drained {
                    TurnInputState::Drained
                } else {
                    TurnInputState::Pending
                }
            );
            assert_input_observation(
                &input,
                "original-closed-run",
                Some("original-closed-attempt"),
            )
            .await;
        }
        Ok(())
    }

    #[derive(Default)]
    struct ObservedTerminalSink {
        terminals: Mutex<Vec<&'static str>>,
    }

    #[async_trait]
    impl EventSink for ObservedTerminalSink {
        async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            let terminal = match envelope.payload {
                AgentEvent::ExecutionStatus(status)
                    if status.status == ExecutionStatusKind::Cancelled =>
                {
                    Some("cancelled")
                }
                AgentEvent::ExecutionStatus(status)
                    if status.status == ExecutionStatusKind::Settled =>
                {
                    Some("completed")
                }
                _ => None,
            };
            if let Some(terminal) = terminal {
                self.terminals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(terminal);
            }
            Ok(SinkControl::Continue)
        }
    }

    #[tokio::test]
    async fn native_source_terminal_delivery_waits_for_original_raw_eof_before_receipt()
    -> echo_core::error::Result<()> {
        for cancelled in [true, false] {
            let agent = ObservationAgent {
                physical: None,
                setup_observation: false,
                stream_observation: false,
                release_terminal: Arc::new(tokio::sync::Notify::new()),
                release_eof: Arc::new(tokio::sync::Notify::new()),
                raw_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            };
            let sink = RecordingSink::default();
            let cancel = CancellationToken::new();
            let id = identity("held-terminal")?;
            let (invocation, capacity, drops) = guarded_invocation().await?;
            let (request, input) = TurnRequest::new(id.clone(), "task")
                .mode(TurnMode::Execute)
                .cancel(cancel.clone())
                .invocation(invocation)
                .with_input_receipt();
            if !cancelled {
                agent.release_terminal.notify_one();
            }
            let mut drive = Box::pin(AgentTurnDriver.drive(&agent, request, &sink));
            assert!(futures::poll!(drive.as_mut()).is_pending());
            if cancelled {
                cancel.cancel();
                assert!(futures::poll!(drive.as_mut()).is_pending());
            }
            let expected_status = if cancelled { "cancelled" } else { "completed" };
            // 原生 source 可以发布正文和 terminal；receipt 仍必须等原 EOF。
            let held_trace: &[&str] = if cancelled {
                &["started", "assistant"]
            } else {
                &["started", "assistant", "final_answer", "settled"]
            };
            assert_delivery_trace(
                &sink
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                &id,
                0,
                held_trace,
            )?;
            assert!(!agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(capacity.available_permits(), 0);
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            assert!(!matches!(input.state(), TurnInputState::TurnSettled { .. }));
            if cancelled {
                agent.release_terminal.notify_one();
                assert!(futures::poll!(drive.as_mut()).is_pending());
                assert!(!agent.raw_closed.load(Ordering::SeqCst));
                assert_eq!(capacity.available_permits(), 0);
                assert_eq!(drops.load(Ordering::SeqCst), 0);
                assert!(!matches!(input.state(), TurnInputState::TurnSettled { .. }));
            }
            agent.release_eof.notify_one();
            let receipt = drive.await?;
            assert!(agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(capacity.available_permits(), 1);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert_eq!(receipt.status(), expected_status);
            assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
            assert_eq!(receipt.last_event_sequence, 4);
            assert_eq!(
                receipt.final_answer.as_deref(),
                if cancelled {
                    None
                } else {
                    Some("later terminal")
                }
            );
            let expected = if cancelled {
                [
                    "started",
                    "assistant",
                    "cancelled_projection",
                    "runtime_cancelled",
                ]
            } else {
                ["started", "assistant", "final_answer", "settled"]
            };
            assert_delivery_trace(
                &sink
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                &id,
                0,
                &expected,
            )?;
            assert!(
                matches!(input.state(), TurnInputState::TurnSettled { outcome, .. }
                if outcome == if cancelled { AgentSteerTurnOutcome::Cancelled } else { AgentSteerTurnOutcome::Completed })
            );
        }
        Ok(())
    }

    struct TailObservationAgent {
        terminal_execution_id: Option<&'static str>,
        release_physical_terminal: tokio::sync::Notify,
        release_terminal: tokio::sync::Notify,
        release_observation: tokio::sync::Notify,
        release_eof: tokio::sync::Notify,
        raw_closed: std::sync::atomic::AtomicBool,
    }

    impl Agent for TailObservationAgent {
        fn name(&self) -> &str {
            "tail-observation-agent"
        }
        fn model_name(&self) -> &str {
            "test-model"
        }
        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                Ok(async_stream::stream! {
                    let _retained_input_and_resources = (message, invocation);
                    yield Ok(native_status("original-observed-attempt", ExecutionStatusKind::Started, 0));
                    yield Ok(commentary("before terminal".to_string()));
                    self.release_terminal.notified().await;
                    yield Ok(AgentEvent::FinalAnswer("original terminal".to_string()));
                    self.release_observation.notified().await;
                    yield Err(pending_observation());
                    if let Some(execution) = self.terminal_execution_id {
                        self.release_physical_terminal.notified().await;
                        yield Ok(native_status(execution, echo_core::agent::ExecutionStatusKind::Settled, 1));
                    }
                    self.release_eof.notified().await;
                    self.raw_closed.store(true, Ordering::SeqCst);
                }
                .boxed())
            })
        }
    }

    #[tokio::test]
    async fn late_cleanup_observation_prevents_receipt_after_delivered_terminal()
    -> echo_core::error::Result<()> {
        for structured_input in [false, true] {
            for matching_identity in [false, true] {
                for cancelled in [true, false] {
                    let agent = TailObservationAgent {
                        terminal_execution_id: None,
                        release_physical_terminal: tokio::sync::Notify::new(),
                        release_terminal: tokio::sync::Notify::new(),
                        release_observation: tokio::sync::Notify::new(),
                        release_eof: tokio::sync::Notify::new(),
                        raw_closed: std::sync::atomic::AtomicBool::new(false),
                    };
                    let sink = RecordingSink::default();
                    let cancel = CancellationToken::new();
                    let id = if matching_identity {
                        identity("late-cleanup-observation")?
                            .with_run_id("original-observed-run")?
                            .with_execution_id("original-observed-attempt")?
                    } else {
                        identity("late-cleanup-observation")?
                    };
                    let (invocation, capacity, drops) = guarded_invocation().await?;
                    let initial = if structured_input {
                        TurnRequest::from_message(id.clone(), Message::user("task".to_string()))
                    } else {
                        TurnRequest::new(id.clone(), "task")
                    };
                    let (request, input) = initial
                        .mode(TurnMode::Execute)
                        .cancel(cancel.clone())
                        .invocation(invocation)
                        .with_input_receipt();
                    if !cancelled {
                        agent.release_terminal.notify_one();
                    }
                    let mut drive = Box::pin(AgentTurnDriver.drive(&agent, request, &sink));
                    assert!(futures::poll!(drive.as_mut()).is_pending());
                    if cancelled {
                        cancel.cancel();
                        assert!(futures::poll!(drive.as_mut()).is_pending());
                        agent.release_terminal.notify_one();
                        assert!(futures::poll!(drive.as_mut()).is_pending());
                    }
                    // 两种输入都调用同一原生 source，确认正文不代表物理完成。
                    let expected = vec!["started", "assistant", "final_answer"];
                    assert_delivery_trace(
                        &sink
                            .envelopes
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()),
                        &id,
                        0,
                        &expected,
                    )?;
                    assert_eq!(input.state(), TurnInputState::Accepted);
                    assert!(!agent.raw_closed.load(Ordering::SeqCst));
                    assert_eq!(capacity.available_permits(), 0);
                    assert_eq!(drops.load(Ordering::SeqCst), 0);
                    agent.release_observation.notify_one();
                    assert!(futures::poll!(drive.as_mut()).is_pending());
                    assert_eq!(input.state(), TurnInputState::Accepted);
                    assert_input_observation(
                        &input,
                        "original-observed-run",
                        Some("original-observed-attempt"),
                    )
                    .await;
                    assert!(!agent.raw_closed.load(Ordering::SeqCst));
                    assert_eq!(capacity.available_permits(), 0);
                    assert_eq!(drops.load(Ordering::SeqCst), 0);
                    agent.release_eof.notify_one();
                    assert!(
                        matches!(drive.await, Err(ReactError::ExecutionNotSettled { run_id, execution_id })
                        if run_id == "original-observed-run" && execution_id.as_deref() == Some("original-observed-attempt"))
                    );
                    assert!(agent.raw_closed.load(Ordering::SeqCst));
                    assert_eq!(capacity.available_permits(), 1);
                    assert_eq!(drops.load(Ordering::SeqCst), 1);
                    assert_eq!(input.state(), TurnInputState::Accepted);
                    assert_input_observation(
                        &input,
                        "original-observed-run",
                        Some("original-observed-attempt"),
                    )
                    .await;
                    assert_delivery_trace(
                        &sink
                            .envelopes
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()),
                        &id,
                        0,
                        &expected,
                    )?;
                }
            }
        }
        Ok(())
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
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move { Err(ReactError::Other("stream could not start".to_string())) })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move { Err(ReactError::Other("stream could not start".to_string())) })
        }
    }

    struct CancelledStartAgent;

    impl Agent for CancelledStartAgent {
        fn name(&self) -> &str {
            "cancelled-start"
        }
        fn model_name(&self) -> &str {
            "test-model"
        }
        fn system_prompt(&self) -> &str {
            ""
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
            Box::pin(async move {
                Err(echo_core::error::AgentError::Cancelled(
                    "cancelled before stream start".to_string(),
                )
                .into())
            })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            _message: Message,
            _cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                Err(echo_core::error::AgentError::Cancelled(
                    "cancelled before stream start".to_string(),
                )
                .into())
            })
        }
    }

    #[tokio::test]
    async fn stream_start_failure_maps_to_failed_receipt() -> echo_core::error::Result<()> {
        let agent: Arc<dyn Agent> = Arc::new(FailingAgent);
        let request = TurnRequest::new(identity("startfail")?, "hello");
        let sink = RecordingSink::default();
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &sink)
            .await?;
        assert_eq!(receipt.outcome.status(), "failed");
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.last_event_sequence, 1);
        assert_eq!(
            *sink
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![1]
        );
        Ok(())
    }

    #[tokio::test]
    async fn stream_start_cancellation_maps_to_cancelled_receipt() -> echo_core::error::Result<()> {
        let (request, mut input) = TurnRequest::new(identity("start-cancelled")?, "hello")
            .mode(TurnMode::Execute)
            .with_input_receipt();
        let sink = RecordingSink::default();
        let receipt = AgentTurnDriver
            .drive(&CancelledStartAgent, request, &sink)
            .await?;

        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.last_event_sequence, 1);
        assert_eq!(
            input.wait_for_turn_settled().await?,
            TurnInputState::TurnSettled {
                outcome: AgentSteerTurnOutcome::Cancelled,
                drained: false,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn stream_start_delivery_failure_keeps_original_failure() -> echo_core::error::Result<()>
    {
        let receipt = AgentTurnDriver
            .drive(
                &FailingAgent,
                TurnRequest::new(identity("start-delivery-failure")?, "hello"),
                &FailingSink,
            )
            .await?;
        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert!(matches!(receipt.delivery, TurnDeliveryOutcome::Failed(_)));
        assert_eq!(receipt.last_event_sequence, 1);
        Ok(())
    }

    struct FailingSink;

    #[async_trait]
    impl EventSink for FailingSink {
        async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            // 三个既有调用方检查物理终态或 setup Error 的投递失败；
            // 新 Started/正文不能提前触发故障。正文失败由 SemanticSink 单独覆盖。
            let setup_error = matches!(&envelope.payload, AgentEvent::Error { source, .. } if source == "turn_driver");
            if envelope.payload.is_execution_terminal() || setup_error {
                Err(ReactError::Other(
                    "injected durable sink failure".to_string(),
                ))
            } else {
                Ok(SinkControl::Continue)
            }
        }
    }

    #[tokio::test]
    async fn sink_failure_is_failed_not_cancelled() -> echo_core::error::Result<()> {
        let cancel = CancellationToken::new();
        let agent = CancelledTailAgent {
            typed_error_tail: true,
            cancel: cancel.clone(),
            release_eof: tokio::sync::Notify::new(),
            cancel_seen: std::sync::atomic::AtomicBool::new(false),
            typed_cancel_emitted: std::sync::atomic::AtomicBool::new(false),
            raw_closed: std::sync::atomic::AtomicBool::new(false),
            slots: Arc::new(tokio::sync::Semaphore::new(1)),
            drops: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            original_status: Mutex::new(None),
        };
        agent.release_eof.notify_one();
        let id = identity("sink-failure")?;
        let sink = SemanticSink::new(
            |event| matches!(event, AgentEvent::AssistantMessage { block } if block.delta == "partial"),
            false,
        );
        let receipt = AgentTurnDriver
            .drive(
                &agent,
                TurnRequest::new(id.clone(), "hello")
                    .mode(TurnMode::Execute)
                    .cancel(cancel.clone()),
                &sink,
            )
            .await?;
        let failure = AgentFailure::message("other", "injected durable sink failure");
        assert_eq!(receipt.outcome, TurnOutcome::Failed(failure.clone()));
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Failed(failure));
        assert!(receipt.final_answer.is_none());
        assert_eq!(receipt.last_event_sequence, 2);
        assert_eq!(sink.hits.load(Ordering::SeqCst), 1);
        assert!(cancel.is_cancelled());
        assert!(agent.cancel_seen.load(Ordering::SeqCst));
        assert!(agent.typed_cancel_emitted.load(Ordering::SeqCst));
        assert!(agent.raw_closed.load(Ordering::SeqCst));
        assert_eq!(agent.slots.available_permits(), 1);
        assert_eq!(agent.drops.load(Ordering::SeqCst), 1);
        assert!(
            agent
                .original_status
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_none()
        );
        assert_delivery_trace(
            &sink
                .recording
                .envelopes
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            &id,
            0,
            &["started", "assistant"],
        )?;
        Ok(())
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
                    .unwrap_or_else(|error| error.into_inner()) = invocation.working_dir.clone();
                Ok(async_stream::stream! {
                    let _retained_input_and_resources = (message, invocation);
                    yield Ok(native_status("invocation-recording", ExecutionStatusKind::Started, 0));
                    yield Ok(AgentEvent::FinalAnswer("message-ok".to_string()));
                    yield Ok(native_status("invocation-recording", ExecutionStatusKind::Settled, 1));
                }.boxed())
            })
        }
    }

    #[test]
    fn boxed_message_input_preserves_serialized_message_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        use echo_core::llm::types::{ContentPart, LinkedResource};
        let mut message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: "原始输入🙂".into(),
            },
            ContentPart::ResourceLink {
                resource: Box::new(LinkedResource {
                    annotations: None,
                    description: None,
                    mime_type: Some("text/plain".into()),
                    name: "source".into(),
                    size: Some(12),
                    title: None,
                    uri: "echo-tool-output:sha256:original".into(),
                    meta: Some(serde_json::json!({"opaque": [null, true, "中文"]})),
                }),
            },
        ]);
        message.assistant_blocks = Some(Vec::new());
        message.approved_assistant_blocks = Some(Vec::new());
        let expected = serde_json::to_value(&message)?;
        let request = TurnRequest::from_message(identity("boxed-message-payload")?, message);
        let TurnInput::Message(boxed) = request.input else {
            return Err("structured input lost its message payload".into());
        };
        // TurnInput不是Serde类型；只核真正可序列化的原Message，不伪称外层协议测试。
        let encoded = serde_json::to_value(boxed.as_ref())?;
        assert_eq!(encoded, expected);
        let restored: Message = serde_json::from_value(encoded)?;
        assert_eq!(serde_json::to_value(restored)?, expected);
        Ok(())
    }

    #[tokio::test]
    async fn structured_chat_message_preserves_invocation_context() -> echo_core::error::Result<()>
    {
        let agent = Arc::new(InvocationRecordingAgent::default());
        let expected = std::path::PathBuf::from("/tmp/eko-turn-driver-test");
        let request = TurnRequest::from_message(
            identity("message-invocation")?,
            Message::user("hello".to_string()),
        )
        .invocation(AgentInvocationContext {
            working_dir: Some(expected.clone()),
            ..AgentInvocationContext::default()
        });
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(receipt.final_answer.as_deref(), Some("message-ok"));
        assert_eq!(
            *agent
                .working_dir
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            Some(expected)
        );
        Ok(())
    }

    #[tokio::test]
    async fn text_chat_with_invocation_uses_structured_chat_contract()
    -> echo_core::error::Result<()> {
        let agent = Arc::new(InvocationRecordingAgent::default());
        let expected = std::path::PathBuf::from("/tmp/eko-text-chat-test");
        let request = TurnRequest::new(identity("text-invocation")?, "text hello").invocation(
            AgentInvocationContext {
                working_dir: Some(expected.clone()),
                ..AgentInvocationContext::default()
            },
        );

        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await?;
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
        Ok(())
    }

    #[tokio::test]
    async fn request_cancel_precedes_invocation_cancel_and_invocation_only_is_retained()
    -> echo_core::error::Result<()> {
        let agent = Arc::new(InvocationRecordingAgent::default());
        let invocation_only = CancellationToken::new();
        invocation_only.cancel();
        let first = TurnRequest::new(identity("invocation-cancel")?, "first").invocation(
            AgentInvocationContext {
                cancel: Some(invocation_only),
                ..AgentInvocationContext::default()
            },
        );
        AgentTurnDriver
            .drive(agent.as_ref(), first, &RecordingSink::default())
            .await?;

        let request_cancel = CancellationToken::new();
        let shadowed_invocation_cancel = CancellationToken::new();
        shadowed_invocation_cancel.cancel();
        let second = TurnRequest::new(identity("request-cancel")?, "second")
            .invocation(AgentInvocationContext {
                cancel: Some(shadowed_invocation_cancel),
                ..AgentInvocationContext::default()
            })
            .cancel(request_cancel);
        AgentTurnDriver
            .drive(agent.as_ref(), second, &RecordingSink::default())
            .await?;

        assert_eq!(
            *agent
                .cancel_states
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![(true, true), (false, false)]
        );
        Ok(())
    }

    struct DelayedSink {
        events: tokio::sync::Mutex<Vec<(u64, bool)>>,
        envelopes: tokio::sync::Mutex<Vec<EventEnvelope>>,
    }

    #[async_trait]
    impl EventSink for DelayedSink {
        async fn on_event(&self, envelope: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            tokio::time::sleep(StdDuration::from_millis(5)).await;
            self.events
                .lock()
                .await
                .push((envelope.sequence, envelope.payload.is_execution_terminal()));
            self.envelopes.lock().await.push(envelope);
            Ok(SinkControl::Continue)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_sink_preserves_order_and_allows_runtime_progress() -> echo_core::error::Result<()>
    {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                commentary("one".to_string()),
                commentary("two".to_string()),
                AgentEvent::FinalAnswer("done".to_string()),
                native_status("scripted", ExecutionStatusKind::Settled, 1),
            ]
        }));
        let sink = DelayedSink {
            events: tokio::sync::Mutex::new(Vec::new()),
            envelopes: tokio::sync::Mutex::new(Vec::new()),
        };
        let runtime_progressed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let progressed = Arc::clone(&runtime_progressed);
        let heartbeat = tokio::spawn(async move {
            tokio::time::sleep(StdDuration::from_millis(1)).await;
            progressed.store(true, Ordering::SeqCst);
        });

        let id = identity("async-sink")?;
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), TurnRequest::new(id.clone(), "hello"), &sink)
            .await?;
        assert!(heartbeat.await.is_ok());
        assert!(runtime_progressed.load(Ordering::SeqCst));
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            *sink.events.lock().await,
            vec![(1, false), (2, false), (3, false), (4, false), (5, true)]
        );
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        assert_eq!(receipt.final_answer.as_deref(), Some("done"));
        assert_eq!(receipt.last_event_sequence, 5);
        assert_delivery_trace(
            &sink.envelopes.lock().await,
            &id,
            0,
            &[
                "started",
                "assistant",
                "assistant",
                "final_answer",
                "settled",
            ],
        )?;
        let tokens: Vec<_> = sink
            .envelopes
            .lock()
            .await
            .iter()
            .filter_map(|event| match &event.payload {
                AgentEvent::AssistantMessage { block } => Some(block.delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tokens, ["one", "two"]);
        Ok(())
    }

    #[tokio::test]
    async fn missing_terminal_is_a_failed_receipt() -> echo_core::error::Result<()> {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                native_status("scripted", ExecutionStatusKind::Started, 0),
                commentary("partial".to_string()),
            ]
        }));
        let request = TurnRequest::new(identity("noterminal")?, "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &RecordingSink::default())
            .await?;
        assert_eq!(receipt.outcome.status(), "failed");
        assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
        Ok(())
    }

    #[tokio::test]
    async fn sink_takes_terminal_envelope_after_receipt_accounting() -> echo_core::error::Result<()>
    {
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
                native_status("scripted", ExecutionStatusKind::Started, 0),
                usage_event(7, 3),
                commentary("t".to_string()),
                AgentEvent::FinalAnswer("fin".to_string()),
                native_status("scripted", ExecutionStatusKind::Settled, 1),
            ]
        }));
        let sink = OwningSink::default();
        let id = identity("exactly")?;
        let request = TurnRequest::new(id.clone(), "hello");
        let receipt = AgentTurnDriver
            .drive(agent.as_ref(), request, &sink)
            .await?;
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
            .filter(|envelope| envelope.payload.is_execution_terminal())
            .collect();
        assert_eq!(terminals.len(), 1);
        assert!(matches!(
            terminals.first().map(|envelope| &envelope.payload),
            Some(AgentEvent::ExecutionStatus(status)) if status.status == echo_core::agent::ExecutionStatusKind::Settled
        ));
        assert_delivery_trace(
            &envelopes,
            &id,
            0,
            &["started", "usage", "assistant", "final_answer", "settled"],
        )?;
        assert_eq!(envelopes.iter().filter(|event| matches!(&event.payload, AgentEvent::FinalAnswer(answer) if answer == "fin")).count(), 1);
        assert_eq!(receipt.last_event_sequence, 5);
        Ok(())
    }

    struct CancelledTailAgent {
        typed_error_tail: bool,
        cancel: CancellationToken,
        release_eof: tokio::sync::Notify,
        cancel_seen: std::sync::atomic::AtomicBool,
        typed_cancel_emitted: std::sync::atomic::AtomicBool,
        raw_closed: std::sync::atomic::AtomicBool,
        slots: Arc<tokio::sync::Semaphore>,
        drops: Arc<std::sync::atomic::AtomicUsize>,
        original_status: Mutex<Option<echo_core::agent::ExecutionStatus>>,
    }

    impl Agent for CancelledTailAgent {
        fn name(&self) -> &str {
            "cancelled-tail-original-source"
        }
        fn model_name(&self) -> &str {
            "test-model"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                let permit = Arc::clone(&self.slots)
                    .acquire_owned()
                    .await
                    .map_err(|error| ReactError::Other(error.to_string()))?;
                let guard = ObservationResource {
                    _permit: permit,
                    drops: Arc::clone(&self.drops),
                };
                Ok(async_stream::stream! {
                    let _guard = guard;
                    let _retained_input_and_resources = (message, invocation);
                    yield Ok(native_status("native-cancel-tail", ExecutionStatusKind::Started, 0));
                    yield Ok(commentary("partial".into()));
                    self.cancel.cancelled().await;
                    self.cancel_seen.store(true, Ordering::SeqCst);
                    if !self.typed_error_tail {
                        let event = native_status("native-cancel-tail", echo_core::agent::ExecutionStatusKind::Cancelled, 1);
                        if let AgentEvent::ExecutionStatus(status) = &event {
                            *self.original_status.lock().unwrap_or_else(|error| error.into_inner()) = Some(status.clone());
                        }
                        yield Ok(event);
                    } else {
                        self.typed_cancel_emitted.store(true, Ordering::SeqCst);
                        yield Err(ReactError::Agent(Box::new(echo_core::error::AgentError::Cancelled("original source cancelled".into()))));
                    }
                    self.release_eof.notified().await;
                    self.raw_closed.store(true, Ordering::SeqCst);
                }.boxed())
            })
        }
    }

    #[tokio::test]
    async fn sink_failure_preserves_typed_error_cause_and_native_cancelled_at_original_eof()
    -> echo_core::error::Result<()> {
        for typed_error_tail in [true, false] {
            let cancel = CancellationToken::new();
            let agent = CancelledTailAgent {
                typed_error_tail,
                cancel: cancel.clone(),
                release_eof: tokio::sync::Notify::new(),
                cancel_seen: std::sync::atomic::AtomicBool::new(false),
                typed_cancel_emitted: std::sync::atomic::AtomicBool::new(false),
                raw_closed: std::sync::atomic::AtomicBool::new(false),
                slots: Arc::new(tokio::sync::Semaphore::new(1)),
                drops: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                original_status: Mutex::new(None),
            };
            let sink = SemanticSink::new(
                |event| matches!(event, AgentEvent::AssistantMessage { block } if block.delta == "partial"),
                false,
            );
            let id = identity("cancelled-tail-cause")?;
            let (request, input) = TurnRequest::new(id.clone(), "task")
                .mode(TurnMode::Execute)
                .cancel(cancel.clone())
                .with_input_receipt();
            let mut drive = Box::pin(AgentTurnDriver.drive(&agent, request, &sink));
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert!(cancel.is_cancelled());
            assert!(
                agent.cancel_seen.load(Ordering::SeqCst),
                "sink 失败必须触发同一个原 source 的 typed cancel 尾部"
            );
            assert_eq!(sink.hits.load(Ordering::SeqCst), 1);
            assert_eq!(
                agent.typed_cancel_emitted.load(Ordering::SeqCst),
                typed_error_tail
            );
            assert!(!agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(agent.slots.available_permits(), 0);
            assert_eq!(agent.drops.load(Ordering::SeqCst), 0);
            assert_eq!(input.state(), TurnInputState::Accepted);
            assert_delivery_trace(
                &sink
                    .recording
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                &id,
                0,
                &["started", "assistant"],
            )?;
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert_eq!(
                agent.drops.load(Ordering::SeqCst),
                0,
                "不能把 cancel/terminal 当物理 EOF"
            );
            agent.release_eof.notify_one();
            let receipt = drive.await?;
            let failure = AgentFailure::message("other", "injected durable sink failure");
            assert_eq!(
                receipt.outcome,
                if !typed_error_tail {
                    TurnOutcome::Cancelled
                } else {
                    TurnOutcome::Failed(failure.clone())
                }
            );
            assert_eq!(receipt.delivery, TurnDeliveryOutcome::Failed(failure));
            assert!(receipt.final_answer.is_none());
            assert!(receipt.final_message_id.is_none());
            assert_eq!(
                receipt.last_event_sequence, 2,
                "raw drain 不增加 sink 试投序号"
            );
            assert!(agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(agent.slots.available_permits(), 1);
            assert_eq!(agent.drops.load(Ordering::SeqCst), 1);
            let original = agent
                .original_status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !typed_error_tail {
                assert!(
                    matches!(original.as_ref(), Some(status) if !status.adapter_generated
                    && status.execution_id == "native-cancel-tail"
                    && status.status == echo_core::agent::ExecutionStatusKind::Cancelled)
                );
            } else {
                assert!(original.is_none());
            }
            assert!(
                matches!(input.state(), TurnInputState::TurnSettled { outcome, .. }
                if outcome == if !typed_error_tail { AgentSteerTurnOutcome::Cancelled } else { AgentSteerTurnOutcome::Failed })
            );
            assert_delivery_trace(
                &sink
                    .recording
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                &id,
                0,
                &["started", "assistant"],
            )?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn native_failure_then_tail_error_keeps_exact_first_cause_in_delivery_and_raw_drain()
    -> echo_core::error::Result<()> {
        struct NativeFailureTail {
            kind: ExecutionStatusKind,
            wait_for_cancel: bool,
            original: Mutex<Option<echo_core::agent::ExecutionStatus>>,
            eof: std::sync::atomic::AtomicBool,
        }
        impl Agent for NativeFailureTail {
            fn name(&self) -> &str {
                "native-failure-tail"
            }
            fn model_name(&self) -> &str {
                "test-model"
            }
            fn system_prompt(&self) -> &str {
                ""
            }
            fn execute_stream_message_with_invocation_context<'a>(
                &'a self,
                message: Message,
                cancel: CancellationToken,
                invocation: AgentInvocationContext,
            ) -> BoxFuture<
                'a,
                echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
            > {
                Box::pin(async move {
                    Ok(async_stream::stream! {
                        let _retained_input_and_resources = (message, invocation);
                        yield Ok(native_status("native-failure-tail", ExecutionStatusKind::Started, 0));
                        yield Ok(commentary("partial".into()));
                        if self.wait_for_cancel { cancel.cancelled().await; }
                        let mut terminal = native_status("native-failure-tail", self.kind.clone(), 1);
                        if let AgentEvent::ExecutionStatus(status) = &mut terminal {
                            status.reason = Some("original native first cause".into());
                            *self.original.lock().unwrap_or_else(|error| error.into_inner()) = Some(status.clone());
                        }
                        yield Ok(terminal);
                        yield Err(ReactError::Other("later raw tail failure".into()));
                        self.eof.store(true, Ordering::SeqCst);
                    }.boxed())
                })
            }
        }
        for kind in [ExecutionStatusKind::Cancelled, ExecutionStatusKind::Failed] {
            for detached in [false, true] {
                let agent = NativeFailureTail {
                    kind: kind.clone(),
                    wait_for_cancel: detached,
                    original: Mutex::new(None),
                    eof: std::sync::atomic::AtomicBool::new(false),
                };
                let sink = SemanticSink::new(
                    if detached {
                        |event| matches!(event, AgentEvent::AssistantMessage { block } if block.delta == "partial")
                    } else {
                        |_| false
                    },
                    false,
                );
                let cancel = CancellationToken::new();
                let id = identity("native-first-cause")?;
                let (request, input) = TurnRequest::new(id.clone(), "task")
                    .mode(TurnMode::Execute)
                    .cancel(cancel.clone())
                    .with_input_receipt();
                let receipt = AgentTurnDriver.drive(&agent, request, &sink).await?;
                assert!(agent.eof.load(Ordering::SeqCst));
                let expected = if kind == ExecutionStatusKind::Cancelled {
                    TurnOutcome::Cancelled
                } else {
                    TurnOutcome::Failed(AgentFailure::try_from(&ReactError::Other(
                        "original native first cause".into(),
                    ))?)
                };
                assert_eq!(receipt.outcome, expected);
                assert_eq!(
                    receipt.delivery,
                    if detached {
                        TurnDeliveryOutcome::Failed(AgentFailure::message(
                            "other",
                            "injected durable sink failure",
                        ))
                    } else {
                        TurnDeliveryOutcome::Delivered
                    }
                );
                assert_eq!(cancel.is_cancelled(), detached);
                assert_eq!(
                    sink.hits.load(Ordering::SeqCst),
                    if detached { 1 } else { 0 }
                );
                assert!(receipt.final_answer.is_none());
                assert!(receipt.final_message_id.is_none());
                assert_eq!(receipt.last_event_sequence, if detached { 2 } else { 4 });
                let original = agent
                    .original
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                assert!(
                    matches!(original.as_ref(), Some(status) if !status.adapter_generated
                    && status.execution_id == "native-failure-tail" && status.sequence == 1
                    && status.status == kind && status.reason.as_deref() == Some("original native first cause"))
                );
                assert!(
                    matches!(input.state(), TurnInputState::TurnSettled { outcome, drained: false }
                    if outcome == if kind == ExecutionStatusKind::Cancelled { AgentSteerTurnOutcome::Cancelled } else { AgentSteerTurnOutcome::Failed })
                );
                let envelopes = sink
                    .recording
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let expected_trace: &[&str] = if detached {
                    &["started", "assistant"]
                } else if kind == ExecutionStatusKind::Cancelled {
                    &["started", "assistant", "runtime_cancelled", "error"]
                } else {
                    &["started", "assistant", "runtime_failed", "error"]
                };
                // 此专用轨迹保留 native terminal 后的原 tail Error，不放宽通用 helper。
                assert_eq!(
                    envelopes
                        .iter()
                        .map(|event| delivery_kind(&event.payload))
                        .collect::<Vec<_>>(),
                    expected_trace
                );
                let violations = echo_core::agent::validate_envelope_trajectory(&envelopes);
                assert!(violations.is_empty(), "{violations:?}");
                assert_eq!(
                    envelopes.first().map(|event| (
                        &event.stream_id,
                        &event.turn_id,
                        &event.conversation_id,
                        &event.run_id,
                        &event.execution_id,
                        &event.message_id,
                    )),
                    Some((
                        &id.stream_id,
                        &id.turn_id,
                        &id.conversation_id,
                        &id.run_id,
                        &id.execution_id,
                        &id.message_id,
                    ))
                );
                assert_eq!(
                    envelopes
                        .iter()
                        .map(|event| event.sequence)
                        .collect::<Vec<_>>(),
                    if detached {
                        vec![1, 2]
                    } else {
                        vec![1, 2, 3, 4]
                    }
                );
                let terminals: Vec<_> = envelopes
                    .iter()
                    .filter(|event| event.payload.is_execution_terminal())
                    .collect();
                if detached {
                    assert!(terminals.is_empty());
                } else {
                    assert_eq!(terminals.len(), 1);
                    assert_eq!(terminals.first().map(|event| event.sequence), Some(3));
                    assert!(matches!(envelopes.get(2).map(|event| &event.payload),
                        Some(AgentEvent::ExecutionStatus(status)) if !status.adapter_generated
                        && status.execution_id == "native-failure-tail" && status.sequence == 1
                        && status.status == kind && status.reason.as_deref() == Some("original native first cause")));
                    let expected_tail_failure = AgentFailure::try_from(&ReactError::Other(
                        "later raw tail failure".into(),
                    ))?;
                    assert!(matches!(envelopes.get(3).map(|event| &event.payload),
                        Some(AgentEvent::Error { source, message, failure })
                        if source == "agent_stream" && message == "later raw tail failure"
                        && failure == &expected_tail_failure));
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn native_unknown_requires_matching_owner_terminal_and_original_eof()
    -> echo_core::error::Result<()> {
        for matching_owner in [false, true] {
            let agent = TailObservationAgent {
                terminal_execution_id: Some(if matching_owner {
                    "original-observed-attempt"
                } else {
                    "foreign-attempt-B"
                }),
                release_physical_terminal: tokio::sync::Notify::new(),
                release_terminal: tokio::sync::Notify::new(),
                release_observation: tokio::sync::Notify::new(),
                release_eof: tokio::sync::Notify::new(),
                raw_closed: std::sync::atomic::AtomicBool::new(false),
            };
            let sink = RecordingSink::default();
            let id = identity("native-unknown-owner")?
                .with_run_id("original-observed-run")?
                .with_execution_id("original-observed-attempt")?;

            let capacity = Arc::new(tokio::sync::Semaphore::new(1));
            let permit = Arc::clone(&capacity)
                .acquire_owned()
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?;
            let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let invocation = AgentInvocationContext {
                runtime: Some(echo_core::tools::ExternalRunContext {
                    run_id: Some("original-observed-run".into()),
                    execution_id: Some("original-observed-attempt".into()),
                    ..Default::default()
                }),
                resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                    ObservationResource {
                        _permit: permit,
                        drops: Arc::clone(&drops),
                    },
                )],
                ..Default::default()
            };
            let (request, input) =
                TurnRequest::from_message(id.clone(), Message::user("task".to_string()))
                    .mode(TurnMode::Execute)
                    .invocation(invocation)
                    .with_input_receipt();
            agent.release_terminal.notify_one();
            let mut drive = Box::pin(AgentTurnDriver.drive(&agent, request, &sink));
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert_delivery_trace(
                &sink
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                &id,
                0,
                &["started", "assistant", "final_answer"],
            )?;
            assert_eq!(input.state(), TurnInputState::Accepted);
            agent.release_observation.notify_one();
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert_input_observation(
                &input,
                "original-observed-run",
                Some("original-observed-attempt"),
            )
            .await;
            agent.release_physical_terminal.notify_one();
            assert!(futures::poll!(drive.as_mut()).is_pending());
            assert_eq!(input.state(), TurnInputState::Accepted);
            assert!(!agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(capacity.available_permits(), 0);
            assert_eq!(drops.load(Ordering::SeqCst), 0);
            {
                let events = sink
                    .envelopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| event.payload.is_execution_terminal())
                        .count(),
                    1
                );
                assert!(
                    matches!(events.first().map(|event| &event.payload), Some(AgentEvent::ExecutionStatus(status))
                    if !status.adapter_generated && status.execution_id == "original-observed-attempt")
                );
                assert!(
                    matches!(events.last().map(|event| &event.payload), Some(AgentEvent::ExecutionStatus(status))
                    if !status.adapter_generated && status.execution_id == if matching_owner { "original-observed-attempt" } else { "foreign-attempt-B" })
                );
                assert!(echo_core::agent::validate_envelope_trajectory(&events).is_empty());
            }
            agent.release_eof.notify_one();
            let result = drive.await;
            assert!(agent.raw_closed.load(Ordering::SeqCst));
            assert_eq!(capacity.available_permits(), 1);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            if matching_owner {
                let receipt = result?;
                assert_eq!(receipt.outcome, TurnOutcome::Completed);
                assert_eq!(receipt.delivery, TurnDeliveryOutcome::Delivered);
                assert_eq!(receipt.final_answer.as_deref(), Some("original terminal"));
                assert_eq!(receipt.last_event_sequence, 4);
                assert!(matches!(
                    input.state(),
                    TurnInputState::TurnSettled {
                        outcome: AgentSteerTurnOutcome::Completed,
                        ..
                    }
                ));
            } else {
                assert!(
                    matches!(result, Err(ReactError::ExecutionNotSettled { run_id, execution_id })
                    if run_id == "original-observed-run" && execution_id.as_deref() == Some("original-observed-attempt"))
                );
                assert_eq!(input.state(), TurnInputState::Accepted);
                assert_input_observation(
                    &input,
                    "original-observed-run",
                    Some("original-observed-attempt"),
                )
                .await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod canonical_turn_kernel_contract_tests {
    use super::*;
    use echo_core::agent::{AgentPhase, ExecutionStatus, ExecutionStatusKind};
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn status(owner: &str, kind: ExecutionStatusKind, sequence: u64) -> AgentEvent {
        AgentEvent::ExecutionStatus(ExecutionStatus {
            adapter_generated: false,
            execution_id: owner.into(),
            request_id: Some("request".into()),
            sequence,
            phase: AgentPhase::Terminal,
            status: kind,
            reason: None,
        })
    }
    struct NativeSource {
        events: Mutex<Vec<echo_core::error::Result<AgentEvent>>>,
        typed_calls: AtomicUsize,
        text_calls: AtomicUsize,
        seen: Mutex<Option<(Message, AgentInvocationContext)>>,
        physical_eof: AtomicUsize,
        owner_drops: AtomicUsize,
    }
    impl NativeSource {
        fn new(events: Vec<echo_core::error::Result<AgentEvent>>) -> Self {
            Self {
                events: Mutex::new(events),
                typed_calls: AtomicUsize::new(0),
                text_calls: AtomicUsize::new(0),
                seen: Mutex::new(None),
                physical_eof: AtomicUsize::new(0),
                owner_drops: AtomicUsize::new(0),
            }
        }
    }
    impl Agent for NativeSource {
        fn name(&self) -> &str {
            "native-source"
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _: &'a str) -> BoxFuture<'a, echo_core::error::Result<String>> {
            Box::pin(async { Err(ReactError::Other("unscoped entry".into())) })
        }
        fn execute_stream<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            self.text_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ReactError::Other("unscoped source".into())) })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            echo_core::error::Result<BoxStream<'a, echo_core::error::Result<AgentEvent>>>,
        > {
            self.typed_calls.fetch_add(1, Ordering::SeqCst);
            *self.seen.lock().unwrap_or_else(|error| error.into_inner()) =
                Some((message, invocation));
            let events = std::mem::take(
                &mut *self
                    .events
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            );
            Box::pin(async move {
                struct Guard<'a>(&'a AtomicUsize);
                impl Drop for Guard<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let source = async_stream::stream! {
                    let _guard=Guard(&self.owner_drops);
                    for event in events { yield event; }
                    self.physical_eof.fetch_add(1,Ordering::SeqCst);
                };
                Ok(Box::pin(source) as BoxStream<'a, echo_core::error::Result<AgentEvent>>)
            })
        }
    }
    #[derive(Default)]
    struct Sink(Mutex<Vec<EventEnvelope>>);
    #[async_trait]
    impl EventSink for Sink {
        async fn on_event(&self, event: EventEnvelope) -> echo_core::error::Result<SinkControl> {
            self.0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
            Ok(SinkControl::Continue)
        }
    }
    fn identity() -> echo_core::error::Result<EventIdentity> {
        EventIdentity::for_run("run")?.with_execution_id("native-source")
    }
    #[tokio::test]
    async fn canonical_driver_text_uses_typed_input_and_preserves_real_owner_and_usage()
    -> echo_core::error::Result<()> {
        let agent = NativeSource::new(vec![
            Ok(status("native-source", ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::LlmUsage {
                model: "fixture".into(),
                prompt_tokens: 3,
                completion_tokens: 5,
                total_tokens: 8,
                cached_prompt_tokens: 0,
                cache_creation_prompt_tokens: 0,
                usage_reported: true,
            }),
            Ok(AgentEvent::FinalAnswer("中文🙂".into())),
            Ok(status("native-source", ExecutionStatusKind::Settled, 1)),
        ]);
        let sink = Sink::default();
        let receipt = AgentTurnDriver
            .drive(
                &agent,
                TurnRequest::new(identity()?, "same input").mode(TurnMode::Execute),
                &sink,
            )
            .await?;
        assert!(matches!(receipt.outcome, TurnOutcome::Completed));
        assert_eq!(receipt.final_answer.as_deref(), Some("中文🙂"));
        assert_eq!(
            (
                receipt.prompt_tokens,
                receipt.completion_tokens,
                receipt.llm_calls
            ),
            (3, 5, 1)
        );
        assert_eq!(agent.typed_calls.load(Ordering::SeqCst), 1);
        assert_eq!(agent.text_calls.load(Ordering::SeqCst), 0);
        let seen = agent.seen.lock().unwrap_or_else(|error| error.into_inner());
        assert!(seen.as_ref().is_some_and(|(message,_)|message.text_content().as_deref()==Some("same input")));
        assert_eq!(
            sink.0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            4
        );
        Ok(())
    }
    #[test]
    fn canonical_final_answer_notification_is_not_a_turn_completion() {
        assert!(TurnOutcome::classify(&AgentEvent::FinalAnswer("confirmed body".into())).is_none());
    }
    #[tokio::test]
    async fn canonical_driver_rejects_final_only_missing_final_and_foreign_owner()
    -> echo_core::error::Result<()> {
        let scripts = [
            vec![Ok(AgentEvent::FinalAnswer("body only".into()))],
            vec![
                Ok(status("native-source", ExecutionStatusKind::Started, 0)),
                Ok(status("native-source", ExecutionStatusKind::Settled, 1)),
            ],
            vec![
                Ok(status("native-source", ExecutionStatusKind::Started, 0)),
                Ok(AgentEvent::FinalAnswer("body".into())),
                Ok(status("foreign", ExecutionStatusKind::Settled, 1)),
            ],
        ];
        for events in scripts {
            let agent = NativeSource::new(events);
            let request = TurnRequest::from_message(identity()?, Message::user("input".into()))
                .mode(TurnMode::Execute);
            let receipt = AgentTurnDriver
                .drive(&agent, request, &Sink::default())
                .await?;
            assert!(!matches!(receipt.outcome, TurnOutcome::Completed));
            assert!(receipt.final_answer.is_none());
        }
        Ok(())
    }
    #[tokio::test]
    async fn canonical_driver_preserves_original_post_terminal_failure_through_eof()
    -> echo_core::error::Result<()> {
        let agent = NativeSource::new(vec![
            Ok(status("native-source", ExecutionStatusKind::Started, 0)),
            Ok(AgentEvent::FinalAnswer("before tail".into())),
            Ok(status("native-source", ExecutionStatusKind::Settled, 1)),
            Err(ReactError::Other("original tail error".into())),
        ]);
        let request = TurnRequest::from_message(identity()?, Message::user("input".into()))
            .mode(TurnMode::Execute);
        let receipt = AgentTurnDriver
            .drive(&agent, request, &Sink::default())
            .await?;
        assert!(matches!(receipt.outcome, TurnOutcome::Failed(_)));
        assert!(receipt.final_answer.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn canonical_driver_binds_unassigned_request_execution_only_from_verified_started()
    -> echo_core::error::Result<()> {
        for foreign in [false, true] {
            let observation_run = if foreign { "foreign-run" } else { "run" };
            let agent = NativeSource::new(vec![
                Ok(status("native-source", ExecutionStatusKind::Started, 0)),
                Err(ReactError::ExecutionNotSettled {
                    run_id: observation_run.into(),
                    execution_id: Some("native-source".into()),
                }),
                Ok(AgentEvent::FinalAnswer("safe current".into())),
                Ok(status("native-source", ExecutionStatusKind::Settled, 1)),
            ]);
            let identity =
                EventIdentity::new("original-stream", "original-turn")?.with_run_id("run")?;
            assert!(identity.execution_id.is_none());
            let request = TurnRequest::from_message(identity, Message::user("input".into()))
                .mode(TurnMode::Execute);
            let result = AgentTurnDriver
                .drive(&agent, request, &Sink::default())
                .await;
            if foreign {
                assert!(
                    matches!(result, Err(ReactError::ExecutionNotSettled { run_id, .. }) if run_id=="foreign-run")
                );
            } else {
                assert!(
                    matches!(result, Ok(receipt) if matches!(receipt.outcome,TurnOutcome::Completed))
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn canonical_driver_original_terminal_failure_precedes_late_tail_failure()
    -> echo_core::error::Result<()> {
        for kind in [ExecutionStatusKind::Failed, ExecutionStatusKind::Cancelled] {
            let mut terminal = status("native-source", kind.clone(), 1);
            if let AgentEvent::ExecutionStatus(status) = &mut terminal {
                status.reason = Some("original first cause".into());
            }
            let agent = NativeSource::new(vec![
                Ok(status("native-source", ExecutionStatusKind::Started, 0)),
                Ok(terminal),
                Err(ReactError::Other("later tail cause".into())),
            ]);
            let request = TurnRequest::from_message(identity()?, Message::user("input".into()))
                .mode(TurnMode::Execute);
            let receipt = AgentTurnDriver
                .drive(&agent, request, &Sink::default())
                .await?;
            match kind {
                ExecutionStatusKind::Cancelled => {
                    assert!(matches!(receipt.outcome, TurnOutcome::Cancelled))
                }
                _ => assert!(
                    matches!(receipt.outcome,TurnOutcome::Failed(failure) if failure.message=="original first cause")
                ),
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn canonical_conflicting_or_repeated_terminal_cannot_clear_original_observation()
    -> echo_core::error::Result<()> {
        for repeated in [false, true] {
            let mut events = vec![
                Ok(status("native-source", ExecutionStatusKind::Started, 0)),
                Ok(AgentEvent::FinalAnswer("safe current".into())),
            ];
            if repeated {
                events.push(Ok(status("native-source", ExecutionStatusKind::Settled, 1)));
            }
            events.push(Err(ReactError::ExecutionNotSettled {
                run_id: "run".into(),
                execution_id: Some("native-source".into()),
            }));
            events.push(Ok(status(
                "native-source",
                ExecutionStatusKind::Settled,
                if repeated { 1 } else { 0 },
            )));
            let source = NativeSource::new(events);
            let identity =
                EventIdentity::new("original-stream", "original-turn")?.with_run_id("run")?;
            assert!(identity.execution_id.is_none());
            let (request, input) =
                TurnRequest::from_message(identity, Message::user("current".into()))
                    .mode(TurnMode::Execute)
                    .with_input_receipt();
            let result = AgentTurnDriver
                .drive(&source, request, &Sink::default())
                .await;
            assert!(
                matches!(result,Err(ReactError::ExecutionNotSettled{run_id,execution_id:Some(execution)}) if run_id=="run"&&execution=="native-source")
            );
            assert!(matches!(input.state(), TurnInputState::Accepted));
            assert_eq!(source.physical_eof.load(Ordering::SeqCst), 1);
            assert_eq!(source.owner_drops.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }
}
