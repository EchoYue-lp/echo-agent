//! Terminal-state phases: tools-branch verifier-pass, text-branch
//! `FinalAnswer` emission with `Stop`-hook continuation handling, the
//! `NoResponse` failure, and the `MaxIterationsExceeded` failure.

use super::{LoopState, with_reasoning_content};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{AgentError, ReactError, Result};
use crate::llm::types::Message;
use std::ops::ControlFlow;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::info;

async fn settle_final_intervention(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    output: &str,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<Option<crate::agent::AgentSteerTurnOutcome>> {
    let agent = &snap.config.agent_name;
    for intervention in &snap.tools.intervention_callbacks {
        let result = intervention.on_final_answer(agent, output).await;
        if result.cancel {
            let reason = "Agent execution cancelled by intervention at final answer";
            settle_terminal_projection(snap, context, Some(reason.to_string()), tx).await?;
            snap.finalize_run(crate::trace::RunStatus::Cancelled, None, Some(reason))
                .await;
            let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
            snap.fire_hook(
                crate::skills::hooks::HookEvent::SessionEnd,
                Some("cancelled"),
            )
            .await;
            return Ok(Some(crate::agent::AgentSteerTurnOutcome::Cancelled));
        }
        if result.block {
            let reason = result
                .block_reason
                .unwrap_or_else(|| "blocked by intervention at final answer".to_string());
            let detail = format!("Final answer blocked by intervention: {reason}");
            info!(agent = %agent, reason = %reason, "Intervention blocked final answer (streaming)");
            settle_terminal_projection(snap, context, Some(detail.clone()), tx).await?;
            snap.finalize_run(crate::trace::RunStatus::Failed, None, Some(&detail))
                .await;
            let error = ReactError::Other(detail);
            let _ = tx
                .send(Ok(AgentEvent::from_error("intervention", &error)))
                .await;
            snap.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("blocked"))
                .await;
            return Ok(Some(crate::agent::AgentSteerTurnOutcome::Failed));
        }
        if let Some(injected) = result.injected_context {
            super::super::context::push_runtime_context_note(
                context,
                "Intervention:FinalAnswer",
                &injected,
            )
            .await;
        }
    }
    Ok(None)
}

pub(crate) async fn settle_final_guard_failure(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    error: ReactError,
) -> Result<crate::agent::AgentSteerTurnOutcome> {
    let detail = error.to_string();
    settle_terminal_projection(snap, context, Some(detail.clone()), tx).await?;
    snap.finalize_run(crate::trace::RunStatus::Failed, None, Some(&detail))
        .await;
    let _ = tx
        .send(Ok(AgentEvent::from_error("output_guard", &error)))
        .await;
    snap.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("blocked"))
        .await;
    Ok(crate::agent::AgentSteerTurnOutcome::Failed)
}

pub(crate) async fn settle_cancelled_final_candidate(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<crate::agent::AgentSteerTurnOutcome> {
    let reason = "Agent execution cancelled before final answer";
    settle_terminal_projection(snap, context, Some(reason.to_string()), tx).await?;
    snap.finalize_run(crate::trace::RunStatus::Cancelled, None, Some(reason))
        .await;
    let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
    snap.fire_hook(
        crate::skills::hooks::HookEvent::SessionEnd,
        Some("cancelled"),
    )
    .await;
    Ok(crate::agent::AgentSteerTurnOutcome::Cancelled)
}

pub(crate) async fn settle_terminal_projection(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    blocked_reason: Option<String>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<()> {
    let settlement = snap
        .save_transcript_projection(context, blocked_reason)
        .await?;
    if snap.conversation_store.is_some() {
        // The settlement fact was already persisted to the trace by the
        // coordinator. Event delivery is a separate observer boundary.
        snap.mark_transcript_settlement_observed();
        if tx
            .send(Ok(AgentEvent::TranscriptProjectionSettlement(
                settlement.clone(),
            )))
            .await
            .is_err()
        {
            return Err(ReactError::Other(
                "transcript settlement observer closed before terminal".to_string(),
            ));
        }
    }
    match settlement.status {
        crate::memory::TranscriptProjectionSettlementStatus::Settled
        | crate::memory::TranscriptProjectionSettlementStatus::Deferred => Ok(()),
        crate::memory::TranscriptProjectionSettlementStatus::Blocked
        | crate::memory::TranscriptProjectionSettlementStatus::Conflict => Err(
            echo_core::error::RuntimeStateError::TranscriptProjectionBlocked {
                status: format!("{:?}", settlement.status),
                reason: settlement
                    .detail
                    .unwrap_or_else(|| "terminal transcript projection did not settle".to_string()),
            }
            .into(),
        ),
    }
}

/// Tools-branch terminal: a `final_answer` tool call has passed verifier.
/// Applies the one-shot `Stop` continuation first, then final callbacks and
/// interventions, audit, durable transcript settlement, trace finalization,
/// the single `FinalAnswer`, and `SessionEnd("complete")`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_completed_run(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    label: &str,
    output: &str,
    _iteration: usize,
    state: &LoopState,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<ControlFlow<crate::agent::AgentSteerTurnOutcome, ()>> {
    let agent = &snap.config.agent_name;
    let hc = crate::skills::hooks::HookContext::for_stop(
        None,
        snap.config.session_id.as_deref().unwrap_or(""),
        &snap.config.agent_name,
        state.stop_hook_continued,
    );
    let reg = snap.tools.hook_registry.read().await.clone();
    let sr = reg.run_lifecycle_hooks(&hc).await;
    if super::cancellation_requested(snap) {
        return Ok(ControlFlow::Break(
            settle_cancelled_final_candidate(snap, context, tx).await?,
        ));
    }
    if let Some(reason) = &sr.continue_reason
        && !state.stop_hook_continued
    {
        super::super::context::push_runtime_context_note(
            context,
            "Hook:Stop",
            &format!("Continue: {}", reason),
        )
        .await;
        return Ok(ControlFlow::Continue(()));
    }

    if super::cancellation_requested(snap) {
        return Ok(ControlFlow::Break(
            settle_cancelled_final_candidate(snap, context, tx).await?,
        ));
    }

    for cb in snap.config.callbacks.iter() {
        cb.on_final_answer(agent, output).await;
    }
    if let Some(outcome) = settle_final_intervention(snap, context, output, tx).await? {
        return Ok(ControlFlow::Break(outcome));
    }

    info!(agent = %agent, "Streaming execution completed{label}");
    if snap.guard.audit_logger.is_some() {
        let ev = crate::audit::AuditEvent::now(
            snap.config.session_id.clone(),
            snap.config.agent_name.clone(),
            crate::audit::AuditEventType::FinalAnswer {
                content: output.to_string(),
            },
        );
        snap.record_audit_event(ev).await;
    }
    settle_terminal_projection(snap, context, None, tx).await?;
    snap.finalize_run(crate::trace::RunStatus::Completed, Some(output), None)
        .await;
    if tx
        .send(Ok(AgentEvent::FinalAnswer(output.to_string())))
        .await
        .is_err()
    {
        return Ok(ControlFlow::Break(
            crate::agent::AgentSteerTurnOutcome::Completed,
        ));
    }
    snap.fire_hook(
        crate::skills::hooks::HookEvent::SessionEnd,
        Some("complete"),
    )
    .await;
    Ok(ControlFlow::Break(
        crate::agent::AgentSteerTurnOutcome::Completed,
    ))
}

/// Text-branch terminal: the LLM produced content that passed verification.
/// Pushes the assistant message, applies the one-shot `Stop` continuation,
/// then runs final callbacks and interventions, takes an auto-snapshot, audits
/// and durably settles the transcript, finalizes the trace, and emits the
/// single `FinalAnswer` followed by `SessionEnd("complete")`.
///
/// Returns:
/// - [`ControlFlow::Continue`] when the `Stop` hook returned a
///   `continue_reason` and `state.stop_hook_continued` was previously
///   `false` — the flag is flipped and the caller should keep looping.
/// - [`ControlFlow::Break`] otherwise — `SessionEnd("complete")` has been
///   fired and the caller should return `Ok(())`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_final_text(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    state: &mut LoopState,
    iteration: usize,
    pt: usize,
    ct: usize,
    answer: String,
    reasoning_content: String,
    reasoning_blocks: Vec<crate::llm::types::ReasoningBlock>,
    // True only when the ReAct driver already applied Output Guard before
    // schema validation and the optional Critic.
    guard_already_checked: bool,
) -> Result<ControlFlow<crate::agent::AgentSteerTurnOutcome, ()>> {
    let answer = if guard_already_checked {
        answer
    } else {
        match snap.check_final_answer_guard(&answer).await {
            Ok(answer) => answer,
            Err(error) => {
                return Ok(ControlFlow::Break(
                    settle_final_guard_failure(snap, context, tx, error).await?,
                ));
            }
        }
    };
    if super::cancellation_requested(snap) {
        return Ok(ControlFlow::Break(
            settle_cancelled_final_candidate(snap, context, tx).await?,
        ));
    }
    let agent = &snap.config.agent_name;

    let ts = vec![crate::agent::react::StepType::Thought(answer.clone())];
    for cb in snap.config.callbacks.iter() {
        cb.on_think_end(agent, &ts, pt, ct).await;
    }
    let hc = crate::skills::hooks::HookContext::for_stop(
        None,
        snap.config.session_id.as_deref().unwrap_or(""),
        &snap.config.agent_name,
        state.stop_hook_continued,
    );
    let reg = snap.tools.hook_registry.read().await.clone();
    let sr = reg.run_lifecycle_hooks(&hc).await;
    if super::cancellation_requested(snap) {
        return Ok(ControlFlow::Break(
            settle_cancelled_final_candidate(snap, context, tx).await?,
        ));
    }
    if let Some(reason) = &sr.continue_reason
        && !state.stop_hook_continued
    {
        context.lock().await.push(with_reasoning_content(
            Message::assistant(answer.clone()),
            reasoning_content.clone(),
            reasoning_blocks.clone(),
        ));
        super::super::context::push_runtime_context_note(
            context,
            "Hook:Stop",
            &format!("Continue: {}", reason),
        )
        .await;
        state.stop_hook_continued = true;
        return Ok(ControlFlow::Continue(()));
    }
    for cb in snap.config.callbacks.iter() {
        cb.on_final_answer(agent, &answer).await;
    }
    if let Some(outcome) = settle_final_intervention(snap, context, &answer, tx).await? {
        return Ok(ControlFlow::Break(outcome));
    }
    context.lock().await.push(with_reasoning_content(
        Message::assistant(answer.clone()),
        reasoning_content,
        reasoning_blocks,
    ));
    snap.auto_snapshot(context, iteration).await;
    if snap.guard.audit_logger.is_some() {
        let ev = crate::audit::AuditEvent::now(
            snap.config.session_id.clone(),
            snap.config.agent_name.clone(),
            crate::audit::AuditEventType::FinalAnswer {
                content: answer.clone(),
            },
        );
        snap.record_audit_event(ev).await;
    }
    settle_terminal_projection(snap, context, None, tx).await?;
    // Finalize trace before moving the answer into the event
    snap.finalize_run(crate::trace::RunStatus::Completed, Some(&answer), None)
        .await;
    // Text answers are emitted only after Output guard approval and terminal
    // settlement. Publishing the raw model text from think would leak a
    // blocked or transformed answer through the stream before this boundary.
    if tx
        .send(Ok(AgentEvent::Token(answer.clone())))
        .await
        .is_err()
    {
        return Ok(ControlFlow::Break(
            crate::agent::AgentSteerTurnOutcome::Completed,
        ));
    }
    // Sending FinalAnswer is mandatory; on a closed receiver the macro
    // returns Ok(()) from this fn — but we model that as ControlFlow::Break.
    if tx.send(Ok(AgentEvent::FinalAnswer(answer))).await.is_err() {
        return Ok(ControlFlow::Break(
            crate::agent::AgentSteerTurnOutcome::Completed,
        ));
    }
    snap.fire_hook(
        crate::skills::hooks::HookEvent::SessionEnd,
        Some("complete"),
    )
    .await;
    Ok(ControlFlow::Break(
        crate::agent::AgentSteerTurnOutcome::Completed,
    ))
}

/// LLM produced neither tool calls nor content — terminal failure.
pub(crate) async fn finalize_no_response(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: mpsc::Sender<Result<AgentEvent>>,
) -> Result<()> {
    settle_terminal_projection(snap, context, Some("No response from LLM".to_string()), &tx)
        .await?;
    snap.finalize_run(
        crate::trace::RunStatus::Failed,
        None,
        Some("No response from LLM"),
    )
    .await;
    let error = ReactError::Agent(Box::new(AgentError::NoResponse {
        model: snap.config.model_name.clone(),
        agent: snap.config.agent_name.clone(),
    }));
    let _ = tx.send(Ok(AgentEvent::from_error("llm", &error))).await;
    Ok(())
}

/// `max_iterations` hit — terminal failure with full hook fan-out.
pub(crate) async fn finalize_max_iterations(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: mpsc::Sender<Result<AgentEvent>>,
) -> Result<()> {
    settle_terminal_projection(
        snap,
        context,
        Some("Max iterations exceeded".to_string()),
        &tx,
    )
    .await?;
    snap.fire_hook(
        crate::skills::hooks::HookEvent::SessionEnd,
        Some("max_iterations"),
    )
    .await;
    snap.fire_hook(
        crate::skills::hooks::HookEvent::StopFailure,
        Some("max_iterations"),
    )
    .await;
    snap.finalize_run(
        crate::trace::RunStatus::Failed,
        None,
        Some("Max iterations exceeded"),
    )
    .await;
    let error = ReactError::Agent(Box::new(AgentError::MaxIterationsExceeded(
        snap.config.max_iterations,
    )));
    let _ = tx
        .send(Ok(AgentEvent::from_error("react_loop", &error)))
        .await;
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReactAgent;
    use crate::agent::config::AgentConfig;
    use crate::agent::snapshot::AgentRunSnapshot;
    use crate::trace::{InMemoryRunStore, RunStatus, RunStore};

    struct BlockFinalAnswerGuard;

    struct TransformFinalAnswerGuard;

    impl crate::guard::Guard for TransformFinalAnswerGuard {
        fn name(&self) -> &str {
            "transform_final_answer"
        }

        fn check<'a>(
            &'a self,
            _content: &'a str,
            direction: crate::guard::GuardDirection,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>>
        {
            Box::pin(async move {
                if direction == crate::guard::GuardDirection::Output {
                    Ok(crate::guard::GuardResult::Transform {
                        content: "filtered answer".to_string(),
                        reasons: vec!["final answer redacted".to_string()],
                    })
                } else {
                    Ok(crate::guard::GuardResult::Pass)
                }
            })
        }
    }

    impl crate::guard::Guard for BlockFinalAnswerGuard {
        fn name(&self) -> &str {
            "block_final_answer"
        }

        fn check<'a>(
            &'a self,
            _content: &'a str,
            direction: crate::guard::GuardDirection,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>>
        {
            Box::pin(async move {
                if direction == crate::guard::GuardDirection::Output {
                    Ok(crate::guard::GuardResult::Block {
                        reason: "final answer policy rejected the response".to_string(),
                    })
                } else {
                    Ok(crate::guard::GuardResult::Pass)
                }
            })
        }
    }

    #[tokio::test]
    async fn closed_settlement_observer_preserves_persisted_projection_fact() -> Result<()> {
        use crate::memory::{ConversationStore, FileConversationStore};
        use crate::state::FileRuntimeStateStore;
        use crate::trace::RunEvent;

        let root = tempfile::tempdir()?;
        let conversations = Arc::new(FileConversationStore::new(
            root.path().join("conversations"),
        )?);
        let runtime = Arc::new(FileRuntimeStateStore::new(root.path().join("runtime"))?);
        let trace = Arc::new(InMemoryRunStore::new());
        let config =
            AgentConfig::new("test-model", "agent", "system").conversation_id("closed-observer");
        let mut agent = ReactAgent::new(config);
        agent.set_conversation_store(conversations.clone());
        agent.set_state_store(runtime);
        agent.set_run_store(trace.clone());
        agent
            .memory
            .context
            .lock()
            .await
            .push(crate::llm::types::Message::user(
                "durable input".to_string(),
            ));
        let legacy = agent.capture_legacy_external_context();
        let trace_run_id = agent
            .start_legacy_trace_run("durable input", &legacy)
            .await
            .ok_or_else(|| ReactError::Other("trace did not start".to_string()))?;
        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let error = settle_terminal_projection(&snap, &agent.memory.context, None, &tx)
            .await
            .err()
            .ok_or_else(|| {
                ReactError::Other("closed observer unexpectedly accepted event".to_string())
            })?;
        assert!(error.to_string().contains("observer closed"));
        assert!(snap.transcript_settlement_was_observed());
        assert!(
            !conversations
                .get_messages("closed-observer")
                .await?
                .is_empty()
        );
        let run = trace
            .load(&trace_run_id)
            .await?
            .ok_or_else(|| ReactError::Other("trace disappeared".to_string()))?;
        let settlements = run
            .events
            .iter()
            .filter_map(|event| match event {
                RunEvent::TranscriptProjectionSettlement { settlement } => Some(settlement),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(settlements.len(), 1);
        assert_eq!(
            settlements.first().map(|settlement| settlement.status),
            Some(crate::memory::TranscriptProjectionSettlementStatus::Settled)
        );
        Ok(())
    }

    /// Build a snapshot whose `trace_run_id` is wired up so trace
    /// finalization can update the in-memory run store.
    async fn snap_with_trace(
        agent_name: &str,
    ) -> (AgentRunSnapshot, Arc<InMemoryRunStore>, ReactAgent) {
        let store: Arc<InMemoryRunStore> = Arc::new(InMemoryRunStore::new());
        let mut agent = ReactAgent::new(AgentConfig::new("test-model", agent_name, "sys"));
        agent.set_run_store(store.clone());
        let legacy = agent.capture_legacy_external_context();
        agent.start_legacy_trace_run("test input", &legacy).await;

        let snap = AgentRunSnapshot::from_agent(&agent);
        (snap, store, agent)
    }

    /// `finalize_no_response` sends a `NoResponse` error onto the channel
    /// and finalizes the trace as `Failed`.
    #[tokio::test]
    async fn finalize_no_response_sends_error_and_marks_trace_failed() {
        let (snap, store, agent) = snap_with_trace("agent-noresp").await;
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        finalize_no_response(&snap, &agent.memory.context, tx)
            .await
            .expect("finalize_no_response must succeed");

        let item = rx.recv().await.expect("error must be forwarded to tx");
        let event = item.expect("terminal error event must use the typed event stream");
        let (source, msg) = match event {
            AgentEvent::Error {
                source, message, ..
            } => (source, message),
            other => panic!("expected AgentEvent::Error, got: {other:?}"),
        };
        assert_eq!(source, "llm");
        assert!(
            msg.contains("No response from LLM"),
            "expected NoResponse error, got: {msg}",
        );
        assert!(
            msg.contains("test-model"),
            "error should carry the model name, got: {msg}",
        );

        // Trace should be marked Failed.
        let run_id = snap.trace_run_id.clone().expect("run_id must be set");
        let run = store
            .load(&run_id)
            .await
            .expect("load must succeed")
            .expect("run row must exist");
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.error.as_deref(), Some("No response from LLM"));
    }

    /// `finalize_max_iterations` sends a `MaxIterationsExceeded` error,
    /// runs its hook fan-out without panicking, and marks the trace
    /// `Failed` with the canonical error string.
    #[tokio::test]
    async fn finalize_max_iterations_sends_error_and_marks_trace_failed() {
        let (snap, store, agent) = snap_with_trace("agent-maxiter").await;
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        finalize_max_iterations(&snap, &agent.memory.context, tx)
            .await
            .expect("finalize_max_iterations must succeed");

        let item = rx.recv().await.expect("error must be forwarded to tx");
        let event = item.expect("terminal error event must use the typed event stream");
        let (source, msg) = match event {
            AgentEvent::Error {
                source, message, ..
            } => (source, message),
            other => panic!("expected AgentEvent::Error, got: {other:?}"),
        };
        assert_eq!(source, "react_loop");
        assert!(
            msg.contains("Max iterations exceeded"),
            "expected MaxIterationsExceeded error, got: {msg}",
        );

        let run_id = snap.trace_run_id.clone().expect("run_id must be set");
        let run = store
            .load(&run_id)
            .await
            .expect("load must succeed")
            .expect("run row must exist");
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.error.as_deref(), Some("Max iterations exceeded"));
    }

    #[tokio::test]
    async fn blocked_max_iterations_settlement_does_not_publish_business_hooks() -> Result<()> {
        use crate::memory::ConversationStore;
        use crate::skills::hooks::{HookAction, HookEvent, HookRule, HooksDefinition};
        use crate::state::RuntimeStateStore;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let conversation_root = tempfile::tempdir()?;
        let runtime_root = tempfile::tempdir()?;
        let conversations = Arc::new(crate::memory::FileConversationStore::new(
            conversation_root.path(),
        )?);
        let runtime = Arc::new(crate::state::FileRuntimeStateStore::new(
            runtime_root.path(),
        )?);
        let acquired = conversations
            .ensure_projection_epoch(crate::memory::EnsureConversationProjectionRequest {
                conversation: crate::memory::NewConversation {
                    conversation_id: "blocked-max-iterations".to_string(),
                    user_id: "default".to_string(),
                    agent_type: None,
                    title: None,
                },
                expected_tombstone_epoch: None,
            })
            .await?;
        let delete = crate::memory::ManagedConversationDelete::prepare(
            "blocked-max-iterations",
            acquired.authority.epoch,
        )?;
        runtime
            .begin_scope_retirement(crate::state::ScopeRetirementRequest::prepare(
                "blocked-max-iterations",
                0,
                &delete,
            )?)
            .await?;

        let mut agent = ReactAgent::new(
            AgentConfig::new("test-model", "agent-max-blocked", "sys")
                .conversation_id("blocked-max-iterations"),
        );
        agent.set_conversation_store(conversations);
        agent.set_state_store(runtime);
        let hook_calls = Arc::new(AtomicUsize::new(0));
        {
            let calls = hook_calls.clone();
            let mut hooks = agent.hook_registry().write().await;
            hooks.set_subagent_executor(Arc::new(move |_name, _task| {
                let calls = calls.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok("called".to_string())
                })
            }));
            let mut definition = HooksDefinition::default();
            for event in [HookEvent::SessionEnd, HookEvent::StopFailure] {
                definition.add_rules(
                    event,
                    vec![HookRule {
                        matcher: "max_iterations".to_string(),
                        hooks: vec![HookAction::Subagent {
                            name: "terminal-probe".to_string(),
                            task: None,
                            timeout: 0,
                        }],
                    }],
                );
            }
            hooks.register_user_hooks(definition);
        }

        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let error = finalize_max_iterations(&snapshot, &agent.memory.context, tx)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("blocked settlement unexpectedly completed".into()))?;
        assert!(matches!(error, ReactError::RuntimeState(_)));
        assert_eq!(hook_calls.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn final_answer_guard_failure_settles_failed_terminal() -> Result<()> {
        let store = Arc::new(InMemoryRunStore::new());
        let mut agent = ReactAgent::new(AgentConfig::new("test-model", "guarded", "sys"));
        agent.set_run_store(store.clone());
        let legacy = agent.capture_legacy_external_context();
        let run_id = agent
            .start_legacy_trace_run("blocked answer", &legacy)
            .await
            .ok_or_else(|| ReactError::Other("trace did not start".into()))?;
        agent.set_guard_manager(echo_core::guard::GuardManager::from_guards(vec![Arc::new(
            BlockFinalAnswerGuard,
        )]));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let mut state = LoopState::new();
        let outcome = emit_final_text(
            &snapshot,
            &agent.memory.context,
            &tx,
            &mut state,
            0,
            0,
            0,
            "blocked answer".to_string(),
            String::new(),
            Vec::new(),
            false,
        )
        .await?;
        assert!(matches!(
            outcome,
            ControlFlow::Break(crate::agent::AgentSteerTurnOutcome::Failed)
        ));
        let mut saw_error = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                Ok(AgentEvent::Error { source, .. }) if source == "output_guard" => {
                    saw_error = true;
                }
                Ok(AgentEvent::FinalAnswer(_)) => {
                    return Err(ReactError::Other(
                        "blocked final answer was emitted".to_string(),
                    ));
                }
                Ok(AgentEvent::Token(_)) => {
                    return Err(ReactError::Other(
                        "blocked final answer leaked through Token".to_string(),
                    ));
                }
                _ => {}
            }
        }
        assert!(saw_error);
        let run = store
            .load(&run_id)
            .await?
            .ok_or_else(|| ReactError::Other("trace run disappeared".into()))?;
        assert_eq!(run.status, RunStatus::Failed);
        assert!(
            run.error
                .as_deref()
                .is_some_and(|error| error.contains("final answer policy rejected"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn final_answer_transform_is_identical_in_token_and_terminal() -> Result<()> {
        let mut agent = ReactAgent::new(AgentConfig::new("test-model", "guarded", "sys"));
        agent.set_guard_manager(echo_core::guard::GuardManager::from_guards(vec![Arc::new(
            TransformFinalAnswerGuard,
        )]));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let mut state = LoopState::new();
        let outcome = emit_final_text(
            &snapshot,
            &agent.memory.context,
            &tx,
            &mut state,
            0,
            0,
            0,
            "raw answer".to_string(),
            String::new(),
            Vec::new(),
            false,
        )
        .await?;
        assert!(matches!(
            outcome,
            ControlFlow::Break(crate::agent::AgentSteerTurnOutcome::Completed)
        ));
        let mut delivered = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                Ok(AgentEvent::Token(text)) | Ok(AgentEvent::FinalAnswer(text)) => {
                    delivered.push(text);
                }
                _ => {}
            }
        }
        assert_eq!(delivered, vec!["filtered answer", "filtered answer"]);
        let messages = agent.memory.context.lock().await.messages().to_vec();
        assert!(
            messages
                .iter()
                .any(|message| message.content.as_text().as_deref() == Some("filtered answer"))
        );
        assert!(
            !messages
                .iter()
                .any(|message| message.content.as_text().as_deref() == Some("raw answer"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn unconfigured_output_guard_keeps_final_answer_verbatim() -> Result<()> {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "unguarded", "sys"));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let answer = "token: sk-ant-api03-abc123def456ghi789";
        assert!(crate::security::contains_secrets(answer));
        assert_eq!(snapshot.check_final_answer_guard(answer).await?, answer);
        Ok(())
    }
}
