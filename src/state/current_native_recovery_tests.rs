//! 当前 typed runtime 恢复控制；不构造旧控制工具记录、不调用迁移适配器。
//!
//! 旧模块中仍适用于当前版本的 CAS、lineage、deadline 和 unknown owner 断言迁到这里。
//! 仅旧 think/final 来源推断/归一化成功的义务随用户最新裁决退役，不能写成已匹配或跳过。

use super::*;
use crate::agent::{Agent, AgentInvocationContext, CancellationToken, ReactAgentBuilder};
use crate::error::{ReactError, Result};
use crate::llm::types::{FunctionCall, Message, ToolCall};
use crate::memory::PersistenceCallContext;
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

const SCOPE: &str = "current-native-scope";
const RUNTIME: &str = "current-native-runtime";

// 只覆盖当前schema3合法历史；不恢复旧control origin归一或旧断点成功接纳。
#[tokio::test]
async fn completed_tool_ids_can_be_reused_in_current_history_and_original_model_request()
-> Result<()> {
    use crate::tools::Tool;
    use futures::StreamExt;

    // 只在本测试保留同一MockTool的计数观察，不给公开MockTool添加Clone能力。
    struct ObservedTool(Arc<crate::testing::MockTool>);
    impl Tool for ObservedTool {
        fn name(&self) -> &str {
            self.0.name()
        }
        fn description(&self) -> &str {
            self.0.description()
        }
        fn parameters(&self) -> serde_json::Value {
            self.0.parameters()
        }
        fn execute<'a>(
            &'a self,
            parameters: crate::tools::ToolParameters,
        ) -> BoxFuture<'a, Result<crate::tools::ToolResult>> {
            self.0.execute(parameters)
        }
    }

    for name in ["read_file", "custom"] {
        let mut history = Vec::new();
        for result in ["first-result", "second-result"] {
            history.push(Message::assistant_with_tools(vec![ToolCall {
                provider_source: None,
                id: "completed-reusable-id".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                },
            }]));
            history.push(Message::tool_result(
                "completed-reusable-id".to_string(),
                name.to_string(),
                result.to_string(),
            ));
        }
        let mut checkpoint = AgentCheckpoint::new("current-reused-id-runtime");
        checkpoint.messages_json = AgentCheckpoint::serialize_payload(history.clone(), None)?;
        let original_payload = checkpoint.messages_json.clone();
        let restored = checkpoint.restore_runtime_payload()?;
        assert_eq!(checkpoint.messages_json, original_payload);
        assert_eq!(
            serde_json::to_value(&restored.messages)?,
            serde_json::to_value(&history)?
        );
        assert_eq!(
            checkpoint.completed_tool_call_ids()?,
            vec![
                "completed-reusable-id".to_string(),
                "completed-reusable-id".to_string()
            ]
        );

        let model = Arc::new(crate::testing::MockLlmClient::new().with_response("当前历史已读取"));
        let tool = Arc::new(crate::testing::MockTool::new(name).with_response("must not replay"));
        let agent = ReactAgentBuilder::new()
            .llm_client(model.clone())
            .auto_project_rules(false)
            .tool(Box::new(ObservedTool(tool.clone())))
            .build()?;
        let stream = agent
            .chat_stream_message_with_invocation_context(
                Message::user("仅根据已经完成的两轮原结果回答".to_string()),
                CancellationToken::new(),
                AgentInvocationContext {
                    history: Some(restored.messages),
                    ..Default::default()
                },
            )
            .await?;
        let events = tokio::time::timeout(Duration::from_secs(5), stream.collect::<Vec<_>>())
            .await
            .map_err(|error| fixture_error(error.to_string()))?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(model.call_count(), 1);
        assert_eq!(
            tool.call_count(),
            0,
            "已完成历史不重放工具，也不补造工具结果"
        );
        let requests = model.all_calls();
        let request = requests
            .first()
            .ok_or_else(|| fixture_error("actual model request missing"))?;
        let replayed = request
            .iter()
            .filter(|message| {
                message
                    .tool_calls
                    .iter()
                    .flatten()
                    .any(|call| call.id == "completed-reusable-id")
                    || message.tool_call_id.as_deref() == Some("completed-reusable-id")
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            serde_json::to_value(replayed)?,
            serde_json::to_value(history)?
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    crate::agent::AgentEvent::FinalAnswer(answer) => Some(answer.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec!["当前历史已读取"]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
                    crate::agent::AgentEvent::ExecutionStatus(status)
                        if status.status == echo_core::agent::ExecutionStatusKind::Settled
                ))
                .count(),
            1
        );
        agent.close().await?;
    }
    Ok(())
}

fn fixture_error(message: impl Into<String>) -> ReactError {
    ReactError::Other(message.into())
}

fn invocation() -> AgentInvocationContext {
    AgentInvocationContext {
        runtime_state_id: Some(RUNTIME.to_string()),
        transcript_generation_id: Some(RUNTIME.to_string()),
        runtime: Some(echo_core::tools::ExternalRunContext {
            conversation_id: Some(SCOPE.to_string()),
            run_id: Some("current-run".to_string()),
            turn_id: Some("current-turn".to_string()),
            execution_id: Some("current-execution".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn current_checkpoint(messages: Vec<Message>) -> Result<AgentCheckpoint> {
    let rows = crate::memory::project_messages(SCOPE, &messages)?;
    let projected = rows
        .iter()
        .enumerate()
        .map(|(position, row)| {
            Ok(TranscriptProjectionMessage {
                ordinal: u64::try_from(position)
                    .map_err(|error| fixture_error(error.to_string()))?,
                digest: crate::memory::transcript_projection_message_digest(row)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let cursor = TranscriptProjectionCheckpoint {
        generation_id: RUNTIME.to_string(),
        next_ordinal: u64::try_from(projected.len())
            .map_err(|error| fixture_error(error.to_string()))?,
        projected,
        context_only: Some(Vec::new()),
    };
    let mut checkpoint = AgentCheckpoint::new(RUNTIME);
    checkpoint.messages_json = AgentCheckpoint::serialize_payload(messages, Some(cursor))?;
    checkpoint.restore_runtime_payload()?;
    Ok(checkpoint)
}

async fn seeded_current_owner(
    root: &Path,
) -> Result<(FileRuntimeStateStore, ManagedRuntimeStateSnapshot)> {
    let store = FileRuntimeStateStore::new(root)?;
    let checkpoint = current_checkpoint(vec![Message::user("当前耐久事实 🧭".to_string())])?;
    let receipt = store
        .compare_and_save_checkpoint(RuntimeCheckpointCasRequest {
            scope_id: SCOPE.to_string(),
            runtime_state_id: RUNTIME.to_string(),
            conversation_epoch: Some(1),
            expected_scope_revision: 0,
            expected_state_version: RuntimeStateExpectedVersion::Absent,
            checkpoint,
            managed_import: None,
        })
        .await?;
    assert_eq!(receipt.status, RuntimeCheckpointCasStatus::Applied);
    let state = store
        .load_runtime_state(SCOPE, RUNTIME)
        .await?
        .ok_or_else(|| fixture_error("current managed owner missing"))?;
    Ok((store, state))
}

fn owner_path(root: &Path) -> Result<PathBuf> {
    let safe = echo_core::utils::fs::encode_path_segment_identity(RUNTIME)?;
    Ok(root
        .join("runtime_state/_runtime_owners")
        .join(format!("{safe}.json")))
}

#[test]
fn current_unpaired_tool_cannot_bypass_strict_restore_or_become_final() -> Result<()> {
    let call = ToolCall {
        provider_source: None,
        id: "pending-current-effect".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "current_effect".to_string(),
            arguments: "{}".to_string(),
        },
    };
    let mut checkpoint = AgentCheckpoint::new(RUNTIME);
    checkpoint.messages_json = AgentCheckpoint::serialize_payload(
        vec![
            Message::user("当前请求".to_string()),
            Message::assistant_with_tools(vec![call]),
        ],
        None,
    )?;
    let original = checkpoint.messages_json.clone();
    assert!(checkpoint.restore_managed_runtime_payload().is_err());
    assert!(checkpoint.restore_runtime_payload().is_err());
    assert_eq!(
        checkpoint.messages_json, original,
        "严格恢复不补结果、不改历史、更不执行副作用"
    );
    Ok(())
}

#[tokio::test]
async fn current_context_view_derivation_keeps_cursor_lineage_and_original_source() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (_store, state) = seeded_current_owner(root.path()).await?;
    let checkpoint = state
        .checkpoint
        .as_ref()
        .ok_or_else(|| fixture_error("checkpoint missing"))?;
    let original = serde_json::to_vec(checkpoint)?;
    let original_cursor = checkpoint
        .restore_runtime_payload()?
        .transcript_projection
        .ok_or_else(|| fixture_error("cursor missing"))?;
    let mut messages = checkpoint.restore_messages()?;
    messages.push(Message::user(
        "本轮派生上下文，不伪造transcript事实".to_string(),
    ));
    let derived = crate::agent::snapshot::derive_context_view_checkpoint(&state, messages.clone())?;
    assert_eq!(serde_json::to_vec(checkpoint)?, original);
    let payload = derived.restore_runtime_payload()?;
    let cursor = payload
        .transcript_projection
        .ok_or_else(|| fixture_error("derived cursor missing"))?;
    assert_eq!(cursor.generation_id, original_cursor.generation_id);
    assert_eq!(cursor.next_ordinal, original_cursor.next_ordinal);
    assert_eq!(cursor.projected, original_cursor.projected);
    assert!(
        cursor
            .context_only
            .as_ref()
            .is_some_and(|bindings| bindings.len() == 1)
    );
    assert_eq!(
        serde_json::to_value(payload.messages)?,
        serde_json::to_value(messages)?
    );
    Ok(())
}

#[tokio::test]
async fn current_managed_cursor_corruption_is_rejected_without_owner_rewrite() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (store, state) = seeded_current_owner(root.path()).await?;
    let path = owner_path(root.path())?;
    let mut owner: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let checkpoint = owner
        .get_mut("checkpoint")
        .ok_or_else(|| fixture_error("checkpoint missing"))?;
    let encoded = checkpoint
        .get("messages_json")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| fixture_error("payload missing"))?;
    let mut payload: serde_json::Value = serde_json::from_str(encoded)?;
    let digest = payload
        .get_mut("transcript_projection")
        .and_then(|cursor| cursor.get_mut("projected"))
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|facts| facts.first_mut())
        .and_then(|fact| fact.get_mut("digest"))
        .ok_or_else(|| fixture_error("digest missing"))?;
    *digest = serde_json::json!("0".repeat(64));
    let encoded = checkpoint
        .get_mut("messages_json")
        .ok_or_else(|| fixture_error("payload missing"))?;
    *encoded = serde_json::json!(payload.to_string());
    let corrupted = serde_json::to_vec(&owner)?;
    std::fs::write(&path, &corrupted)?;
    assert!(store.load_runtime_state(SCOPE, RUNTIME).await.is_err());
    assert_eq!(std::fs::read(&path)?, corrupted);
    assert_eq!(state.scope.conversation_epoch, Some(1));
    Ok(())
}

#[tokio::test]
async fn current_cas_replay_and_stale_hydration_preserve_revision_and_epoch() -> Result<()> {
    for concurrent_update in [false, true] {
        let root = tempfile::tempdir()?;
        let (store, current) = seeded_current_owner(root.path()).await?;
        let path = owner_path(root.path())?;
        let agent = ReactAgentBuilder::new()
            .conversation_id(SCOPE)
            .state_store(Arc::new(store.clone()))
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        agent
            .resume_from_state_store_scope(SCOPE, RUNTIME, None)
            .await?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &invocation(),
        );
        if concurrent_update {
            let RuntimeStateVersion::Managed { revision } = current.version else {
                return Err(fixture_error("current revision missing"));
            };
            let mut messages = current
                .checkpoint
                .as_ref()
                .ok_or_else(|| fixture_error("checkpoint missing"))?
                .restore_messages()?;
            messages.push(Message::user(
                "并发后继事实，不能被旧hydrate覆盖".to_string(),
            ));
            let checkpoint =
                crate::agent::snapshot::derive_context_view_checkpoint(&current, messages)?;
            let request = RuntimeCheckpointCasRequest {
                scope_id: SCOPE.to_string(),
                runtime_state_id: RUNTIME.to_string(),
                conversation_epoch: Some(1),
                expected_scope_revision: current.scope.revision,
                expected_state_version: RuntimeStateExpectedVersion::Managed { revision },
                checkpoint,
                managed_import: None,
            };
            let next = store.compare_and_save_checkpoint(request.clone()).await?;
            assert_eq!(next.status, RuntimeCheckpointCasStatus::Applied);
            let replay = store.compare_and_save_checkpoint(request).await?;
            assert_eq!(replay.status, RuntimeCheckpointCasStatus::AlreadyCurrent);
            assert_eq!(replay.version, next.version);
        }
        let before = std::fs::read(&path)?;
        let result = snapshot
            .save_runtime_checkpoint(agent.context(), None)
            .await;
        if concurrent_update {
            assert!(result.is_err());
            assert_eq!(std::fs::read(&path)?, before);
        } else {
            result?;
            let after = store
                .load_runtime_state(SCOPE, RUNTIME)
                .await?
                .ok_or_else(|| fixture_error("owner missing"))?;
            assert_eq!(after.scope.conversation_epoch, Some(1));
            let payload = after
                .checkpoint
                .ok_or_else(|| fixture_error("checkpoint missing"))?
                .restore_runtime_payload()?;
            assert!(
                payload
                    .transcript_projection
                    .as_ref()
                    .is_some_and(|cursor| cursor.context_only.is_some())
            );
        }
    }
    Ok(())
}

/// 当前测试Store保留已接纳普通Load/CAS任务，在可控闸后调用真实File CAS；丢弃调用future
/// 不会取消该任务，模拟现keyed blocking Store已准入后的物理所有权。
struct CurrentStateGateStore {
    inner: FileRuntimeStateStore,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
    completed: std::sync::Arc<tokio::sync::Notify>,
    normal_gate: Option<(&'static str, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
}
impl CurrentStateGateStore {
    fn gate(&self, phase: &str) -> bool {
        self.normal_gate.as_ref().is_some_and(|(selected, armed)| {
            *selected == phase && armed.swap(false, std::sync::atomic::Ordering::SeqCst)
        })
    }
    fn await_gate<T: Send + 'static>(
        &self,
        operation: impl std::future::Future<Output = crate::error::Result<T>> + Send + 'static,
    ) -> BoxFuture<'static, crate::error::Result<T>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let completed = self.completed.clone();
        Box::pin(async move {
            let task = tokio::spawn(async move {
                release.notified().await;
                let result = operation.await;
                completed.notify_one();
                result
            });
            entered.notify_one();
            task.await
                .map_err(|error| fixture_error(error.to_string()))?
        })
    }
}
impl RuntimeStateStore for CurrentStateGateStore {
    fn runtime_state_capability(&self) -> crate::state::RuntimeStateCapability {
        self.inner.runtime_state_capability()
    }
    fn persistence_call_capability(&self) -> crate::memory::PersistenceCallCapability {
        self.inner.persistence_call_capability()
    }
    fn load_runtime_state<'a>(
        &'a self,
        scope: &'a str,
        runtime: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>> {
        self.inner.load_runtime_state(scope, runtime)
    }
    fn load_runtime_state_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        scope: &'a str,
        runtime: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>> {
        if self.gate("runtime_load") {
            let inner = self.inner.clone();
            let scope = scope.to_string();
            let runtime = runtime.to_string();
            self.await_gate(async move {
                inner
                    .load_runtime_state_with_context(context, &scope, &runtime)
                    .await
            })
        } else {
            self.inner
                .load_runtime_state_with_context(context, scope, runtime)
        }
    }
    fn load_scope_authority<'a>(
        &'a self,
        scope: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<crate::state::RuntimeScopeAuthority>>> {
        self.inner.load_scope_authority(scope)
    }
    fn compare_and_save_checkpoint<'a>(
        &'a self,
        request: crate::state::RuntimeCheckpointCasRequest,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>> {
        self.inner.compare_and_save_checkpoint(request)
    }
    fn load_scope_authority_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        scope: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<crate::state::RuntimeScopeAuthority>>> {
        if self.gate("scope_load") {
            let inner = self.inner.clone();
            let scope = scope.to_string();
            self.await_gate(async move {
                inner
                    .load_scope_authority_with_context(context, &scope)
                    .await
            })
        } else {
            self.inner.load_scope_authority_with_context(context, scope)
        }
    }
    fn compare_and_save_checkpoint_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        request: crate::state::RuntimeCheckpointCasRequest,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>> {
        if self.gate("normal_cas") {
            let inner = self.inner.clone();
            self.await_gate(async move {
                inner
                    .compare_and_save_checkpoint_with_context(context, request)
                    .await
            })
        } else {
            self.inner
                .compare_and_save_checkpoint_with_context(context, request)
        }
    }
    fn acknowledge_transcript_projection<'a>(
        &'a self,
        request: crate::state::RuntimeTranscriptAckRequest,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>> {
        self.inner.acknowledge_transcript_projection(request)
    }
    fn acknowledge_transcript_projection_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        request: crate::state::RuntimeTranscriptAckRequest,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>> {
        self.inner
            .acknowledge_transcript_projection_with_context(context, request)
    }
    fn get_checkpoint<'a>(
        &'a self,
        id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
        self.inner.get_checkpoint(id)
    }
    fn save_checkpoint<'a>(
        &'a self,
        cp: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        self.inner.save_checkpoint(cp)
    }
    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope: &'a str,
        cp: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        self.inner.save_checkpoint_for_scope(scope, cp)
    }
    fn runtime_state_ids<'a>(
        &'a self,
        scope: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Vec<String>>> {
        self.inner.runtime_state_ids(scope)
    }
    fn clear_runtime_state<'a>(
        &'a self,
        scope: &'a str,
        id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeStateClearReceipt>> {
        self.inner.clear_runtime_state(scope, id)
    }
    fn clear_runtime_state_scope<'a>(
        &'a self,
        scope: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeStateScopeClearReceipt>> {
        self.inner.clear_runtime_state_scope(scope)
    }
    fn clear_conversation<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::error::Result<()>> {
        self.inner.clear_conversation(id)
    }
}

async fn at_barrier(barrier: &tokio::sync::Notify) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), barrier.notified())
        .await
        .map_err(|_| fixture_error("current Store phase did not reach barrier"))
}

#[tokio::test]
async fn runtime_only_deadline_waits_for_actual_scope_load_runtime_load_and_cas_settlement()
-> crate::error::Result<()> {
    use crate::agent::ReactAgentBuilder;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for phase in ["scope_load", "runtime_load", "normal_cas"] {
        let root = tempfile::tempdir()?;
        let (inner, _) = seeded_current_owner(root.path()).await?;
        let path = owner_path(root.path())?;
        let armed = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let store = Arc::new(CurrentStateGateStore {
            inner: inner.clone(),
            entered: entered.clone(),
            release: release.clone(),
            completed: Arc::new(tokio::sync::Notify::new()),
            normal_gate: Some((phase, armed.clone())),
        });
        let agent = ReactAgentBuilder::new()
            .conversation_id(SCOPE)
            .state_store(store)
            .persistence_settlement_timeout(Duration::from_millis(250))
            .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
            .build()?;
        agent
            .resume_from_state_store_scope(SCOPE, RUNTIME, None)
            .await?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent_with_invocation(
            &agent,
            &invocation(),
        );
        let context = agent.context().clone();
        let original = std::fs::read(&path)?;
        armed.store(true, Ordering::SeqCst);
        let mut saving =
            tokio::spawn(async move { snapshot.save_runtime_checkpoint(&context, None).await });
        at_barrier(&entered).await?;
        // 观察同一原future跨过其原deadline仍等实际Store结算，不把deadline当物理receipt。
        let prematurely_finished =
            tokio::time::timeout(Duration::from_millis(500), &mut saving).await;
        release.notify_one();
        let completed = match prematurely_finished {
            Ok(result) => {
                let _early_observation =
                    result.map_err(|error| fixture_error(error.to_string()))?;
                return Err(fixture_error(format!(
                    "{phase} released caller before physical settlement"
                )));
            }
            Err(_) => saving
                .await
                .map_err(|error| fixture_error(error.to_string()))?,
        };
        assert!(
            completed.is_err(),
            "原deadline未延长，真实Store迟到后仍拒新写"
        );
        assert_eq!(std::fs::read(path)?, original);
    }
    Ok(())
}

struct UnknownCurrentStore {
    inner: FileRuntimeStateStore,
    loads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    normal_unknown: bool,
}
impl RuntimeStateStore for UnknownCurrentStore {
    fn runtime_state_capability(&self) -> crate::state::RuntimeStateCapability {
        self.inner.runtime_state_capability()
    }
    fn persistence_call_capability(&self) -> crate::memory::PersistenceCallCapability {
        self.inner.persistence_call_capability()
    }
    fn load_runtime_state_with_context<'a>(
        &'a self,
        ctx: PersistenceCallContext,
        scope: &'a str,
        runtime: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>> {
        Box::pin(async move {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.normal_unknown {
                return Err(crate::error::ReactError::ExecutionNotSettled {
                    run_id: "current-store-run".into(),
                    execution_id: Some("current-store-execution".into()),
                });
            }
            self.inner
                .load_runtime_state_with_context(ctx, scope, runtime)
                .await
        })
    }
    fn load_scope_authority<'a>(
        &'a self,
        scope: &'a str,
    ) -> BoxFuture<'a, Result<Option<RuntimeScopeAuthority>>> {
        self.inner.load_scope_authority(scope)
    }
    fn load_scope_authority_with_context<'a>(
        &'a self,
        context: PersistenceCallContext,
        scope: &'a str,
    ) -> BoxFuture<'a, Result<Option<RuntimeScopeAuthority>>> {
        self.inner.load_scope_authority_with_context(context, scope)
    }
    fn get_checkpoint<'a>(
        &'a self,
        id: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Option<AgentCheckpoint>>> {
        self.inner.get_checkpoint(id)
    }
    fn save_checkpoint<'a>(
        &'a self,
        cp: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        self.inner.save_checkpoint(cp)
    }
    fn save_checkpoint_for_scope<'a>(
        &'a self,
        scope: &'a str,
        cp: &'a AgentCheckpoint,
    ) -> BoxFuture<'a, crate::error::Result<()>> {
        self.inner.save_checkpoint_for_scope(scope, cp)
    }
    fn runtime_state_ids<'a>(
        &'a self,
        scope: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<Vec<String>>> {
        self.inner.runtime_state_ids(scope)
    }
    fn clear_runtime_state<'a>(
        &'a self,
        scope: &'a str,
        runtime: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeStateClearReceipt>> {
        self.inner.clear_runtime_state(scope, runtime)
    }
    fn clear_runtime_state_scope<'a>(
        &'a self,
        scope: &'a str,
    ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeStateScopeClearReceipt>> {
        self.inner.clear_runtime_state_scope(scope)
    }
    fn clear_conversation<'a>(&'a self, id: &'a str) -> BoxFuture<'a, crate::error::Result<()>> {
        self.inner.clear_conversation(id)
    }
}

#[tokio::test]
async fn current_normal_load_unknown_keeps_exact_owner_and_resources_without_fallback() -> Result<()>
{
    struct ResourceDrop(Arc<AtomicUsize>);
    impl Drop for ResourceDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let root = tempfile::tempdir()?;
    let (inner, _) = seeded_current_owner(root.path()).await?;
    let path = owner_path(root.path())?;
    let original = std::fs::read(&path)?;
    let loads = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(crate::testing::MockLlmClient::new().with_response("must not call model"));
    let agent = ReactAgentBuilder::new()
        .conversation_id(SCOPE)
        .state_store(Arc::new(UnknownCurrentStore {
            inner,
            loads: loads.clone(),
            normal_unknown: true,
        }))
        .llm_client(model.clone())
        .build()?;
    let mut scoped = invocation();
    scoped
        .resource_guards
        .push(echo_core::tools::InvocationResourceGuard::new(
            ResourceDrop(drops.clone()),
        ));
    let result = agent
        .execute_stream_with_invocation_context("current input", CancellationToken::new(), scoped)
        .await;
    let error = result
        .err()
        .ok_or_else(|| fixture_error("normal unknown unexpectedly admitted model"))?;
    assert!(
        matches!(error,ReactError::ExecutionNotSettled {run_id,execution_id:Some(execution)} if run_id=="current-store-run"&&execution=="current-store-execution")
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(model.call_count(), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(
        matches!(agent.close().await,Err(ReactError::ExecutionNotSettled {run_id,execution_id:Some(execution)}) if run_id=="current-store-run"&&execution=="current-store-execution")
    );
    assert!(matches!(
        agent.execute("next input").await,
        Err(ReactError::ExecutionNotSettled { .. })
    ));
    assert_eq!(std::fs::read(&path)?, original);
    Ok(())
}

#[tokio::test]
async fn new_custom_tools_named_think_and_final_answer_execute_as_ordinary_tools()
-> crate::error::Result<()> {
    use crate::agent::{Agent, AgentEvent, ReactAgentBuilder};
    use crate::testing::{MockLlmClient, MockTool};
    use futures::StreamExt;
    use std::sync::Arc;
    use std::time::Duration;

    // 新执行直接走既有ReAct/pipeline，从不调用旧历史helper按名称拦截。
    for name in ["think", "final_answer"] {
        let llm = Arc::new(
            MockLlmClient::new()
                .then_tool_call("custom-call", name, r#"{"user_argument":"ordinary"}"#)
                .with_response("普通工具结束后才形成最终正文"),
        );
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .system_prompt("用户自定义工具没有结束执行的特权")
            .tool(Box::new(MockTool::new(name).with_response("custom result")))
            .build()?;
        let stream = agent.execute_stream("运行用户自定义工具").await?;
        let raw: Vec<_> = tokio::time::timeout(Duration::from_secs(5), stream.collect())
            .await
            .map_err(|error| fixture_error(format!("自定义工具正控未结算：{error}")))?;
        let events = raw.into_iter().collect::<crate::error::Result<Vec<_>>>()?;
        assert_eq!(llm.call_count(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
                    AgentEvent::ToolResult { call_id, name: actual, result, .. }
                    if call_id == "custom-call" && actual == name && result.success
                ))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,
                    AgentEvent::FinalAnswer(text) if text == "普通工具结束后才形成最终正文"
                ))
                .count(),
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn dropped_admitted_current_load_keeps_original_owner_until_physical_observation()
-> Result<()> {
    struct ResourceDrop(Arc<AtomicUsize>);
    impl Drop for ResourceDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let root = tempfile::tempdir()?;
    let (inner, _) = seeded_current_owner(root.path()).await?;
    let path = owner_path(root.path())?;
    let original = std::fs::read(&path)?;
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let completed = Arc::new(tokio::sync::Notify::new());
    let drops = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(crate::testing::MockLlmClient::new().with_response("must not call model"));
    let store = CurrentStateGateStore {
        inner,
        entered: entered.clone(),
        release: release.clone(),
        completed: completed.clone(),
        normal_gate: Some(("runtime_load", armed)),
    };
    let agent = Arc::new(
        ReactAgentBuilder::new()
            .conversation_id(SCOPE)
            .state_store(Arc::new(store))
            .llm_client(model.clone())
            .build()?,
    );
    let mut scoped = invocation();
    scoped
        .resource_guards
        .push(echo_core::tools::InvocationResourceGuard::new(
            ResourceDrop(drops.clone()),
        ));
    let running_agent = agent.clone();
    let running = tokio::spawn(async move {
        running_agent
            .execute_stream_with_invocation_context(
                "current input",
                CancellationToken::new(),
                scoped,
            )
            .await
            .map(drop)
    });
    at_barrier(&entered).await?;
    running.abort();
    assert!(running.await.is_err());
    let retained_before_settlement =
        drops.load(Ordering::SeqCst) == 0 && agent.execution_mutex.try_lock().is_err();
    // 即使前面的断言将失败，也必须放行原Store工作并等其真实末端，不能遗留测试闸。
    release.notify_one();
    at_barrier(&completed).await?;
    assert!(
        retained_before_settlement,
        "caller drop不能在原Store仍active时释放execution mutex或资源束"
    );
    assert_eq!(model.call_count(), 0);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(
        matches!(
            agent.close().await,
            Err(ReactError::ExecutionNotSettled { .. })
        ),
        "原等待者丢失仍是unknown，不能以后台任务结束猜成功"
    );
    assert_eq!(std::fs::read(path)?, original);
    Ok(())
}

// 原Critic trait的受控失败；returned只定位错误返回，不是运行结算收据。
struct R04FailingCritic {
    typed_cancelled: bool,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    returned: Arc<tokio::sync::Notify>,
}
impl echo_core::agent::Critic for R04FailingCritic {
    fn critique<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<echo_core::agent::Critique>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            struct Returned(Arc<tokio::sync::Notify>);
            impl Drop for Returned {
                fn drop(&mut self) {
                    self.0.notify_one();
                }
            }
            let _returned = Returned(self.returned.clone());
            if self.typed_cancelled {
                Err(ReactError::Agent(Box::new(
                    crate::error::AgentError::Cancelled("r04 typed critic cancelled".into()),
                )))
            } else {
                Err(ReactError::Other(
                    "r04 original critic ordinary failure".into(),
                ))
            }
        })
    }
}

#[tokio::test]
async fn original_critic_failure_keeps_owner_through_metadata_save_and_late_cancel() -> Result<()> {
    use crate::llm::types::{DeltaMessage, ProviderResponseMetadata};
    use crate::testing::{MockLlmClient, StreamChunk};
    use std::sync::atomic::AtomicBool;
    const BODY: &str = "R04_UNAPPROVED_CRITIC_BODY";
    struct ResourceDrop(Arc<AtomicUsize>);
    impl Drop for ResourceDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    // 普通失败正控制、metadata retain await内late cancel、真实Store save内late cancel、typed取消。
    for (typed_cancelled, late_at) in [
        (false, "none"),
        (false, "context"),
        (false, "store"),
        (true, "none"),
    ] {
        let root = tempfile::tempdir()?;
        let metadata = ProviderResponseMetadata {
            protocol: crate::llm::LlmApiProtocol::ChatCompletions,
            provider: "r04-critic-fixture".into(),
            request_id: Some("r04-critic-response".into()),
            model: Some(serde_json::json!("mock-model")),
            stop_reason: Some(serde_json::json!("stop")),
            stop_sequence: Some(serde_json::Value::Null),
            status: None,
            incomplete_details: None,
            usage: Some(serde_json::json!({"r04":{"observed":true}})),
        };
        let llm = Arc::new(MockLlmClient::new().with_stream_script(vec![
            StreamChunk::Delta(Box::new(DeltaMessage {
                content: Some(BODY.into()),
                provider_response: Some(metadata.clone()),
                ..Default::default()
            })),
            StreamChunk::Terminal {
                finish_reason: Some("stop".into()),
                usage: None,
            },
        ]));
        let critic_entered = Arc::new(tokio::sync::Notify::new());
        let critic_release = Arc::new(tokio::sync::Notify::new());
        let critic_returned = Arc::new(tokio::sync::Notify::new());
        let save_entered = Arc::new(tokio::sync::Notify::new());
        let save_release = Arc::new(tokio::sync::Notify::new());
        let save_completed = Arc::new(tokio::sync::Notify::new());
        let armed = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let save_released = Arc::new(AtomicBool::new(false));
        let early_terminal = Arc::new(AtomicBool::new(false));
        let state = Arc::new(CurrentStateGateStore {
            inner: FileRuntimeStateStore::new(root.path())?,
            entered: save_entered.clone(),
            release: save_release.clone(),
            completed: save_completed.clone(),
            normal_gate: Some(("normal_cas", armed.clone())),
        });
        let conversations = Arc::new(crate::memory::FileConversationStore::new(root.path())?);
        let run_store = Arc::new(crate::trace::InMemoryRunStore::new());
        let mut built = ReactAgentBuilder::new()
            .conversation_id(SCOPE)
            .llm_client(llm.clone())
            .state_store(state.clone())
            .conversation_store(conversations.clone())
            .build()?;
        built.config_mut().verifier_enabled = true;
        built.set_critic(Arc::new(R04FailingCritic {
            typed_cancelled,
            entered: critic_entered.clone(),
            release: critic_release.clone(),
            returned: critic_returned.clone(),
        }));
        built.set_run_store(run_store.clone());
        let agent = Arc::new(built);
        let cancel = CancellationToken::new();
        let mut scoped = invocation();
        scoped.cancel = Some(cancel.clone());
        scoped
            .resource_guards
            .push(echo_core::tools::InvocationResourceGuard::new(
                ResourceDrop(drops.clone()),
            ));
        let source_agent = agent.clone();
        let source_cancel = cancel.clone();
        let released = save_released.clone();
        let early = early_terminal.clone();
        let running = tokio::spawn(async move {
            let mut stream = source_agent
                .execute_stream_with_invocation_context(
                    "r04 original Critic failure",
                    source_cancel,
                    scoped,
                )
                .await?;
            let mut raw = Vec::new();
            while let Some(item) = futures::StreamExt::next(&mut stream).await {
                if matches!(&item,Ok(crate::agent::AgentEvent::ExecutionStatus(status))
                    if matches!(status.status,echo_core::agent::ExecutionStatusKind::Failed|echo_core::agent::ExecutionStatusKind::Cancelled|echo_core::agent::ExecutionStatusKind::Settled))
                    && !released.load(Ordering::SeqCst)
                {
                    early.store(true, Ordering::SeqCst);
                }
                raw.push(item);
            }
            Ok::<_, ReactError>(raw)
        });
        let mut held_context = None;
        let mut reached_save = false;
        let mut waiting_phase = "critic_entered";
        let probe: Result<(bool, bool)> = async {
            at_barrier(&critic_entered).await?;
            held_context = Some(agent.context().lock().await);
            armed.store(true, Ordering::SeqCst);
            critic_release.notify_one();
            // 不预置cancel：Critic已返回Err，同一poll无后续await返回原select，再阻于Context。
            waiting_phase = "critic_returned";
            at_barrier(&critic_returned).await?;
            if late_at == "context" {
                cancel.cancel();
            }
            tokio::task::yield_now().await;
            let pending_context = !running.is_finished()
                && drops.load(Ordering::SeqCst) == 0
                && agent.execution_mutex.try_lock().is_err();
            drop(held_context.take());
            waiting_phase = "save_entered";
            at_barrier(&save_entered).await?;
            reached_save = true;
            if late_at == "store" {
                cancel.cancel();
            }
            tokio::task::yield_now().await;
            let pending_save = !running.is_finished()
                && drops.load(Ordering::SeqCst) == 0
                && agent.execution_mutex.try_lock().is_err();
            Ok((pending_context, pending_save))
        }
        .await;
        if let Err(error) = &probe {
            eprintln!(
                "R04_PHASE typed_cancelled={typed_cancelled} late_at={late_at} waiting_phase={waiting_phase} llm_calls={} source_finished={} gate_armed={} drops={} error={error}",
                llm.call_count(),
                running.is_finished(),
                armed.load(Ordering::SeqCst),
                drops.load(Ordering::SeqCst),
            );
        }
        // RED也先放开全部既有闸并真实join原owner，不能遗留等待或把ACK当EOF。
        drop(held_context);
        critic_release.notify_one();
        save_released.store(true, Ordering::SeqCst);
        save_release.notify_one();
        let physical_save = if reached_save {
            at_barrier(&save_completed).await
        } else {
            Ok(())
        };
        let raw = running
            .await
            .map_err(|error| fixture_error(error.to_string()))?;
        if probe.is_err() || physical_save.is_err() {
            eprintln!(
                "R04_SOURCE typed_cancelled={typed_cancelled} late_at={late_at} waiting_phase={waiting_phase} reached_save={reached_save} physical_save={physical_save:?} raw={raw:?}",
            );
        }
        let (pending_context, pending_save) = probe.map_err(|error| {
            fixture_error(format!(
                "r04 typed_cancelled={typed_cancelled} late_at={late_at} waiting_phase={waiting_phase}: {error}",
            ))
        })?;
        physical_save.map_err(|error| {
            fixture_error(format!(
                "r04 typed_cancelled={typed_cancelled} late_at={late_at} waiting_phase=save_completed: {error}",
            ))
        })?;
        let raw = raw?;
        assert!(
            pending_context,
            "original owner was lost while metadata retain awaited Context"
        );
        assert!(
            pending_save,
            "original owner was lost while original Store save remained gated"
        );
        assert!(
            !early_terminal.load(Ordering::SeqCst),
            "terminal escaped before metadata save was released"
        );
        assert_eq!(llm.call_count(), 1);
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "guards outlived original physical source EOF"
        );
        let expected_status = if typed_cancelled {
            echo_core::agent::ExecutionStatusKind::Cancelled
        } else {
            echo_core::agent::ExecutionStatusKind::Failed
        };
        let terminal = raw
            .iter()
            .filter_map(|item| match item {
                Ok(crate::agent::AgentEvent::ExecutionStatus(status))
                    if matches!(
                        status.status,
                        echo_core::agent::ExecutionStatusKind::Failed
                            | echo_core::agent::ExecutionStatusKind::Cancelled
                            | echo_core::agent::ExecutionStatusKind::Settled
                    ) =>
                {
                    Some(status.status.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal, vec![expected_status]);
        assert!(
            !raw.iter()
                .any(|item| matches!(item, Ok(crate::agent::AgentEvent::FinalAnswer(_))))
        );
        if !typed_cancelled {
            assert!(
                !raw.iter()
                    .any(|item| matches!(item, Ok(crate::agent::AgentEvent::Cancelled)))
            );
            assert!(raw.iter().any(|item| {
                match item {
                    Ok(crate::agent::AgentEvent::Error { message, .. }) => {
                        message.contains("r04 original critic ordinary failure")
                    }
                    Err(error) => error
                        .to_string()
                        .contains("r04 original critic ordinary failure"),
                    _ => false,
                }
            }));
        }
        let summaries = crate::trace::RunStore::list_all(run_store.as_ref(), 1).await?;
        let summary = summaries
            .first()
            .ok_or_else(|| fixture_error("r04 original run trace missing"))?;
        let trace = crate::trace::RunStore::load(run_store.as_ref(), &summary.run_id)
            .await?
            .ok_or_else(|| fixture_error("r04 original run body missing"))?;
        assert_eq!(
            trace.status,
            if typed_cancelled {
                crate::trace::RunStatus::Cancelled
            } else {
                crate::trace::RunStatus::Failed
            }
        );
        let context_messages = agent.context().lock().await.messages().to_vec();
        let observed = context_messages
            .iter()
            .filter(|message| message.provider_response.is_some())
            .collect::<Vec<_>>();
        assert_eq!(observed.len(), 1);
        assert!(
            observed
                .iter()
                .all(|message| message.is_provider_response_observation_only())
        );
        assert_eq!(
            observed
                .first()
                .and_then(|message| message.provider_response.as_ref()),
            Some(&metadata)
        );
        assert!(
            !context_messages
                .iter()
                .any(|message| message.text_content().unwrap_or_default().contains(BODY))
        );
        agent.close().await?;
        drop(agent);
        drop(state);
        drop(conversations);
        let cold = crate::memory::FileConversationStore::new(root.path())?;
        let restored = crate::memory::restore_messages(
            &crate::memory::ConversationStore::get_messages(&cold, SCOPE).await?,
        )?;
        let cold_metadata = restored
            .iter()
            .filter(|message| message.provider_response.is_some())
            .collect::<Vec<_>>();
        assert_eq!(cold_metadata.len(), 1);
        assert!(
            cold_metadata
                .iter()
                .all(|message| message.is_provider_response_observation_only())
        );
        assert_eq!(
            cold_metadata
                .first()
                .and_then(|message| message.provider_response.as_ref()),
            Some(&metadata)
        );
    }
    Ok(())
}
