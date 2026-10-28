//! 原Store的只读恢复准备，不开工具/模型，不写原checkpoint或签发执行完成。

use crate::compression::{PreModelContextProjector, ProjectionContext};
use crate::memory::PersistenceCallContext;
use crate::state::{ManagedRuntimeStateSnapshot, RuntimeScopeLifecycle, RuntimeStateStore};

pub struct PreparedPendingControlRecovery {
    /// 原Store返回的同一owner/scope/revision/checkpoint，未替换为归一化消息。
    pub original: ManagedRuntimeStateSnapshot,
    /// 只读派生值；candidate仍须当前Resume/Guard/Critic/Stop/settle，不是成功。
    pub messages: Vec<crate::llm::types::Message>,
    pub final_candidate: Option<String>,
    pub pending_real_call_ids: Vec<String>,
    pub origins: Vec<crate::compression::RecoveredToolOrigin>,
}

/// 入口绑定原runtime/scope与现来源projector，复用同一kernel。
/// 真实pending effect只报告给原owner，不能由此函数启动或重放。
pub(crate) async fn prepare_pending_control_recovery(
    store: &dyn RuntimeStateStore,
    call_context: PersistenceCallContext,
    runtime_id: &str,
    projection_context: &ProjectionContext,
    projector: &dyn PreModelContextProjector,
    cancel: &crate::agent::CancellationToken,
    prior_terminal: Option<crate::agent::ExecutionStatusKind>,
) -> crate::error::Result<PreparedPendingControlRecovery> {
    use crate::agent::ExecutionStatusKind;
    let paused = super::unavailable;
    if cancel.is_cancelled() {
        return Err(paused("原恢复已取消，不读取或批准旧候选"));
    }
    if matches!(
        prior_terminal,
        Some(
            ExecutionStatusKind::Settled
                | ExecutionStatusKind::Failed
                | ExecutionStatusKind::Cancelled
        )
    ) {
        return Err(paused("原owner已结算/失败/取消，旧候选不能重新成功"));
    }
    let scope_id = projection_context
        .conversation_id
        .as_deref()
        .ok_or_else(|| paused("当前来源projector缺少原scope"))?;
    let remaining = call_context.remaining_duration()?;
    let original = tokio::select! {
        result = tokio::time::timeout(remaining, store.inspect_runtime_state_for_recovery_with_context(call_context, scope_id, runtime_id)) => {
            result.map_err(|_| paused("原恢复绝对deadline在读取等待时到达"))??
        },
        _ = cancel.cancelled() => return Err(paused("只读恢复检查期间取消，未批准旧候选")),
    }.ok_or_else(|| paused("原runtime恢复记录不存在"))?;
    if original.runtime_state_id != runtime_id
        || original.scope.scope_id != scope_id
        || original.scope.lifecycle != RuntimeScopeLifecycle::Active
    {
        return Err(paused("原runtime/scope已变化或不可准入"));
    }
    let checkpoint = original
        .checkpoint
        .as_ref()
        .ok_or_else(|| paused("原checkpoint不存在"))?;
    if checkpoint.conversation_id != runtime_id {
        return Err(paused("原checkpoint身份与runtime不匹配"));
    }
    let payload = checkpoint.inspect_payload_for_recovery()?;
    if payload.pending_transcript_projection.is_some() {
        return Err(paused(
            "原耐久投递仍有债务，先由现reconcile结算，不归一化覆盖",
        ));
    }
    // 继续传播调用方原绝对预算，不重建deadline，也不为HITL设置新默认超时。
    let remaining = call_context.remaining_duration()?;
    let sources = tokio::select! {
        result = tokio::time::timeout(remaining, projector.recovered_tool_origins(projection_context, &payload.messages)) => {
            result.map_err(|_| paused("原恢复调用绝对deadline已到，来源未获确认"))??
        }
        _ = cancel.cancelled() => return Err(paused("来源恢复期间取消，原记录未改")),
    };
    let verified = super::resolve_legacy_origins(&payload.messages, sources.clone())?;
    if verified.is_empty() {
        return Err(paused(
            "没有已知旧builtin来源；普通/unknown/custom未配对记录仍由原恢复拒绝",
        ));
    }
    let mut projection = super::project_verified_control_messages(&payload.messages, &verified)?;
    // 原记录若已经携带真terminal事实，不从历史参数重新签候选。
    if payload.messages.iter().any(|message| {
        message.execution_status.as_ref().is_some_and(|status| {
            matches!(
                &status.status,
                ExecutionStatusKind::Settled
                    | ExecutionStatusKind::Failed
                    | ExecutionStatusKind::Cancelled
            )
        })
    }) {
        projection.final_candidate = None;
    }
    if cancel.is_cancelled() {
        return Err(paused("恢复准备期间取消，原文件和候选未发布"));
    }
    call_context.ensure_not_expired()?;
    Ok(PreparedPendingControlRecovery {
        original,
        messages: projection.messages,
        final_candidate: projection.final_candidate,
        pending_real_call_ids: projection.pending_real_call_ids,
        origins: sources,
    })
}

/// 原CAS kernel再次独立核转换输入，不能把任意checkpoint当恢复结果提交。
pub(crate) fn validate_recovery_request(
    request: &crate::state::RuntimeCheckpointRecoveryRequest,
) -> crate::error::Result<()> {
    let checkpoint = request
        .original
        .checkpoint
        .as_ref()
        .ok_or_else(|| super::unavailable("恢复源checkpoint缺失"))?;
    let payload = checkpoint.inspect_payload_for_recovery()?;
    if payload.pending_transcript_projection.is_some() {
        return Err(super::unavailable("恢复提交不能覆盖未结算投递"));
    }
    let verified = super::resolve_legacy_origins(&payload.messages, request.origins.clone())?;
    if verified.is_empty() {
        return Err(super::unavailable("恢复提交没有已核builtin来源"));
    }
    let projection = super::project_verified_control_messages(&payload.messages, &verified)?;
    if !projection.pending_real_call_ids.is_empty() {
        return Err(super::unavailable(
            "真实工具仍pending，不能提交候选或重放副作用",
        ));
    }
    let expected = crate::agent::snapshot::derive_control_recovery_checkpoint(
        &request.original,
        projection.messages,
    )?;
    if serde_json::to_value(&expected)? != serde_json::to_value(&request.checkpoint)? {
        return Err(super::unavailable(
            "派生checkpoint不是原来源/原context-only转换的结果",
        ));
    }
    request.checkpoint.restore_runtime_payload()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::{ContextProjection, RecoveredToolKind, RecoveredToolOrigin};
    use crate::llm::types::{FunctionCall, Message, ToolCall};
    use crate::state::{AgentCheckpoint, FileRuntimeStateStore};
    use futures::future::BoxFuture;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    struct KnownOldFactory(bool, Option<&'static str>);
    impl PreModelContextProjector for KnownOldFactory {
        fn project<'a>(
            &'a self,
            _: &'a ProjectionContext,
        ) -> BoxFuture<'a, crate::error::Result<Vec<ContextProjection>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn recovered_tool_origins<'a>(
            &'a self,
            _: &'a ProjectionContext,
            messages: &'a [Message],
        ) -> BoxFuture<'a, crate::error::Result<Vec<RecoveredToolOrigin>>> {
            Box::pin(async move {
                Ok(
                    if self.0
                        && messages
                            .iter()
                            .filter_map(|m| m.tool_calls.as_ref())
                            .flatten()
                            .any(|call| call.id == "old-control")
                    {
                        vec![RecoveredToolOrigin {
                            call_id: "old-control".to_string(),
                            kind: RecoveredToolKind::FrameworkBuiltin {
                                definition_source_sha256: self
                                    .1
                                    .unwrap_or(super::super::FINAL_SOURCE)
                                    .to_string(),
                            },
                        }]
                    } else {
                        Vec::new()
                    },
                )
            })
        }
    }

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            provider_source: None,
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    fn old_owner(
        root: &Path,
        real_pending: bool,
    ) -> crate::error::Result<(FileRuntimeStateStore, PathBuf, Vec<u8>)> {
        old_owner_with_control(root, real_pending, "final_answer")
    }

    fn old_owner_with_control(
        root: &Path,
        real_pending: bool,
        name: &str,
    ) -> crate::error::Result<(FileRuntimeStateStore, PathBuf, Vec<u8>)> {
        let store = FileRuntimeStateStore::new(root)?;
        let mut effect = Message::assistant(String::new());
        effect.tool_calls = Some(vec![call(
            "completed-effect",
            "write_file",
            r#"{"path":"old"}"#,
        )]);
        let mut tail = Message::assistant(String::new());
        let arguments = if name == "think" {
            r#"{"reasoning":"历史执行说明🙂"}"#
        } else {
            r#"{"answer":"必须经过当前校验的候选🙂"}"#
        };
        let mut calls = vec![call("old-control", name, arguments)];
        if real_pending {
            calls.push(call(
                "still-pending-effect",
                "write_file",
                r#"{"path":"unknown"}"#,
            ));
        }
        tail.tool_calls = Some(calls);
        let messages = vec![
            effect,
            Message::tool_result(
                "completed-effect".to_string(),
                "write_file".to_string(),
                "原已完成副作用结果".to_string(),
            ),
            tail,
        ];
        let mut checkpoint = AgentCheckpoint::new("pending-runtime");
        checkpoint.messages_json = serde_json::to_string(&messages)?;
        // 今天的save严格拒此未配对记录。夹具模拟磁盘已有旧owner，不把save失败伪作合法新写。
        let owner = serde_json::json!({"version":1,"runtime_state_id":"pending-runtime","scope_id":"pending-scope","phase":"active","checkpoint":checkpoint});
        let raw = serde_json::to_vec(&owner)?;
        let safe = echo_core::utils::fs::encode_path_segment_identity("pending-runtime")
            .map_err(|error| super::super::unavailable(error.to_string()))?;
        let path = root
            .join("runtime_state/_runtime_owners")
            .join(format!("{safe}.json"));
        std::fs::write(&path, &raw)?;
        Ok((store, path, raw))
    }

    fn context() -> ProjectionContext {
        ProjectionContext {
            iteration: 0,
            agent_name: "old-eko".to_string(),
            session_id: None,
            conversation_id: Some("pending-scope".to_string()),
            run_id: None,
            turn_id: None,
        }
    }

    fn managed_owner_and_scope(
        root: &Path,
        owner_path: &Path,
        scope_revision: u64,
    ) -> crate::error::Result<(PathBuf, Vec<u8>, Vec<u8>)> {
        let raw = std::fs::read(owner_path)?;
        let mut value: serde_json::Value = serde_json::from_slice(&raw)?;
        let checkpoint = value
            .get("checkpoint")
            .cloned()
            .ok_or_else(|| super::super::unavailable("fixture checkpoint missing"))?;
        let mut checkpoint: AgentCheckpoint = serde_json::from_value(checkpoint)?;
        let messages = checkpoint.inspect_payload_for_recovery()?.messages;
        let projected = crate::memory::project_messages("pending-runtime", &messages)?;
        let facts = projected
            .iter()
            .enumerate()
            .map(|(position, message)| {
                Ok(crate::state::TranscriptProjectionMessage {
                    ordinal: u64::try_from(position)
                        .map_err(|error| super::super::unavailable(error.to_string()))?,
                    digest: crate::memory::transcript_projection_message_digest(message)?,
                })
            })
            .collect::<crate::error::Result<Vec<_>>>()?;
        let cursor = crate::state::TranscriptProjectionCheckpoint {
            generation_id: "pending-runtime".to_string(),
            next_ordinal: u64::try_from(facts.len())
                .map_err(|error| super::super::unavailable(error.to_string()))?,
            projected: facts,
            context_only: Some(Vec::new()),
        };
        checkpoint.messages_json = AgentCheckpoint::serialize_payload(messages, Some(cursor))?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| super::super::unavailable("fixture owner not object"))?;
        object.insert("version".to_string(), serde_json::json!(2));
        object.insert(
            "state_version".to_string(),
            serde_json::to_value(crate::state::RuntimeStateVersion::Managed { revision: 1 })?,
        );
        object.insert("scope_revision".to_string(), serde_json::json!(2));
        object.insert("conversation_epoch".to_string(), serde_json::json!(1));
        object.insert("checkpoint".to_string(), serde_json::to_value(checkpoint)?);
        let owner_bytes = serde_json::to_vec(&value)?;
        std::fs::write(owner_path, &owner_bytes)?;
        let safe = echo_core::utils::fs::encode_path_segment_identity("pending-scope")?;
        let scope_path = root
            .join("runtime_state/_scope_owners")
            .join(format!("{safe}.json"));
        let scope = serde_json::json!({"version":1,"authority":{"scope_id":"pending-scope","conversation_epoch":1,"revision":scope_revision,"lifecycle":"active"},"current_retirement":null,"completed_retirements":[]});
        let scope_bytes = serde_json::to_vec(&scope)?;
        std::fs::write(&scope_path, &scope_bytes)?;
        Ok((scope_path, owner_bytes, scope_bytes))
    }

    #[tokio::test]
    async fn managed_schema3_cursor_is_preserved_and_stale_scope_is_not_silently_reconciled()
    -> crate::error::Result<()> {
        for revision in [1, 2] {
            let root = tempfile::tempdir()?;
            let (store, path, _) = old_owner(root.path(), false)?;
            let (scope_path, owner_bytes, scope_bytes) =
                managed_owner_and_scope(root.path(), &path, revision)?;
            let result = prepare(&store, true).await;
            if revision == 1 {
                assert!(result.is_err());
            } else {
                let prepared = result?;
                assert_eq!(prepared.original.scope.revision, 2);
                assert!(prepared.final_candidate.is_some());
            }
            assert_eq!(std::fs::read(path)?, owner_bytes);
            assert_eq!(std::fs::read(scope_path)?, scope_bytes);
        }
        Ok(())
    }

    #[tokio::test]
    async fn bad_managed_cursor_digest_is_not_hidden_by_known_builtin_origin()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, _) = old_owner(root.path(), false)?;
        managed_owner_and_scope(root.path(), &path, 2)?;
        let mut owner: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let checkpoint = owner
            .get_mut("checkpoint")
            .ok_or_else(|| super::super::unavailable("missing checkpoint"))?;
        let encoded = checkpoint
            .get("messages_json")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| super::super::unavailable("missing messages_json"))?;
        let mut payload: serde_json::Value = serde_json::from_str(encoded)?;
        let first = payload
            .get_mut("transcript_projection")
            .and_then(|cursor| cursor.get_mut("projected"))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|facts| facts.first_mut())
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| super::super::unavailable("missing first fact"))?;
        first.insert("digest".to_string(), serde_json::json!("0".repeat(64)));
        checkpoint
            .as_object_mut()
            .ok_or_else(|| super::super::unavailable("checkpoint not object"))?
            .insert(
                "messages_json".to_string(),
                serde_json::json!(payload.to_string()),
            );
        let raw = serde_json::to_vec(&owner)?;
        std::fs::write(&path, &raw)?;
        assert!(prepare(&store, true).await.is_err());
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    struct UnansweredOrigin;
    impl PreModelContextProjector for UnansweredOrigin {
        fn project<'a>(
            &'a self,
            _: &'a ProjectionContext,
        ) -> BoxFuture<'a, crate::error::Result<Vec<ContextProjection>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn recovered_tool_origins<'a>(
            &'a self,
            _: &'a ProjectionContext,
            _: &'a [Message],
        ) -> BoxFuture<'a, crate::error::Result<Vec<RecoveredToolOrigin>>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn origin_wait_uses_original_absolute_budget_and_cancellation_without_writes()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner(root.path(), false)?;
        let cancel = crate::agent::CancellationToken::new();
        let projection_context = context();
        let projector = UnansweredOrigin;
        let future = prepare_pending_control_recovery(
            &store,
            PersistenceCallContext::with_timeout(Duration::from_millis(50))?,
            "pending-runtime",
            &projection_context,
            &projector,
            &cancel,
            None,
        );
        let result = tokio::time::timeout(Duration::from_secs(1), future)
            .await
            .map_err(|error| super::super::unavailable(error.to_string()))?;
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path)?, raw);
        Ok(())
    }

    async fn prepare(
        store: &FileRuntimeStateStore,
        known: bool,
    ) -> crate::error::Result<PreparedPendingControlRecovery> {
        prepare_pending_control_recovery(
            store,
            PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
            "pending-runtime",
            &context(),
            &KnownOldFactory(known, None),
            &crate::agent::CancellationToken::new(),
            None,
        )
        .await
    }

    #[tokio::test]
    async fn real_file_owner_known_pending_control_reaches_readonly_candidate_without_mutation()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner(root.path(), false)?;
        assert!(store.get_checkpoint("pending-runtime").await.is_err());
        let prepared = prepare(&store, true).await?;
        assert_eq!(
            prepared.final_candidate.as_deref(),
            Some("必须经过当前校验的候选🙂")
        );
        assert!(prepared.pending_real_call_ids.is_empty());
        assert_eq!(
            prepared
                .messages
                .iter()
                .filter(|m| m.role == crate::llm::types::Role::Tool)
                .count(),
            1
        );
        assert_eq!(
            prepared
                .original
                .checkpoint
                .as_ref()
                .map(|cp| cp.conversation_id.as_str()),
            Some("pending-runtime")
        );
        assert_eq!(std::fs::read(&path)?, raw);
        assert!(
            store.get_checkpoint("pending-runtime").await.is_err(),
            "只读准备不能偷偷修写原记录"
        );
        Ok(())
    }

    #[tokio::test]
    async fn actual_agent_recovery_entry_reaches_original_file_and_source_projector()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner(root.path(), false)?;
        let agent = crate::agent::ReactAgentBuilder::new()
            .conversation_id("pending-scope")
            .state_store(std::sync::Arc::new(store))
            .build()?;
        agent.set_pre_model_context_projector(Some(std::sync::Arc::new(KnownOldFactory(
            true, None,
        ))));
        // 正常resume仍严格，不能为让该反控变绿偷偷放宽旧load。
        assert!(
            agent
                .resume_from_state_store_id("pending-runtime")
                .await
                .is_err()
        );
        let prepared = agent
            .prepare_pending_control_checkpoint(
                "pending-runtime",
                PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
                &crate::agent::CancellationToken::new(),
                None,
            )
            .await?;
        assert!(prepared.final_candidate.is_some());
        assert!(agent.get_messages().await.iter().all(|message| {
            message
                .tool_calls
                .as_ref()
                .is_none_or(|calls| calls.iter().all(|call| call.id != "old-control"))
        }));
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn unknown_origin_and_cancelled_owner_remain_paused_without_replaying_effects()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner(root.path(), false)?;
        assert!(prepare(&store, false).await.is_err());
        let cancel = crate::agent::CancellationToken::new();
        cancel.cancel();
        assert!(
            prepare_pending_control_recovery(
                &store,
                PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
                "pending-runtime",
                &context(),
                &KnownOldFactory(true, None),
                &cancel,
                None
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn mixed_pending_real_effect_defers_candidate_and_reports_original_call_id()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner(root.path(), true)?;
        let prepared = prepare(&store, true).await?;
        assert!(prepared.final_candidate.is_none());
        assert_eq!(
            prepared.pending_real_call_ids,
            vec!["still-pending-effect".to_string()]
        );
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn known_pending_think_is_history_explanation_and_has_no_synthetic_result()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner_with_control(root.path(), false, "think")?;
        let prepared = prepare_pending_control_recovery(
            &store,
            PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
            "pending-runtime",
            &context(),
            &KnownOldFactory(true, Some(super::super::THINK_SOURCE)),
            &crate::agent::CancellationToken::new(),
            None,
        )
        .await?;
        assert!(prepared.final_candidate.is_none());
        assert!(prepared.messages.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("历史执行说明"))
        }));
        assert_eq!(
            prepared
                .messages
                .iter()
                .filter(|message| message.role == crate::llm::types::Role::Tool)
                .count(),
            1
        );
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn recorded_terminal_never_becomes_another_success_candidate() -> crate::error::Result<()>
    {
        let root = tempfile::tempdir()?;
        let (store, path, raw) = old_owner(root.path(), false)?;
        for status in [
            crate::agent::ExecutionStatusKind::Settled,
            crate::agent::ExecutionStatusKind::Failed,
            crate::agent::ExecutionStatusKind::Cancelled,
        ] {
            assert!(
                prepare_pending_control_recovery(
                    &store,
                    PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
                    "pending-runtime",
                    &context(),
                    &KnownOldFactory(true, None),
                    &crate::agent::CancellationToken::new(),
                    Some(status)
                )
                .await
                .is_err()
            );
        }
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    fn invocation() -> crate::agent::AgentInvocationContext {
        crate::agent::AgentInvocationContext {
            runtime_state_id: Some("pending-runtime".to_string()),
            transcript_generation_id: Some("pending-runtime".to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn known_pending_reenters_original_resume_and_finalization_with_preserved_source()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, AgentEvent, ReactAgentBuilder};
        use futures::StreamExt;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct EffectCounter(Arc<AtomicUsize>);
        impl crate::tools::Tool for EffectCounter {
            fn name(&self) -> &str {
                "write_file"
            }
            fn description(&self) -> &str {
                "原已完成副作用反控"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type":"object","properties":{}})
            }
            fn execute(
                &self,
                _: crate::tools::ToolParameters,
            ) -> BoxFuture<'_, crate::error::Result<crate::tools::ToolResult>> {
                Box::pin(async move {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    Ok(crate::tools::ToolResult::success(
                        "must not replay".to_string(),
                    ))
                })
            }
        }
        for name in ["final_answer", "think"] {
            let root = tempfile::tempdir()?;
            let (store, path, original_raw) = old_owner_with_control(root.path(), false, name)?;
            let llm =
                Arc::new(crate::testing::MockLlmClient::new().with_response("当前校验后回答"));
            let effects = Arc::new(AtomicUsize::new(0));
            let agent = ReactAgentBuilder::new()
                .conversation_id("pending-scope")
                .state_store(Arc::new(store))
                .llm_client(llm.clone())
                .tool(Box::new(EffectCounter(effects.clone())))
                .build()?;
            agent.set_pre_model_context_projector(Some(Arc::new(KnownOldFactory(
                true,
                Some(if name == "think" {
                    super::super::THINK_SOURCE
                } else {
                    super::super::FINAL_SOURCE
                }),
            ))));
            let raw = agent
                .execute_stream_with_invocation_context(
                    "继续原恢复并核验",
                    crate::agent::CancellationToken::new(),
                    invocation(),
                )
                .await?
                .collect::<Vec<_>>()
                .await;
            let events = raw.into_iter().collect::<crate::error::Result<Vec<_>>>()?;
            assert_eq!(effects.load(Ordering::SeqCst), 0, "原完成effect不得重放");
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, AgentEvent::FinalAnswer(_)))
                    .count(),
                1
            );
            assert_eq!(events.iter().filter(|event| matches!(event, AgentEvent::ExecutionStatus(status) if status.status == crate::agent::ExecutionStatusKind::Settled)).count(), 1);
            let source_root = root.path().join("runtime_state/_recovery_sources");
            let mut preserved = false;
            for directory in std::fs::read_dir(&source_root)? {
                for entry in std::fs::read_dir(directory?.path())? {
                    let entry = entry?;
                    if entry.file_name().to_string_lossy().ends_with(".owner.json") {
                        preserved |= std::fs::read(entry.path())? == original_raw;
                    }
                }
            }
            assert!(preserved, "正常CAS后仍可完整回读旧owner原始bytes");
            assert_ne!(
                std::fs::read(path)?,
                original_raw,
                "当前owner应是正常后继版本，不假报只读完成"
            );
            assert_eq!(llm.call_count(), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn resumed_candidate_guard_denial_publishes_no_success_and_stop_continues_once()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, AgentEvent, ReactAgentBuilder};
        use echo_core::guard::{Guard, GuardDirection, GuardResult};
        use futures::StreamExt;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Reject(Arc<AtomicUsize>);
        impl Guard for Reject {
            fn name(&self) -> &str {
                "pending-recovery-original-output-guard"
            }
            fn check<'a>(
                &'a self,
                _: &'a str,
                direction: GuardDirection,
            ) -> BoxFuture<'a, crate::error::Result<GuardResult>> {
                Box::pin(async move {
                    Ok(if direction == GuardDirection::Output {
                        self.0.fetch_add(1, Ordering::SeqCst);
                        GuardResult::Block {
                            reason: "deny recovered answer".to_string(),
                        }
                    } else {
                        GuardResult::Pass
                    })
                })
            }
        }
        for deny in [true, false] {
            let root = tempfile::tempdir()?;
            let (store, _, _) = old_owner(root.path(), false)?;
            let llm = Arc::new(
                crate::testing::MockLlmClient::new()
                    .with_responses(["待当前Guard/Stop校验", "Stop续行后完成"]),
            );
            let guards = Arc::new(AtomicUsize::new(0));
            let mut builder = ReactAgentBuilder::new()
                .conversation_id("pending-scope")
                .state_store(Arc::new(store))
                .llm_client(llm.clone());
            if deny {
                builder = builder.guard(Arc::new(Reject(guards.clone())));
            }
            let agent = builder.build()?;
            agent.set_pre_model_context_projector(Some(Arc::new(KnownOldFactory(true, None))));
            let stops = Arc::new(AtomicUsize::new(0));
            let calls = stops.clone();
            agent.hook_registry().write().await.set_programmatic_hook(
                "pending-resume-continue",
                &[crate::skills::hooks::HookEvent::Stop],
                Arc::new(move |_| {
                    let prior = calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move {
                        let mut result = crate::skills::hooks::HookResult::allow();
                        if prior == 0 {
                            result.continue_reason = Some("再验证一次".to_string());
                        }
                        result
                    })
                }),
            );
            let raw = agent
                .execute_stream_with_invocation_context(
                    "继续当前验证",
                    crate::agent::CancellationToken::new(),
                    invocation(),
                )
                .await?
                .collect::<Vec<_>>()
                .await;
            let answers = raw
                .iter()
                .filter(|event| matches!(event, Ok(AgentEvent::FinalAnswer(_))))
                .count();
            if deny {
                assert!(guards.load(Ordering::SeqCst) > 0, "必须真实到达原Guard");
                assert_eq!(answers, 0);
                assert!(raw.iter().all(|event| !matches!(event, Ok(AgentEvent::ExecutionStatus(status)) if status.status == crate::agent::ExecutionStatusKind::Settled)));
            } else {
                assert_eq!(stops.load(Ordering::SeqCst), 2);
                assert_eq!(answers, 1);
                assert!(raw.iter().any(|event| matches!(event, Ok(AgentEvent::FinalAnswer(text)) if text == "Stop续行后完成")));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn recovery_commit_reuses_original_cas_and_replay_cannot_overwrite_newer_work()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, path, original_raw) = old_owner(root.path(), false)?;
        let prepared = prepare(&store, true).await?;
        let checkpoint = crate::agent::snapshot::derive_control_recovery_checkpoint(
            &prepared.original,
            prepared.messages,
        )?;
        let request = crate::state::RuntimeCheckpointRecoveryRequest {
            original: prepared.original,
            checkpoint,
            origins: prepared.origins,
        };
        let budget = PersistenceCallContext::with_timeout(Duration::from_secs(10))?;
        let first = store
            .commit_checkpoint_recovery_with_context(budget, request.clone())
            .await?;
        let retry = store
            .commit_checkpoint_recovery_with_context(budget, request.clone())
            .await?;
        assert_eq!(
            first.status,
            crate::state::RuntimeCheckpointCasStatus::Applied
        );
        assert_eq!(
            retry.status,
            crate::state::RuntimeCheckpointCasStatus::AlreadyCurrent
        );
        assert_eq!(first.version, retry.version);
        let mut newer = request.checkpoint.clone();
        let mut messages = newer.restore_messages()?;
        messages.push(Message::user("更晚实际输入".to_string()));
        newer.messages_json = AgentCheckpoint::serialize_payload(messages, None)?;
        let crate::state::RuntimeStateVersion::Managed { revision } = first.version else {
            return Err(super::super::unavailable(
                "恢复未进入原revisioned authority",
            ));
        };
        let advanced = store
            .compare_and_save_checkpoint(crate::state::RuntimeCheckpointCasRequest {
                scope_id: "pending-scope".to_string(),
                runtime_state_id: "pending-runtime".to_string(),
                conversation_epoch: None,
                expected_scope_revision: first.scope.revision,
                expected_state_version: crate::state::RuntimeStateExpectedVersion::Managed {
                    revision,
                },
                checkpoint: newer,
                managed_import: None,
            })
            .await?;
        assert_eq!(
            advanced.status,
            crate::state::RuntimeCheckpointCasStatus::Applied
        );
        let bytes_before_retry = std::fs::read(&path)?;
        let stale = store
            .commit_checkpoint_recovery_with_context(budget, request)
            .await?;
        assert_eq!(
            stale.status,
            crate::state::RuntimeCheckpointCasStatus::RevisionConflict
        );
        assert_eq!(std::fs::read(path)?, bytes_before_retry);
        let source = root.path().join("runtime_state/_recovery_sources");
        let mut originals = 0;
        for directory in std::fs::read_dir(source)? {
            for file in std::fs::read_dir(directory?.path())? {
                let file = file?;
                if file.file_name().to_string_lossy().ends_with(".owner.json") {
                    assert_eq!(std::fs::read(file.path())?, original_raw);
                    originals += 1;
                }
            }
        }
        assert_eq!(originals, 1);
        Ok(())
    }

    async fn recovery_request(
        store: &dyn RuntimeStateStore,
    ) -> crate::error::Result<crate::state::RuntimeCheckpointRecoveryRequest> {
        let prepared = prepare_pending_control_recovery(
            store,
            PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
            "pending-runtime",
            &context(),
            &KnownOldFactory(true, None),
            &crate::agent::CancellationToken::new(),
            None,
        )
        .await?;
        Ok(crate::state::RuntimeCheckpointRecoveryRequest {
            checkpoint: crate::agent::snapshot::derive_control_recovery_checkpoint(
                &prepared.original,
                prepared.messages,
            )?,
            original: prepared.original,
            origins: prepared.origins,
        })
    }

    fn source_owner_path(root: &Path) -> crate::error::Result<PathBuf> {
        for directory in std::fs::read_dir(root.join("runtime_state/_recovery_sources"))? {
            for entry in std::fs::read_dir(directory?.path())? {
                let entry = entry?;
                if entry.file_name().to_string_lossy().ends_with(".owner.json") {
                    return Ok(entry.path());
                }
            }
        }
        Err(super::super::unavailable("fixture source owner missing"))
    }

    #[tokio::test]
    async fn recovery_retry_checks_full_raw_source_bytes_including_unknown_owner_fields()
    -> crate::error::Result<()> {
        for mutation in ["whitespace", "non_checkpoint_field", "unknown_field"] {
            let root = tempfile::tempdir()?;
            let (store, owner_path, original) = old_owner(root.path(), false)?;
            let request = recovery_request(&store).await?;
            let budget = PersistenceCallContext::with_timeout(Duration::from_secs(10))?;
            store
                .commit_checkpoint_recovery_with_context(budget, request.clone())
                .await?;
            let archived = source_owner_path(root.path())?;
            assert_eq!(std::fs::read(&archived)?, original);
            let current = std::fs::read(&owner_path)?;
            let corrupt = if mutation == "whitespace" {
                let mut raw = original.clone();
                raw.push(b' ');
                raw
            } else {
                let mut value: serde_json::Value = serde_json::from_slice(&original)?;
                let object = value
                    .as_object_mut()
                    .ok_or_else(|| super::super::unavailable("fixture owner not object"))?;
                object.insert(
                    if mutation == "unknown_field" {
                        "unknown_old_field"
                    } else {
                        "cas_expected_scope_revision"
                    }
                    .to_string(),
                    serde_json::json!(17),
                );
                serde_json::to_vec(&value)?
            };
            std::fs::write(&archived, &corrupt)?;
            assert!(
                store
                    .commit_checkpoint_recovery_with_context(budget, request)
                    .await
                    .is_err(),
                "full raw digest must reject {mutation}"
            );
            assert_eq!(std::fs::read(owner_path)?, current);
        }
        Ok(())
    }

    #[tokio::test]
    async fn recovery_replay_reconciles_owner_first_crash_cut_to_correct_scope_receipt()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir()?;
        let (store, owner_path, _) = old_owner(root.path(), false)?;
        let (scope_path, original_owner, old_scope) =
            managed_owner_and_scope(root.path(), &owner_path, 2)?;
        let request = recovery_request(&store).await?;
        let budget = PersistenceCallContext::with_timeout(Duration::from_secs(10))?;
        let first = store
            .commit_checkpoint_recovery_with_context(budget, request.clone())
            .await?;
        assert_eq!(first.scope.revision, 3);
        let new_owner = std::fs::read(&owner_path)?;
        // 模拟owner成功落盘但scope写未完成的真实文件切点；只改tempdir夹具。
        std::fs::write(&scope_path, old_scope)?;
        let retry = store
            .commit_checkpoint_recovery_with_context(budget, request)
            .await?;
        assert_eq!(
            retry.status,
            crate::state::RuntimeCheckpointCasStatus::AlreadyCurrent
        );
        assert_eq!(retry.scope, first.scope);
        assert_eq!(retry.version, first.version);
        assert_eq!(
            store.load_scope_authority("pending-scope").await?,
            Some(first.scope)
        );
        assert_eq!(std::fs::read(owner_path)?, new_owner);
        assert_eq!(
            std::fs::read(source_owner_path(root.path())?)?,
            original_owner
        );
        Ok(())
    }

    #[tokio::test]
    async fn runtime_only_save_preserves_epoch_and_rejects_context_from_older_revision()
    -> crate::error::Result<()> {
        use crate::agent::ReactAgentBuilder;
        use std::sync::Arc;
        for concurrent_update in [false, true] {
            let root = tempfile::tempdir()?;
            let (store, path, _) = old_owner(root.path(), false)?;
            managed_owner_and_scope(root.path(), &path, 2)?;
            let request = recovery_request(&store).await?;
            let current = store
                .commit_checkpoint_recovery_with_context(
                    PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
                    request.clone(),
                )
                .await?;
            let agent = ReactAgentBuilder::new()
                .conversation_id("pending-scope")
                .state_store(Arc::new(store.clone()))
                .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
                .build()?;
            agent
                .resume_from_state_store_scope("pending-scope", "pending-runtime")
                .await?;
            let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent_with_invocation(
                &agent,
                &invocation(),
            );
            if concurrent_update {
                let crate::state::RuntimeStateVersion::Managed { revision } = current.version
                else {
                    return Err(super::super::unavailable(
                        "fixture managed revision missing",
                    ));
                };
                let mut newer_messages = request.checkpoint.restore_messages()?;
                newer_messages.push(Message::user("外部后继输入，不许被旧内存覆盖".to_string()));
                let checkpoint = crate::agent::snapshot::derive_control_recovery_checkpoint(
                    &store
                        .load_runtime_state("pending-scope", "pending-runtime")
                        .await?
                        .ok_or_else(|| super::super::unavailable("fixture current missing"))?,
                    newer_messages,
                )?;
                let next = store
                    .compare_and_save_checkpoint(crate::state::RuntimeCheckpointCasRequest {
                        scope_id: "pending-scope".to_string(),
                        runtime_state_id: "pending-runtime".to_string(),
                        conversation_epoch: Some(1),
                        expected_scope_revision: current.scope.revision,
                        expected_state_version:
                            crate::state::RuntimeStateExpectedVersion::Managed { revision },
                        checkpoint,
                        managed_import: None,
                    })
                    .await?;
                assert_eq!(
                    next.status,
                    crate::state::RuntimeCheckpointCasStatus::Applied
                );
            }
            let before = std::fs::read(&path)?;
            let saved = snapshot
                .save_runtime_checkpoint(agent.context(), None)
                .await;
            if concurrent_update {
                assert!(saved.is_err());
                assert_eq!(std::fs::read(path)?, before);
            } else {
                saved?;
                let after = store
                    .load_runtime_state("pending-scope", "pending-runtime")
                    .await?
                    .ok_or_else(|| super::super::unavailable("fixture current missing"))?;
                assert_eq!(after.scope.conversation_epoch, Some(1));
                assert!(
                    after
                        .checkpoint
                        .ok_or_else(|| super::super::unavailable("fixture checkpoint missing"))?
                        .restore_runtime_payload()?
                        .transcript_projection
                        .is_some()
                );
            }
        }
        Ok(())
    }

    /// 测试Store保留已接纳CAS任务，在可控闸后调用真实File CAS；丢弃调用future
    /// 不会取消该任务，模拟现keyed blocking Store已准入后的物理所有权。
    struct AdmittedRecoveryStore {
        inner: FileRuntimeStateStore,
        entered: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
        completed: std::sync::Arc<tokio::sync::Notify>,
        cancel_after_commit: Option<crate::agent::CancellationToken>,
        normal_gate: Option<(&'static str, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
    }
    impl AdmittedRecoveryStore {
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
                    .map_err(|error| super::super::unavailable(error.to_string()))?
            })
        }
    }
    impl RuntimeStateStore for AdmittedRecoveryStore {
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
        fn inspect_runtime_state_for_recovery_with_context<'a>(
            &'a self,
            context: PersistenceCallContext,
            scope: &'a str,
            runtime: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<Option<ManagedRuntimeStateSnapshot>>> {
            self.inner
                .inspect_runtime_state_for_recovery_with_context(context, scope, runtime)
        }
        fn load_scope_authority<'a>(
            &'a self,
            scope: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<Option<crate::state::RuntimeScopeAuthority>>>
        {
            self.inner.load_scope_authority(scope)
        }
        fn compare_and_save_checkpoint<'a>(
            &'a self,
            request: crate::state::RuntimeCheckpointCasRequest,
        ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>>
        {
            self.inner.compare_and_save_checkpoint(request)
        }
        fn load_scope_authority_with_context<'a>(
            &'a self,
            context: PersistenceCallContext,
            scope: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<Option<crate::state::RuntimeScopeAuthority>>>
        {
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
        ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>>
        {
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
        fn commit_checkpoint_recovery_with_context<'a>(
            &'a self,
            context: PersistenceCallContext,
            request: crate::state::RuntimeCheckpointRecoveryRequest,
        ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>>
        {
            let inner = self.inner.clone();
            let release = self.release.clone();
            let completed = self.completed.clone();
            let entered = self.entered.clone();
            let cancel = self.cancel_after_commit.clone();
            Box::pin(async move {
                // 仅测试控制闸；真实生产没有新增任务/执行器。
                let task = tokio::spawn(async move {
                    release.notified().await;
                    let result = inner
                        .commit_checkpoint_recovery_with_context(context, request)
                        .await;
                    if result.is_ok()
                        && let Some(cancel) = cancel
                    {
                        cancel.cancel();
                    }
                    completed.notify_one();
                    result
                });
                entered.notify_one();
                task.await
                    .map_err(|error| super::super::unavailable(error.to_string()))?
            })
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
        ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeStateScopeClearReceipt>>
        {
            self.inner.clear_runtime_state_scope(scope)
        }
        fn clear_conversation<'a>(
            &'a self,
            id: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<()>> {
            self.inner.clear_conversation(id)
        }
    }

    async fn at_barrier(barrier: &tokio::sync::Notify) -> crate::error::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), barrier.notified())
            .await
            .map_err(|error| super::super::unavailable(error.to_string()))
    }

    #[tokio::test]
    async fn dropping_admitted_recovery_keeps_original_resources_for_execute_and_explicit_resume()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, ReactAgentBuilder};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct DropProof(Arc<AtomicUsize>);
        impl Drop for DropProof {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        for explicit_resume in [false, true] {
            let root = tempfile::tempdir()?;
            let (inner, path, raw) = old_owner(root.path(), false)?;
            if explicit_resume {
                // public Resume使用相同scope/runtime id；temp旧owner合法绑定该scope。
                let mut owner: serde_json::Value = serde_json::from_slice(&raw)?;
                owner
                    .as_object_mut()
                    .ok_or_else(|| super::super::unavailable("fixture owner missing"))?
                    .insert("scope_id".to_string(), serde_json::json!("pending-runtime"));
                std::fs::write(&path, serde_json::to_vec(&owner)?)?;
            }
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let completed = Arc::new(tokio::sync::Notify::new());
            let store = Arc::new(AdmittedRecoveryStore {
                inner: inner.clone(),
                entered: entered.clone(),
                release: release.clone(),
                completed: completed.clone(),
                cancel_after_commit: None,
                normal_gate: None,
            });
            let drops = Arc::new(AtomicUsize::new(0));
            let agent = Arc::new(
                ReactAgentBuilder::new()
                    .conversation_id(if explicit_resume {
                        "pending-runtime"
                    } else {
                        "pending-scope"
                    })
                    .state_store(store)
                    .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
                    .build()?,
            );
            agent.set_pre_model_context_projector(Some(Arc::new(KnownOldFactory(true, None))));
            let original = agent.clone();
            let proof = echo_core::tools::InvocationResourceGuard::new(DropProof(drops.clone()));
            let caller = tokio::spawn(async move {
                if explicit_resume {
                    original.resume_from_state_store().await.map(|_| ())
                } else {
                    let mut invocation = invocation();
                    invocation.resource_guards.push(proof);
                    original
                        .execute_stream_with_invocation_context(
                            "恢复",
                            crate::agent::CancellationToken::new(),
                            invocation,
                        )
                        .await
                        .map(drop)
                }
            });
            at_barrier(&entered).await?;
            caller.abort();
            let cancelled = caller.await;
            assert!(cancelled.is_err());
            assert!(
                agent.close().await.is_err(),
                "同一原admission未观测物理结算，close不能成功"
            );
            if !explicit_resume {
                assert_eq!(drops.load(Ordering::SeqCst), 0, "保留原invocation资源");
            }
            release.notify_one();
            at_barrier(&completed).await?;
            assert!(
                inner
                    .load_runtime_state(
                        if explicit_resume {
                            "pending-runtime"
                        } else {
                            "pending-scope"
                        },
                        "pending-runtime"
                    )
                    .await?
                    .is_some()
            );
            assert!(
                agent.close().await.is_err(),
                "观察者丢失不能把后续写完成反推成已观测结算"
            );
            drop(agent);
            if !explicit_resume {
                assert_eq!(drops.load(Ordering::SeqCst), 1);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_after_real_recovery_commit_does_not_publish_old_or_new_success()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, ReactAgentBuilder};
        use std::sync::Arc;
        let root = tempfile::tempdir()?;
        let (inner, _, _) = old_owner(root.path(), false)?;
        let cancel = crate::agent::CancellationToken::new();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let completed = Arc::new(tokio::sync::Notify::new());
        let store = Arc::new(AdmittedRecoveryStore {
            inner: inner.clone(),
            entered: entered.clone(),
            release: release.clone(),
            completed,
            cancel_after_commit: Some(cancel.clone()),
            normal_gate: None,
        });
        let llm = Arc::new(crate::testing::MockLlmClient::new().with_response("不许发布"));
        let agent = Arc::new(
            ReactAgentBuilder::new()
                .conversation_id("pending-scope")
                .state_store(store)
                .llm_client(llm.clone())
                .build()?,
        );
        agent.set_pre_model_context_projector(Some(Arc::new(KnownOldFactory(true, None))));
        let original = agent.clone();
        let caller = tokio::spawn(async move {
            original
                .execute_stream_with_invocation_context("恢复后立即取消", cancel, invocation())
                .await
                .map(drop)
        });
        at_barrier(&entered).await?;
        release.notify_one();
        assert!(
            caller
                .await
                .map_err(|error| super::super::unavailable(error.to_string()))?
                .is_err()
        );
        assert_eq!(
            llm.call_count(),
            0,
            "CAS物理结算后取消，不能继续模型/FinalAnswer"
        );
        assert!(
            inner
                .load_runtime_state("pending-scope", "pending-runtime")
                .await?
                .is_some()
        );
        agent.close().await?;
        Ok(())
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
            let (inner, path, _) = old_owner(root.path(), false)?;
            let request = recovery_request(&inner).await?;
            inner
                .commit_checkpoint_recovery_with_context(
                    PersistenceCallContext::with_timeout(Duration::from_secs(10))?,
                    request,
                )
                .await?;
            let armed = Arc::new(AtomicBool::new(false));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let store = Arc::new(AdmittedRecoveryStore {
                inner: inner.clone(),
                entered: entered.clone(),
                release: release.clone(),
                completed: Arc::new(tokio::sync::Notify::new()),
                cancel_after_commit: None,
                normal_gate: Some((phase, armed.clone())),
            });
            let agent = ReactAgentBuilder::new()
                .conversation_id("pending-scope")
                .state_store(store)
                .persistence_settlement_timeout(Duration::from_millis(250))
                .llm_client(Arc::new(crate::testing::MockLlmClient::new()))
                .build()?;
            agent
                .resume_from_state_store_scope("pending-scope", "pending-runtime")
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
                        result.map_err(|error| super::super::unavailable(error.to_string()))?;
                    return Err(super::super::unavailable(format!(
                        "{phase} released caller before physical settlement"
                    )));
                }
                Err(_) => saving
                    .await
                    .map_err(|error| super::super::unavailable(error.to_string()))?,
            };
            assert!(
                completed.is_err(),
                "原deadline未延长，真实Store迟到后仍拒新写"
            );
            assert_eq!(std::fs::read(path)?, original);
        }
        Ok(())
    }

    /// 默认inspect没有实现纯读能力时，不能借may-write的normal load冒充只读。
    struct DefaultInspectionProbe {
        inner: FileRuntimeStateStore,
        loads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        commits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        normal_unknown: bool,
    }
    impl RuntimeStateStore for DefaultInspectionProbe {
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
                        run_id: "normal-original-run".into(),
                        execution_id: Some("normal-original-execution".into()),
                    });
                }
                self.inner
                    .load_runtime_state_with_context(ctx, scope, runtime)
                    .await
            })
        }
        fn commit_checkpoint_recovery_with_context<'a>(
            &'a self,
            ctx: PersistenceCallContext,
            request: crate::state::RuntimeCheckpointRecoveryRequest,
        ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeCheckpointCasReceipt>>
        {
            Box::pin(async move {
                self.commits
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.inner
                    .commit_checkpoint_recovery_with_context(ctx, request)
                    .await
            })
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
        ) -> BoxFuture<'a, crate::error::Result<crate::state::RuntimeStateScopeClearReceipt>>
        {
            self.inner.clear_runtime_state_scope(scope)
        }
        fn clear_conversation<'a>(
            &'a self,
            id: &'a str,
        ) -> BoxFuture<'a, crate::error::Result<()>> {
            self.inner.clear_conversation(id)
        }
    }
    struct CountedFactory(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl PreModelContextProjector for CountedFactory {
        fn project<'a>(
            &'a self,
            _: &'a ProjectionContext,
        ) -> BoxFuture<'a, crate::error::Result<Vec<ContextProjection>>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn recovered_tool_origins<'a>(
            &'a self,
            ctx: &'a ProjectionContext,
            messages: &'a [Message],
        ) -> BoxFuture<'a, crate::error::Result<Vec<RecoveredToolOrigin>>> {
            Box::pin(async move {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                KnownOldFactory(true, None)
                    .recovered_tool_origins(ctx, messages)
                    .await
            })
        }
    }

    #[tokio::test]
    async fn default_inspection_is_unsupported_without_load_source_or_commit()
    -> crate::error::Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let root = tempfile::tempdir()?;
        let (inner, path, raw) = old_owner(root.path(), false)?;
        let loads = Arc::new(AtomicUsize::new(0));
        let commits = Arc::new(AtomicUsize::new(0));
        let sources = Arc::new(AtomicUsize::new(0));
        let store = DefaultInspectionProbe {
            inner,
            loads: loads.clone(),
            commits: commits.clone(),
            normal_unknown: false,
        };
        let result = prepare_pending_control_recovery(
            &store,
            PersistenceCallContext::with_timeout(Duration::from_secs(1))?,
            "pending-runtime",
            &context(),
            &CountedFactory(sources.clone()),
            &crate::agent::CancellationToken::new(),
            None,
        )
        .await;
        assert!(
            matches!(result, Err(crate::error::ReactError::RuntimeState(error)) if matches!(error.as_ref(), echo_core::error::RuntimeStateError::Unsupported(_)))
        );
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert_eq!(sources.load(Ordering::SeqCst), 0);
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn explicit_readonly_adapter_preserves_file_inspection_without_normal_load()
    -> crate::error::Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let root = tempfile::tempdir()?;
        let (inner, path, raw) = old_owner(root.path(), false)?;
        let armed = Arc::new(AtomicBool::new(true));
        let adapter = AdmittedRecoveryStore {
            inner,
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
            completed: Arc::new(tokio::sync::Notify::new()),
            cancel_after_commit: None,
            normal_gate: Some(("runtime_load", armed.clone())),
        };
        let prepared = tokio::time::timeout(
            Duration::from_secs(1),
            prepare_pending_control_recovery(
                &adapter,
                PersistenceCallContext::with_timeout(Duration::from_secs(1))?,
                "pending-runtime",
                &context(),
                &KnownOldFactory(true, None),
                &crate::agent::CancellationToken::new(),
                None,
            ),
        )
        .await
        .map_err(|error| super::super::unavailable(error.to_string()))??;
        assert!(prepared.final_candidate.is_some());
        assert!(
            armed.load(Ordering::SeqCst),
            "显式纯读inspect不能进入normal load闸"
        );
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn normal_load_unknown_keeps_original_owner_before_inspection_fallback()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, ReactAgentBuilder};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct RecoveryDrop(Arc<AtomicUsize>);
        impl Drop for RecoveryDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let root = tempfile::tempdir()?;
        let (inner, path, raw) = old_owner(root.path(), false)?;
        let loads = Arc::new(AtomicUsize::new(0));
        let commits = Arc::new(AtomicUsize::new(0));
        let sources = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let store = DefaultInspectionProbe {
            inner,
            loads: loads.clone(),
            commits: commits.clone(),
            normal_unknown: true,
        };
        let model =
            Arc::new(crate::testing::MockLlmClient::new().with_response("must not call model"));
        let agent = ReactAgentBuilder::new()
            .conversation_id("pending-scope")
            .state_store(Arc::new(store))
            .llm_client(model.clone())
            .build()?;
        agent.set_pre_model_context_projector(Some(Arc::new(CountedFactory(sources.clone()))));
        let mut scoped = invocation();
        scoped
            .resource_guards
            .push(echo_core::tools::InvocationResourceGuard::new(
                RecoveryDrop(drops.clone()),
            ));
        let result = agent
            .execute_stream_with_invocation_context(
                "recovery input",
                crate::agent::CancellationToken::new(),
                scoped,
            )
            .await;
        let error = result
            .err()
            .ok_or_else(|| super::super::unavailable("normal unknown was swallowed by recovery"))?;
        assert!(
            matches!(error, crate::error::ReactError::ExecutionNotSettled { ref run_id, execution_id: Some(ref execution_id) } if run_id == "normal-original-run" && execution_id == "normal-original-execution")
        );
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(sources.load(Ordering::SeqCst), 0);
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(model.call_count(), 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(
            matches!(agent.close().await, Err(crate::error::ReactError::ExecutionNotSettled { ref run_id, execution_id: Some(ref execution_id) }) if run_id == "normal-original-run" && execution_id == "normal-original-execution")
        );
        assert!(matches!(
            agent.execute("next admission").await,
            Err(crate::error::ReactError::ExecutionNotSettled { .. })
        ));
        assert_eq!(std::fs::read(path)?, raw);
        Ok(())
    }

    #[tokio::test]
    async fn queued_explicit_resume_retains_original_legacy_resources_before_execution_lock()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, ReactAgentBuilder};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct QueuedResumeDrop(Arc<AtomicUsize>);
        impl Drop for QueuedResumeDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let root = tempfile::tempdir()?;
        let (inner, path, raw) = old_owner(root.path(), false)?;
        let loads = Arc::new(AtomicUsize::new(0));
        let commits = Arc::new(AtomicUsize::new(0));
        let sources = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let model = Arc::new(crate::testing::MockLlmClient::new());
        let agent = ReactAgentBuilder::new()
            .conversation_id("pending-scope")
            .state_store(Arc::new(DefaultInspectionProbe {
                inner,
                loads: loads.clone(),
                commits: commits.clone(),
                normal_unknown: true,
            }))
            .llm_client(model.clone())
            .build()?;
        agent.set_pre_model_context_projector(Some(Arc::new(CountedFactory(sources.clone()))));
        agent.set_external_context(&echo_core::tools::ExternalRunContext {
            run_id: Some("queued-original-run".into()),
            execution_id: Some("queued-original-execution".into()),
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                QueuedResumeDrop(drops.clone()),
            )],
            ..Default::default()
        });
        let execution = agent.execution_mutex.clone().lock_owned().await;
        let mut queued = Box::pin(agent.resume_from_state_store());
        assert!(futures::poll!(&mut queued).is_pending());
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        agent.clear_external_context();
        assert_eq!(
            drops.load(Ordering::SeqCst),
            0,
            "排队中的原Resume仍持有原资源"
        );
        drop(execution);
        let result = queued.await;
        assert!(
            matches!(result, Err(crate::error::ReactError::ExecutionNotSettled { ref run_id, execution_id: Some(ref execution_id) })
            if run_id == "normal-original-run" && execution_id == "normal-original-execution")
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(commits.load(Ordering::SeqCst), 0);
        assert_eq!(sources.load(Ordering::SeqCst), 0);
        assert_eq!(model.call_count(), 0);
        assert!(
            matches!(agent.close().await, Err(crate::error::ReactError::ExecutionNotSettled { ref run_id, .. }) if run_id == "normal-original-run")
        );
        assert!(
            matches!(agent.resume_from_state_store().await, Err(crate::error::ReactError::ExecutionNotSettled { ref run_id, .. }) if run_id == "normal-original-run")
        );
        assert_eq!(std::fs::read(path)?, raw);
        drop(agent);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
