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
    async fn prepare_react_context(
        &self,
        message: &str,
        legacy: &super::super::LegacyExternalContextSnapshot,
    ) -> Result<(usize, String)> {
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
        self.start_legacy_trace_run(&effective_message, legacy)
            .await;

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
        // Capture legacy mutable context before queueing. A later caller may
        // update or clear the shared setters while this invocation waits for
        // the execution mutex, but cannot change this invocation's ownership.
        let legacy_runtime = self.capture_legacy_external_context();
        // ★ Serialize all execution on this agent — only one run at a time.
        let _execution_guard = self.execution_mutex.lock().await;

        // Prepare context (guard check, memory recall, push message, start trace)
        let (recalled, effective_message) =
            match self.prepare_react_context(message, &legacy_runtime).await {
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
        let turn_id = legacy_runtime
            .turn_id
            .clone()
            .or_else(|| legacy_runtime.current_run_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let active_turn_lease = self.turn_steer_mailbox.begin(turn_id.clone());

        // Intent routing may activate a skill. Direct answers still use the
        // canonical ReAct lifecycle so they share terminal and persistence semantics.
        if let Some(ref router) = self.intent_router {
            let messages = self.memory.context.lock().await.messages().to_vec();
            let cancel = legacy_runtime
                .cancel
                .as_ref()
                .map(|token| token.as_ref().clone())
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
        let mut snap = AgentRunSnapshot::from_agent_with_legacy_context(self, &legacy_runtime);
        snap.current_turn_id = Some(turn_id);
        snap.turn_steer_incarnation = Some(active_turn_lease.incarnation());
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
    use crate::agent::Agent;
    use crate::skills::hooks::{
        HookAction, HookEvent, HookRegistry, HookResult, HookRule, HooksDefinition,
    };
    use crate::testing::MockLlmClient;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct QueuedGuardDrop(Arc<AtomicUsize>);

    impl Drop for QueuedGuardDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct SecondRunBarrierProjection {
        calls: AtomicUsize,
        contexts: std::sync::Mutex<Vec<crate::compression::ProjectionContext>>,
        second_started: tokio::sync::Notify,
        release_second: tokio::sync::Notify,
    }

    impl SecondRunBarrierProjection {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                contexts: std::sync::Mutex::new(Vec::new()),
                second_started: tokio::sync::Notify::new(),
                release_second: tokio::sync::Notify::new(),
            }
        }
    }

    impl crate::compression::PreModelContextProjector for SecondRunBarrierProjection {
        fn project<'a>(
            &'a self,
            context: &'a crate::compression::ProjectionContext,
        ) -> futures::future::BoxFuture<'a, Result<Vec<crate::compression::ContextProjection>>>
        {
            Box::pin(async move {
                self.contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(context.clone());
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 1 {
                    self.second_started.notify_one();
                    self.release_second.notified().await;
                }
                Ok(Vec::new())
            })
        }
    }

    fn legacy_guard_context(
        label: &str,
        drops: Arc<AtomicUsize>,
    ) -> echo_core::tools::ExternalRunContext {
        echo_core::tools::ExternalRunContext {
            conversation_id: Some(format!("conversation-{label}")),
            run_id: Some(label.to_string()),
            turn_id: Some(format!("turn-{label}")),
            execution_id: Some(format!("execution-{label}")),
            isolation_id: Some(format!("isolation-{label}")),
            message_id: Some(format!("message-{label}")),
            cancel: None,
            trace_sink: None,
            delegation_policy: None,
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                QueuedGuardDrop(drops),
            )],
            subagent_lineage: None,
            uplink: None,
        }
    }

    #[tokio::test]
    async fn queued_nonstream_invocations_keep_captured_resource_guards_isolated() -> Result<()> {
        let llm = Arc::new(MockLlmClient::new().with_responses(["first", "second"]));
        let trace_store = Arc::new(crate::trace::InMemoryRunStore::new());
        let mut agent = crate::agent::ReactAgentBuilder::new()
            .llm_client(llm)
            .system_prompt("test")
            .build()?;
        agent.set_run_store(trace_store.clone());
        let second_barrier = Arc::new(SecondRunBarrierProjection::new());
        agent.set_pre_model_context_projector(Some(second_barrier.clone()));
        let agent = Arc::new(agent);
        let queue_barrier = Arc::clone(&agent.execution_mutex).lock_owned().await;
        let first_drops = Arc::new(AtomicUsize::new(0));
        let second_drops = Arc::new(AtomicUsize::new(0));

        let first_context = legacy_guard_context("queued-first", Arc::clone(&first_drops));
        agent.set_external_context(&first_context);
        drop(first_context);
        let mut first = Box::pin({
            let agent = Arc::clone(&agent);
            async move { agent.run_react_loop("first").await }
        });
        std::future::poll_fn(|context| match first.as_mut().poll(context) {
            std::task::Poll::Pending => std::task::Poll::Ready(Ok(())),
            std::task::Poll::Ready(result) => std::task::Poll::Ready(Err(ReactError::Other(
                format!("first run bypassed queue barrier: {result:?}"),
            ))),
        })
        .await?;
        agent.clear_external_context();
        assert_eq!(first_drops.load(Ordering::SeqCst), 0);

        let second_context = legacy_guard_context("queued-second", Arc::clone(&second_drops));
        agent.set_external_context(&second_context);
        drop(second_context);
        let mut second = Box::pin({
            let agent = Arc::clone(&agent);
            async move { agent.run_react_loop("second").await }
        });
        std::future::poll_fn(|context| match second.as_mut().poll(context) {
            std::task::Poll::Pending => std::task::Poll::Ready(Ok(())),
            std::task::Poll::Ready(result) => std::task::Poll::Ready(Err(ReactError::Other(
                format!("second run bypassed queue barrier: {result:?}"),
            ))),
        })
        .await?;
        agent.clear_external_context();
        assert_eq!(second_drops.load(Ordering::SeqCst), 0);
        drop(queue_barrier);
        let first = tokio::spawn(first);
        let second = tokio::spawn(second);

        first
            .await
            .map_err(|error| ReactError::Other(format!("first queued run failed: {error}")))??;
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            second_barrier.second_started.notified(),
        )
        .await
        .map_err(|_| ReactError::Other("second queued run did not reach barrier".to_string()))?;
        assert_eq!(first_drops.load(Ordering::SeqCst), 1);
        assert_eq!(second_drops.load(Ordering::SeqCst), 0);
        second_barrier.release_second.notify_one();
        second
            .await
            .map_err(|error| ReactError::Other(format!("second queued run failed: {error}")))??;
        assert_eq!(second_drops.load(Ordering::SeqCst), 1);
        let projection_contexts = second_barrier
            .contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        assert_eq!(projection_contexts.len(), 2);
        for (context, label) in projection_contexts
            .iter()
            .zip(["queued-first", "queued-second"])
        {
            assert_eq!(
                context.conversation_id.as_deref(),
                Some(format!("conversation-{label}").as_str())
            );
            assert_eq!(context.run_id.as_deref(), Some(label));
            assert_eq!(
                context.turn_id.as_deref(),
                Some(format!("turn-{label}").as_str())
            );
        }
        let traces = crate::trace::RunStore::list_all(trace_store.as_ref(), 10).await?;
        for (input, label) in [("first", "queued-first"), ("second", "queued-second")] {
            let trace = traces
                .iter()
                .find(|trace| trace.input_preview == input)
                .ok_or_else(|| ReactError::Other(format!("queued trace missing: {input}")))?;
            assert_eq!(trace.parent_run_id.as_deref(), Some(label));
            assert_eq!(trace.session_id, format!("conversation-{label}"));
            assert_eq!(
                trace.turn_id.as_deref(),
                Some(format!("turn-{label}").as_str())
            );
            assert_eq!(
                trace.execution_id.as_deref(),
                Some(format!("execution-{label}").as_str())
            );
        }
        Ok(())
    }

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
