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
        let mut invocation = init.invocation;
        // Capture value-carried run metadata before the execution mutex wait.
        // Concurrent callers may update/clear the agent's shared external
        // context while this invocation is queued, but this snapshot belongs
        // to the invocation that entered here.
        let legacy_runtime = if invocation.is_none() {
            Some((
                self.current_run_id
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone(),
                self.external_cancel
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone(),
                self.external_trace_sink
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone(),
                *self
                    .external_delegation_policy
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            ))
        } else {
            None
        };

        // ★ Acquire execution mutex BEFORE context mutation — using lock_owned()
        // so the guard can be moved into the spawned task and held for the
        // entire stream lifetime.
        let execution_guard = self.execution_mutex.clone().lock_owned().await;

        // Start trace run BEFORE prepare. Value-scoped invocations never write
        // the agent-wide current_run_id; legacy calls retain the old behavior.
        if let Some(invocation) = invocation.as_mut() {
            if invocation.runtime.is_none()
                && let Some(run_id) = self.start_scoped_trace_run(&text).await
            {
                invocation.runtime = Some(echo_core::tools::ExternalRunContext {
                    conversation_id: self.config.conversation_id.clone(),
                    run_id: Some(run_id),
                    turn_id: None,
                    execution_id: None,
                    message_id: None,
                    cancel: None,
                    trace_sink: None,
                    delegation_policy: None,
                });
            }
        } else {
            self.start_trace_run(&text).await;
        }
        let turn_id = invocation
            .as_ref()
            .and_then(|value| value.runtime.as_ref())
            .and_then(|runtime| runtime.turn_id.clone().or_else(|| runtime.run_id.clone()))
            .or_else(|| {
                self.current_run_id
                    .lock()
                    .ok()
                    .and_then(|value| value.clone())
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let active_turn_lease = self.turn_steer_mailbox.begin(turn_id.clone());

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
                crate::intent::Intent::DirectAnswer { confidence }
                    if self.allows_direct_answer_shortcut() =>
                {
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
                    snap.external_cancel = self
                        .external_cancel
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    snap.external_trace_sink = self
                        .external_trace_sink
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    snap.external_delegation_policy = *self
                        .external_delegation_policy
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
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
                    drop(active_turn_lease);
                    return Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)));
                }
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
                    if let Err(e) = self.activate_skill_for_context(&skill_name).await {
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

        let mut snap = if let Some(invocation) = invocation.as_ref() {
            AgentSnapshot::from_agent_with_invocation(self, invocation)
        } else {
            make_snapshot(self)
        };
        if let Some((
            current_run_id,
            external_cancel,
            external_trace_sink,
            external_delegation_policy,
        )) = legacy_runtime
        {
            snap.current_run_id = current_run_id;
            snap.external_cancel = external_cancel;
            snap.external_trace_sink = external_trace_sink;
            snap.external_delegation_policy = external_delegation_policy;
        }
        active_turn_lease.set_steerable(true);

        tokio::spawn(async move {
            // Move the guard into the spawned task — held for full stream duration
            let _execution_guard = execution_guard;
            let _active_turn_lease = active_turn_lease;
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
    async fn drain_steer_into_context(
        &self,
        context: &Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
        assistant_draft: Option<String>,
    ) -> usize {
        let Some(turn_id) = self.current_run_id.as_deref() else {
            return 0;
        };
        let pending = self.turn_steer_mailbox.drain(turn_id);
        if pending.is_empty() {
            return 0;
        }
        let count = pending.len();
        let mut guard = context.lock().await;
        if let Some(draft) = assistant_draft
            && !draft.is_empty()
        {
            guard.push(Message::assistant(draft));
        }
        for message in pending {
            guard.push(message);
        }
        count
    }

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

        let stream = super::phases::think::create_llm_stream(self, messages, false).await?;
        let mut stream = std::pin::pin!(stream);
        let mut content = String::new();
        let mut last_usage: Option<echo_core::llm::types::Usage> = None;
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
            // Capture usage from the final streaming chunk (when
            // stream_options.include_usage is supported by the provider).
            if chunk.usage.is_some() {
                last_usage = chunk.usage.clone();
            }
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

        // Emit LlmUsage so the observability system records token counts.
        // This mirrors what run_think() does for the full ReAct path.
        let pt = last_usage
            .as_ref()
            .and_then(|u| u.prompt_tokens)
            .unwrap_or(0) as usize;
        let ct = last_usage
            .as_ref()
            .and_then(|u| u.completion_tokens)
            .unwrap_or(0) as usize;
        let total_tokens = last_usage
            .as_ref()
            .and_then(|u| u.total_tokens)
            .map(|t| t as usize)
            .unwrap_or_else(|| pt.saturating_add(ct));
        let cached_prompt_tokens = last_usage
            .as_ref()
            .map(|u| u.cached_prompt_tokens() as usize)
            .unwrap_or(0);
        let cache_creation_prompt_tokens = last_usage
            .as_ref()
            .map(|u| u.cache_creation_prompt_tokens() as usize)
            .unwrap_or(0);
        let usage_reported = last_usage.is_some();

        let _ = tx
            .send(Ok(AgentEvent::LlmUsage {
                model: self.config.model_name.clone(),
                prompt_tokens: pt,
                completion_tokens: ct,
                total_tokens,
                cached_prompt_tokens,
                cache_creation_prompt_tokens,
                usage_reported,
            }))
            .await;

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

            let _ = self.drain_steer_into_context(&context, None).await;

            let remaining = max_iterations.saturating_sub(iteration);
            if !state.budget.wind_down_emitted
                && self.config.max_iterations > 0
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
                        return Ok(());
                    }
                };

            // Think: callbacks + interventions + LLM stream → buffered output
            let final_only = state.budget.final_only;
            let think = match phases::think::run_think(
                &self, &context, &tx, &mut state, messages, final_only,
            )
            .await?
            {
                phases::ThinkOutcome::Continue(t) => t,
                phases::ThinkOutcome::Abandoned
                | phases::ThinkOutcome::Cancelled
                | phases::ThinkOutcome::Blocked => {
                    return Ok(());
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
                if self
                    .drain_steer_into_context(&context, Some(think.content_buffer.clone()))
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
                    if self.drain_steer_into_context(&context, None).await > 0 {
                        continue;
                    }
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
    use crate::compression::{ContextProjection, PreModelContextProjector, ProjectionContext};
    use crate::intent::{Intent, IntentClassifier, IntentRouter, IntentRouterConfig};
    use echo_core::agent::Agent;
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
                message_id: Some("message-a".to_string()),
                cancel: None,
                trace_sink: None,
                delegation_policy: Some(echo_core::tools::NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: 1,
                    max_delegate_depth: 2,
                }),
            }),
            working_dir: Some(std::path::PathBuf::from("/tmp/worktree-a")),
            cancel: None,
            disabled_tools: None,
            run_budget: None,
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
                message_id: Some("message-b".to_string()),
                cancel: None,
                trace_sink: None,
                delegation_policy: Some(echo_core::tools::NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: 2,
                    max_delegate_depth: 3,
                }),
            }),
            working_dir: Some(std::path::PathBuf::from("/tmp/worktree-b")),
            cancel: None,
            disabled_tools: None,
            run_budget: None,
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
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            working_dir: None,
            cancel: None,
            disabled_tools: None,
            run_budget: None,
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
    async fn test_run_stream_cancelled_mid_llm_call() {
        use std::time::Duration;

        use crate::agent::CancellationToken;

        // A mock LLM that sleeps 30s before responding, but honors cancel.
        let llm = MockLlmClient::new()
            .with_response("slow answer")
            .with_delay(Duration::from_secs(30));
        let agent = agent_with_mock_llm(llm);

        // Set a cancel token on the agent so the think phase picks it up via
        // the snapshot and passes it to the LLM client.
        let cancel = CancellationToken::new();
        {
            let mut guard = agent.cancel_token.lock().await;
            *guard = Some(cancel.clone());
        }

        // Start streaming — the LLM call will block for 30s unless cancelled.
        let stream_fut = agent.run_stream_channel(
            StreamInit {
                text: "hello".into(),
                message: None,
                label: String::new(),
                invocation: None,
            },
            StreamMode::Chat,
        );

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
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            working_dir: None,
            cancel: None,
            disabled_tools: None,
            run_budget: None,
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
        let accepted = agent
            .steer_input(
                Some("turn-steer-1"),
                Message::user("steer correction".to_string()),
            )
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert_eq!(accepted, "turn-steer-1");

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
        Ok(())
    }
}
