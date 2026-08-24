//! ReAct loop core

use super::super::ReactAgent;
use super::types::StreamMode;
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{ReactError, Result};
use crate::guard::GuardDirection;
use crate::llm::types::Message;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

impl ReactAgent {
    /// Prepare non-streaming execution context: guard check, turn tracking,
    /// memory recall, push user message, start trace run.
    ///
    /// Returns recalled memory count and the guard-approved input.
    async fn prepare_react_context(&self, message: &str) -> Result<(usize, String)> {
        let agent = self.config.agent_name.clone();
        let mut effective_message = message.to_string();

        // Clear read-before-edit tracking for the new conversation turn
        self.clear_read_files();

        // Input guard check
        if let Some(gm) = &self.guard.guard_manager {
            info!(agent = %agent, direction = "input", "🛡️ Guard check started");
            let result = gm.check_all(message, GuardDirection::Input).await?;
            match result {
                crate::guard::GuardResult::Block { reason } => {
                    info!(agent = %agent, reason = %reason, "🛡️ Input blocked by guard");
                    if let Some(al) = &self.guard.audit_logger {
                        let event = crate::audit::AuditEvent::now(
                            self.config.session_id.clone(),
                            agent.clone(),
                            crate::audit::AuditEventType::GuardBlock {
                                guard: "guard_manager".to_string(),
                                direction: GuardDirection::Input,
                                reason: reason.clone(),
                            },
                        );
                        if let Err(e) = al.log(event).await {
                            tracing::warn!(error = %e, "Failed to log guard audit event");
                        }
                    }
                    return Err(ReactError::Other(format!(
                        "Request blocked by safety guard: {reason}"
                    )));
                }
                crate::guard::GuardResult::Transform { content, .. } => {
                    effective_message = content;
                }
                crate::guard::GuardResult::Pass | crate::guard::GuardResult::Warn { .. } => {}
            }
        }

        // Trace identity is independent from the product run identity and is
        // established before the first observable execution phase.
        self.start_trace_run(&effective_message).await;

        // Phase: Recall
        self.record_trace_event(crate::trace::RunEvent::PhaseTransition {
            phase: "recall".into(),
            iteration: 0,
        })
        .await;
        // Persist memory-worthy triggers before recall injection mutates context.
        self.detect_and_write_memory_triggers(&effective_message)
            .await;

        // Inject relevant long-term memories
        let mut recalled = 0usize;
        let mut memory_context = None;
        match self.recall_long_term_memories(&effective_message).await {
            Ok(items) if !items.is_empty() => {
                recalled = items.len();
                debug!(agent = %agent, count = items.len(), "📚 Injecting relevant long-term memories");
                memory_context = Some(super::context::format_memory_context(&items));
            }
            Ok(_) => {}
            Err(e) => {
                warn!(agent = %agent, error = %e, "⚠️ Long-term memory retrieval failed, skipping injection");
            }
        }

        let wd = self.config.working_dir.lock().ok().and_then(|g| g.clone());
        let ws_block = crate::agent::react::ReactAgent::build_workspace_context_block(wd.as_ref());
        let mut context = self.memory.context.lock().await;
        context.replace_projection(
            super::context::WORKSPACE_CONTEXT_PROJECTION,
            (!ws_block.trim().is_empty())
                .then(|| super::context::runtime_context_note("workspace", &ws_block)),
        );
        context.replace_tail_projection(
            super::context::TURN_MEMORY_CONTEXT_PROJECTION,
            memory_context
                .map(|body| super::context::runtime_context_note("memory", body.as_str())),
        );
        context.push(Message::user(effective_message.clone()));

        Ok((recalled, effective_message))
    }

    /// Core ReAct loop — thin wrapper that delegates to the shared `run_core_loop`.
    ///
    /// Creates a snapshot + channel, runs the unified core loop in a spawned task,
    /// then collects `FinalAnswer` from the event stream.
    #[tracing::instrument(skip(self, message), fields(agent = %self.config.agent_name, model = %self.config.model_name))]
    pub(crate) async fn run_react_loop(&self, message: &str) -> Result<String> {
        // ★ Serialize all execution on this agent — only one run at a time.
        let _execution_guard = self.execution_mutex.lock().await;

        // Prepare context (guard check, memory recall, push message, start trace)
        let (recalled, effective_message) = match self.prepare_react_context(message).await {
            Ok(prepared) => prepared,
            Err(e) => {
                // Guard blocked — return the message directly (not an error)
                let msg = e.to_string();
                if msg.starts_with("Request blocked by safety guard:") {
                    return Ok(msg);
                }
                return Err(e);
            }
        };
        let turn_id = self
            .current_run_id
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let active_turn_lease = self.turn_steer_mailbox.begin(turn_id.clone());

        // Intent routing may activate a skill. Direct answers still use the
        // canonical ReAct lifecycle so they share terminal and persistence semantics.
        if let Some(ref router) = self.intent_router {
            let messages = self.memory.context.lock().await.messages().to_vec();
            let cancel = self
                .external_cancel
                .lock()
                .ok()
                .and_then(|token| token.as_ref().map(|token| token.as_ref().clone()))
                .unwrap_or_default();
            let intent = router
                .classify_with_cancel(&effective_message, &messages, cancel)
                .await;
            match intent {
                crate::intent::Intent::DirectAnswer { confidence } => {
                    tracing::debug!(
                        agent = %self.config.agent_name,
                        confidence,
                        "DirectAnswer routed through ReAct for pre-model projection"
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
                        "🎯 IntentRouter: activating skill"
                    );
                    if let Err(e) = self.activate_skill(&skill_name).await {
                        tracing::warn!(skill = %skill_name, error = %e, "IntentRouter: failed to activate skill");
                    }
                }
                crate::intent::Intent::Fallback => {
                    tracing::debug!(agent = %self.config.agent_name, "IntentRouter: Fallback to ReAct");
                }
            }
        }

        // Create channel + snapshot
        active_turn_lease.set_steerable(true);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(self.config.stream_buffer_size);
        let mut snap = AgentRunSnapshot::from_agent(self);
        snap.current_run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        snap.current_turn_id = Some(turn_id);
        snap.turn_steer_incarnation = Some(active_turn_lease.incarnation());
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
        let turn_cancel = snap
            .external_cancel
            .as_ref()
            .map(|cancel| cancel.as_ref().clone());

        // Run the shared core loop in a spawned task
        let context = self.memory.context.clone();
        let text = effective_message;
        let core = tokio::spawn(snap.run_core_loop(
            context,
            text,
            None,
            String::new(),
            StreamMode::Chat,
            recalled,
            false,
            tx,
        ));

        // Collect events, extract FinalAnswer
        let mut terminal = None;
        while let Some(event) = rx.recv().await {
            match event {
                Ok(AgentEvent::FinalAnswer(a)) => {
                    terminal = Some(Ok(a));
                    break;
                }
                Ok(AgentEvent::Cancelled) => {
                    terminal = Some(Err(ReactError::Agent(Box::new(
                        crate::error::AgentError::Cancelled("agent run".to_string()),
                    ))));
                    break;
                }
                Ok(AgentEvent::Error { message, .. }) => {
                    terminal = Some(Err(ReactError::Other(message)));
                    break;
                }
                Err(e) => {
                    terminal = Some(Err(e));
                    break;
                }
                _ => {} // Ignore intermediate events (Token, ToolCall, etc.)
            }
        }

        let core_result = core.await.map_err(|error| {
            ReactError::Other(format!("Core loop task failed before terminal: {error}"))
        });
        match core_result {
            Ok(Ok(outcome)) => active_turn_lease.settle(outcome),
            Ok(Err(error)) => {
                let outcome = if turn_cancel
                    .as_ref()
                    .is_some_and(crate::agent::CancellationToken::is_cancelled)
                {
                    crate::agent::AgentSteerTurnOutcome::Cancelled
                } else {
                    crate::agent::AgentSteerTurnOutcome::Failed
                };
                active_turn_lease.settle(outcome);
                return Err(error);
            }
            Err(error) => {
                drop(active_turn_lease);
                return Err(error);
            }
        }
        terminal.unwrap_or_else(|| {
            Err(ReactError::Other(
                "Core loop closed without a terminal event".to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::hooks::{
        HookAction, HookEvent, HookRegistry, HookResult, HookRule, HooksDefinition,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn tracked_steer_nonstream_hook_block_is_failed_before_drain() -> Result<()> {
        let hook_entered = Arc::new(tokio::sync::Notify::new());
        let release_hook = Arc::new(tokio::sync::Notify::new());
        let mut definition = HooksDefinition::default();
        definition.add_rules(
            HookEvent::UserPromptSubmit,
            vec![HookRule {
                matcher: String::new(),
                hooks: vec![HookAction::McpTool {
                    server: "test".to_string(),
                    tool: "blocking-hook".to_string(),
                    arguments: None,
                    timeout: 5,
                }],
            }],
        );
        let mut registry = HookRegistry::new();
        let entered = hook_entered.clone();
        let release = release_hook.clone();
        registry.set_mcp_executor(Arc::new(move |_, _, _| {
            let entered = entered.clone();
            let release = release.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                HookResult {
                    block: true,
                    block_reason: Some("blocked by test hook".to_string()),
                    ..HookResult::default()
                }
            })
        }));
        registry.register("slow-blocker", "/tmp", definition);

        let mut agent = ReactAgent::new(crate::agent::AgentConfig::new(
            "test-model",
            "agent",
            "system",
        ));
        agent.set_hook_registry(Arc::new(tokio::sync::RwLock::new(registry)));
        *agent
            .current_run_id
            .lock()
            .map_err(|_| ReactError::Other("run identity lock poisoned".to_string()))? =
            Some("hook-block-turn".to_string());
        let agent = Arc::new(agent);
        let running = {
            let agent = agent.clone();
            tokio::spawn(async move { agent.run_react_loop("blocked request").await })
        };

        tokio::time::timeout(std::time::Duration::from_secs(1), hook_entered.notified())
            .await
            .map_err(|_| ReactError::Other("UserPromptSubmit hook did not start".to_string()))?;
        let mut receipt = agent
            .steer_input_tracked(
                Some("hook-block-turn"),
                Message::user("must not be consumed".to_string()),
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert_eq!(receipt.state(), crate::agent::AgentSteerState::Accepted);
        release_hook.notify_one();

        let response = running
            .await
            .map_err(|error| ReactError::Other(format!("nonstream task failed: {error}")))??;
        assert!(response.contains("blocked by test hook"));
        assert_eq!(
            receipt.wait_for_turn_settled().await,
            crate::agent::AgentSteerState::TurnSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
                drained: false,
            }
        );
        Ok(())
    }
}
