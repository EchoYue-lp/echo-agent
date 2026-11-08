//! 非流式API的结果投影；执行、取消、租约与结算复用原Managed流路径。

use super::super::ReactAgent;
use super::types::StreamMode;
use crate::agent::AgentEvent;
use crate::error::{AgentError, ReactError, Result};
#[cfg(test)]
use crate::llm::types::Message;
use futures::StreamExt;

impl ReactAgent {
    #[cfg(test)]
    pub(crate) async fn run_react_loop(&self, message: &str) -> Result<String> {
        self.run_react_loop_mode(message, StreamMode::Chat).await
    }

    pub(crate) async fn run_react_loop_mode(
        &self,
        message: &str,
        mode: StreamMode,
    ) -> Result<String> {
        // 同一流入口在排队前捕获当前 invocation snapshot；这里不再持有第二份 execution 锁/owner。
        let mut stream = self.run_stream_entry(message, mode, None).await?;
        let mut answer = None;
        let mut first_error = None;
        let mut settled = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(AgentEvent::FinalAnswer(value)) => {
                    answer.get_or_insert(value);
                }
                Ok(AgentEvent::ExecutionStatus(status)) => {
                    settled = status.status == echo_core::agent::ExecutionStatusKind::Settled;
                }
                Ok(AgentEvent::Cancelled) if first_error.is_none() => {
                    first_error = Some(AgentError::Cancelled("agent run".to_string()).into());
                }
                Ok(AgentEvent::Error {
                    message, failure, ..
                }) if first_error.is_none() => {
                    first_error = Some(
                        if failure.terminal_kind == echo_core::error::AgentTerminalKind::Cancelled {
                            AgentError::Cancelled(message).into()
                        } else {
                            ReactError::Other(message)
                        },
                    );
                }
                Err(error) if first_error.is_none() => {
                    first_error = Some(error);
                }
                _ => {}
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if settled {
            return answer.ok_or_else(|| {
                AgentError::NoResponse {
                    model: self.config.model_name.clone(),
                    agent: self.config.agent_name.clone(),
                }
                .into()
            });
        }
        Err(ReactError::Other(
            "Agent event stream ended without runtime settlement".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::hooks::{
        HookAction, HookEvent, HookRegistry, HookResult, HookRule, HooksDefinition,
    };
    use crate::testing::MockLlmClient;
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct QueuedFiniteSink;
    #[async_trait::async_trait]
    impl crate::runtime::EventSink for QueuedFiniteSink {
        async fn on_event(
            &self,
            _event: echo_core::agent::EventEnvelope,
        ) -> Result<crate::runtime::SinkControl> {
            Ok(crate::runtime::SinkControl::Continue)
        }
    }

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

    fn invocation_guard_context(
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
    async fn queued_finite_invocations_keep_captured_resource_guards_isolated() -> Result<()> {
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

        let first_context = invocation_guard_context("queued-first", Arc::clone(&first_drops));
        let first_identity =
            echo_core::agent::EventIdentity::from_runtime_context(Some(&first_context))?;
        let mut first = Box::pin({
            let agent = Arc::clone(&agent);
            async move {
                crate::runtime::AgentTurnDriver
                    .drive(
                        agent.as_ref(),
                        crate::runtime::TurnRequest::from_message(
                            first_identity,
                            Message::user("first".into()),
                        )
                        .mode(crate::runtime::TurnMode::Chat)
                        .invocation(
                            echo_core::agent::AgentInvocationContext {
                                runtime: Some(first_context),
                                ..Default::default()
                            },
                        ),
                        &QueuedFiniteSink,
                    )
                    .await
            }
        });
        std::future::poll_fn(|context| match first.as_mut().poll(context) {
            std::task::Poll::Pending => std::task::Poll::Ready(Ok(())),
            std::task::Poll::Ready(result) => std::task::Poll::Ready(Err(ReactError::Other(
                format!("first run bypassed queue barrier: {result:?}"),
            ))),
        })
        .await?;
        assert_eq!(first_drops.load(Ordering::SeqCst), 0);

        let second_context = invocation_guard_context("queued-second", Arc::clone(&second_drops));
        let second_identity =
            echo_core::agent::EventIdentity::from_runtime_context(Some(&second_context))?;
        let mut second = Box::pin({
            let agent = Arc::clone(&agent);
            async move {
                crate::runtime::AgentTurnDriver
                    .drive(
                        agent.as_ref(),
                        crate::runtime::TurnRequest::from_message(
                            second_identity,
                            Message::user("second".into()),
                        )
                        .mode(crate::runtime::TurnMode::Chat)
                        .invocation(
                            echo_core::agent::AgentInvocationContext {
                                runtime: Some(second_context),
                                ..Default::default()
                            },
                        ),
                        &QueuedFiniteSink,
                    )
                    .await
            }
        });
        std::future::poll_fn(|context| match second.as_mut().poll(context) {
            std::task::Poll::Pending => std::task::Poll::Ready(Ok(())),
            std::task::Poll::Ready(result) => std::task::Poll::Ready(Err(ReactError::Other(
                format!("second run bypassed queue barrier: {result:?}"),
            ))),
        })
        .await?;
        assert_eq!(second_drops.load(Ordering::SeqCst), 0);
        drop(queue_barrier);
        let first = tokio::spawn(first);
        let second = tokio::spawn(second);

        let first_receipt = first
            .await
            .map_err(|error| ReactError::Other(format!("first queued run failed: {error}")))??;
        assert_eq!(
            first_receipt.outcome,
            crate::runtime::TurnOutcome::Completed
        );
        assert_eq!(first_receipt.final_answer.as_deref(), Some("first"));
        assert_eq!(
            first_receipt.delivery,
            crate::runtime::TurnDeliveryOutcome::Delivered
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            second_barrier.second_started.notified(),
        )
        .await
        .map_err(|_| ReactError::Other("second queued run did not reach barrier".to_string()))?;
        assert_eq!(first_drops.load(Ordering::SeqCst), 1);
        assert_eq!(second_drops.load(Ordering::SeqCst), 0);
        second_barrier.release_second.notify_one();
        let second_receipt = second
            .await
            .map_err(|error| ReactError::Other(format!("second queued run failed: {error}")))??;
        assert_eq!(
            second_receipt.outcome,
            crate::runtime::TurnOutcome::Completed
        );
        assert_eq!(second_receipt.final_answer.as_deref(), Some("second"));
        assert_eq!(
            second_receipt.delivery,
            crate::runtime::TurnDeliveryOutcome::Delivered
        );
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
        let agent = Arc::new(agent);
        let running = {
            let agent = agent.clone();
            tokio::spawn(async move { agent.run_react_loop("blocked request").await })
        };

        tokio::time::timeout(std::time::Duration::from_secs(1), hook_entered.notified())
            .await
            .map_err(|_| ReactError::Other("UserPromptSubmit hook did not start".to_string()))?;
        // String 入口不注入 runtime ID；直接向原 active mailbox 入队，
        // receipt 返回的 turn ID 才是本次真实 owner，不把 current_run_id 当 turn_id。
        let mut receipt = agent
            .steer_input_tracked(None, Message::user("must not be consumed".to_string()))
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(!receipt.turn_id().is_empty());
        assert_eq!(receipt.state(), crate::agent::AgentSteerState::Accepted);
        release_hook.notify_one();

        let response = running
            .await
            .map_err(|error| ReactError::Other(format!("nonstream task failed: {error}")))?;
        let error = response
            .err()
            .ok_or_else(|| ReactError::Other("blocked hook unexpectedly succeeded".into()))?;
        assert!(error.to_string().contains("blocked by test hook"));
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
