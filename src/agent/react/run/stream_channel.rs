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
//! **Known gap (P1-3/P1-4):** The streaming path does not run IntentRouter
//! classification (short-circuit, skill activation) or `GuardDirection::Input`
//! checks. Both are present in the non-streaming path (`react_loop.rs`).
//! Streaming callers bypass these pre-flight checks; a future PR should
//! converge the two paths so that `run_stream_channel` also calls
//! `prepare_react_context` instead of the lighter `prepare_stream_context`.
//! and reports back via outcome enums; the driver translates those into
//! either "continue", a terminal `finalize_*`, or an early return.

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
        let (tx, rx) = mpsc::channel::<Result<AgentEvent>>(self.config.stream_buffer_size);
        let context = self.memory.context.clone();
        let text = init.text.clone();
        let message = init.message.clone();
        let label = init.label.clone();

        // ★ Acquire execution mutex BEFORE context mutation — using lock_owned()
        // so the guard can be moved into the spawned task and held for the
        // entire stream lifetime.
        let execution_guard = self.execution_mutex.clone().lock_owned().await;

        let recalled = if let Some(ref msg) = init.message {
            self.prepare_stream_context_with_message(mode, msg).await
        } else {
            self.prepare_stream_context(mode, &init.text).await
        };

        // Start trace run for streaming path
        self.start_trace_run(&text).await;

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
