//! Phase functions for `AgentRunSnapshot::run_core_loop`.
//!
//! `run_core_loop` is the **single, unified core loop** — both the streaming
//! and non-streaming entry points spawn the same loop body. To keep that body
//! reviewable, the per-iteration work is split into focused phase functions:
//!
//! ```text
//! prepare_turn  →  ┌─ run_compact ─→ run_think ─→ ┬─ run_tools (verifier-pass → finalize_completed_run)
//!                  │                              ├─ verify_final_text (verifier-pass → emit_final_text)
//!                  │                              └─ NoResponse → finalize_no_response
//!                  └─ (loop continues …)         max_iterations exhausted → finalize_max_iterations
//! ```
//!
//! Phase fns are `pub(crate) async fn` and all return `Result<…>`. The
//! `yield_event!` / `yield_final_event!` / `try_send!` macros from
//! `super::super::stream_macros` short-circuit the **enclosing function** with
//! `return Ok(())` when the receiver is closed, so phases must return
//! `Result<…>` for that "abandon stream" behavior to bubble up to the loop
//! driver — which then exits with `Ok(())`.

use crate::llm::types::Message;
use std::collections::HashMap;

pub(crate) mod compact;
pub(crate) mod finalize;
pub(crate) mod prepare;
pub(crate) mod think;
pub(crate) mod tools;
pub(crate) mod verify;

// ── Loop-level mutable state ─────────────────────────────────────────

/// Mutable per-turn state shared between phase invocations.
///
/// `agent` and `callbacks` are read directly from `snap.config` instead of
/// being mirrored here — they're cheap to clone and never mutated.
pub(crate) struct LoopState {
    /// Whether the `Stop` lifecycle hook has already been allowed to inject a
    /// continue_reason this turn. The hook is one-shot to avoid loops.
    pub stop_hook_continued: bool,
    /// Number of times the verifier has rejected an answer this turn.
    pub verifier_retry_count: usize,
    /// TaskNode id created by `prepare_turn` for DAG status tracking. `None`
    /// when no `RuntimeStateStore` is configured.
    pub task_node_id: Option<String>,
    /// Invocation-local budget counters. They remain in the loop state while
    /// HITL approval awaits and therefore are preserved across pause/resume.
    pub budget: RunBudgetState,
}

#[derive(Default)]
pub(crate) struct RunBudgetState {
    pub reported_model_tokens: usize,
    pub usage_complete: bool,
    pub wind_down_emitted: bool,
    pub final_only_emitted: bool,
    pub final_only: bool,
}

impl RunBudgetState {
    pub(crate) fn record_usage(&mut self, tokens: usize, reported: bool) {
        self.reported_model_tokens = self.reported_model_tokens.saturating_add(tokens);
        self.usage_complete = self.usage_complete && reported;
    }
}

impl LoopState {
    pub(crate) fn new(task_node_id: Option<String>) -> Self {
        Self {
            stop_hook_continued: false,
            verifier_retry_count: 0,
            task_node_id,
            budget: RunBudgetState {
                usage_complete: true,
                ..RunBudgetState::default()
            },
        }
    }
}

// ── Outcome enums ────────────────────────────────────────────────────

/// What `prepare_turn` decided.
pub(crate) enum PrepareOutcome {
    /// Proceed into the iteration loop with the given task node id.
    Continue { task_node_id: Option<String> },
    /// `UserPromptSubmit` hook returned `block: true`. A `FinalAnswer` event
    /// has already been yielded and `SessionEnd("blocked")` has been fired.
    BlockedAndDone,
    /// Channel was closed mid-prepare; loop driver returns `Ok(())`.
    Abandoned,
}

/// What `run_compact` produced for this iteration.
pub(crate) enum CompactOutcome {
    /// Prepared LLM messages ready for the think phase.
    Continue(Vec<Message>),
    /// Channel closed; loop driver returns `Ok(())`.
    Abandoned,
}

/// What `run_think` produced for this iteration.
pub(crate) enum ThinkOutcome {
    /// Stream consumed; tool calls / content / token usage extracted.
    Continue(ThinkOutput),
    /// Channel closed mid-think.
    Abandoned,
    /// Intervention callback issued cancel.
    Cancelled,
    /// Intervention callback issued block.
    Blocked,
}

/// Output of the think phase, fed into the tools or verify branches.
pub(crate) struct ThinkOutput {
    /// The messages sent to the LLM. Kept around in case a downstream phase
    /// wants to reference what was actually fed into the model (currently
    /// unused but useful for diagnostics).
    #[allow(dead_code)]
    pub messages: Vec<Message>,
    /// Plain assistant text accumulated from streaming chunks.
    pub content_buffer: String,
    /// Tool calls accumulated by index → (tool_call_id, function_name, args).
    pub tool_call_map: HashMap<u32, (String, String, String)>,
    /// Prompt tokens reported by the LLM.
    pub pt: usize,
    /// Completion tokens reported by the LLM.
    pub ct: usize,
    /// Whether the provider supplied usage metadata for this response.
    pub usage_reported: bool,
}

/// What a single iteration body decided.
pub(crate) enum IterOutcome {
    /// Verifier-fail or hook-continue: go to the next iteration.
    Continue,
    /// Tools branch: a `final_answer` tool call was verified and accepted.
    /// The driver invokes `phases::finalize::finalize_completed_run`.
    Finish { output: String },
    /// Text-only branch: the LLM produced a content answer that passed
    /// verification. The driver invokes `phases::finalize::emit_final_text`.
    FinalText { answer: String },
    /// LLM produced neither tool calls nor content. Terminal failure.
    NoResponse,
    /// Channel closed mid-iteration (a yield/try_send macro fired
    /// `return Ok(())`). The driver returns `Ok(())` immediately.
    Abandoned,
}

#[cfg(test)]
mod budget_tests {
    use super::LoopState;

    #[tokio::test]
    async fn budget_counters_survive_async_pause_resume_boundary() {
        let mut state = LoopState::new(None);
        state.budget.record_usage(12, true);

        // HITL approval pauses inside the same run future. Crossing an await
        // boundary must not reset invocation-local counters.
        tokio::task::yield_now().await;
        state.budget.record_usage(8, false);

        assert_eq!(state.budget.reported_model_tokens, 20);
        assert!(!state.budget.usage_complete);
    }
}
