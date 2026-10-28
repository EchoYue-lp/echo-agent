//! Phase functions for `AgentRunSnapshot::run_core_loop`.
//!
//! `run_core_loop` is the **single, unified core loop** — both the streaming
//! and non-streaming entry points spawn the same loop body. To keep that body
//! reviewable, the per-iteration work is split into focused phase functions:
//!
//! ```text
//! prepare_turn  →  ┌─ run_compact ─→ run_think ─→ ┬─ run_tools (回填结果后继续原循环)
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

use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::llm::types::{Message, ResponseFormat};

pub(crate) mod compact;
pub(crate) mod finalize;
pub(crate) mod prepare;
pub(crate) mod think;
pub(crate) mod tools;
pub(crate) mod verify;

/// 必要输入不可用是可由应用恢复的执行拒绝，不是物理未结算观察。
/// 使用原 typed failure envelope 的 code 扩点；取消和未结算仍保留原通道。
pub(crate) fn required_context_failure_event(
    snap: &AgentRunSnapshot,
    error: crate::error::ReactError,
) -> crate::error::Result<AgentEvent> {
    let observed = crate::error::AgentFailure::try_from(&error)?;
    if cancellation_requested(snap)
        || observed.terminal_kind == crate::error::AgentTerminalKind::Cancelled
    {
        return Err(crate::error::AgentError::Cancelled(
            "required context interrupted".to_string(),
        )
        .into());
    }
    Ok(AgentEvent::error_message(
        "required_context_unavailable",
        error.to_string(),
    ))
}

pub(crate) fn cancellation_requested(snap: &AgentRunSnapshot) -> bool {
    snap.cancel_token
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        || snap
            .external_cancel
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
}

pub(crate) async fn wait_for_cancellation(snap: &AgentRunSnapshot) {
    let invocation = async {
        match snap.cancel_token.as_ref() {
            Some(cancel) => cancel.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    let external = async {
        match snap.external_cancel.as_ref() {
            Some(cancel) => cancel.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = invocation => {}
        _ = external => {}
    }
}

pub(crate) fn pre_model_block_terminal(
    _format: Option<&ResponseFormat>,
    source: &str,
    reason: String,
) -> AgentEvent {
    // 输入被拒绝是失败事实，不能通过兼容FinalAnswer投影变成成功。
    AgentEvent::error_message(source, reason)
}

/// 只从同request/block原观察派生最后明确phase；不改raw增量，也不据工具或正文猜类型。
pub(super) fn effective_assistant_phase(
    blocks: &[echo_core::agent::AssistantMessageBlock],
    block: &echo_core::agent::AssistantMessageBlock,
) -> echo_core::agent::AssistantPhase {
    blocks
        .iter()
        .rev()
        .find(|last| {
            last.request_id == block.request_id
                && last.block_id == block.block_id
                && last.phase != echo_core::agent::AssistantPhase::Unknown
        })
        .map(|last| last.phase)
        .unwrap_or(echo_core::agent::AssistantPhase::Unknown)
}

/// 文字阶段按完整块事实判定，空metadata不凭空制造进度或成功。
pub(super) fn assistant_progress_only(blocks: &[echo_core::agent::AssistantMessageBlock]) -> bool {
    let mut has_text = false;
    for block in blocks {
        let phase = effective_assistant_phase(blocks, block);
        if phase == echo_core::agent::AssistantPhase::Reasoning {
            continue;
        }
        if !matches!(
            phase,
            echo_core::agent::AssistantPhase::Commentary
                | echo_core::agent::AssistantPhase::PartialAnswer
        ) {
            return false;
        }
        has_text |= !block.delta.is_empty();
    }
    has_text
}

// ── Loop-level mutable state ─────────────────────────────────────────

/// Mutable per-turn state shared between phase invocations.
///
/// `agent` and `callbacks` are read directly from `snap.config` instead of
/// being mirrored here — they're cheap to clone and never mutated.
pub(crate) struct LoopState {
    /// 本执行是否已按Stop反馈继续；传入Hook上下文帮助其避免重复。
    /// 此标记不覆盖新的block/continue，原ReAct预算决定有界终止。
    pub stop_hook_continued: bool,
    /// Number of times the verifier has rejected an answer this turn.
    pub verifier_retry_count: usize,
    /// JSON/schema repair attempts are independent of the optional critic.
    pub schema_retry_count: usize,
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
    pub(crate) fn new() -> Self {
        Self {
            stop_hook_continued: false,
            verifier_retry_count: 0,
            schema_retry_count: 0,
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
    /// Proceed into the iteration loop.
    Continue,
    /// `UserPromptSubmit` hook returned `block: true`. A `FinalAnswer` event
    /// has already been yielded and `SessionEnd("blocked")` has been fired.
    BlockedAndDone,
    /// Channel was closed mid-prepare; loop driver returns `Ok(())`.
    Abandoned,
}

/// What `run_compact` produced for this iteration.
#[derive(Debug)]
pub(crate) enum CompactOutcome {
    /// Prepared LLM messages ready for the think phase.
    Continue(Vec<Message>),
    /// Channel closed; loop driver returns `Ok(())`.
    Abandoned,
    /// Context preparation failed after emitting an error terminal.
    Failed,
}

/// What `run_think` produced for this iteration.
pub(crate) enum ThinkOutcome {
    /// Stream consumed; tool calls / content / token usage extracted.
    Continue(Box<ThinkOutput>),
    /// Channel closed mid-think.
    Abandoned,
    /// This phase already settled persistence, trace, and one terminal event.
    TerminalSettled {
        outcome: crate::agent::AgentSteerTurnOutcome,
    },
}

/// Output of the think phase, fed into the tools or verify branches.
pub(crate) struct ThinkOutput {
    /// Plain assistant text accumulated from streaming chunks.
    pub content_buffer: String,
    /// 完整响应形成的最终候选；显式进度/部分回答不会进入该分支。
    pub candidate_answer: Option<String>,
    /// provider pause_turn等完整继续边界，不能冒充成功或异常EOF。
    pub continue_response: bool,
    /// DeepSeek/Qwen reasoning accumulated for assistant-message replay.
    pub reasoning_buffer: String,
    /// Signed/redacted reasoning blocks required for provider-valid replay.
    pub reasoning_blocks: Vec<crate::llm::types::ReasoningBlock>,
    /// Ordered native assistant blocks retained in the durable Message.
    pub assistant_blocks: Vec<echo_core::agent::AssistantMessageBlock>,
    /// 已通过输出校验且实际发布的块投影；raw块仍只供协议回放。
    pub approved_assistant_blocks: Vec<echo_core::agent::AssistantMessageBlock>,
    /// Tool calls accumulated by index → (tool_call_id, function_name, args).
    pub tool_call_map: super::processor::ToolCallFragments,
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
    /// Text-only branch: the LLM produced a content answer that passed
    /// verification. The driver invokes `phases::finalize::emit_final_text`.
    FinalText {
        answer: String,
        reasoning_content: String,
        reasoning_blocks: Vec<crate::llm::types::ReasoningBlock>,
        assistant_blocks: Vec<echo_core::agent::AssistantMessageBlock>,
    },
    /// LLM produced neither tool calls nor content. Terminal failure.
    NoResponse,
    /// Channel closed mid-iteration (a yield/try_send macro fired
    /// `return Ok(())`). The driver returns `Ok(())` immediately.
    Abandoned,
    /// A phase already settled persistence, trace, and exactly one terminal.
    TerminalSettled {
        outcome: crate::agent::AgentSteerTurnOutcome,
    },
}

pub(crate) fn with_reasoning_content(
    mut message: Message,
    reasoning_content: String,
    reasoning_blocks: Vec<crate::llm::types::ReasoningBlock>,
) -> Message {
    if !reasoning_content.is_empty() {
        message.reasoning_content = Some(reasoning_content);
    }
    if !reasoning_blocks.is_empty() {
        message.reasoning_blocks = Some(reasoning_blocks);
    }
    message
}

#[cfg(test)]
mod budget_tests {
    use super::LoopState;

    #[tokio::test]
    async fn budget_counters_survive_async_pause_resume_boundary() {
        let mut state = LoopState::new();
        state.budget.record_usage(12, true);

        // HITL approval pauses inside the same run future. Crossing an await
        // boundary must not reset invocation-local counters.
        tokio::task::yield_now().await;
        state.budget.record_usage(8, false);

        assert_eq!(state.budget.reported_model_tokens, 20);
        assert!(!state.budget.usage_complete);
    }
}
