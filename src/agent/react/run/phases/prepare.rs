//! Pre-iteration setup: audit user input, run `UserPromptSubmit` hook,
//! create the execution `TaskNode`. See `phases/mod.rs` for the wider flow.

use super::super::stream_macros::{yield_event_or, yield_final_event_or};
use super::super::types::StreamMode;
use super::PrepareOutcome;
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::info;

/// One-shot pre-loop preparation: emit `MemoryRecalled`, audit the user
/// input, run the `UserPromptSubmit` lifecycle hook, create the execution
/// `TaskNode`.
///
/// On a closed receiver mid-prepare, returns `Ok(PrepareOutcome::Abandoned)`
/// so the loop driver simply returns `Ok(())`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_turn(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    text: &str,
    label: &str,
    mode: StreamMode,
    recalled: usize,
    user_prompt_hook_already_run: bool,
) -> Result<PrepareOutcome> {
    let agent = &snap.config.agent_name;
    match mode {
        StreamMode::Execute => info!(agent = %agent, "Agent streaming task execution{label}"),
        StreamMode::Chat => info!(agent = %agent, "Agent streaming conversation{label}"),
    }
    if recalled > 0 {
        yield_event_or!(
            tx,
            AgentEvent::MemoryRecalled { count: recalled },
            PrepareOutcome::Abandoned
        );
    }

    // Audit: user input
    if let Some(al) = &snap.guard.audit_logger {
        let event = crate::audit::AuditEvent::now(
            snap.config.session_id.clone(),
            snap.config.agent_name.clone(),
            crate::audit::AuditEventType::UserInput {
                content: text.to_string(),
            },
        );
        if let Err(e) = al.log(event).await {
            tracing::error!(error = %e, "audit log write failed — event dropped");
        }
    }

    // Streaming preparation runs this hook before routing; the core receives
    // that fact explicitly so lifecycle side effects remain exactly once.
    if !user_prompt_hook_already_run {
        let hook_ctx = crate::skills::hooks::HookContext::for_user_prompt_submit(
            text,
            None,
            snap.config.session_id.as_deref().unwrap_or(""),
            &snap.config.agent_name,
        );
        let registry = snap.tools.hook_registry.read().await.clone();
        let result = registry.run_lifecycle_hooks(&hook_ctx).await;
        if result.block {
            yield_final_event_or!(
                tx,
                AgentEvent::FinalAnswer(format!(
                    "Blocked by UserPromptSubmit hook: {}",
                    result.block_reason.unwrap_or_default()
                )),
                PrepareOutcome::BlockedAndDone
            );
            snap.fire_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("blocked"))
                .await;
            return Ok(PrepareOutcome::BlockedAndDone);
        }
        if let Some(ctx) = &result.injected_context {
            super::super::context::push_runtime_context_note(context, "Hook:UserPromptSubmit", ctx)
                .await;
        }
        for msg in &result.messages {
            super::super::context::push_runtime_context_note(context, "Hook:UserPromptSubmit", msg)
                .await;
        }
    }

    // Create TaskNode for this execution turn (DAG tracking)
    let task_node_id = snap.create_execution_node(text).await;
    Ok(PrepareOutcome::Continue { task_node_id })
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ReactAgent;
    use crate::agent::config::AgentConfig;
    use crate::audit::{AuditFilter, AuditLogger, InMemoryAuditLogger};
    use crate::skills::hooks::{HookAction, HookEvent, HookRegistry, HookRule, HooksDefinition};

    /// No hook registered + no recall → `prepare_turn` returns `Continue`,
    /// `task_node_id` is `None` (no state store), no events were yielded
    /// (recalled = 0 means we skip the `MemoryRecalled` event), and the
    /// audit logger captured the user input.
    #[tokio::test]
    async fn prepare_turn_continue_audits_user_input() {
        let audit = Arc::new(InMemoryAuditLogger::new());
        let mut agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        agent.set_audit_logger(audit.clone());
        let snap = AgentRunSnapshot::from_agent(&agent);

        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let outcome = prepare_turn(
            &snap,
            &agent.memory.context,
            &tx,
            "hello agent",
            "",
            StreamMode::Execute,
            0,
            false,
        )
        .await
        .expect("prepare_turn must succeed");

        match outcome {
            PrepareOutcome::Continue { task_node_id } => {
                assert!(
                    task_node_id.is_none(),
                    "no state_store configured → no TaskNode id",
                );
            }
            other => panic!("expected Continue, got {:?}", phase_outcome_label(&other)),
        }

        // recalled = 0 → no MemoryRecalled event; no other event either.
        assert!(
            rx.try_recv().is_err(),
            "no events should have been yielded by prepare_turn on the happy path",
        );

        // Audit logger should have exactly one UserInput row.
        let events = audit
            .query(AuditFilter::default())
            .await
            .expect("audit query must succeed");
        let user_inputs: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.event_type {
                crate::audit::AuditEventType::UserInput { content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(user_inputs, vec!["hello agent"]);
    }

    /// `recalled > 0` → a `MemoryRecalled { count }` event is yielded
    /// before audit / hook work.
    #[tokio::test]
    async fn prepare_turn_yields_memory_recalled_event() {
        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        let snap = AgentRunSnapshot::from_agent(&agent);

        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let _ = prepare_turn(
            &snap,
            &agent.memory.context,
            &tx,
            "anything",
            "",
            StreamMode::Chat,
            7,
            false,
        )
        .await
        .expect("prepare_turn must succeed");

        let event = rx
            .recv()
            .await
            .expect("MemoryRecalled event must be yielded")
            .expect("MemoryRecalled is a success event");
        match event {
            AgentEvent::MemoryRecalled { count } => assert_eq!(count, 7),
            other => panic!("expected MemoryRecalled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prepare_turn_does_not_append_cwd_system_message_each_turn() {
        let config = AgentConfig::new("test-model", "agent", "sys")
            .working_dir(Some(std::path::PathBuf::from("/tmp/eko-cache-test")));
        let agent = ReactAgent::new(config);
        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, _rx) = mpsc::channel::<Result<AgentEvent>>(8);

        let _ = prepare_turn(
            &snap,
            &agent.memory.context,
            &tx,
            "first",
            "",
            StreamMode::Chat,
            0,
            false,
        )
        .await
        .expect("first prepare_turn must succeed");
        let _ = prepare_turn(
            &snap,
            &agent.memory.context,
            &tx,
            "second",
            "",
            StreamMode::Chat,
            0,
            false,
        )
        .await
        .expect("second prepare_turn must succeed");

        let messages = agent.memory.context.lock().await.messages().to_vec();
        let system_messages = messages
            .iter()
            .filter(|message| matches!(message.role, echo_core::llm::types::Role::System))
            .count();
        // CWD is now injected as runtime context (not system) so the system
        // prefix stays cache-stable across workspace changes.
        let cwd_system_messages = messages
            .iter()
            .filter(|message| {
                matches!(message.role, echo_core::llm::types::Role::System)
                    && message.text_content().is_some_and(|text| {
                        text.contains("Current working directory: /tmp/eko-cache-test")
                    })
            })
            .count();
        assert_eq!(
            system_messages, 1,
            "only one system message (the base prompt)"
        );
        assert_eq!(
            cwd_system_messages, 0,
            "CWD must NOT be in system messages — it breaks prompt cache"
        );
        // CWD is injected via runtime context by prepare_stream_context
        // (tested separately). This test only verifies system message stability.
    }

    /// `UserPromptSubmit` hook returns `block: true` (via a Permission
    /// `deny` action) → `prepare_turn` yields a `FinalAnswer` event whose
    /// body starts with `"Blocked by UserPromptSubmit hook:"` and returns
    /// `BlockedAndDone`.
    #[tokio::test]
    async fn prepare_turn_user_prompt_submit_block_short_circuits() {
        let mut def = HooksDefinition::default();
        def.add_rules(
            HookEvent::UserPromptSubmit,
            vec![HookRule {
                matcher: String::new(), // matches everything
                hooks: vec![HookAction::Permission {
                    decision: "deny".into(),
                    reason: Some("test-deny".into()),
                    suggestions: vec![],
                }],
            }],
        );
        let mut registry = HookRegistry::new();
        registry.register("test-blocker", "/tmp", def);

        let agent = ReactAgent::new(AgentConfig::new("test-model", "agent", "sys"));
        // Replace the registry with our blocking one. set_hook_registry
        // takes an Arc<RwLock<...>>, so wrap before passing.
        let hr = Arc::new(tokio::sync::RwLock::new(registry));
        let mut agent = agent;
        agent.set_hook_registry(hr);

        let snap = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(8);
        let outcome = prepare_turn(
            &snap,
            &agent.memory.context,
            &tx,
            "any user message",
            "",
            StreamMode::Execute,
            0,
            false,
        )
        .await
        .expect("prepare_turn must succeed even when the hook blocks");

        assert!(
            matches!(outcome, PrepareOutcome::BlockedAndDone),
            "block hook → BlockedAndDone, got {:?}",
            phase_outcome_label(&outcome),
        );

        let final_event = rx
            .recv()
            .await
            .expect("FinalAnswer must be yielded")
            .expect("FinalAnswer is a success event");
        match final_event {
            AgentEvent::FinalAnswer(text) => {
                assert!(
                    text.starts_with("Blocked by UserPromptSubmit hook:"),
                    "final answer text: {text:?}",
                );
                assert!(
                    text.contains("test-deny"),
                    "final answer should carry the deny reason: {text:?}",
                );
            }
            other => panic!("expected FinalAnswer, got {other:?}"),
        }
    }

    fn phase_outcome_label(o: &PrepareOutcome) -> &'static str {
        match o {
            PrepareOutcome::Continue { .. } => "Continue",
            PrepareOutcome::BlockedAndDone => "BlockedAndDone",
            PrepareOutcome::Abandoned => "Abandoned",
        }
    }
}
