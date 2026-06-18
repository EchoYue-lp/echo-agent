//! Channel-based streaming execution — returns `BoxStream<'static>`.
//!
//! Uses a tokio::mpsc channel + spawned task instead of `try_stream!`.
//! All streaming execution goes through this module.
//!
//! The body of [`AgentRunSnapshot::run_core_loop`] is a thin driver that
//! sequences the phase functions in [`super::phases`]: the loop itself stays
//! here so there is one — and only one — place to read the unified ReAct
//! control flow. Each phase is responsible for a focused subset of work
//! (audit, compaction, LLM call, tool execution, verification, finalization)
//!
//! **Converged with the non-streaming path:** `run_stream_channel` runs the
//! same pre-flight checks as `prepare_react_context` — `GuardDirection::Input`
//! and `IntentRouter` classification (DirectAnswer shortcut, skill activation).
//! A blocked guard or a DirectAnswer short-circuit yields a stream pre-filled
//! with terminal events without entering `run_core_loop`.

use super::super::ReactAgent;
use super::phases::{self, IterOutcome, LoopState, PrepareOutcome};
use super::types::{StreamInit, StreamMode};
use crate::agent::AgentEvent;
use crate::error::Result;
use crate::llm::types::Message;
use std::ops::ControlFlow;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::debug;

// ── ReactAgent: entry point ──────────────────────────────────────────

impl ReactAgent {
    /// Channel-based streaming entry point. Returns `BoxStream<'static>`.
    pub(crate) async fn run_stream_channel(
        &self,
        init: StreamInit,
        mode: StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let buffer = self.config.stream_buffer_size;
        let (tx, rx) = mpsc::channel::<Result<AgentEvent>>(buffer);
        let context = self.memory.context.clone();
        let text = init.text.clone();
        let message = init.message.clone();
        let label = init.label.clone();

        // ★ Acquire execution mutex BEFORE context mutation — using lock_owned()
        // so the guard can be moved into the spawned task and held for the
        // entire stream lifetime.
        let execution_guard = self.execution_mutex.clone().lock_owned().await;

        // Start trace run BEFORE the prepare phase so trace events emitted
        // below (PhaseTransition, GuardBlock audit) are recorded rather than
        // silently dropped when current_run_id is None.
        self.start_trace_run(&text).await;

        // ── Restore thread context (Execute mode) + memory triggers/recall ──
        let recalled = if let Some(ref msg) = init.message {
            self.prepare_stream_context_with_message(mode, msg).await
        } else {
            self.prepare_stream_context(mode, &init.text).await
        };

        // ── G1: Guard input check (converged with prepare_react_context) ──
        // A blocked guard yields a stream pre-filled with a single terminal
        // FinalAnswer event (mirrors non-streaming Ok(msg) semantics) and does
        // NOT spawn run_core_loop. We must drop the owned execution_guard here
        // or the agent's mutex leaks (the spawn below owns it normally).
        if let Some(gm) = &self.guard.guard_manager {
            let result = gm
                .check_all(&text, crate::guard::GuardDirection::Input)
                .await;
            if let Ok(crate::guard::GuardResult::Block { reason }) = &result {
                let agent = self.config.agent_name.clone();
                debug!(agent = %agent, reason = %reason, "🛡️ Stream input blocked by guard");
                if let Some(al) = &self.guard.audit_logger {
                    let event = crate::audit::AuditEvent::now(
                        self.config.session_id.clone(),
                        agent.clone(),
                        crate::audit::AuditEventType::GuardBlock {
                            guard: "guard_manager".to_string(),
                            direction: crate::guard::GuardDirection::Input,
                            reason: reason.clone(),
                        },
                    );
                    if let Err(e) = al.log(event).await {
                        tracing::warn!(error = %e, "Failed to log guard audit event");
                    }
                }
                let _ = tx
                    .send(Ok(AgentEvent::FinalAnswer(format!(
                        "Request blocked by safety guard: {reason}"
                    ))))
                    .await;
                // Drop the owned guard to release the execution mutex — the
                // spawned task normally owns it, but we short-circuited.
                drop(execution_guard);
                return Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)));
            }
        }

        // ── G2: IntentRouter classification (converged with run_react_loop) ──
        // DirectAnswer streams its tokens then a FinalAnswer and skips the
        // core loop; SkillRequired injects a system message and falls through
        // to the normal core loop.
        if let Some(ref router) = self.intent_router {
            let messages = self.memory.context.lock().await.messages().to_vec();
            let intent = router.classify(&text, &messages).await;
            match intent {
                crate::intent::Intent::DirectAnswer { confidence } => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        confidence = confidence,
                        "🎯 Stream IntentRouter: DirectAnswer shortcut"
                    );
                    let mut snap = make_snapshot(self);
                    snap.current_run_id = self
                        .current_run_id
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    // DirectAnswer uses trimmed [system, user] messages and does
                    // not consume the recalled context, so the recall count is
                    // informational only.
                    let _ = recalled;
                    let content = snap
                        .direct_answer_stream(&self.config.system_prompt, &text, &tx)
                        .await?;
                    // Push assistant message so the agent remembers this turn.
                    self.memory
                        .context
                        .lock()
                        .await
                        .push(Message::assistant(content));
                    drop(execution_guard);
                    return Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)));
                }
                crate::intent::Intent::SkillRequired {
                    skill_name,
                    confidence,
                } => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        skill = %skill_name,
                        confidence = confidence,
                        "🎯 Stream IntentRouter: activating skill"
                    );
                    if self.tools.skill_registry.is_installed(&skill_name)
                        && !self.tools.skill_registry.is_activated(&skill_name)
                    {
                        match self.tools.skill_registry.activate(&skill_name).await {
                            Ok(content) => {
                                self.memory
                                    .context
                                    .lock()
                                    .await
                                    .push(Message::system(content.instructions));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    skill = %skill_name,
                                    error = %e,
                                    "Stream IntentRouter: failed to activate skill"
                                );
                            }
                        }
                    }
                    // Fall through to run_core_loop.
                }
                crate::intent::Intent::WorkflowRequired {
                    workflow_name,
                    confidence,
                } => {
                    tracing::info!(
                        agent = %self.config.agent_name,
                        workflow = %workflow_name,
                        confidence = confidence,
                        "🎯 Stream IntentRouter: WorkflowRequired (fallback to ReAct for now)"
                    );
                    // Fall through to run_core_loop.
                }
                crate::intent::Intent::Fallback => {
                    tracing::debug!(
                        agent = %self.config.agent_name,
                        "Stream IntentRouter: Fallback to ReAct"
                    );
                }
            }
        }

        let mut snap = make_snapshot(self);
        // Pass current run_id from the agent to the snapshot
        snap.current_run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        tokio::spawn(async move {
            // Move the guard into the spawned task — held for full stream duration
            let _execution_guard = execution_guard;
            if let Err(e) = snap
                .run_core_loop(context, text, message, label, mode, recalled, tx.clone())
                .await
            {
                let _ = tx.try_send(Err(e));
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

// ── AgentRunSnapshot: core loop driver ───────────────────────────────

use crate::agent::snapshot::AgentRunSnapshot as AgentSnapshot;

// Helper to create snapshot from agent (keeps the same API for rest of file)
fn make_snapshot(agent: &ReactAgent) -> AgentSnapshot {
    AgentSnapshot::from_agent(agent)
}

impl AgentSnapshot {
    /// Streaming "direct answer" shortcut used by IntentRouter.
    ///
    /// Bypasses the ReAct loop and calls the LLM directly with a trimmed
    /// `[system, user]` message pair (no tools, no ContextManager history),
    /// streaming `AgentEvent::Token` for each content chunk and finishing with
    /// a single `AgentEvent::FinalAnswer`. Mirrors the non-streaming
    /// `ReactAgent::direct_answer` semantics but yields tokens as they arrive.
    ///
    /// Returns the full accumulated text so the caller can push the assistant
    /// message into context. On error, the error is forwarded to `tx` and an
    /// empty string is returned.
    pub(crate) async fn direct_answer_stream(
        &self,
        system_prompt: &str,
        message: &str,
        tx: &mpsc::Sender<Result<AgentEvent>>,
    ) -> Result<String> {
        let messages = vec![
            Message::system(system_prompt.to_string()),
            Message::user(message.to_string()),
        ];

        let stream = super::phases::think::create_llm_stream(self, messages).await?;
        let mut stream = std::pin::pin!(stream);
        let mut content = String::new();
        use futures::StreamExt;
        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx
                        .send(Ok(AgentEvent::Error {
                            source: "direct_answer".into(),
                            message: e.to_string(),
                        }))
                        .await;
                    return Ok(content);
                }
            };
            // DirectAnswer is plain text — only forward content deltas, ignore
            // reasoning/tool_call deltas (no tools are attached).
            if let Some(choice) = chunk.choices.first()
                && let Some(delta) = &choice.delta.content
                && !delta.is_empty()
            {
                content.push_str(delta);
                if tx.send(Ok(AgentEvent::Token(delta.clone()))).await.is_err() {
                    // Receiver dropped — caller cancelled the stream.
                    break;
                }
            }
        }

        // Terminal event — frontend treats FinalAnswer as end-of-stream.
        let _ = tx.send(Ok(AgentEvent::FinalAnswer(content.clone()))).await;
        Ok(content)
    }

    /// Unified ReAct core loop — shared by both streaming and non-streaming paths.
    ///
    /// The non-streaming path (`run_react_loop`) creates a channel and runs this
    /// method, then collects `FinalAnswer` from the events. The streaming path
    /// (`run_stream_channel`) spawns this in a `tokio::spawn` and returns the
    /// receiver as a `BoxStream`.
    ///
    /// This body is intentionally thin: each block of work lives in
    /// [`super::phases`]. The single iteration loop
    /// is the project's only ReAct loop — phase functions are called from
    /// here, never from a sibling driver.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_core_loop(
        self,
        context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        text: String,
        _message: Option<Message>,
        label: String,
        mode: StreamMode,
        recalled: usize,
        tx: mpsc::Sender<Result<AgentEvent>>,
    ) -> Result<()> {
        // NOTE: execution_mutex is already held by the spawned task
        // (acquired in run_stream_channel via lock_owned()), so we don't
        // need to lock again here.

        // ── Pre-loop preparation ─────────────────────────────────────
        let mut state = match phases::prepare::prepare_turn(
            &self, &context, &tx, &text, &label, mode, recalled,
        )
        .await?
        {
            PrepareOutcome::Continue { task_node_id } => LoopState::new(task_node_id),
            PrepareOutcome::BlockedAndDone | PrepareOutcome::Abandoned => return Ok(()),
        };

        let agent_name = self.config.agent_name.clone();

        // ── The single core ReAct loop ───────────────────────────────
        // max_iterations == 0 means unlimited. Use usize::MAX as a practical
        // sentinel so the rest of the loop keeps normal for-loop semantics.
        let max_iterations = if self.config.max_iterations == 0 {
            usize::MAX
        } else {
            self.config.max_iterations
        };
        for iteration in 0..max_iterations {
            for cb in self.config.callbacks.iter() {
                cb.on_iteration(&agent_name, iteration).await;
            }
            debug!(
                agent = %agent_name,
                iteration = iteration + 1,
                "--- Streaming iteration{label} ---",
            );

            // Compact: PreCompact hook → checkpoint → ContextManager.prepare
            //          → PostCompact hook (if compression occurred)
            let messages =
                match phases::compact::run_compact(&self, &context, &tx, iteration).await? {
                    phases::CompactOutcome::Continue(m) => m,
                    phases::CompactOutcome::Abandoned => {
                        return Ok(());
                    }
                };

            // Think: callbacks + interventions + LLM stream → buffered output
            let think =
                match phases::think::run_think(&self, &context, &tx, &mut state, messages).await? {
                    phases::ThinkOutcome::Continue(t) => t,
                    phases::ThinkOutcome::Abandoned
                    | phases::ThinkOutcome::Cancelled
                    | phases::ThinkOutcome::Blocked => {
                        return Ok(());
                    }
                };

            // Branch: tool calls vs text answer vs no-response
            let outcome = if !think.tool_call_map.is_empty() {
                phases::tools::run_tools(&self, &context, &tx, &mut state, iteration, think, &label)
                    .await?
            } else if !think.content_buffer.is_empty() {
                let pt = think.pt;
                let ct = think.ct;
                match phases::verify::verify_final_text(
                    &self, &context, &tx, &mut state, iteration, think, &label,
                )
                .await?
                {
                    IterOutcome::Continue => continue,
                    IterOutcome::FinalText { answer } => {
                        match phases::finalize::emit_final_text(
                            &self, &context, &tx, &mut state, iteration, pt, ct, answer,
                        )
                        .await?
                        {
                            ControlFlow::Continue(()) => continue,
                            ControlFlow::Break(()) => return Ok(()),
                        }
                    }
                    other => other,
                }
            } else {
                IterOutcome::NoResponse
            };

            match outcome {
                IterOutcome::Continue => {
                    continue;
                }
                IterOutcome::Finish { output } => {
                    return phases::finalize::finalize_completed_run(
                        &self, context, &label, &output, iteration, &state, tx,
                    )
                    .await;
                }
                // FinalText is only produced by verify_final_text and is
                // already handled inline in the text branch above. Reaching
                // it here would mean a phase returned an outcome out of band
                // — guard against future refactors by treating it as a
                // terminal text emission.
                IterOutcome::FinalText { answer } => {
                    let pt = 0;
                    let ct = 0;
                    match phases::finalize::emit_final_text(
                        &self, &context, &tx, &mut state, iteration, pt, ct, answer,
                    )
                    .await?
                    {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => return Ok(()),
                    }
                }
                IterOutcome::NoResponse => {
                    return phases::finalize::finalize_no_response(&self, &state, tx).await;
                }
                IterOutcome::Abandoned => {
                    return Ok(());
                }
            }
        }

        // ── Post-loop: max iterations exceeded ───────────────────────
        phases::finalize::finalize_max_iterations(&self, &context, &state, tx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::AgentConfig;
    use echo_core::guard::{Guard, GuardDirection, GuardResult};
    use futures::StreamExt;
    use futures::future::BoxFuture;
    use std::sync::Arc;

    /// A guard that blocks every input with a fixed reason.
    struct BlockingGuard;
    impl Guard for BlockingGuard {
        fn name(&self) -> &str {
            "test-blocking-guard"
        }
        fn check<'a>(
            &'a self,
            _content: &'a str,
            direction: GuardDirection,
        ) -> BoxFuture<'a, Result<GuardResult>> {
            Box::pin(async move {
                if matches!(direction, GuardDirection::Input) {
                    Ok(GuardResult::Block {
                        reason: "test-input-blocked".into(),
                    })
                } else {
                    Ok(GuardResult::Pass)
                }
            })
        }
    }

    /// Build a minimal agent with a blocking input guard.
    fn agent_with_blocking_guard() -> ReactAgent {
        let mut agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        let mut gm = echo_core::guard::GuardManager::new();
        gm.add(Arc::new(BlockingGuard));
        agent.set_guard_manager(gm);
        agent
    }

    /// A blocked input guard yields a stream containing exactly one terminal
    /// FinalAnswer carrying the block reason — mirroring the non-streaming
    /// path's `Ok("Request blocked...")` semantics — and must NOT enter the
    /// core loop (no ThinkStart/Token events).
    #[tokio::test]
    async fn stream_guard_block_yields_single_final_answer() {
        let agent = agent_with_blocking_guard();
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "anything".into(),
                    message: None,
                    label: String::new(),
                },
                StreamMode::Chat,
            )
            .await
            .expect("run_stream_channel must return a stream even when blocked");

        let events: Vec<_> = stream.collect().await;

        // Exactly one event, and it is a terminal FinalAnswer.
        assert_eq!(
            events.len(),
            1,
            "blocked stream should have exactly one event"
        );
        match events[0].as_ref().expect("event is Ok") {
            AgentEvent::FinalAnswer(text) => {
                assert!(
                    text.starts_with("Request blocked by safety guard:"),
                    "expected block message, got: {text:?}",
                );
                assert!(text.contains("test-input-blocked"));
            }
            other => panic!("expected FinalAnswer, got {other:?}"),
        }
    }

    /// After a guard blocks one stream, the execution mutex must be released so
    /// the very next call can proceed. This guards against the lock-leak
    /// regression where a short-circuited branch forgets to drop the owned
    /// `execution_guard` (the agent would then deadlock forever).
    #[tokio::test]
    async fn stream_guard_block_releases_execution_mutex() {
        let agent = agent_with_blocking_guard();

        // First call: blocked, must release the lock.
        let s1 = agent
            .run_stream_channel(
                StreamInit {
                    text: "first".into(),
                    message: None,
                    label: String::new(),
                },
                StreamMode::Chat,
            )
            .await
            .expect("first stream");
        let _: Vec<_> = s1.collect().await;

        // Second call must not hang — if the lock leaked, this await never
        // completes and the test times out.
        let s2 = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            agent.run_stream_channel(
                StreamInit {
                    text: "second".into(),
                    message: None,
                    label: String::new(),
                },
                StreamMode::Chat,
            ),
        )
        .await
        .expect("second run_stream_channel must not hang (execution_guard leaked?)")
        .expect("second stream");

        let events: Vec<_> = s2.collect().await;
        assert_eq!(events.len(), 1, "second blocked stream also has one event");
        assert!(matches!(
            events[0].as_ref().unwrap(),
            AgentEvent::FinalAnswer(_)
        ));
    }

    // ── End-to-end run_core_loop tests driven by MockLlmClient ─────────────
    // These exercise the shared core loop (the one ReAct loop) through the
    // public streaming entrypoint `run_stream_channel`, with a scripted mock
    // LLM. No guard / no intent router on these agents, so the stream falls
    // straight through to run_core_loop.

    use crate::agent::react::builder::ReactAgentBuilder;
    use crate::testing::{MockLlmClient, MockTool};

    /// Build a streaming-capable agent backed by a scripted mock LLM, with
    /// no guard and no intent router (so requests reach run_core_loop).
    fn agent_with_mock_llm(llm: MockLlmClient) -> ReactAgent {
        ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("You are a test assistant.")
            .build()
            .expect("agent builds")
    }

    /// Collect the AgentEvents emitted by one streaming turn.
    async fn collect_events(agent: &ReactAgent, text: &str) -> Vec<AgentEvent> {
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: text.into(),
                    message: None,
                    label: String::new(),
                },
                StreamMode::Chat,
            )
            .await
            .expect("stream starts");
        let results: Vec<_> = stream.collect().await;
        results
            .into_iter()
            .map(|r| r.expect("event is Ok"))
            .collect()
    }

    /// A single-turn text answer: the mock LLM replies with plain content, so
    /// the loop should emit a Token + a terminal FinalAnswer and stop (no tool
    /// phase). Guards the core-loop text branch.
    #[tokio::test]
    async fn run_core_loop_text_only_yields_final_answer() {
        let llm = MockLlmClient::new().with_response("Paris is the capital of France.");
        let agent = agent_with_mock_llm(llm);

        let events = collect_events(&agent, "What is the capital of France?").await;

        // Must end with a FinalAnswer carrying the mock content.
        let last = events.last().expect("at least one event");
        match last {
            AgentEvent::FinalAnswer(text) => {
                assert!(
                    text.contains("Paris"),
                    "final answer should contain the mock text, got: {text:?}"
                );
            }
            other => panic!("expected FinalAnswer as last event, got {other:?}"),
        }
    }

    /// A full ReAct cycle: the mock LLM first requests a tool call, the tool
    /// returns a scripted result, then the LLM produces a final text answer.
    /// Guards the core-loop tool-call branch (think → tools → think → finalize).
    #[tokio::test]
    async fn run_core_loop_tool_call_cycle_completes() {
        // The mock tool returns a fixed result; the LLM script is:
        //   1. request tool_call "mock_calc" with args {"x": 6, "y": 7}
        //   2. after seeing the tool result, emit a final text answer.
        let llm = MockLlmClient::new()
            .then_tool_call("call_1", "mock_calc", r#"{"x":6,"y":7}"#)
            .with_response("The result is 42.");

        let tool = MockTool::new("mock_calc").with_response("42");

        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("You are a test assistant. Use tools when asked.")
            .tool(Box::new(tool))
            .build()
            .expect("agent builds");

        let events = collect_events(&agent, "What is 6 times 7?").await;

        // The last event must be a FinalAnswer.
        let last = events.last().expect("at least one event");
        assert!(
            matches!(last, AgentEvent::FinalAnswer(t) if t.contains("42")),
            "expected FinalAnswer with 42, got: {last:?}"
        );
    }

    /// When the mock LLM exhausts its response queue (returns EmptyResponse
    /// error), the loop must terminate gracefully with an Error event rather
    /// than hanging or panicking. Guards the empty-response / error branch.
    #[tokio::test]
    async fn run_core_loop_empty_llm_response_terminates_gracefully() {
        // No preset responses — first LLM call yields EmptyResponse error.
        let llm = MockLlmClient::new();
        let agent = agent_with_mock_llm(llm);

        // Collect raw results (may include Err — the point of this test is
        // graceful termination, not a specific event shape).
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "anything".into(),
                    message: None,
                    label: String::new(),
                },
                StreamMode::Chat,
            )
            .await
            .expect("stream starts");
        let results = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.collect::<Vec<_>>(),
        )
        .await;

        // The loop must TERMINATE within the timeout — not hang forever on
        // repeated retries of an empty mock queue.
        assert!(
            results.is_ok(),
            "stream must terminate, not hang, on empty LLM response"
        );
    }
}
