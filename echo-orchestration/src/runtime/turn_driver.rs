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
//! use echo_orchestration::runtime::turn_driver::{AgentTurnDriver, EventSink, TurnMode, TurnRequest};
//! use std::sync::Arc;
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
//! # impl EventSink for PrintSink {
//! #     fn on_event(&self, _envelope: &echo_core::agent::EventEnvelope) -> bool {
//! #         true
//! #     }
//! # }
//! # async fn run() -> echo_core::error::Result<()> {
//! let agent: Arc<dyn Agent> = Arc::new(MyAgent);
//! let identity = EventIdentity::new("stream-1", "turn-1")?;
//! let request = TurnRequest::new(identity, "hello").mode(TurnMode::Execute);
//! let receipt = AgentTurnDriver.drive(agent, request, &PrintSink).await;
//! assert_eq!(receipt.outcome.status(), "completed");
//! assert_eq!(receipt.final_answer.as_deref(), Some("done"));
//! # Ok(())
//! # }
//! ```

use echo_core::agent::{
    Agent, AgentEvent, CancellationToken, EventEnvelope, EventIdentity, TurnId,
    envelope_event_stream_after,
};
use echo_core::error::{AgentFailure, ReactError};
use futures::StreamExt;
use std::sync::Arc;
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
    /// User-visible instruction or task text.
    pub message: String,
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
}

impl TurnRequest {
    pub fn new(identity: EventIdentity, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            identity,
            mode: TurnMode::Chat,
            cancel: None,
            last_persisted_sequence: 0,
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
    pub fn from_agent_event(event: &AgentEvent) -> Option<Self> {
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
    /// Provider-reported prompt tokens accumulated over the turn.
    pub prompt_tokens: u64,
    /// Provider-reported completion tokens accumulated over the turn.
    pub completion_tokens: u64,
    /// Number of provider calls that reported usage.
    pub llm_calls: u64,
    /// Last envelope sequence emitted for the turn.
    pub last_event_sequence: u64,
    /// Wall-clock turn duration.
    pub elapsed: Duration,
}

impl TurnReceipt {
    /// Stable status string for projections (`completed`/`cancelled`/`failed`).
    pub fn status(&self) -> &'static str {
        self.outcome.status()
    }

    fn failed(turn_id: TurnId, failure: AgentFailure, started: Instant) -> Self {
        Self {
            turn_id,
            outcome: TurnOutcome::Failed(failure),
            final_answer: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            llm_calls: 0,
            last_event_sequence: 0,
            elapsed: started.elapsed(),
        }
    }
}

/// Per-consumer event handler for one driven turn.
///
/// `on_event` receives every envelope in order, including the terminal one.
/// Returning `false` declares the consumer closed: the driver cancels the
/// invocation and still produces a typed terminal receipt.
pub trait EventSink: Send + Sync {
    fn on_event(&self, envelope: &EventEnvelope) -> bool;
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
        agent: Arc<dyn Agent>,
        request: TurnRequest,
        sink: &dyn EventSink,
    ) -> TurnReceipt {
        let started = Instant::now();
        let turn_id = request.identity.turn_id.clone();
        let token = request.cancel.clone().unwrap_or_default();

        let raw = match request.mode {
            TurnMode::Chat => {
                agent
                    .chat_stream_with_cancel(&request.message, token.clone())
                    .await
            }
            TurnMode::Execute => {
                agent
                    .execute_stream_with_cancel(&request.message, token.clone())
                    .await
            }
        };
        let raw = match raw {
            Ok(stream) => stream,
            Err(error) => {
                return TurnReceipt::failed(
                    turn_id,
                    AgentFailure::from_react_error(&error),
                    started,
                );
            }
        };

        let mut outcome: Option<TurnOutcome> = None;
        let mut final_answer: Option<String> = None;
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut llm_calls: u64 = 0;
        let mut last_event_sequence = request.last_persisted_sequence;
        let mut sink_closed = false;

        let mut stream =
            envelope_event_stream_after(raw, request.identity, request.last_persisted_sequence);
        while let Some(item) = stream.next().await {
            let envelope = match item {
                Ok(envelope) => envelope,
                Err(error) => {
                    // The transport only errors before the first event (an
                    // invalid identity); map it to a typed failure.
                    outcome = Some(TurnOutcome::Failed(AgentFailure::from_react_error(&error)));
                    break;
                }
            };
            last_event_sequence = envelope.sequence;
            if !sink_closed && !sink.on_event(&envelope) {
                sink_closed = true;
                token.cancel();
            }
            match &envelope.payload {
                AgentEvent::LlmUsage {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    ..
                } => {
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
                    }
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
        }

        let outcome = outcome.unwrap_or_else(|| {
            TurnOutcome::Failed(AgentFailure::from_react_error(&ReactError::Other(
                "turn stream ended without a terminal event".to_string(),
            )))
        });
        TurnReceipt {
            turn_id,
            outcome,
            final_answer,
            prompt_tokens,
            completion_tokens,
            llm_calls,
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<u64>>,
        close_after: Option<usize>,
    }

    impl EventSink for RecordingSink {
        fn on_event(&self, envelope: &EventEnvelope) -> bool {
            let mut events = self
                .events
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            events.push(envelope.sequence);
            match self.close_after {
                Some(limit) => events.len() < limit,
                None => true,
            }
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
    async fn completed_turn_maps_final_answer_and_sequences_from_one() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                usage_event(10, 5),
                AgentEvent::FinalAnswer("done".to_string()),
            ]
        }));
        let sink = RecordingSink::default();
        let request = TurnRequest::new(identity("ok"), "hello");
        let receipt = AgentTurnDriver.drive(agent, request, &sink).await;
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
        let receipt = AgentTurnDriver.drive(agent, request, &sink).await;
        assert_eq!(receipt.outcome, TurnOutcome::Cancelled);
        assert_eq!(receipt.status(), "cancelled");
        assert!(receipt.final_answer.is_none());
    }

    #[tokio::test]
    async fn error_terminal_maps_typed_failure() {
        let failure = AgentFailure::from_react_error(&ReactError::Other("boom".to_string()));
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![AgentEvent::Error {
                source: "test".to_string(),
                message: "boom".to_string(),
                failure: AgentFailure::from_react_error(&ReactError::Other("boom".to_string())),
            }]
        }));
        let request = TurnRequest::new(identity("fail"), "hello");
        let receipt = AgentTurnDriver
            .drive(agent, request, &RecordingSink::default())
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
            .drive(agent, request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.prompt_tokens, 107);
        assert_eq!(receipt.completion_tokens, 43);
        assert_eq!(receipt.llm_calls, 2);
    }

    #[tokio::test]
    async fn resumed_sequencing_continues_after_persisted_sequence() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| vec![AgentEvent::Cancelled]));
        let sink = RecordingSink::default();
        let request = TurnRequest::new(identity("resume"), "hello").last_persisted_sequence(41);
        let receipt = AgentTurnDriver.drive(agent, request, &sink).await;
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
        let receipt = AgentTurnDriver.drive(agent, request, &sink).await;
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
        let receipt = AgentTurnDriver
            .drive(agent, request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome.status(), "failed");
        assert_eq!(receipt.last_event_sequence, 0);
    }

    #[tokio::test]
    async fn missing_terminal_is_a_failed_receipt() {
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![AgentEvent::Token("partial".to_string())]
        }));
        let request = TurnRequest::new(identity("noterminal"), "hello");
        let receipt = AgentTurnDriver
            .drive(agent, request, &RecordingSink::default())
            .await;
        assert_eq!(receipt.outcome.status(), "failed");
    }

    #[tokio::test]
    async fn sink_receives_exactly_one_terminal_envelope() {
        let terminal_count = AtomicUsize::new(0);
        struct TerminalCountingSink<'a> {
            count: &'a AtomicUsize,
        }
        impl EventSink for TerminalCountingSink<'_> {
            fn on_event(&self, envelope: &EventEnvelope) -> bool {
                if TurnOutcome::from_agent_event(&envelope.payload).is_some() {
                    self.count.fetch_add(1, Ordering::SeqCst);
                }
                true
            }
        }
        let agent: Arc<dyn Agent> = Arc::new(ScriptedAgent::new(|| {
            vec![
                usage_event(1, 1),
                AgentEvent::Token("t".to_string()),
                AgentEvent::FinalAnswer("fin".to_string()),
            ]
        }));
        let sink = TerminalCountingSink {
            count: &terminal_count,
        };
        let request = TurnRequest::new(identity("exactly"), "hello");
        let receipt = AgentTurnDriver.drive(agent, request, &sink).await;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(terminal_count.load(Ordering::SeqCst), 1);
    }
}
