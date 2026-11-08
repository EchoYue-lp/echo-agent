use super::*;
use echo_core::agent::{AgentPhase, AssistantMessageBlock, ExecutionStatus};
use echo_core::llm::types::ReasoningBlock;

fn status(execution: &str, sequence: u64, kind: ExecutionStatusKind) -> AgentEvent {
    AgentEvent::ExecutionStatus(ExecutionStatus {
        adapter_generated: false,
        execution_id: execution.to_string(),
        request_id: Some("request".to_string()),
        sequence,
        phase: AgentPhase::Thinking,
        status: kind,
        reason: None,
    })
}

fn block(id: &str, sequence: u64, phase: AssistantPhase, text: &str) -> AssistantMessageBlock {
    AssistantMessageBlock {
        provider_source: None,
        block_id: id.to_string(),
        request_id: "request".to_string(),
        sequence,
        phase,
        delta: text.to_string(),
        complete: false,
        provider: None,
        reasoning: None,
    }
}

fn text_chunks(updates: &[SessionUpdate], thought: bool) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| {
            let chunk = match update {
                SessionUpdate::AgentThoughtChunk(chunk) if thought => chunk,
                SessionUpdate::AgentMessageChunk(chunk) if !thought => chunk,
                _ => return None,
            };
            match &chunk.content {
                ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            }
        })
        .collect()
}

fn started() -> Result<ProjectionState> {
    let mut state = ProjectionState::default();
    assert!(
        state
            .updates(&status("source", 0, ExecutionStatusKind::Started), 4_096)?
            .is_empty()
    );
    Ok(state)
}

#[test]
fn native_thought_and_message_chunks_never_publish_reasoning_replay_secrets() -> Result<()> {
    let mut state = started()?;
    let mut signed = block("signed", 0, AssistantPhase::Reasoning, "思路🙂");
    signed.reasoning = Some(ReasoningBlock::Signed {
        thinking: "思路🙂".to_string(),
        signature: "private-signature".to_string(),
    });
    let first = state.updates(
        &AgentEvent::AssistantMessage {
            block: signed.clone(),
        },
        4_096,
    )?;
    assert_eq!(text_chunks(&first, true), vec!["思路🙂"]);
    assert!(
        state
            .updates(
                &AgentEvent::AssistantMessage {
                    block: signed.clone()
                },
                4_096
            )?
            .is_empty()
    );
    signed.sequence = 1;
    signed.delta.clear();
    signed.complete = true;
    assert!(
        state
            .updates(&AgentEvent::AssistantMessage { block: signed }, 4_096)?
            .is_empty()
    );

    let mut opaque = block("opaque", 2, AssistantPhase::Reasoning, "");
    opaque.complete = true;
    opaque.reasoning = Some(ReasoningBlock::Opaque {
        provider: "provider".to_string(),
        id: "opaque-item".to_string(),
        data: "private-encrypted-state".to_string(),
        summary: vec!["可展示摘要".to_string()],
    });
    let summary = state.updates(&AgentEvent::AssistantMessage { block: opaque }, 4_096)?;
    assert_eq!(text_chunks(&summary, true), vec!["可展示摘要"]);
    let mut redacted = block("redacted", 3, AssistantPhase::Reasoning, "");
    redacted.reasoning = Some(ReasoningBlock::Redacted {
        data: "private-redacted-state".to_string(),
    });
    assert!(
        state
            .updates(&AgentEvent::AssistantMessage { block: redacted }, 4_096)?
            .is_empty()
    );

    let message = state.updates(
        &AgentEvent::AssistantMessage {
            block: block("commentary", 4, AssistantPhase::Commentary, "正在检查"),
        },
        4_096,
    )?;
    assert_eq!(text_chunks(&message, false), vec!["正在检查"]);
    let encoded = serde_json::to_string(&(first, summary, message))?;
    for secret in [
        "private-signature",
        "private-encrypted-state",
        "private-redacted-state",
    ] {
        assert!(!encoded.contains(secret));
    }
    Ok(())
}

#[test]
fn duplicate_native_blocks_and_final_confirmation_do_not_duplicate_final_body() -> Result<()> {
    let mut state = started()?;
    let answer = AgentEvent::AssistantMessage {
        block: block("final", 0, AssistantPhase::FinalAnswer, "安全答复"),
    };
    assert_eq!(
        text_chunks(&state.updates(&answer, 4_096)?, false),
        vec!["安全答复"]
    );
    assert!(state.updates(&answer, 4_096)?.is_empty());
    let confirmed = AgentEvent::FinalAnswer("安全答复".to_string());
    assert!(state.updates(&confirmed, 4_096)?.is_empty());
    assert!(state.updates(&confirmed, 4_096)?.is_empty());
    let settled = status("source", 1, ExecutionStatusKind::Settled);
    assert!(state.updates(&settled, 4_096)?.is_empty());
    assert!(state.updates(&settled, 4_096)?.is_empty());
    Ok(())
}

#[test]
fn progress_text_cannot_hide_different_guard_approved_final_body() -> Result<()> {
    for progress_phase in [AssistantPhase::Commentary, AssistantPhase::Unknown] {
        let mut state = started()?;
        let progress = AgentEvent::AssistantMessage {
            block: block("progress", 0, progress_phase, "正在处理"),
        };
        assert_eq!(
            text_chunks(&state.updates(&progress, 4_096)?, false),
            vec!["正在处理"]
        );
        assert!(
            state
                .updates(
                    &AgentEvent::FinalAnswer("已通过最终校验的正文".to_string()),
                    4_096
                )?
                .is_empty()
        );
        let settled = status("source", 1, ExecutionStatusKind::Settled);
        assert_eq!(
            text_chunks(&state.updates(&settled, 4_096)?, false),
            vec!["已通过最终校验的正文"]
        );
        assert!(state.updates(&settled, 4_096)?.is_empty());
    }
    Ok(())
}

#[test]
fn final_unknown_prefix_gets_only_its_missing_approved_tail() -> Result<()> {
    let mut state = started()?;
    state.updates(
        &AgentEvent::AssistantMessage {
            block: block("unknown", 0, AssistantPhase::Unknown, "最终"),
        },
        4_096,
    )?;
    state.updates(&AgentEvent::FinalAnswer("最终正文".to_string()), 4_096)?;
    assert_eq!(
        text_chunks(
            &state.updates(&status("source", 1, ExecutionStatusKind::Settled), 4_096)?,
            false
        ),
        vec!["正文"]
    );
    Ok(())
}

#[test]
fn invalid_native_status_never_recovers_into_successful_projection() -> Result<()> {
    let mut synthetic = status("source", 0, ExecutionStatusKind::Started);
    if let AgentEvent::ExecutionStatus(value) = &mut synthetic {
        value.adapter_generated = true;
    }
    let mut without_start = ProjectionState::default();
    assert!(without_start.updates(&synthetic, 4_096).is_err());
    assert!(
        without_start
            .updates(&status("source", 0, ExecutionStatusKind::Started), 4_096)
            .is_err()
    );
    let mut without_start = ProjectionState::default();
    assert!(
        without_start
            .updates(&status("source", 1, ExecutionStatusKind::Settled), 4_096)
            .is_err()
    );

    let mut conflict = status("source", 0, ExecutionStatusKind::Started);
    if let AgentEvent::ExecutionStatus(value) = &mut conflict {
        value.reason = Some("conflicting observation".to_string());
    }
    let mut missing_request = status("source", 1, ExecutionStatusKind::Settled);
    if let AgentEvent::ExecutionStatus(value) = &mut missing_request {
        value.request_id = None;
    }
    for invalid in [
        status("foreign", 1, ExecutionStatusKind::Settled),
        conflict,
        missing_request,
    ] {
        let mut state = started()?;
        state.updates(&AgentEvent::FinalAnswer("approved".to_string()), 4_096)?;
        assert!(state.updates(&invalid, 4_096).is_err());
        assert!(
            state
                .updates(&status("source", 2, ExecutionStatusKind::Settled), 4_096)
                .is_err()
        );
    }
    assert!(
        serde_json::from_value::<ExecutionStatus>(serde_json::json!({
            "execution_id":"source", "request_id":"request", "sequence":1,
            "phase":"terminal", "status":"unknown-terminal", "reason":null
        }))
        .is_err()
    );
    Ok(())
}

#[test]
fn failed_or_cancelled_native_status_never_emits_final_success_body() -> Result<()> {
    for kind in [ExecutionStatusKind::Failed, ExecutionStatusKind::Cancelled] {
        let mut state = started()?;
        state.updates(
            &AgentEvent::FinalAnswer("must-not-display".to_string()),
            4_096,
        )?;
        assert!(state.updates(&status("source", 1, kind), 4_096)?.is_empty());
        assert!(
            state
                .updates(&status("source", 2, ExecutionStatusKind::Settled), 4_096)
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn conflicting_block_identity_fails_before_a_second_text_chunk() -> Result<()> {
    let mut state = started()?;
    state.updates(
        &AgentEvent::AssistantMessage {
            block: block("same", 0, AssistantPhase::Unknown, "original"),
        },
        4_096,
    )?;
    assert!(
        state
            .updates(
                &AgentEvent::AssistantMessage {
                    block: block("same", 0, AssistantPhase::Unknown, "changed")
                },
                4_096
            )
            .is_err()
    );
    assert!(
        state
            .updates(&AgentEvent::FinalAnswer("changed".to_string()), 4_096)
            .is_err()
    );
    Ok(())
}

#[test]
fn projected_tool_updates_keep_the_original_call_id_and_terminal_status() -> Result<()> {
    let call_id = "原工具-call";
    for success in [true, false] {
        let mut state = started()?;
        let call = AgentEvent::ToolCall {
            call_id: call_id.to_string(),
            invocation: crate::agent::ToolInvocation {
                requested_name: "inspect".to_string(),
                requested_args: serde_json::json!({"path":"原始"}),
                name: "inspect".to_string(),
                args: serde_json::json!({"path":"原始"}),
                rewrites: Vec::new(),
            },
        };
        let call_update = state.updates(&call, 4_096)?;
        assert!(matches!(
            call_update.first(),
            Some(SessionUpdate::ToolCall(call)) if call.status == ToolCallStatus::Pending
        ));
        let encoded = serde_json::to_string(&call_update)?;
        assert!(encoded.contains(call_id));
        let progress = state.updates(
            &AgentEvent::ToolStream {
                call_id: call_id.to_string(),
                name: "inspect".to_string(),
                event: crate::tools::ToolStreamEvent::Progress {
                    message: "正在读取".to_string(),
                    percent: Some(50),
                },
            },
            4_096,
        )?;
        let encoded = serde_json::to_string(&progress)?;
        assert!(encoded.contains(call_id));
        assert!(encoded.contains("in_progress"));
        let mut result = crate::tools::ToolResult::success("original result");
        result.success = success;
        let updates = state.updates(
            &AgentEvent::ToolResult {
                call_id: call_id.to_string(),
                name: "inspect".to_string(),
                result,
            },
            4_096,
        )?;
        assert!(matches!(
            updates.first(),
            Some(SessionUpdate::ToolCallUpdate(_))
        ));
        let encoded = serde_json::to_string(&updates)?;
        assert!(encoded.contains(call_id));
        assert!(encoded.contains(if success { "completed" } else { "failed" }));
    }
    Ok(())
}

#[test]
fn unicode_text_and_serialized_notification_budgets_keep_exact_boundaries() -> Result<()> {
    assert!(validate_update_size("中🙂", 2).is_ok());
    assert!(validate_update_size("中🙂", 1).is_err());
    let mut oversized = started()?;
    assert!(
        oversized
            .updates(
                &AgentEvent::AssistantMessage {
                    block: block("oversized", 0, AssistantPhase::Unknown, "中🙂文"),
                },
                2
            )
            .is_err()
    );
    let notification = SessionNotification::new(
        SessionId::new("session"),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            "中🙂",
        )))),
    );
    let chars = serde_json::to_string(&notification)?.chars().count();
    let twice = chars
        .checked_mul(2)
        .ok_or_else(|| ReactError::Other("test budget overflow".to_string()))?;
    let mut exact = ProjectionState::default();
    exact.reserve_notification(&notification, chars, 2, twice)?;
    exact.reserve_notification(&notification, chars, 2, twice)?;
    assert_eq!((exact.updates_sent, exact.total_update_chars), (2, twice));
    assert!(
        exact
            .reserve_notification(&notification, chars, 2, usize::MAX)
            .is_err()
    );
    assert_eq!((exact.updates_sent, exact.total_update_chars), (2, twice));
    let mut short = ProjectionState::default();
    assert!(
        short
            .reserve_notification(&notification, chars.saturating_sub(1), 2, twice)
            .is_err()
    );
    assert_eq!((short.updates_sent, short.total_update_chars), (0, 0));
    let mut total = ProjectionState::default();
    total.reserve_notification(&notification, chars, 2, chars)?;
    assert!(
        total
            .reserve_notification(&notification, chars, 2, chars)
            .is_err()
    );
    assert_eq!((total.updates_sent, total.total_update_chars), (1, chars));
    let mut overflow = ProjectionState {
        updates_sent: usize::MAX,
        ..Default::default()
    };
    assert!(
        overflow
            .reserve_notification(&notification, chars, usize::MAX, usize::MAX)
            .is_err()
    );
    let mut overflow = ProjectionState {
        total_update_chars: usize::MAX,
        ..Default::default()
    };
    assert!(
        overflow
            .reserve_notification(&notification, chars, usize::MAX, usize::MAX)
            .is_err()
    );
    Ok(())
}
