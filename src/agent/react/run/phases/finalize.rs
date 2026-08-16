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

/// Tools-branch terminal: a `final_answer` tool call has passed verifier.
/// Runs `on_final_answer` callbacks + interventions, audit, runtime
/// checkpoint and transcript projection, emits the
/// `FinalAnswer` event, runs the `Stop` hook (best-effort continuation
/// injection without retry — `finish` is genuinely terminal here), then
/// fires `SessionEnd("complete")`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_completed_run(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    label: &str,
    output: &str,
    _iteration: usize,
    state: &LoopState,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<ControlFlow<(), ()>> {
    let agent = &snap.config.agent_name;
    for cb in snap.config.callbacks.iter() {
        cb.on_final_answer(agent, output).await;
    }

    // ── Intervention callbacks for final answer (streaming path) ──
    for intervention in &snap.tools.intervention_callbacks {
        let result = intervention.on_final_answer(agent, output).await;
        if result.cancel {
            return Err(ReactError::Other(
                "Agent execution cancelled by intervention at final answer".into(),
            ));
        }
        if result.block {
            let reason = result
                .block_reason
                .unwrap_or_else(|| "blocked by intervention at final answer".into());
            info!(agent = %agent, reason = %reason, "Intervention blocked final answer (streaming)");
            return Err(ReactError::Other(format!(
                "Final answer blocked by intervention: {}",
                reason
            )));
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

    let hc = crate::skills::hooks::HookContext::for_stop(
        None,
        snap.config.session_id.as_deref().unwrap_or(""),
        &snap.config.agent_name,
        state.stop_hook_continued,
    );
    let reg = snap.tools.hook_registry.read().await.clone();
    let sr = reg.run_lifecycle_hooks(&hc).await;
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

    info!(agent = %agent, "Streaming execution completed{label}");
    if let Some(al) = &snap.guard.audit_logger {
        let ev = crate::audit::AuditEvent::now(
            snap.config.session_id.clone(),
            snap.config.agent_name.clone(),
            crate::audit::AuditEventType::FinalAnswer {
                content: output.to_string(),
            },
        );
        if let Err(e) = al.log(ev).await {
            tracing::error!(error = %e, "audit log write failed — event dropped");
        }
    }
    // Rich runtime checkpoint
    snap.save_runtime_checkpoint(context, None).await?;
    // Persist transcript projection so product layers see the final state.
    snap.save_transcript_projection(context).await;
    snap.finalize_run(crate::trace::RunStatus::Completed, Some(output), None)
        .await;
    if tx
        .send(Ok(AgentEvent::FinalAnswer(output.to_string())))
        .await
        .is_err()
    {
        return Ok(ControlFlow::Break(()));
    }
    snap.fire_hook(
        crate::skills::hooks::HookEvent::SessionEnd,
        Some("complete"),
    )
    .await;
    Ok(ControlFlow::Break(()))
}

/// Text-branch terminal: the LLM produced content that passed verification.
/// Runs `on_think_end` + `on_final_answer` callbacks, pushes the assistant
/// message, takes an auto-snapshot, audits the final answer, takes a
/// runtime checkpoint + transcript projection, finalizes the trace, emits the
/// `FinalAnswer` event, and runs
/// the `Stop` hook with one-shot continuation.
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
) -> Result<ControlFlow<(), ()>> {
    let agent = &snap.config.agent_name;

    let ts = vec![crate::agent::react::StepType::Thought(answer.clone())];
    for cb in snap.config.callbacks.iter() {
        cb.on_think_end(agent, &ts, pt, ct).await;
    }
    context.lock().await.push(with_reasoning_content(
        Message::assistant(answer.clone()),
        reasoning_content,
        reasoning_blocks,
    ));
    let hc = crate::skills::hooks::HookContext::for_stop(
        None,
        snap.config.session_id.as_deref().unwrap_or(""),
        &snap.config.agent_name,
        state.stop_hook_continued,
    );
    let reg = snap.tools.hook_registry.read().await.clone();
    let sr = reg.run_lifecycle_hooks(&hc).await;
    if let Some(reason) = &sr.continue_reason
        && !state.stop_hook_continued
    {
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
    snap.auto_snapshot(context, iteration).await;
    if let Some(al) = &snap.guard.audit_logger {
        let ev = crate::audit::AuditEvent::now(
            snap.config.session_id.clone(),
            snap.config.agent_name.clone(),
            crate::audit::AuditEventType::FinalAnswer {
                content: answer.clone(),
            },
        );
        if let Err(e) = al.log(ev).await {
            tracing::error!(error = %e, "audit log write failed — event dropped");
        }
    }
    // Rich runtime checkpoint (messages + plan + skills + blocked reason)
    snap.save_runtime_checkpoint(context, None).await?;
    // Persist user-visible transcript projection — single source of truth
    // for GUI/TUI history. Product layers should rely on this instead of
    // re-implementing save_messages on every chat turn.
    snap.save_transcript_projection(context).await;
    // Finalize trace before moving the answer into the event
    snap.finalize_run(crate::trace::RunStatus::Completed, Some(&answer), None)
        .await;
    // Sending FinalAnswer is mandatory; on a closed receiver the macro
    // returns Ok(()) from this fn — but we model that as ControlFlow::Break.
    if tx.send(Ok(AgentEvent::FinalAnswer(answer))).await.is_err() {
        return Ok(ControlFlow::Break(()));
    }
    snap.fire_hook(
        crate::skills::hooks::HookEvent::SessionEnd,
        Some("complete"),
    )
    .await;
    Ok(ControlFlow::Break(()))
}

/// LLM produced neither tool calls nor content — terminal failure.
pub(crate) async fn finalize_no_response(
    snap: &AgentRunSnapshot,
    tx: mpsc::Sender<Result<AgentEvent>>,
) -> Result<()> {
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
    // Save runtime checkpoint with blocked reason before failing
    snap.save_runtime_checkpoint(context, Some("Max iterations exceeded".to_string()))
        .await?;
    // Even on failure we save the transcript so the user sees what was
    // attempted in the GUI/TUI history pane.
    snap.save_transcript_projection(context).await;
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

    /// Build a snapshot whose `trace_run_id` is wired up so trace
    /// finalization can update the in-memory run store.
    async fn snap_with_trace(
        agent_name: &str,
    ) -> (AgentRunSnapshot, Arc<InMemoryRunStore>, ReactAgent) {
        let store: Arc<InMemoryRunStore> = Arc::new(InMemoryRunStore::new());
        let mut agent = ReactAgent::new(AgentConfig::new("test-model", agent_name, "sys"));
        agent.set_run_store(store.clone());
        agent.start_trace_run("test input").await;

        let snap = AgentRunSnapshot::from_agent(&agent);
        (snap, store, agent)
    }

    /// `finalize_no_response` sends a `NoResponse` error onto the channel
    /// and finalizes the trace as `Failed`.
    #[tokio::test]
    async fn finalize_no_response_sends_error_and_marks_trace_failed() {
        let (snap, store, _agent) = snap_with_trace("agent-noresp").await;
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        finalize_no_response(&snap, tx)
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
}
