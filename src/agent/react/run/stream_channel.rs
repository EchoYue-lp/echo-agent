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
//! and `IntentRouter` classification (including skill activation). A blocked
//! guard yields a terminal stream without entering `run_core_loop`.

use super::super::ReactAgent;
use super::STREAM_CANCELLATION_SETTLE_PERIOD;
use super::phases::{self, IterOutcome, LoopState, PrepareOutcome};
use super::types::{StreamInit, StreamMode};
use crate::agent::AgentEvent;
use crate::error::Result;
use crate::llm::types::{ContentPart, Message, MessageContent};
use futures::Stream;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tracing::debug;

struct ManagedAgentEventStream {
    receiver: tokio_stream::wrappers::ReceiverStream<Result<AgentEvent>>,
    cancel: crate::agent::CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
    runtime: tokio::runtime::Handle,
}

impl Stream for ManagedAgentEventStream {
    type Item = Result<AgentEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        Pin::new(&mut stream.receiver).poll_next(cx)
    }
}

impl Drop for ManagedAgentEventStream {
    fn drop(&mut self) {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return;
        };
        let reaper = self.runtime.spawn(async move {
            if tokio::time::timeout(STREAM_CANCELLATION_SETTLE_PERIOD, &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        });
        // The runtime owns the bounded reaper after the consumer releases the stream.
        std::mem::drop(reaper);
    }
}

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
        let mut text = init.text.clone();
        let mut message = init.message.clone();
        let label = init.label.clone();
        let invocation = init.invocation;
        // Capture value-carried run metadata before the execution mutex wait.
        // Concurrent callers may update/clear the agent's shared external
        // context while this invocation is queued, but this snapshot belongs
        // to the invocation that entered here.
        let legacy_runtime = if invocation.is_none() {
            Some(self.capture_legacy_external_context())
        } else {
            None
        };

        // ★ Acquire execution mutex BEFORE context mutation — using lock_owned()
        // so the guard can be moved into the spawned task and held for the
        // entire stream lifetime.
        let execution_guard = self.execution_mutex.clone().lock_owned().await;

        // Guard raw input before trace, hooks, memory, or conversation context
        // can retain it. Transformations become the authoritative turn input.
        if let Some(gm) = &self.guard.guard_manager {
            let result = gm
                .check_all(&text, crate::guard::GuardDirection::Input)
                .await?;
            match result {
                crate::guard::GuardResult::Block { reason } => {
                    let agent = self.config.agent_name.clone();
                    debug!(agent = %agent, reason = %reason, "🛡️ Stream input blocked by guard");
                    if let Some(al) = &self.guard.audit_logger {
                        let event = crate::audit::AuditEvent::now(
                            self.config.session_id.clone(),
                            agent,
                            crate::audit::AuditEventType::GuardBlock {
                                guard: "guard_manager".to_string(),
                                direction: crate::guard::GuardDirection::Input,
                                reason: reason.clone(),
                            },
                        );
                        if let Err(error) = al.log(event).await {
                            tracing::warn!(%error, "Failed to log guard audit event");
                        }
                    }
                    let trace_run_id = if let Some(runtime) = invocation
                        .as_ref()
                        .and_then(|context| context.runtime.as_ref())
                    {
                        self.start_scoped_trace_run(
                            "[input blocked by guard]",
                            runtime.run_id.as_deref(),
                            runtime.conversation_id.as_deref(),
                            runtime.turn_id.as_deref(),
                            runtime.execution_id.as_deref(),
                        )
                        .await
                    } else if let Some(legacy) = legacy_runtime.as_ref() {
                        self.start_legacy_trace_run("[input blocked by guard]", legacy)
                            .await
                    } else {
                        None
                    };
                    let _ = tx
                        .send(Ok(AgentEvent::FinalAnswer(format!(
                            "Request blocked by safety guard: {reason}"
                        ))))
                        .await;
                    self.finalize_scoped_trace_run(
                        trace_run_id.as_deref(),
                        crate::trace::RunStatus::Failed,
                        None,
                        Some(&reason),
                    )
                    .await;
                    drop(execution_guard);
                    return Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)));
                }
                crate::guard::GuardResult::Transform { content, .. } => {
                    text = content;
                    if let Some(original) = message.take() {
                        message = Some(message_with_replaced_text(original, &text));
                    }
                }
                crate::guard::GuardResult::Pass | crate::guard::GuardResult::Warn { .. } => {}
            }
        }

        // Start a unique trace invocation after guard transformation. Product run identity
        // remains in ExternalRunContext and is only used as correlation.
        let trace_run_id;
        if let Some(invocation) = invocation.as_ref() {
            let parent_run_id = invocation
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.run_id.clone());
            let conversation_id = invocation
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.conversation_id.clone());
            let trace_turn_id = invocation
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.turn_id.clone());
            let execution_id = invocation
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.execution_id.clone());
            trace_run_id = self
                .start_scoped_trace_run(
                    &text,
                    parent_run_id.as_deref(),
                    conversation_id.as_deref(),
                    trace_turn_id.as_deref(),
                    execution_id.as_deref(),
                )
                .await;
        } else {
            trace_run_id = match legacy_runtime.as_ref() {
                Some(legacy) => self.start_legacy_trace_run(&text, legacy).await,
                None => None,
            };
        }
        let turn_id = invocation
            .as_ref()
            .and_then(|value| value.runtime.as_ref())
            .and_then(|runtime| runtime.turn_id.clone().or_else(|| runtime.run_id.clone()))
            .or_else(|| {
                legacy_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.turn_id.clone())
                    .or_else(|| {
                        legacy_runtime
                            .as_ref()
                            .and_then(|runtime| runtime.current_run_id.clone())
                    })
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let active_turn_lease = self.turn_steer_mailbox.begin(turn_id.clone());

        // ── Restore thread context (Execute mode) + memory triggers/recall ──
        let history = invocation
            .as_ref()
            .and_then(|value| value.history.as_deref())
            .unwrap_or_default();
        let runtime_state_id = invocation
            .as_ref()
            .and_then(|context| context.runtime_state_id.as_deref())
            .or_else(|| {
                invocation
                    .as_ref()
                    .and_then(|context| context.runtime.as_ref())
                    .and_then(|runtime| runtime.conversation_id.as_deref())
            })
            .or_else(|| {
                legacy_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.conversation_id.as_deref())
            })
            .or(self.config.conversation_id.as_deref());
        let recalled = if let Some(ref msg) = message {
            self.prepare_stream_context_with_message(mode, msg, history, runtime_state_id)
                .await
        } else {
            self.prepare_stream_context(mode, &text, history, runtime_state_id)
                .await
        }?;
        // A tracked cold-input receipt may be published only after the initial
        // message has been inserted into ContextManager and before intent
        // routing or provider execution begins. Generic Agent implementations
        // that do not provide this publisher intentionally leave the receipt
        // at `drained = false`.
        if let Some(lifecycle) = invocation
            .as_ref()
            .and_then(|context| context.input_lifecycle.as_ref())
        {
            lifecycle.mark_drained();
        }

        // ── G2: IntentRouter classification (converged with run_react_loop) ──
        // Routing may activate a skill. DirectAnswer uses the canonical loop
        // so every invocation has the same lifecycle and terminal authority.
        if let Some(ref router) = self.intent_router {
            let messages = self.memory.context.lock().await.messages().to_vec();
            let cancel = invocation
                .as_ref()
                .and_then(|context| context.cancel.clone())
                .or_else(|| {
                    legacy_runtime
                        .as_ref()
                        .and_then(|runtime| runtime.cancel.as_ref())
                        .map(|token| token.as_ref().clone())
                })
                .unwrap_or_default();
            let intent = router.classify_with_cancel(&text, &messages, cancel).await;
            match intent {
                crate::intent::Intent::DirectAnswer { confidence } => {
                    tracing::debug!(
                        agent = %self.config.agent_name,
                        confidence,
                        "Stream DirectAnswer routed through ReAct for pre-model projection"
                    );
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
                    if let Err(e) = self.activate_skill(&skill_name).await {
                        tracing::warn!(
                            skill = %skill_name,
                            error = %e,
                            "Stream IntentRouter: failed to activate skill"
                        );
                    }
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

        let mut snap = match (invocation.as_ref(), legacy_runtime.as_ref()) {
            (Some(invocation), _) => AgentSnapshot::from_agent_with_invocation(self, invocation),
            (None, Some(legacy)) => AgentSnapshot::from_agent_with_legacy_context(self, legacy),
            (None, None) => make_snapshot(self),
        };
        snap.current_turn_id = Some(turn_id);
        snap.turn_steer_incarnation = Some(active_turn_lease.incarnation());
        snap.current_message = message.clone();
        snap.trace_run_id = trace_run_id;
        let consumer_cancel = snap
            .cancel_token
            .as_ref()
            .or(snap.external_cancel.as_deref())
            .map(crate::agent::CancellationToken::child_token)
            .unwrap_or_default();
        snap.cancel_token = Some(consumer_cancel.clone());
        snap.external_cancel = Some(Arc::new(consumer_cancel.clone()));
        active_turn_lease.set_steerable(true);
        let terminal_cancel = consumer_cancel.clone();

        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            crate::error::ReactError::Other(format!(
                "stream execution requires a Tokio runtime: {error}"
            ))
        })?;
        let task = runtime.spawn(async move {
            // Move the guard into the spawned task — held for full stream duration
            let _execution_guard = execution_guard;
            match snap
                .run_core_loop(
                    context,
                    text,
                    message,
                    label,
                    mode,
                    recalled,
                    true,
                    tx.clone(),
                )
                .await
            {
                Ok(outcome) => active_turn_lease.settle(outcome),
                Err(error) => {
                    let outcome = if terminal_cancel.is_cancelled() {
                        crate::agent::AgentSteerTurnOutcome::Cancelled
                    } else {
                        crate::agent::AgentSteerTurnOutcome::Failed
                    };
                    active_turn_lease.settle(outcome);
                    let _ = tx
                        .send(Ok(AgentEvent::from_error("react_loop", &error)))
                        .await;
                }
            }
        });

        Ok(Box::pin(ManagedAgentEventStream {
            receiver: tokio_stream::wrappers::ReceiverStream::new(rx),
            cancel: consumer_cancel,
            task: Some(task),
            runtime,
        }))
    }
}

fn message_with_replaced_text(mut message: Message, replacement: &str) -> Message {
    message.content = match message.content {
        MessageContent::Parts(parts) => {
            let mut retained = Vec::with_capacity(parts.len().saturating_add(1));
            retained.push(ContentPart::Text {
                text: replacement.to_string(),
            });
            retained.extend(
                parts
                    .into_iter()
                    .filter(|part| !matches!(part, ContentPart::Text { .. })),
            );
            MessageContent::Parts(retained)
        }
        MessageContent::Text(_) | MessageContent::Empty => {
            MessageContent::Text(replacement.to_string())
        }
    };
    message
}

// ── AgentRunSnapshot: core loop driver ───────────────────────────────

use crate::agent::snapshot::AgentRunSnapshot as AgentSnapshot;

// Helper to create snapshot from agent (keeps the same API for rest of file)
fn make_snapshot(agent: &ReactAgent) -> AgentSnapshot {
    AgentSnapshot::from_agent(agent)
}

impl AgentSnapshot {
    fn failure_terminal(&self) -> crate::agent::AgentSteerTurnOutcome {
        if self
            .cancel_token
            .as_ref()
            .is_some_and(crate::agent::CancellationToken::is_cancelled)
            || self
                .external_cancel
                .as_deref()
                .is_some_and(crate::agent::CancellationToken::is_cancelled)
        {
            crate::agent::AgentSteerTurnOutcome::Cancelled
        } else {
            crate::agent::AgentSteerTurnOutcome::Failed
        }
    }

    async fn drain_steer_into_context(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        assistant_draft: Option<Message>,
    ) -> usize {
        let Some(turn_id) = self.current_turn_id.as_deref() else {
            return 0;
        };
        let Some(incarnation) = self.turn_steer_incarnation.as_ref() else {
            return 0;
        };
        let mut guard = context.lock().await;
        let pending = self.turn_steer_mailbox.take_pending(turn_id, incarnation);
        if pending.is_empty() {
            return 0;
        }
        let count = pending.len();
        if let Some(draft) = assistant_draft {
            guard.push(draft);
        }
        for message in pending.messages() {
            guard.push(message.clone());
        }
        pending.mark_drained();
        count
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
        user_prompt_hook_already_run: bool,
        tx: mpsc::Sender<Result<AgentEvent>>,
    ) -> Result<crate::agent::AgentSteerTurnOutcome> {
        // NOTE: execution_mutex is already held by the spawned task
        // (acquired in run_stream_channel via lock_owned()), so we don't
        // need to lock again here.

        // ── Pre-loop preparation ─────────────────────────────────────
        let mut state = match phases::prepare::prepare_turn(
            &self,
            &context,
            &tx,
            &text,
            &label,
            mode,
            recalled,
            user_prompt_hook_already_run,
        )
        .await?
        {
            PrepareOutcome::Continue => LoopState::new(),
            PrepareOutcome::BlockedAndDone => {
                return Ok(crate::agent::AgentSteerTurnOutcome::Failed);
            }
            PrepareOutcome::Abandoned => {
                self.finalize_run(
                    crate::trace::RunStatus::Cancelled,
                    None,
                    Some("event consumer disconnected during preparation"),
                )
                .await;
                return Ok(crate::agent::AgentSteerTurnOutcome::Cancelled);
            }
        };

        let agent_name = self.config.agent_name.clone();

        // ── The single core ReAct loop ───────────────────────────────
        // Builders reject zero. A directly constructed or restored invalid
        // config still remains bounded here and reaches typed finalization.
        let max_iterations = self.config.max_iterations;
        for iteration in 0..max_iterations {
            for cb in self.config.callbacks.iter() {
                cb.on_iteration(&agent_name, iteration).await;
            }
            debug!(
                agent = %agent_name,
                iteration = iteration + 1,
                "--- Streaming iteration{label} ---",
            );

            let _ = self.drain_steer_into_context(&context, None).await;

            let remaining = max_iterations.saturating_sub(iteration);
            if !state.budget.wind_down_emitted
                && self
                    .config
                    .run_budget
                    .iteration_wind_down_remaining
                    .is_some_and(|threshold| threshold > 0 && remaining <= threshold)
            {
                state.budget.wind_down_emitted = true;
                super::context::push_runtime_context_note(
                    &context,
                    "RunBudget:WindDown",
                    "The iteration budget is nearly exhausted. Stop opening new branches and converge on a final answer.",
                )
                .await;
                let _ = tx
                    .send(Ok(AgentEvent::BudgetDecision {
                        decision: echo_core::agent::BudgetDecision::WindDown,
                        reason: "iteration_wind_down".to_string(),
                        iteration: iteration.saturating_add(1),
                        reported_model_tokens: state.budget.reported_model_tokens,
                        usage_complete: state.budget.usage_complete,
                    }))
                    .await;
                self.record_event(crate::trace::RunEvent::BudgetDecision {
                    decision: "wind_down".to_string(),
                    reason: "iteration_wind_down".to_string(),
                    iteration: iteration.saturating_add(1),
                    reported_model_tokens: state.budget.reported_model_tokens,
                    usage_complete: state.budget.usage_complete,
                })
                .await;
            }

            if state.budget.final_only && !state.budget.final_only_emitted {
                state.budget.final_only_emitted = true;
                super::context::push_runtime_context_note(
                    &context,
                    "RunBudget:FinalOnly",
                    "The model-token budget is exhausted. Produce the best final answer now without calling tools.",
                )
                .await;
            }

            // Compact: PreCompact hook → (stage4 E1) pre_compaction_flush →
            //          checkpoint → ContextManager.prepare → PostCompact hook.
            // The flush itself lives inside `run_compact` so every compaction
            // path benefits and it is gated on `should_compress()`.
            let messages =
                match phases::compact::run_compact(&self, &context, &tx, iteration).await? {
                    phases::CompactOutcome::Continue(m) => m,
                    phases::CompactOutcome::Abandoned => {
                        self.finalize_run(
                            crate::trace::RunStatus::Cancelled,
                            None,
                            Some("event consumer disconnected during compaction"),
                        )
                        .await;
                        return Ok(crate::agent::AgentSteerTurnOutcome::Cancelled);
                    }
                    phases::CompactOutcome::Failed => {
                        let outcome = self.failure_terminal();
                        let status = if outcome == crate::agent::AgentSteerTurnOutcome::Cancelled {
                            crate::trace::RunStatus::Cancelled
                        } else {
                            crate::trace::RunStatus::Failed
                        };
                        self.finalize_run(status, None, Some("context preparation failed"))
                            .await;
                        return Ok(outcome);
                    }
                };

            // Think: callbacks + interventions + LLM stream → buffered output
            let final_only = state.budget.final_only;
            let think =
                match phases::think::run_think(&self, &context, &tx, messages, final_only).await? {
                    phases::ThinkOutcome::Continue(t) => t,
                    phases::ThinkOutcome::Abandoned => {
                        self.finalize_run(
                            crate::trace::RunStatus::Cancelled,
                            None,
                            Some("event consumer disconnected or intervention blocked the run"),
                        )
                        .await;
                        return Ok(crate::agent::AgentSteerTurnOutcome::Cancelled);
                    }
                    phases::ThinkOutcome::Blocked => {
                        self.finalize_run(
                            crate::trace::RunStatus::Failed,
                            None,
                            Some("intervention blocked the run"),
                        )
                        .await;
                        return Ok(crate::agent::AgentSteerTurnOutcome::Failed);
                    }
                    phases::ThinkOutcome::Cancelled => {
                        return Ok(crate::agent::AgentSteerTurnOutcome::Cancelled);
                    }
                    phases::ThinkOutcome::Failed => {
                        self.finalize_run(
                            crate::trace::RunStatus::Failed,
                            None,
                            Some("model response failed"),
                        )
                        .await;
                        return Ok(self.failure_terminal());
                    }
                };

            let iteration_tokens = think.pt.saturating_add(think.ct);
            state
                .budget
                .record_usage(iteration_tokens, think.usage_reported);
            if !state.budget.final_only
                && self
                    .config
                    .run_budget
                    .max_model_tokens
                    .is_some_and(|limit| limit > 0 && state.budget.reported_model_tokens >= limit)
            {
                state.budget.final_only = true;
                let _ = tx
                    .send(Ok(AgentEvent::BudgetDecision {
                        decision: echo_core::agent::BudgetDecision::FinalOnly,
                        reason: "model_token_budget".to_string(),
                        iteration: iteration.saturating_add(1),
                        reported_model_tokens: state.budget.reported_model_tokens,
                        usage_complete: state.budget.usage_complete,
                    }))
                    .await;
                self.record_event(crate::trace::RunEvent::BudgetDecision {
                    decision: "final_only".to_string(),
                    reason: "model_token_budget".to_string(),
                    iteration: iteration.saturating_add(1),
                    reported_model_tokens: state.budget.reported_model_tokens,
                    usage_complete: state.budget.usage_complete,
                })
                .await;
            }

            if state.budget.final_only && !think.tool_call_map.is_empty() {
                super::context::push_runtime_context_note(
                    &context,
                    "RunBudget:BlockedTools",
                    "Tool calls were ignored because this invocation is in final-only mode. Return a text answer.",
                )
                .await;
                continue;
            }

            // Branch: tool calls vs text answer vs no-response
            let outcome = if !think.tool_call_map.is_empty() {
                phases::tools::run_tools(&self, &context, &tx, &mut state, iteration, think, &label)
                    .await?
            } else if !think.content_buffer.is_empty() {
                let assistant_draft = phases::with_reasoning_content(
                    Message::assistant(think.content_buffer.clone()),
                    think.reasoning_buffer.clone(),
                    think.reasoning_blocks.clone(),
                );
                if self
                    .drain_steer_into_context(&context, Some(assistant_draft))
                    .await
                    > 0
                {
                    continue;
                }
                let pt = think.pt;
                let ct = think.ct;
                match phases::verify::verify_final_text(
                    &self, &context, &tx, &mut state, iteration, think, &label,
                )
                .await?
                {
                    IterOutcome::Continue => continue,
                    IterOutcome::FinalText {
                        answer,
                        reasoning_content,
                        reasoning_blocks,
                    } => {
                        match phases::finalize::emit_final_text(
                            &self,
                            &context,
                            &tx,
                            &mut state,
                            iteration,
                            pt,
                            ct,
                            answer,
                            reasoning_content,
                            reasoning_blocks,
                        )
                        .await?
                        {
                            ControlFlow::Continue(()) => continue,
                            ControlFlow::Break(()) => {
                                return Ok(crate::agent::AgentSteerTurnOutcome::Completed);
                            }
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
                    if self.drain_steer_into_context(&context, None).await > 0 {
                        continue;
                    }
                    match phases::finalize::finalize_completed_run(
                        &self, &context, &label, &output, iteration, &state, &tx,
                    )
                    .await?
                    {
                        ControlFlow::Continue(()) => {
                            state.stop_hook_continued = true;
                            continue;
                        }
                        ControlFlow::Break(()) => {
                            return Ok(crate::agent::AgentSteerTurnOutcome::Completed);
                        }
                    }
                }
                // FinalText is only produced by verify_final_text and is
                // already handled inline in the text branch above. Reaching
                // it here would mean a phase returned an outcome out of band
                // — guard against future refactors by treating it as a
                // terminal text emission.
                IterOutcome::FinalText {
                    answer,
                    reasoning_content,
                    reasoning_blocks,
                } => {
                    let pt = 0;
                    let ct = 0;
                    match phases::finalize::emit_final_text(
                        &self,
                        &context,
                        &tx,
                        &mut state,
                        iteration,
                        pt,
                        ct,
                        answer,
                        reasoning_content,
                        reasoning_blocks,
                    )
                    .await?
                    {
                        ControlFlow::Continue(()) => continue,
                        ControlFlow::Break(()) => {
                            return Ok(crate::agent::AgentSteerTurnOutcome::Completed);
                        }
                    }
                }
                IterOutcome::NoResponse => {
                    phases::finalize::finalize_no_response(&self, tx).await?;
                    return Ok(crate::agent::AgentSteerTurnOutcome::Failed);
                }
                IterOutcome::Abandoned => {
                    self.finalize_run(
                        crate::trace::RunStatus::Cancelled,
                        None,
                        Some("event consumer disconnected or tool batch was abandoned"),
                    )
                    .await;
                    return Ok(crate::agent::AgentSteerTurnOutcome::Cancelled);
                }
            }
        }

        // ── Post-loop: max iterations exceeded ───────────────────────
        phases::finalize::finalize_max_iterations(&self, &context, tx).await?;
        Ok(crate::agent::AgentSteerTurnOutcome::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::AgentConfig;
    use crate::compression::{ContextProjection, PreModelContextProjector, ProjectionContext};
    use crate::intent::{Intent, IntentClassifier, IntentRouter, IntentRouterConfig};
    use echo_core::agent::{Agent, AgentInputLifecycle};
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

    #[derive(Default)]
    struct InputLifecycleProbe {
        drained: std::sync::atomic::AtomicUsize,
    }

    impl AgentInputLifecycle for InputLifecycleProbe {
        fn mark_drained(&self) {
            self.drained
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn prepare_context_publishes_initial_input_drain_before_provider() -> Result<()> {
        let llm = Arc::new(MockLlmClient::new().with_response("done"));
        let agent = ReactAgentBuilder::new()
            .llm_client(llm)
            .system_prompt("system")
            .build()?;
        let probe = Arc::new(InputLifecycleProbe::default());
        let invocation = echo_core::agent::AgentInvocationContext {
            input_lifecycle: Some(probe.clone()),
            ..echo_core::agent::AgentInvocationContext::default()
        };
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "initial input".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation),
                },
                StreamMode::Chat,
            )
            .await?;
        let events = stream.collect::<Vec<_>>().await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))))
        );
        assert_eq!(probe.drained.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    struct AlwaysDirectClassifier;

    impl IntentClassifier for AlwaysDirectClassifier {
        fn classify<'a>(
            &'a self,
            _user_input: &'a str,
            _context: &'a [Message],
        ) -> BoxFuture<'a, Intent> {
            Box::pin(async { Intent::DirectAnswer { confidence: 1.0 } })
        }
    }

    struct RoutingProjection;

    impl PreModelContextProjector for RoutingProjection {
        fn project(
            &self,
            _context: &ProjectionContext,
        ) -> BoxFuture<'_, Result<Vec<ContextProjection>>> {
            Box::pin(async {
                Ok(vec![ContextProjection {
                    marker: "routing-test".to_string(),
                    message: Some(Message::user("required routing projection".to_string())),
                }])
            })
        }
    }

    struct BlockingRunIdProjection {
        run_ids: std::sync::Mutex<Vec<Option<String>>>,
        calls: std::sync::atomic::AtomicUsize,
        first_started: tokio::sync::Notify,
        release_first: tokio::sync::Notify,
    }

    impl BlockingRunIdProjection {
        fn new() -> Self {
            Self {
                run_ids: std::sync::Mutex::new(Vec::new()),
                calls: std::sync::atomic::AtomicUsize::new(0),
                first_started: tokio::sync::Notify::new(),
                release_first: tokio::sync::Notify::new(),
            }
        }
    }

    impl PreModelContextProjector for BlockingRunIdProjection {
        fn project<'a>(
            &'a self,
            context: &'a ProjectionContext,
        ) -> BoxFuture<'a, Result<Vec<ContextProjection>>> {
            Box::pin(async move {
                self.run_ids
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(context.run_id.clone());
                let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if call == 0 {
                    self.first_started.notify_one();
                    self.release_first.notified().await;
                }
                Ok(Vec::new())
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
                    invocation: None,
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

    #[tokio::test]
    async fn legacy_guard_block_trace_uses_captured_context() -> Result<()> {
        let mut agent = agent_with_blocking_guard();
        let store = Arc::new(crate::trace::InMemoryRunStore::new());
        agent.set_run_store(store.clone());
        agent.set_external_context(&echo_core::tools::ExternalRunContext {
            conversation_id: Some("blocked-conversation".to_string()),
            run_id: Some("blocked-parent".to_string()),
            turn_id: Some("blocked-turn".to_string()),
            execution_id: Some("blocked-execution".to_string()),
            isolation_id: None,
            message_id: Some("blocked-message".to_string()),
            cancel: None,
            trace_sink: None,
            delegation_policy: None,
            resource_guards: Vec::new(),
        });

        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "blocked input".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?;
        let _: Vec<_> = stream.collect().await;
        agent.clear_external_context();

        let traces = crate::trace::RunStore::list_all(store.as_ref(), 10).await?;
        let trace = traces
            .first()
            .ok_or_else(|| ReactError::Other("blocked trace missing".to_string()))?;
        assert_eq!(trace.parent_run_id.as_deref(), Some("blocked-parent"));
        assert_eq!(trace.session_id, "blocked-conversation");
        assert_eq!(trace.turn_id.as_deref(), Some("blocked-turn"));
        assert_eq!(trace.execution_id.as_deref(), Some("blocked-execution"));
        Ok(())
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
                    invocation: None,
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
                    invocation: None,
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
    use crate::error::{LlmError, ReactError};
    use crate::llm::types::{DeltaMessage, FunctionCall, ToolCall};
    use crate::state::RuntimeStateStore;
    use crate::testing::{MockLlmClient, MockTool, StreamChunk};

    struct InvocationRewrite;

    impl crate::agent::InterventionCallback for InvocationRewrite {
        fn on_tool_call<'a>(
            &'a self,
            _agent: &'a str,
            _tool: &'a str,
            _args: &'a serde_json::Value,
        ) -> futures::future::BoxFuture<'a, crate::agent::InterventionResult> {
            Box::pin(async {
                crate::agent::InterventionResult {
                    redirect_to: Some("effective_tool".to_string()),
                    modified_args: Some(serde_json::json!({"value": "rewritten"})),
                    ..crate::agent::InterventionResult::default()
                }
            })
        }
    }

    struct CapturingArgsTool {
        name: &'static str,
        calls: Arc<std::sync::Mutex<Vec<crate::tools::ToolParameters>>>,
        permissions: Vec<echo_core::tools::permission::ToolPermission>,
    }

    impl crate::tools::Tool for CapturingArgsTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "Returns the effective value and records the executed arguments"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            })
        }

        fn permissions(&self) -> Vec<echo_core::tools::permission::ToolPermission> {
            self.permissions.clone()
        }

        fn execute(
            &self,
            params: crate::tools::ToolParameters,
        ) -> futures::future::BoxFuture<'_, Result<crate::tools::ToolResult>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(params.clone());
                let value = params
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| ReactError::Other("effective value is missing".to_string()))?;
                Ok(crate::tools::ToolResult::success(value))
            })
        }
    }

    #[cfg(feature = "human-loop")]
    struct ModifiedArgsApproval;

    #[cfg(feature = "human-loop")]
    impl crate::human_loop::HumanLoopProvider for ModifiedArgsApproval {
        fn request(
            &self,
            _request: crate::human_loop::HumanLoopRequest,
        ) -> futures::future::BoxFuture<'_, Result<crate::human_loop::HumanLoopResponse>> {
            Box::pin(async {
                Ok(crate::human_loop::HumanLoopResponse::ModifiedArgs {
                    args: serde_json::json!({"value": "approved"}),
                    scope: crate::human_loop::ApprovalScope::Once,
                })
            })
        }
    }

    struct DelayedTerminalTool {
        started: Arc<tokio::sync::Notify>,
        completed: Arc<tokio::sync::Notify>,
        finished: Arc<std::sync::atomic::AtomicBool>,
    }

    impl crate::tools::Tool for DelayedTerminalTool {
        fn name(&self) -> &str {
            "delayed_terminal"
        }

        fn description(&self) -> &str {
            "Finishes a durable terminal write shortly after parent cancellation"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }

        fn execute(
            &self,
            _params: crate::tools::ToolParameters,
        ) -> futures::future::BoxFuture<'_, Result<crate::tools::ToolResult>> {
            Box::pin(async move {
                self.started.notify_one();
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
                self.finished
                    .store(true, std::sync::atomic::Ordering::Release);
                self.completed.notify_one();
                Ok(crate::tools::ToolResult::success(
                    "terminal state persisted",
                ))
            })
        }
    }

    #[derive(Default)]
    struct RecordingRuntimeStateStore {
        checkpoint: std::sync::Mutex<Option<crate::state::AgentCheckpoint>>,
        scopes: std::sync::Mutex<
            std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        >,
    }

    impl crate::state::RuntimeStateStore for RecordingRuntimeStateStore {
        fn get_checkpoint<'a>(
            &'a self,
            _conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, Result<Option<crate::state::AgentCheckpoint>>> {
            Box::pin(async move {
                Ok(self
                    .checkpoint
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone())
            })
        }

        fn save_checkpoint<'a>(
            &'a self,
            checkpoint: &'a crate::state::AgentCheckpoint,
        ) -> futures::future::BoxFuture<'a, Result<()>> {
            self.save_checkpoint_for_scope(&checkpoint.conversation_id, checkpoint)
        }

        fn save_checkpoint_for_scope<'a>(
            &'a self,
            scope_id: &'a str,
            checkpoint: &'a crate::state::AgentCheckpoint,
        ) -> futures::future::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.scopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entry(scope_id.to_string())
                    .or_default()
                    .insert(checkpoint.conversation_id.clone());
                *self
                    .checkpoint
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(checkpoint.clone());
                Ok(())
            })
        }

        fn runtime_state_ids<'a>(
            &'a self,
            scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, Result<Vec<String>>> {
            Box::pin(async move {
                Ok(self
                    .scopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(scope_id)
                    .map(|ids| ids.iter().cloned().collect())
                    .unwrap_or_default())
            })
        }

        fn clear_runtime_state<'a>(
            &'a self,
            scope_id: &'a str,
            runtime_state_id: &'a str,
        ) -> futures::future::BoxFuture<'a, Result<crate::state::RuntimeStateClearReceipt>>
        {
            Box::pin(async move {
                let mut scopes = self
                    .scopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let indexed = scopes
                    .get_mut(scope_id)
                    .is_some_and(|ids| ids.remove(runtime_state_id));
                if scopes.get(scope_id).is_some_and(|ids| ids.is_empty()) {
                    scopes.remove(scope_id);
                }
                drop(scopes);
                let mut checkpoint = self
                    .checkpoint
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let checkpoint_removed = checkpoint.as_ref().is_some_and(|checkpoint| {
                    checkpoint.conversation_id == runtime_state_id
                        && (indexed || scope_id == runtime_state_id)
                });
                if checkpoint_removed {
                    *checkpoint = None;
                }
                Ok(crate::state::RuntimeStateClearReceipt {
                    scope_id: scope_id.to_string(),
                    runtime_state_id: runtime_state_id.to_string(),
                    checkpoint_removed,
                })
            })
        }

        fn clear_runtime_state_scope<'a>(
            &'a self,
            scope_id: &'a str,
        ) -> futures::future::BoxFuture<'a, Result<crate::state::RuntimeStateScopeClearReceipt>>
        {
            Box::pin(async move {
                let mut runtime_state_ids = self
                    .scopes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(scope_id)
                    .map(|ids| ids.into_iter().collect::<Vec<_>>())
                    .unwrap_or_default();
                let mut checkpoint = self
                    .checkpoint
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.conversation_id == scope_id)
                    && !runtime_state_ids
                        .iter()
                        .any(|runtime_id| runtime_id == scope_id)
                {
                    runtime_state_ids.push(scope_id.to_string());
                    runtime_state_ids.sort();
                }
                if checkpoint.as_ref().is_some_and(|checkpoint| {
                    runtime_state_ids
                        .iter()
                        .any(|runtime_id| runtime_id == &checkpoint.conversation_id)
                }) {
                    *checkpoint = None;
                }
                Ok(crate::state::RuntimeStateScopeClearReceipt {
                    scope_id: scope_id.to_string(),
                    runtime_state_ids,
                })
            })
        }

        fn clear_conversation<'a>(
            &'a self,
            conversation_id: &'a str,
        ) -> futures::future::BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.clear_runtime_state(conversation_id, conversation_id)
                    .await
                    .map(|_receipt| ())
            })
        }
    }

    /// Build a streaming-capable agent backed by a scripted mock LLM, with
    /// no guard and no intent router (so requests reach run_core_loop).
    fn agent_with_mock_llm(llm: MockLlmClient) -> ReactAgent {
        ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("You are a test assistant.")
            .build()
            .expect("agent builds")
    }

    fn agent_with_direct_router_and_projection(llm: Arc<MockLlmClient>) -> Result<ReactAgent> {
        let router = IntentRouter::new(
            Box::new(AlwaysDirectClassifier),
            IntentRouterConfig::default(),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm)
            .intent_router(router)
            .system_prompt("You are a test assistant.")
            .build()?;
        agent.set_pre_model_context_projector(Some(Arc::new(RoutingProjection)));
        Ok(agent)
    }

    #[tokio::test]
    async fn queued_stream_keeps_invocation_run_contexts_atomic() -> Result<()> {
        let llm = MockLlmClient::new().with_responses(["first", "second"]);
        let agent = Arc::new(agent_with_mock_llm(llm));
        let projector = Arc::new(BlockingRunIdProjection::new());
        agent.set_pre_model_context_projector(Some(projector.clone()));
        let invocation_a = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("run-a".to_string()),
                turn_id: None,
                execution_id: Some("execution-a".to_string()),
                isolation_id: None,
                message_id: Some("message-a".to_string()),
                cancel: None,
                trace_sink: None,
                delegation_policy: Some(echo_core::tools::NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: 1,
                    max_delegate_depth: 2,
                }),
                resource_guards: Vec::new(),
            }),
            runtime_state_id: None,
            transcript_generation_id: None,
            working_dir: Some(std::path::PathBuf::from("/tmp/worktree-a")),
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };
        let first_stream = agent
            .execute_stream_with_invocation_context(
                "first",
                crate::agent::CancellationToken::new(),
                invocation_a,
            )
            .await?;
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            projector.first_started.notified(),
        )
        .await
        .map_err(|_| {
            crate::error::ReactError::Other("first projection did not start".to_string())
        })?;

        let invocation_b = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("run-b".to_string()),
                turn_id: None,
                execution_id: Some("execution-b".to_string()),
                isolation_id: None,
                message_id: Some("message-b".to_string()),
                cancel: None,
                trace_sink: None,
                delegation_policy: Some(echo_core::tools::NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: 2,
                    max_delegate_depth: 3,
                }),
                resource_guards: Vec::new(),
            }),
            runtime_state_id: None,
            transcript_generation_id: None,
            working_dir: Some(std::path::PathBuf::from("/tmp/worktree-b")),
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };
        let mut queued = Box::pin(agent.execute_stream_with_invocation_context(
            "second",
            crate::agent::CancellationToken::new(),
            invocation_b,
        ));
        tokio::select! {
            result = &mut queued => {
                return Err(crate::error::ReactError::Other(format!(
                    "queued stream unexpectedly bypassed execution mutex: {:?}",
                    result.is_ok()
                )));
            }
            _ = tokio::task::yield_now() => {}
        }
        projector.release_first.notify_one();

        let _: Vec<_> = first_stream.collect().await;
        let second_stream = queued.await?;
        let _: Vec<_> = second_stream.collect().await;
        let run_ids = projector
            .run_ids
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(
            run_ids,
            vec![Some("run-a".to_string()), Some("run-b".to_string())]
        );
        Ok(())
    }

    #[tokio::test]
    async fn value_scoped_stream_does_not_mutate_agent_run_id() -> Result<()> {
        let mut agent = agent_with_mock_llm(MockLlmClient::new().with_response("done"));
        agent.set_run_store(Arc::new(crate::trace::InMemoryRunStore::new()));
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("value-run".to_string()),
                turn_id: None,
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
                resource_guards: Vec::new(),
            }),
            runtime_state_id: None,
            transcript_generation_id: None,
            working_dir: None,
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };

        let stream = agent
            .execute_stream_with_invocation_context(
                "run",
                crate::agent::CancellationToken::new(),
                invocation,
            )
            .await?;
        let _: Vec<_> = stream.collect().await;
        assert!(agent.current_run_id().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn value_scoped_direct_answer_records_usage_in_child_trace() -> Result<()> {
        use crate::trace::RunStore;

        let usage = crate::llm::types::Usage {
            prompt_tokens: Some(500),
            completion_tokens: Some(40),
            total_tokens: Some(540),
            prompt_tokens_details: Some(crate::llm::types::TokenUsageDetails {
                cached_tokens: Some(400),
                ..Default::default()
            }),
            ..Default::default()
        };
        let router = IntentRouter::new(
            Box::new(AlwaysDirectClassifier),
            IntentRouterConfig::default(),
        );
        let mut agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(
                MockLlmClient::new().with_response_usage("done", usage),
            ))
            .intent_router(router)
            .system_prompt("You are a test assistant.")
            .build()?;
        let store = Arc::new(crate::trace::InMemoryRunStore::new());
        agent.set_run_store(store.clone());
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("value-direct-run".to_string()),
                turn_id: None,
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
                resource_guards: Vec::new(),
            }),
            runtime_state_id: None,
            transcript_generation_id: None,
            working_dir: None,
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };

        let stream = agent
            .execute_stream_with_invocation_context(
                "run",
                crate::agent::CancellationToken::new(),
                invocation,
            )
            .await?;
        let _: Vec<_> = stream.collect().await;
        let child = store
            .list_by_parent_run("value-direct-run")
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| crate::error::ReactError::Other("child trace missing".into()))?;
        assert_ne!(child.run_id, "value-direct-run");
        assert_eq!(child.parent_run_id.as_deref(), Some("value-direct-run"));
        let run = store
            .load(&child.run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("child trace run missing".into()))?;
        assert!(run.events.iter().any(|event| matches!(
            event,
            crate::trace::RunEvent::LlmCall {
                prompt_tokens: 500,
                cached_prompt_tokens: 400,
                usage_reported: true,
                ..
            }
        )));
        assert!(agent.current_run_id().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn non_streaming_direct_answer_routes_through_projection_boundary() -> Result<()> {
        let llm = Arc::new(MockLlmClient::new().with_response("normal loop answer"));
        let agent = agent_with_direct_router_and_projection(llm.clone())?;

        let _ = agent.run_chat_direct("hello").await?;

        let messages = llm.last_messages().unwrap_or_default();
        assert!(messages.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("required routing projection"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn streaming_direct_answer_routes_through_projection_boundary() -> Result<()> {
        let llm = Arc::new(MockLlmClient::new().with_response("normal loop answer"));
        let agent = agent_with_direct_router_and_projection(llm.clone())?;

        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "hello".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?;
        let _: Vec<_> = stream.collect().await;

        let messages = llm.last_messages().unwrap_or_default();
        assert!(messages.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("required routing projection"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn sequential_execute_stream_calls_reset_prior_task_messages() -> Result<()> {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_response("first answer")
                .with_response("second answer"),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("You are a test assistant.")
            .build()?;

        let first = agent
            .execute_stream_with_cancel("first task", crate::agent::CancellationToken::new())
            .await?;
        let _: Vec<_> = first.collect().await;
        let second = agent
            .execute_stream_with_cancel("second task", crate::agent::CancellationToken::new())
            .await?;
        let _: Vec<_> = second.collect().await;

        let calls = llm.all_calls();
        let second_messages = calls.get(1).ok_or_else(|| {
            crate::error::ReactError::Other("missing second LLM call".to_string())
        })?;
        assert!(second_messages.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("second task"))
        }));
        assert!(second_messages.iter().all(|message| {
            message
                .content
                .as_text()
                .is_none_or(|text| !text.contains("first task"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn sequential_structured_chat_calls_preserve_prior_turn_messages() -> Result<()> {
        let llm = Arc::new(
            MockLlmClient::new()
                .with_response("first answer")
                .with_response("second answer"),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("You are a test assistant.")
            .build()?;

        let first = agent
            .chat_stream_message_with_invocation_context(
                Message::user("first question".to_string()),
                crate::agent::CancellationToken::new(),
                echo_core::agent::AgentInvocationContext::default(),
            )
            .await?;
        let _: Vec<_> = first.collect().await;
        let second = agent
            .chat_stream_message_with_invocation_context(
                Message::user("second question".to_string()),
                crate::agent::CancellationToken::new(),
                echo_core::agent::AgentInvocationContext::default(),
            )
            .await?;
        let _: Vec<_> = second.collect().await;

        let calls = llm.all_calls();
        let second_messages = calls.get(1).ok_or_else(|| {
            crate::error::ReactError::Other("missing second LLM call".to_string())
        })?;
        for expected in ["first question", "first answer", "second question"] {
            assert!(second_messages.iter().any(|message| {
                message
                    .content
                    .as_text()
                    .is_some_and(|text| text.contains(expected))
            }));
        }
        Ok(())
    }

    /// Collect the AgentEvents emitted by one streaming turn.
    async fn collect_events(agent: &ReactAgent, text: &str) -> Vec<AgentEvent> {
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: text.into(),
                    message: None,
                    label: String::new(),
                    invocation: None,
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

    async fn collect_events_result(agent: &ReactAgent, text: &str) -> Result<Vec<AgentEvent>> {
        let mut stream = agent
            .run_stream_channel(
                StreamInit {
                    text: text.into(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event?);
        }
        Ok(events)
    }

    #[tokio::test]
    async fn q_flt_v09_effective_rewrite_is_the_only_executed_invocation() -> Result<()> {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let llm = MockLlmClient::new()
            .then_tool_call("rewrite-1", "requested_tool", r#"{"value":"requested"}"#)
            .with_response("done");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("Use the requested tool.")
            .tool(Box::new(
                MockTool::new("requested_tool").with_response("wrong tool"),
            ))
            .tool(Box::new(CapturingArgsTool {
                name: "effective_tool",
                calls: calls.clone(),
                permissions: Vec::new(),
            }))
            .intervention_callback(Arc::new(InvocationRewrite))
            .build()?;

        let events = collect_events_result(&agent, "rewrite the call").await?;
        let invocation = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall {
                    call_id,
                    invocation,
                } if call_id == "rewrite-1" => Some(invocation),
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("canonical ToolCall was not emitted".to_string()))?;
        assert_eq!(invocation.requested_name, "requested_tool");
        assert_eq!(
            invocation.requested_args,
            serde_json::json!({"value": "requested"})
        );
        assert_eq!(invocation.name, "effective_tool");
        assert_eq!(invocation.args, serde_json::json!({"value": "rewritten"}));
        assert_eq!(
            invocation.rewrites,
            vec![
                crate::agent::ToolInvocationRewrite::InterventionRedirect,
                crate::agent::ToolInvocationRewrite::InterventionArguments,
            ]
        );
        let terminal = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult {
                    call_id,
                    name,
                    result,
                } if call_id == "rewrite-1" => Some((name, result)),
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("typed ToolResult was not emitted".to_string()))?;
        assert_eq!(terminal.0, "effective_tool");
        assert_eq!(terminal.1.output, "rewritten");
        let executed = calls.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(executed.len(), 1);
        assert_eq!(
            executed
                .first()
                .and_then(|params| params.get("value"))
                .and_then(serde_json::Value::as_str),
            Some("rewritten")
        );
        Ok(())
    }

    #[cfg(feature = "human-loop")]
    #[tokio::test]
    async fn react_emits_and_executes_approval_modified_arguments() -> Result<()> {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let service = Arc::new(crate::human_loop::PermissionService::from_provider(
            Arc::new(ModifiedArgsApproval),
        ));
        let llm = MockLlmClient::new()
            .then_tool_call("approval-1", "approval_tool", r#"{"value":"requested"}"#)
            .with_response("done");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("Use the approval tool.")
            .enable_human_in_loop()
            .permission_service(service)
            .tool(Box::new(CapturingArgsTool {
                name: "approval_tool",
                calls: calls.clone(),
                permissions: vec![echo_core::tools::permission::ToolPermission::Write],
            }))
            .build()?;

        let events = collect_events_result(&agent, "run the approved call").await?;
        let invocation = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall {
                    call_id,
                    invocation,
                } if call_id == "approval-1" => Some(invocation),
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("approval ToolCall was not emitted".to_string()))?;
        assert_eq!(
            invocation.requested_args,
            serde_json::json!({"value": "requested"})
        );
        assert_eq!(invocation.args, serde_json::json!({"value": "approved"}));
        assert_eq!(
            invocation.rewrites,
            vec![crate::agent::ToolInvocationRewrite::Approval]
        );
        let result = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult {
                    call_id, result, ..
                } if call_id == "approval-1" => Some(result),
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("approval ToolResult was not emitted".to_string()))?;
        assert_eq!(result.output, "approved");
        let executed = calls.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            executed
                .first()
                .and_then(|params| params.get("value"))
                .and_then(serde_json::Value::as_str),
            Some("approved")
        );
        Ok(())
    }

    async fn assert_spilled_tool_result(
        agent: &ReactAgent,
        call_id: &str,
        marker_key: &str,
        marker_value: &str,
        original: &str,
    ) -> Result<()> {
        let events = collect_events_result(agent, "produce a large result").await?;
        assert!(events.iter().all(|event| {
            !matches!(
                event,
                AgentEvent::ToolStream {
                    event: crate::tools::ToolStreamEvent::Complete(_),
                    ..
                }
            )
        }));
        let result = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult {
                    call_id: current,
                    result,
                    ..
                } if current == call_id => Some(result),
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("spilled ToolResult was not emitted".to_string()))?;
        assert!(result.success);
        assert!(result.truncated);
        assert_eq!(
            result.metadata.get(marker_key).map(String::as_str),
            Some(marker_value)
        );
        assert_eq!(
            result.metadata.get("output_handling").map(String::as_str),
            Some("spilled")
        );
        let artifact =
            echo_core::tools::artifact::ToolOutputArtifactRef::from_metadata(&result.metadata)
                .ok_or_else(|| {
                    ReactError::Other("ToolResult lost its artifact reference".to_string())
                })?;
        let recovered = std::fs::read_to_string(&artifact.path)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(recovered, original);
        Ok(())
    }

    #[tokio::test]
    async fn q_flt_v03_huge_output_spills_to_digest_bound_artifact() -> Result<()> {
        let artifact_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let original = format!("{}END", "non-stream 中文🙂\n".repeat(2_000));
        let llm = MockLlmClient::new()
            .then_tool_call("non-stream-1", "large_non_stream", "{}")
            .with_response("done");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("Use the large output tool.")
            .tool_output_artifacts(
                echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                    artifact_dir.path(),
                    "test",
                )
                .threshold_bytes(8),
            )
            .tool(Box::new(
                MockTool::new("large_non_stream").with_result(
                    crate::tools::ToolResult::success(original.clone())
                        .with_meta("transport", "non_stream"),
                ),
            ))
            .build()?;

        assert_spilled_tool_result(&agent, "non-stream-1", "transport", "non_stream", &original)
            .await
    }

    #[tokio::test]
    async fn react_streaming_tool_preserves_rich_spilled_terminal() -> Result<()> {
        let artifact_dir =
            tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let original = format!("{}END", "stream 中文🙂\n".repeat(2_000));
        let llm = MockLlmClient::new()
            .then_tool_call("stream-1", "large_stream", "{}")
            .with_response("done");
        let stream_result =
            crate::tools::ToolResult::success(original.clone()).with_meta("transport", "stream");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("Use the streaming output tool.")
            .tool_output_artifacts(
                echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                    artifact_dir.path(),
                    "test",
                )
                .threshold_bytes(8),
            )
            .tool(Box::new(MockTool::new("large_stream").with_stream_script(
                vec![
                    crate::tools::ToolStreamEvent::Output {
                        channel: crate::tools::ToolOutputChannel::Stdout,
                        chunk: "partial".to_string(),
                    },
                    crate::tools::ToolStreamEvent::Complete(stream_result),
                ],
            )))
            .build()?;

        assert_spilled_tool_result(&agent, "stream-1", "transport", "stream", &original).await
    }

    #[tokio::test]
    async fn activate_skill_tool_replaces_protected_projection_across_compression() -> Result<()> {
        let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let skill_dir = root.path().join("replaceable-skill");
        std::fs::create_dir_all(&skill_dir)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: replaceable-skill\ndescription: Replaceable instructions\n---\nActivation argument: ${ARGUMENTS}\n",
        )
        .map_err(|error| ReactError::Other(error.to_string()))?;

        let llm = MockLlmClient::new()
            .then_tool_call(
                "activate-first",
                "activate_skill",
                r#"{"name":"replaceable-skill","arguments":"first"}"#,
            )
            .then_tool_call(
                "activate-second",
                "activate_skill",
                r#"{"name":"replaceable-skill","arguments":"second"}"#,
            )
            .with_response("done");
        let mut agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("Activate the requested skill.")
            .build()?;
        agent
            .discover_skills(&[crate::skills::external::DiscoveryScope::Custom(
                root.path().to_path_buf(),
            )])
            .await?;

        let events = collect_events_result(&agent, "activate and refresh the skill").await?;
        let activations = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::ToolResult {
                        result: crate::tools::ToolResult {
                            kind: echo_core::tools::ToolResultKind::SkillActivation { name },
                            ..
                        },
                        ..
                    } if name == "replaceable-skill"
                )
            })
            .count();
        assert_eq!(activations, 2);

        let compressor = crate::compression::compressor::SlidingWindowCompressor::new(1);
        agent.force_compress_with(&compressor).await?;
        let messages = agent.memory.context.lock().await.messages().to_vec();
        let projections = messages
            .iter()
            .filter_map(|message| message.content.as_text_ref())
            .filter(|text| text.contains("echo-agent:skill:replaceable-skill"))
            .collect::<Vec<_>>();
        assert_eq!(projections.len(), 1);
        assert!(projections.first().is_some_and(|text| {
            text.contains("Activation argument: second")
                && !text.contains("Activation argument: first")
        }));
        Ok(())
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

    #[tokio::test]
    async fn deepseek_tool_turn_replays_complete_assistant_message() -> Result<()> {
        let llm = Arc::new(
            MockLlmClient::new()
                .then_reasoning_tool_call(
                    "call_1",
                    "mock_calc",
                    r#"{"x":6,"y":7}"#,
                    "I will calculate that.",
                    "The user requested a multiplication, so I should use the calculator.",
                )
                .with_response("The result is 42."),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("You are a test assistant. Use tools when asked.")
            .tool(Box::new(MockTool::new("mock_calc").with_response("42")))
            .build()?;

        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "What is 6 times 7?".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?;
        let _events = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let calls = llm.all_calls();
        let second_request = calls.get(1).ok_or_else(|| {
            crate::error::ReactError::Other(
                "DeepSeek tool cycle did not issue a second request".to_string(),
            )
        })?;
        let assistant = second_request
            .iter()
            .find(|message| message.role == crate::llm::types::Role::Assistant)
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "second DeepSeek request did not replay the assistant tool turn".to_string(),
                )
            })?;

        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("The user requested a multiplication, so I should use the calculator.")
        );
        assert_eq!(
            assistant.content.as_text().as_deref(),
            Some("I will calculate that.")
        );
        assert_eq!(
            assistant.tool_calls.as_ref().map(Vec::len),
            Some(1),
            "the replayed assistant message must retain its tool call"
        );
        Ok(())
    }

    #[tokio::test]
    async fn q_flt_v01_malformed_tool_json_has_no_phantom_execution() -> Result<()> {
        let llm = Arc::new(
            MockLlmClient::new()
                .then_reasoning_tool_call(
                    "call_1",
                    "mock_calc",
                    r#"{"x":6"#,
                    "",
                    "I should call the calculator, but the arguments were truncated.",
                )
                .with_response("I will retry with a complete call next time."),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("You are a test assistant. Use tools when asked.")
            .tool(Box::new(MockTool::new("mock_calc").with_response("42")))
            .build()?;

        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "What is 6 times 7?".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?;
        let _events = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let calls = llm.all_calls();
        let second_request = calls.get(1).ok_or_else(|| {
            crate::error::ReactError::Other(
                "malformed DeepSeek tool turn did not reach its retry request".to_string(),
            )
        })?;
        let assistant = second_request
            .iter()
            .find(|message| message.role == crate::llm::types::Role::Assistant)
            .ok_or_else(|| {
                crate::error::ReactError::Other(
                    "retry request did not contain the recovered assistant turn".to_string(),
                )
            })?;

        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("I should call the calculator, but the arguments were truncated.")
        );
        assert!(assistant.tool_calls.is_none());
        assert!(
            assistant.content.as_text().is_some_and(|content| {
                content.contains("流式工具调用参数解析失败")
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn q_flt_v08_completed_batch_effects_are_checkpointed_before_progress() -> Result<()> {
        let store = Arc::new(RecordingRuntimeStateStore::default());
        let llm = MockLlmClient::new()
            .then_tool_call("write-1", "mock_write", r#"{"path":"结果-🧪.md"}"#)
            .with_response("done");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .tool(Box::new(
                MockTool::new("mock_write").with_response("written"),
            ))
            .conversation_id("resume-conversation")
            .state_store(store.clone())
            .build()?;

        let mut stream = agent.execute_stream("write it").await?;
        while let Some(event) = stream.next().await {
            if matches!(event?, AgentEvent::ToolBatchEnd) {
                break;
            }
        }
        drop(stream);

        let checkpoint = store
            .get_checkpoint("resume-conversation")
            .await?
            .ok_or_else(|| {
                crate::error::ReactError::Other("tool batch checkpoint missing".to_string())
            })?;
        assert_eq!(checkpoint.completed_tool_call_ids()?, vec!["write-1"]);
        assert!(checkpoint.restore_messages()?.len() >= 2);
        Ok(())
    }

    #[tokio::test]
    async fn resume_records_checkpoint_origin_and_completed_tools_in_trace() -> Result<()> {
        use crate::trace::RunStore;

        let store = Arc::new(RecordingRuntimeStateStore::default());
        let checkpoint_messages = vec![
            Message::system("system".to_string()),
            Message::assistant_with_tools(vec![echo_core::llm::types::ToolCall {
                id: "write-1".to_string(),
                call_type: "function".to_string(),
                function: echo_core::llm::types::FunctionCall {
                    name: "mock_write".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            Message::tool_result(
                "write-1".to_string(),
                "mock_write".to_string(),
                "written".to_string(),
            ),
        ];
        let mut checkpoint = crate::state::AgentCheckpoint::new("resume-conversation");
        checkpoint.messages_json = serde_json::to_string(&checkpoint_messages)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        store.save_checkpoint(&checkpoint).await?;

        let run_store = Arc::new(crate::trace::InMemoryRunStore::new());
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new().with_response("resumed")))
            .conversation_id("resume-conversation")
            .state_store(store)
            .with_run_store(run_store.clone())
            .build()?;
        let events = agent
            .run_stream_channel(
                StreamInit {
                    text: "continue".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Execute,
            )
            .await?
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::FinalAnswer(answer) if answer == "resumed"
            )),
            "unexpected resume events: {events:?}"
        );

        let summary = run_store
            .list_all(1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| crate::error::ReactError::Other("trace run missing".to_string()))?;
        let run = run_store
            .load(&summary.run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("trace payload missing".to_string()))?;
        assert!(
            run.events.iter().any(|event| matches!(
                event,
                crate::trace::RunEvent::CheckpointResumed {
                    conversation_id,
                    completed_tool_call_ids,
                    ..
                } if conversation_id == "resume-conversation"
                    && completed_tool_call_ids == &["write-1".to_string()]
            )),
            "resume trace missing from events: {:?}",
            run.events
        );
        Ok(())
    }

    #[tokio::test]
    async fn invocation_runtime_state_identity_controls_restore_and_save() -> Result<()> {
        let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);

        let mut configured = crate::state::AgentCheckpoint::new("configured-state");
        configured.messages_json = serde_json::to_string(&vec![
            Message::system("system".to_string()),
            Message::user("configured checkpoint marker".to_string()),
        ])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        store.save_checkpoint(&configured).await?;

        let mut invocation_checkpoint = crate::state::AgentCheckpoint::new("runtime-incarnation");
        invocation_checkpoint.messages_json = serde_json::to_string(&vec![
            Message::system("system".to_string()),
            Message::user("invocation checkpoint marker".to_string()),
        ])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        store
            .save_checkpoint_for_scope("product-conversation", &invocation_checkpoint)
            .await?;

        let llm = Arc::new(MockLlmClient::new().with_response("new incarnation answer"));
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("system")
            .conversation_id("configured-state")
            .state_store(store.clone())
            .build()?;
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: Some("product-conversation".to_string()),
                run_id: Some("run-new".to_string()),
                turn_id: Some("turn-new".to_string()),
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
                resource_guards: Vec::new(),
            }),
            runtime_state_id: Some("runtime-incarnation".to_string()),
            transcript_generation_id: Some("runtime-incarnation".to_string()),
            ..echo_core::agent::AgentInvocationContext::default()
        };

        let events = agent
            .run_stream_channel(
                StreamInit {
                    text: "continue in the new incarnation".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation),
                },
                StreamMode::Chat,
            )
            .await?
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::FinalAnswer(answer) if answer == "new incarnation answer")
        ));

        let request = llm
            .last_messages()
            .ok_or_else(|| ReactError::Other("mock LLM request was not recorded".to_string()))?;
        assert!(request.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "invocation checkpoint marker")
        }));
        assert!(!request.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "configured checkpoint marker")
        }));

        let configured_after = store
            .get_checkpoint("configured-state")
            .await?
            .ok_or_else(|| ReactError::Other("configured checkpoint disappeared".to_string()))?;
        assert_eq!(configured_after.messages_json, configured.messages_json);
        let invocation_after = store
            .get_checkpoint("runtime-incarnation")
            .await?
            .ok_or_else(|| ReactError::Other("invocation checkpoint was not saved".to_string()))?;
        let restored = invocation_after.restore_messages()?;
        assert!(restored.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "new incarnation answer")
        }));
        assert!(!restored.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "configured checkpoint marker")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn warm_agent_switches_runtime_state_identity_without_context_leakage() -> Result<()> {
        let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
        let mut configured = crate::state::AgentCheckpoint::new("configured-state");
        configured.messages_json = serde_json::to_string(&vec![
            Message::system("system".to_string()),
            Message::user("configured-only-marker".to_string()),
        ])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        store.save_checkpoint(&configured).await?;
        let llm = Arc::new(MockLlmClient::new().with_responses([
            "assistant-a-one",
            "assistant-b-one",
            "assistant-a-two",
            "assistant-configured",
        ]));
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("system")
            .conversation_id("configured-state")
            .state_store(store.clone())
            .build()?;
        agent.set_snapshot_manager(crate::memory::SnapshotManager::new(
            crate::memory::SnapshotPolicy::Manual,
            4,
        ));
        let invocation =
            |runtime_state_id: &str, turn_id: &str| echo_core::agent::AgentInvocationContext {
                runtime: Some(echo_core::tools::ExternalRunContext {
                    conversation_id: Some("product-conversation".to_string()),
                    run_id: Some(turn_id.to_string()),
                    turn_id: Some(turn_id.to_string()),
                    execution_id: None,
                    isolation_id: None,
                    message_id: None,
                    cancel: None,
                    trace_sink: None,
                    delegation_policy: None,
                    resource_guards: Vec::new(),
                }),
                runtime_state_id: Some(runtime_state_id.to_string()),
                transcript_generation_id: Some(runtime_state_id.to_string()),
                ..echo_core::agent::AgentInvocationContext::default()
            };

        for (index, (runtime_state_id, turn_id, input)) in [
            ("runtime-a", "turn-a-one", "user-a-one"),
            ("runtime-b", "turn-b-one", "user-b-one"),
            ("runtime-a", "turn-a-two", "user-a-two"),
        ]
        .into_iter()
        .enumerate()
        {
            agent
                .run_stream_channel(
                    StreamInit {
                        text: input.to_string(),
                        message: None,
                        label: String::new(),
                        invocation: Some(invocation(runtime_state_id, turn_id)),
                    },
                    StreamMode::Chat,
                )
                .await?
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?;
            if index == 0 {
                let snapshot_id = agent.snapshot().await.ok_or_else(|| {
                    ReactError::Other("runtime A snapshot was not captured".to_string())
                })?;
                assert!(!snapshot_id.is_empty());
                *agent.plan_state.write().await = Some("runtime-a-only-plan".to_string());
                agent
                    .tools
                    .skill_registry
                    .mark_activated("runtime-a-only-skill");
                agent.set_working_dir(Some(std::path::PathBuf::from(
                    "/tmp/runtime-a-only-working-dir",
                )));
            } else if index == 1 {
                assert!(
                    agent.rollback(1).await.is_none(),
                    "runtime B retained runtime A rollback snapshots"
                );
            }
        }

        agent
            .run_stream_channel(
                StreamInit {
                    text: "user-configured".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let calls = llm.all_calls();
        let call = |index: usize| {
            calls
                .get(index)
                .ok_or_else(|| ReactError::Other(format!("mock LLM call {index} was not recorded")))
        };
        let contains = |messages: &[Message], expected: &str| {
            messages.iter().any(|message| {
                message
                    .text_content()
                    .is_some_and(|content| content == expected)
            })
        };
        let first = call(0)?;
        assert!(contains(first, "user-a-one"));
        assert!(!contains(first, "user-b-one"));
        let second = call(1)?;
        assert!(contains(second, "user-b-one"));
        assert!(!contains(second, "user-a-one"));
        assert!(!contains(second, "assistant-a-one"));
        let third = call(2)?;
        assert!(contains(third, "user-a-one"));
        assert!(contains(third, "assistant-a-one"));
        assert!(contains(third, "user-a-two"));
        assert!(!contains(third, "user-b-one"));
        assert!(!contains(third, "assistant-b-one"));
        let configured_call = call(3)?;
        assert!(contains(configured_call, "configured-only-marker"));
        assert!(contains(configured_call, "user-configured"));
        assert!(!contains(configured_call, "user-a-one"));
        assert!(!contains(configured_call, "user-b-one"));

        let runtime_a = store
            .get_checkpoint("runtime-a")
            .await?
            .ok_or_else(|| ReactError::Other("runtime A checkpoint missing".to_string()))?
            .restore_messages()?;
        let runtime_b_checkpoint = store
            .get_checkpoint("runtime-b")
            .await?
            .ok_or_else(|| ReactError::Other("runtime B checkpoint missing".to_string()))?;
        assert!(runtime_b_checkpoint.current_plan.is_none());
        assert!(runtime_b_checkpoint.active_skills.is_empty());
        assert!(runtime_b_checkpoint.working_dir.is_none());
        let runtime_b = runtime_b_checkpoint.restore_messages()?;
        assert!(contains(&runtime_a, "user-a-one"));
        assert!(contains(&runtime_a, "user-a-two"));
        assert!(!contains(&runtime_a, "user-b-one"));
        assert!(contains(&runtime_b, "user-b-one"));
        assert!(!contains(&runtime_b, "user-a-one"));
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_runtime_switch_cannot_publish_partial_hydration() -> Result<()> {
        use crate::skills::hooks::{HookAction, HookEvent, HookRule, HooksDefinition};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
        let mut runtime_b = crate::state::AgentCheckpoint::new("runtime-b");
        runtime_b.messages_json = serde_json::to_string(&vec![
            Message::system("system".to_string()),
            Message::user("runtime-b-checkpoint-marker".to_string()),
        ])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        store
            .save_checkpoint_for_scope("product-conversation", &runtime_b)
            .await?;
        let llm = Arc::new(
            MockLlmClient::new()
                .with_responses(["runtime-a-first-answer", "runtime-a-recovered-answer"]),
        );
        let agent = Arc::new(
            ReactAgentBuilder::new()
                .llm_client(llm.clone())
                .system_prompt("system")
                .conversation_id("configured-state")
                .state_store(store)
                .build()?,
        );
        let invocation =
            |runtime_state_id: &str, turn_id: &str| echo_core::agent::AgentInvocationContext {
                runtime: Some(echo_core::tools::ExternalRunContext {
                    conversation_id: Some("product-conversation".to_string()),
                    run_id: Some(turn_id.to_string()),
                    turn_id: Some(turn_id.to_string()),
                    execution_id: None,
                    isolation_id: None,
                    message_id: None,
                    cancel: None,
                    trace_sink: None,
                    delegation_policy: None,
                    resource_guards: Vec::new(),
                }),
                runtime_state_id: Some(runtime_state_id.to_string()),
                transcript_generation_id: Some(runtime_state_id.to_string()),
                ..echo_core::agent::AgentInvocationContext::default()
            };

        agent
            .run_stream_channel(
                StreamInit {
                    text: "runtime-a-first-user".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation("runtime-a", "turn-a-first")),
                },
                StreamMode::Chat,
            )
            .await?
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let hook_entered = Arc::new(tokio::sync::Notify::new());
        let executor_entered = Arc::clone(&hook_entered);
        let attempts = Arc::new(AtomicUsize::new(0));
        let executor_attempts = Arc::clone(&attempts);
        {
            let mut hooks = agent.tools.hook_registry.write().await;
            hooks.set_subagent_executor(Arc::new(move |_name, _task| {
                let entered = Arc::clone(&executor_entered);
                let attempts = Arc::clone(&executor_attempts);
                Box::pin(async move {
                    if attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                        entered.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok("session hook settled".to_string())
                })
            }));
            let mut definition = HooksDefinition::default();
            definition.add_rules(
                HookEvent::SessionStart,
                vec![HookRule {
                    matcher: "resume".to_string(),
                    hooks: vec![HookAction::Subagent {
                        name: "hydration-barrier".to_string(),
                        task: None,
                        timeout: 0,
                    }],
                }],
            );
            hooks.register_user_hooks(definition);
        }

        let switching_agent = Arc::clone(&agent);
        let switching_invocation = invocation("runtime-b", "turn-b-cancelled");
        let switching = tokio::spawn(async move {
            switching_agent
                .run_stream_channel(
                    StreamInit {
                        text: "runtime-b-cancelled-user".to_string(),
                        message: None,
                        label: String::new(),
                        invocation: Some(switching_invocation),
                    },
                    StreamMode::Chat,
                )
                .await
                .map(|_stream| ())
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), hook_entered.notified())
            .await
            .map_err(|_| ReactError::Other("runtime B hydration did not reach hook".to_string()))?;
        switching.abort();
        let cancelled = switching
            .await
            .err()
            .ok_or_else(|| ReactError::Other("runtime B switch was not cancelled".to_string()))?;
        assert!(cancelled.is_cancelled());

        agent
            .run_stream_channel(
                StreamInit {
                    text: "runtime-a-after-cancel".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation("runtime-a", "turn-a-recovered")),
                },
                StreamMode::Chat,
            )
            .await?
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let calls = llm.all_calls();
        assert_eq!(calls.len(), 2);
        let recovered = calls.get(1).ok_or_else(|| {
            ReactError::Other("recovered runtime A call was not recorded".to_string())
        })?;
        assert!(recovered.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "runtime-a-first-user")
        }));
        assert!(!recovered.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "runtime-b-checkpoint-marker")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn legacy_runtime_conversation_controls_restore_and_save() -> Result<()> {
        let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(root.path())?);
        let mut configured = crate::state::AgentCheckpoint::new("configured-a");
        configured.messages_json = serde_json::to_string(&vec![
            Message::system("system".to_string()),
            Message::user("configured-a-marker".to_string()),
        ])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        store.save_checkpoint(&configured).await?;
        let mut legacy = crate::state::AgentCheckpoint::new("legacy-b");
        legacy.messages_json = serde_json::to_string(&vec![
            Message::system("system".to_string()),
            Message::user("legacy-b-marker".to_string()),
        ])
        .map_err(|error| ReactError::Other(error.to_string()))?;
        store.save_checkpoint(&legacy).await?;

        let llm = Arc::new(MockLlmClient::new().with_response("legacy-b-answer"));
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("system")
            .conversation_id("configured-a")
            .state_store(store.clone())
            .build()?;
        agent.set_external_context(&echo_core::tools::ExternalRunContext {
            conversation_id: Some("legacy-b".to_string()),
            run_id: Some("legacy-run".to_string()),
            turn_id: Some("legacy-turn".to_string()),
            execution_id: None,
            isolation_id: None,
            message_id: None,
            cancel: None,
            trace_sink: None,
            delegation_policy: None,
            resource_guards: Vec::new(),
        });
        agent
            .run_stream_channel(
                StreamInit {
                    text: "legacy-b-user".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: None,
                },
                StreamMode::Chat,
            )
            .await?
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let request = llm
            .last_messages()
            .ok_or_else(|| ReactError::Other("legacy LLM request missing".to_string()))?;
        assert!(request.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "legacy-b-marker")
        }));
        assert!(!request.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "configured-a-marker")
        }));
        let configured_after = store
            .get_checkpoint("configured-a")
            .await?
            .ok_or_else(|| ReactError::Other("configured checkpoint missing".to_string()))?;
        assert_eq!(configured_after.messages_json, configured.messages_json);
        let legacy_after = store
            .get_checkpoint("legacy-b")
            .await?
            .ok_or_else(|| ReactError::Other("legacy checkpoint missing".to_string()))?
            .restore_messages()?;
        assert!(legacy_after.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|content| content == "legacy-b-answer")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn iteration_wind_down_is_injected_once() {
        let llm = Arc::new(
            MockLlmClient::new()
                .then_tool_call("call_1", "mock_calc", r#"{"x":1}"#)
                .with_response("done"),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .tool(Box::new(MockTool::new("mock_calc").with_response("1")))
            .max_iterations(2)
            .run_budget(echo_core::agent::RunBudgetPolicy {
                iteration_wind_down_remaining: Some(1),
                max_model_tokens: None,
            })
            .build()
            .expect("agent builds");

        let events = collect_events(&agent, "run").await;
        let wind_down_count = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::BudgetDecision {
                        decision: echo_core::agent::BudgetDecision::WindDown,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(wind_down_count, 1);
        let calls = llm.all_calls();
        let second = calls.get(1).expect("second LLM call");
        assert!(second.iter().any(|message| {
            message
                .text_content()
                .is_some_and(|text| text.contains("iteration budget is nearly exhausted"))
        }));
    }

    #[tokio::test]
    async fn react_stream_records_real_usage_in_run_trace() -> Result<()> {
        use crate::trace::RunStore;

        let usage = crate::llm::types::Usage {
            prompt_tokens: Some(1000),
            completion_tokens: Some(80),
            total_tokens: Some(1080),
            prompt_tokens_details: Some(crate::llm::types::TokenUsageDetails {
                cached_tokens: Some(750),
                ..Default::default()
            }),
            ..Default::default()
        };
        let store = Arc::new(crate::trace::InMemoryRunStore::new());
        let mut agent =
            agent_with_mock_llm(MockLlmClient::new().with_response_usage("done", usage));
        agent.set_run_store(store.clone());

        let events = collect_events(&agent, "run").await;
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::LlmUsage {
                prompt_tokens: 1000,
                cached_prompt_tokens: 750,
                usage_reported: true,
                ..
            }
        )));
        let summary = store
            .list_all(1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| crate::error::ReactError::Other("trace summary missing".into()))?;
        let run = store
            .load(&summary.run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("trace run missing".into()))?;
        assert!(
            run.events.iter().any(|event| matches!(
                event,
                crate::trace::RunEvent::LlmCall {
                    cached_prompt_tokens: 750,
                    usage_reported: true,
                    ..
                }
            )),
            "LLM usage event missing from trace: {:?}",
            run.events
        );
        assert_eq!(summary.token_usage.prompt_tokens, 1000);
        assert_eq!(summary.token_usage.cached_prompt_tokens, 750);
        Ok(())
    }

    #[tokio::test]
    async fn model_token_budget_blocks_tools_and_forces_final_only_request() {
        let usage = crate::llm::types::Usage {
            prompt_tokens: Some(6),
            completion_tokens: Some(5),
            total_tokens: Some(11),
            ..crate::llm::types::Usage::default()
        };
        let llm = Arc::new(
            MockLlmClient::new()
                .then_tool_call_with_usage("blocked", "mock_calc", r#"{"x":1}"#, usage)
                .with_response("final without tools"),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .tool(Box::new(
                MockTool::new("mock_calc").with_response("must not run"),
            ))
            .max_iterations(3)
            .run_budget(echo_core::agent::RunBudgetPolicy {
                iteration_wind_down_remaining: None,
                max_model_tokens: Some(10),
            })
            .build()
            .expect("agent builds");

        let events = collect_events(&agent, "run").await;
        assert!(events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::BudgetDecision {
                    decision: echo_core::agent::BudgetDecision::FinalOnly,
                    reported_model_tokens: 11,
                    usage_complete: true,
                    ..
                }
            )
        }));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::ToolCall { .. }))
        );
        assert!(
            matches!(events.last(), Some(AgentEvent::FinalAnswer(text)) if text == "final without tools")
        );
        assert_eq!(llm.all_tool_choices(), vec![None, Some("none".to_string())]);
    }

    #[tokio::test]
    async fn final_only_falls_back_to_empty_tool_surface_when_none_is_unsupported() {
        let usage = crate::llm::types::Usage {
            prompt_tokens: Some(6),
            completion_tokens: Some(5),
            total_tokens: Some(11),
            ..crate::llm::types::Usage::default()
        };
        let llm = Arc::new(
            MockLlmClient::new()
                .then_tool_call_with_usage("blocked", "mock_calc", r#"{"x":1}"#, usage)
                .with_response("fallback final"),
        );
        let profile =
            echo_core::llm::capabilities::ModelProfile::from_provider_name("local-model", "ollama");
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .model_profile(profile)
            .tool(Box::new(
                MockTool::new("mock_calc").with_response("must not run"),
            ))
            .max_iterations(3)
            .run_budget(echo_core::agent::RunBudgetPolicy {
                iteration_wind_down_remaining: None,
                max_model_tokens: Some(10),
            })
            .build()
            .expect("agent builds");

        let events = collect_events(&agent, "run").await;
        assert!(
            matches!(events.last(), Some(AgentEvent::FinalAnswer(text)) if text == "fallback final")
        );
        assert_eq!(llm.all_tool_choices(), vec![None, None]);
        let tool_counts = llm.all_tool_counts();
        assert!(tool_counts.first().is_some_and(|count| *count > 0));
        assert_eq!(tool_counts.get(1), Some(&0));
    }

    #[tokio::test]
    async fn missing_provider_usage_does_not_fake_token_budget_exhaustion() {
        let llm = MockLlmClient::new()
            .then_tool_call("allowed", "mock_calc", r#"{"x":1}"#)
            .with_response("done");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .tool(Box::new(MockTool::new("mock_calc").with_response("1")))
            .run_budget(echo_core::agent::RunBudgetPolicy {
                iteration_wind_down_remaining: None,
                max_model_tokens: Some(1),
            })
            .build()
            .expect("agent builds");

        let events = collect_events(&agent, "run").await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ToolCall { .. }))
        );
        assert!(!events.iter().any(|event| {
            matches!(
                event,
                AgentEvent::BudgetDecision {
                    decision: echo_core::agent::BudgetDecision::FinalOnly,
                    ..
                }
            )
        }));
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
                    invocation: None,
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

    /// Phase 3: verify that a running LLM call responds to cancellation.
    /// Uses `MockLlmClient::with_delay` to simulate a slow LLM (30s), then
    /// cancels the agent's token mid-flight. The stream must terminate well
    /// before the 30s delay — proving the cancel propagated to the LLM layer.
    #[tokio::test]
    async fn q_flt_v05_cancellation_stops_the_provider_without_final_success() {
        use std::time::Duration;

        use crate::agent::CancellationToken;

        // A mock LLM that sleeps 30s before responding, but honors cancel.
        let llm = MockLlmClient::new()
            .with_response("slow answer")
            .with_delay(Duration::from_secs(30));
        let agent = agent_with_mock_llm(llm);

        let cancel = CancellationToken::new();
        // Start streaming — the LLM call will block for 30s unless cancelled.
        let stream_fut = agent.chat_stream_with_cancel("hello", cancel.clone());

        // Give the stream a moment to reach the LLM call.
        let stream = tokio::time::timeout(Duration::from_secs(2), stream_fut)
            .await
            .expect("stream init should not hang")
            .expect("stream should start");

        // Cancel after a short delay.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();

        // Collect events with a short timeout — must terminate WAY before 30s.
        let mut events = Vec::new();
        let collect_result = tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = stream;
            use futures::StreamExt;
            while let Some(ev) = stream.next().await {
                events.push(ev);
            }
        })
        .await;

        assert!(
            collect_result.is_ok(),
            "stream must terminate within 5s after cancel (not wait for 30s delay)"
        );

        // The stream should NOT have produced a FinalAnswer (it was cancelled
        // before the LLM could respond).
        let has_final = events
            .iter()
            .any(|e| matches!(e, Ok(AgentEvent::FinalAnswer { .. })));
        assert!(!has_final, "cancelled stream must not emit FinalAnswer");
        assert!(matches!(
            agent.steer_input(None, Message::user("late steer".to_string())),
            Err(crate::agent::TurnSteerError::NoActiveTurn)
        ));
    }

    #[tokio::test]
    async fn cancellation_drains_running_tool_before_abandoning_turn() -> Result<()> {
        use futures::StreamExt;

        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm = MockLlmClient::new()
            .then_tool_call("call-1", "delayed_terminal", "{}")
            .with_response("unused");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("Run the requested tool.")
            .tool(Box::new(DelayedTerminalTool {
                started: Arc::new(tokio::sync::Notify::new()),
                completed: Arc::new(tokio::sync::Notify::new()),
                finished: finished.clone(),
            }))
            .build()?;
        let cancel = crate::agent::CancellationToken::new();
        let mut stream = agent
            .execute_stream_with_cancel("persist terminal state", cancel.clone())
            .await?;

        let saw_tool_call = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Some(event) = stream.next().await {
                if matches!(event?, AgentEvent::ToolCall { .. }) {
                    return Ok::<bool, crate::error::ReactError>(true);
                }
            }
            Ok(false)
        })
        .await
        .map_err(|_| crate::error::ReactError::Other("tool call was not emitted".to_string()))??;
        assert!(saw_tool_call);
        cancel.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Some(event) = stream.next().await {
                event?;
            }
            Ok::<(), crate::error::ReactError>(())
        })
        .await
        .map_err(|_| {
            crate::error::ReactError::Other("cancelled tool drain timed out".to_string())
        })??;

        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
        Ok(())
    }

    #[tokio::test]
    async fn q_flt_v06_dropping_consumer_drains_upstream_and_releases_turn() -> Result<()> {
        let started = Arc::new(tokio::sync::Notify::new());
        let completed = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm = MockLlmClient::new()
            .then_tool_call("drop-1", "delayed_terminal", "{}")
            .with_response("second run completed");
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .tool(Box::new(DelayedTerminalTool {
                started: Arc::clone(&started),
                completed: Arc::clone(&completed),
                finished: Arc::clone(&finished),
            }))
            .build()?;
        let mut stream = agent.execute_stream("start durable tool").await?;
        let saw_call = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while let Some(event) = stream.next().await {
                if matches!(event?, AgentEvent::ToolCall { .. }) {
                    return Ok::<bool, ReactError>(true);
                }
            }
            Ok(false)
        })
        .await
        .map_err(|_| ReactError::Other("durable tool call was not emitted".to_string()))??;
        assert!(saw_call);

        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .map_err(|_| ReactError::Other("durable tool did not start".to_string()))?;

        drop(stream);

        tokio::time::timeout(std::time::Duration::from_secs(1), completed.notified())
            .await
            .map_err(|_| ReactError::Other("dropped stream did not drain its tool".to_string()))?;
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));

        let answer = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            agent.execute("run after consumer disconnect"),
        )
        .await
        .map_err(|_| ReactError::Other("dropped stream retained the turn".to_string()))??;
        assert_eq!(answer, "second run completed");
        assert!(matches!(
            agent.steer_input(None, Message::user("late steer".to_string())),
            Err(crate::agent::TurnSteerError::NoActiveTurn)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn steer_during_llm_call_continues_same_turn_with_new_input() -> Result<()> {
        use futures::StreamExt;
        use std::time::Duration;

        let llm = Arc::new(
            MockLlmClient::new()
                .with_responses(["draft answer", "corrected answer"])
                .with_delay(Duration::from_millis(150)),
        );
        let mut agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("You are a test assistant.")
            .build()?;
        agent.config_mut().max_iterations = 4;
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("turn-steer-1".to_string()),
                turn_id: Some("turn-steer-1".to_string()),
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
                resource_guards: Vec::new(),
            }),
            runtime_state_id: None,
            transcript_generation_id: None,
            working_dir: None,
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "initial request".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation),
                },
                StreamMode::Chat,
            )
            .await?;

        tokio::time::sleep(Duration::from_millis(30)).await;
        let mut receipt = agent
            .steer_input_tracked(
                Some("turn-steer-1"),
                Message::user("steer correction".to_string()),
            )
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert_eq!(receipt.turn_id(), "turn-steer-1");
        assert_eq!(receipt.state(), crate::agent::AgentSteerState::Accepted);

        let events = stream.collect::<Vec<_>>().await;
        assert!(events.iter().any(|event| {
            matches!(event, Ok(AgentEvent::FinalAnswer(answer)) if answer == "corrected answer")
        }));
        assert_eq!(llm.call_count(), 2);
        let last_messages = llm.last_messages().unwrap_or_default();
        assert!(
            last_messages
                .iter()
                .any(|message| { message.content.as_text().as_deref() == Some("draft answer") })
        );
        assert!(
            last_messages.iter().any(|message| {
                message.content.as_text().as_deref() == Some("steer correction")
            })
        );
        assert_eq!(
            receipt.wait_for_turn_settled().await,
            crate::agent::AgentSteerState::TurnSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Completed,
                drained: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_steer_reports_drain_before_provider_failure_terminal() -> Result<()> {
        use futures::StreamExt;
        use std::time::Duration;

        let llm = Arc::new(
            MockLlmClient::new()
                .with_response("draft before failure")
                .with_network_error("provider disconnected")
                .with_delay(Duration::from_millis(100)),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm)
            .system_prompt("You are a test assistant.")
            .build()?;
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime_state_id: None,
            transcript_generation_id: None,
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("tracked-provider-failure".to_string()),
                turn_id: Some("tracked-provider-failure".to_string()),
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
                resource_guards: Vec::new(),
            }),
            working_dir: None,
            cancel: None,
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "initial request".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation),
                },
                StreamMode::Chat,
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut receipt = agent
            .steer_input_tracked(
                Some("tracked-provider-failure"),
                Message::user("consume before provider failure".to_string()),
            )
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;

        assert_eq!(
            receipt.wait_for_drained().await,
            crate::agent::AgentSteerState::Drained
        );
        let events = stream.collect::<Vec<_>>().await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::Error { .. })))
        );
        assert_eq!(
            receipt.wait_for_turn_settled().await,
            crate::agent::AgentSteerState::TurnSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
                drained: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_steer_reports_cancelled_after_real_drain() -> Result<()> {
        use futures::StreamExt;
        use std::time::Duration;

        let llm = Arc::new(
            MockLlmClient::new()
                .with_responses(["draft before cancellation", "unused final"])
                .with_delay(Duration::from_millis(100)),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm)
            .system_prompt("You are a test assistant.")
            .build()?;
        let cancel = crate::agent::CancellationToken::new();
        let invocation = echo_core::agent::AgentInvocationContext {
            runtime_state_id: None,
            transcript_generation_id: None,
            runtime: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some("tracked-cancel".to_string()),
                turn_id: Some("tracked-cancel".to_string()),
                execution_id: None,
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
                resource_guards: Vec::new(),
            }),
            working_dir: None,
            cancel: Some(cancel.clone()),
            disabled_tools: None,
            visible_tools: None,
            run_budget: None,
            history: None,
            resource_guards: Vec::new(),
            input_lifecycle: None,
        };
        let stream = agent
            .run_stream_channel(
                StreamInit {
                    text: "initial request".to_string(),
                    message: None,
                    label: String::new(),
                    invocation: Some(invocation),
                },
                StreamMode::Chat,
            )
            .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut receipt = agent
            .steer_input_tracked(
                Some("tracked-cancel"),
                Message::user("consume before cancellation".to_string()),
            )
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;

        assert_eq!(
            receipt.wait_for_drained().await,
            crate::agent::AgentSteerState::Drained
        );
        cancel.cancel();
        let events = stream.collect::<Vec<_>>().await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))))
        );
        assert_eq!(
            receipt.wait_for_turn_settled().await,
            crate::agent::AgentSteerState::TurnSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                drained: true,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_steer_stays_accepted_behind_context_barrier() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "system"));
        let lease = agent
            .turn_steer_mailbox
            .begin("context-barrier".to_string());
        lease.set_steerable(true);
        let mut snapshot = AgentSnapshot::from_agent(&agent);
        snapshot.current_turn_id = Some("context-barrier".to_string());
        snapshot.turn_steer_incarnation = Some(lease.incarnation());
        let snapshot = Arc::new(snapshot);
        let context = agent.memory.context.clone();
        let context_guard = context.lock().await;
        let mut receipt = agent
            .steer_input_tracked(
                Some("context-barrier"),
                Message::user("barrier input".to_string()),
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let drain = {
            let snapshot = snapshot.clone();
            let context = context.clone();
            tokio::spawn(async move { snapshot.drain_steer_into_context(&context, None).await })
        };

        tokio::task::yield_now().await;
        assert_eq!(receipt.state(), crate::agent::AgentSteerState::Accepted);
        assert!(!drain.is_finished());
        drop(context_guard);
        assert_eq!(
            drain
                .await
                .map_err(|error| ReactError::Other(format!("drain task failed: {error}")))?,
            1
        );
        assert_eq!(
            receipt.wait_for_drained().await,
            crate::agent::AgentSteerState::Drained
        );
        lease.settle(crate::agent::AgentSteerTurnOutcome::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn tracked_turn_settlement_wins_before_blocked_context_drain() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "system"));
        let lease = agent
            .turn_steer_mailbox
            .begin("settle-before-context".to_string());
        lease.set_steerable(true);
        let mut snapshot = AgentSnapshot::from_agent(&agent);
        snapshot.current_turn_id = Some("settle-before-context".to_string());
        snapshot.turn_steer_incarnation = Some(lease.incarnation());
        let snapshot = Arc::new(snapshot);
        let context = agent.memory.context.clone();
        let context_guard = context.lock().await;
        let mut receipt = agent
            .steer_input_tracked(
                Some("settle-before-context"),
                Message::user("must remain unconsumed".to_string()),
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let drain = {
            let snapshot = snapshot.clone();
            let context = context.clone();
            tokio::spawn(async move { snapshot.drain_steer_into_context(&context, None).await })
        };

        tokio::task::yield_now().await;
        lease.settle(crate::agent::AgentSteerTurnOutcome::Cancelled);
        assert_eq!(
            receipt.wait_for_turn_settled().await,
            crate::agent::AgentSteerState::TurnSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                drained: false,
            }
        );
        drop(context_guard);
        assert_eq!(
            drain
                .await
                .map_err(|error| ReactError::Other(format!("drain task failed: {error}")))?,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn tracked_steer_same_id_stale_snapshot_cannot_drain_new_incarnation() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "system"));
        let stale_lease = agent.turn_steer_mailbox.begin("same-id-drain".to_string());
        stale_lease.set_steerable(true);
        let mut stale_snapshot = AgentSnapshot::from_agent(&agent);
        stale_snapshot.current_turn_id = Some("same-id-drain".to_string());
        stale_snapshot.turn_steer_incarnation = Some(stale_lease.incarnation());

        let current_lease = agent.turn_steer_mailbox.begin("same-id-drain".to_string());
        current_lease.set_steerable(true);
        let mut receipt = agent
            .steer_input_tracked(
                Some("same-id-drain"),
                Message::user("new incarnation input".to_string()),
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let context = agent.memory.context.clone();

        assert_eq!(
            stale_snapshot
                .drain_steer_into_context(&context, None)
                .await,
            0
        );
        assert_eq!(receipt.state(), crate::agent::AgentSteerState::Accepted);

        let mut current_snapshot = AgentSnapshot::from_agent(&agent);
        current_snapshot.current_turn_id = Some("same-id-drain".to_string());
        current_snapshot.turn_steer_incarnation = Some(current_lease.incarnation());
        assert_eq!(
            current_snapshot
                .drain_steer_into_context(&context, None)
                .await,
            1
        );
        assert_eq!(
            receipt.wait_for_drained().await,
            crate::agent::AgentSteerState::Drained
        );
        current_lease.settle(crate::agent::AgentSteerTurnOutcome::Completed);
        drop(stale_lease);
        Ok(())
    }

    // ── M1: test-credibility re-basing (mock 隐身衣 removal) ────────────────
    // New fixtures drive the mock through the real provider wire shape:
    // content deltas followed by a separate terminal chunk carrying
    // finish_reason and usage (F-TST-01-P1-01), and concurrent batch results
    // emitted in call order (F-RCT-04-P1-01). Each fixture fails before its
    // fix and passes after.

    #[tokio::test]
    async fn stream_script_terminal_chunk_reports_usage() {
        // Real providers report usage on a separate terminal chunk; the
        // loop-level `usage_reported` must be true only in that shape.
        let usage = crate::llm::types::Usage {
            prompt_tokens: Some(11),
            completion_tokens: Some(7),
            ..Default::default()
        };
        let llm = MockLlmClient::new().with_stream_script(vec![
            StreamChunk::Delta(DeltaMessage {
                role: Some("assistant".to_string()),
                content: Some("Hello world".to_string()),
                reasoning_content: None,
                reasoning_blocks: None,
                tool_calls: None,
            }),
            StreamChunk::Terminal {
                finish_reason: Some("stop".to_string()),
                usage: Some(usage),
            },
        ]);
        let agent = agent_with_mock_llm(llm);
        let events = collect_events(&agent, "Hi").await;

        let usage_event = events
            .iter()
            .find(|event| matches!(event, AgentEvent::LlmUsage { .. }))
            .expect("LlmUsage event must be emitted");
        match usage_event {
            AgentEvent::LlmUsage {
                prompt_tokens,
                completion_tokens,
                usage_reported,
                ..
            } => {
                assert!(usage_reported, "usage_reported must be true");
                assert_eq!(*prompt_tokens, 11);
                assert_eq!(*completion_tokens, 7);
            }
            _ => unreachable!("matched LlmUsage above"),
        }
        let last = events.last().expect("terminal event");
        assert!(matches!(last, AgentEvent::FinalAnswer(t) if t == "Hello world"));
    }

    #[tokio::test]
    async fn stream_script_without_usage_reports_false() {
        // The provider streamed content but never reported usage — the loop
        // must not fabricate a positive accounting (F-TST-01-P1-01's
        // single-chunk mock certified the impossible; this fixture pins the
        // honest negative).
        let llm = MockLlmClient::new().with_stream_script(vec![
            StreamChunk::Delta(DeltaMessage {
                role: Some("assistant".to_string()),
                content: Some("No usage here".to_string()),
                reasoning_content: None,
                reasoning_blocks: None,
                tool_calls: None,
            }),
            StreamChunk::Terminal {
                finish_reason: Some("stop".to_string()),
                usage: None,
            },
        ]);
        let agent = agent_with_mock_llm(llm);
        let events = collect_events(&agent, "Hi").await;

        let usage_event = events
            .iter()
            .find(|event| matches!(event, AgentEvent::LlmUsage { .. }))
            .expect("LlmUsage event must be emitted");
        match usage_event {
            AgentEvent::LlmUsage { usage_reported, .. } => {
                assert!(!usage_reported, "usage_reported must be false");
            }
            _ => unreachable!("matched LlmUsage above"),
        }
    }

    #[tokio::test]
    async fn concurrent_batch_results_follow_call_order() {
        // Two concurrent tool calls: call_1 is slow, call_2 is fast. Results
        // must be emitted and inserted into context in CALL order (stream
        // index order), not completion order — strict providers reject
        // misordered tool results with HTTP 400 (F-RCT-04-P1-01). Before the
        // fix the second request carried [call_2, call_1] results.
        let llm = MockLlmClient::new()
            .then_tool_calls(vec![
                ToolCall {
                    id: "call_1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "slow_tool".to_string(),
                        arguments: r#"{}"#.to_string(),
                    },
                },
                ToolCall {
                    id: "call_2".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "fast_tool".to_string(),
                        arguments: r#"{}"#.to_string(),
                    },
                },
            ])
            .with_response("The result is 42.");

        let slow_tool = MockTool::new("slow_tool")
            .with_response("slow result")
            .with_delay(std::time::Duration::from_millis(60));
        let fast_tool = MockTool::new("fast_tool").with_response("fast result");

        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(llm))
            .system_prompt("You are a test assistant. Use tools when asked.")
            .tool(Box::new(slow_tool))
            .tool(Box::new(fast_tool))
            .build()
            .expect("agent builds");

        let events = collect_events(&agent, "Run both tools.").await;

        // The ToolResult events must arrive in call order.
        let result_ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            result_ids,
            vec!["call_1", "call_2"],
            "batch results must be emitted in call order, got {result_ids:?}"
        );
        // And the final answer still arrives.
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::FinalAnswer(t) if t.contains("42")))
        );
    }

    // Truncated / clean-disconnect stream: the loop must NOT accept the
    // partial output as a complete final answer (Q-FLT-01-P1-01). This
    // fixture remains active in the mandatory suite so terminal regressions
    // cannot be hidden behind an ignored test.
    #[tokio::test]
    async fn truncated_stream_is_not_accepted_as_complete() {
        let llm = MockLlmClient::new().with_stream_script(vec![
            StreamChunk::Delta(DeltaMessage {
                role: Some("assistant".to_string()),
                content: Some("Partial answer that never finished".to_string()),
                reasoning_content: None,
                reasoning_blocks: None,
                tool_calls: None,
            }),
            StreamChunk::Err(ReactError::Llm(Box::new(LlmError::NetworkError(
                "connection closed mid-stream".to_string(),
            )))),
        ]);
        let agent = agent_with_mock_llm(llm);
        let events = collect_events(&agent, "Hi").await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::FinalAnswer(_))),
            "truncated stream must not produce a FinalAnswer"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Error { .. })),
            "truncated stream must surface an error terminal"
        );
        let partial = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Token(token) => Some(token.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(partial, "Partial answer that never finished");
    }
}
