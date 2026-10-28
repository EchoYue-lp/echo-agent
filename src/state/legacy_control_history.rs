//! 旧内置控制工具的只读模型投影；原日志、工具执行和终态 owner 不变。
//!
//! 普通旧 checkpoint 没有逐调用注册来源，不能靠名称/schema 猜测内置身份。
//! 本模块不注册工具、不执行调用、不发布 final，也不回写 checkpoint。

pub(crate) mod pending_recovery;

use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use sha2::{Digest, Sha256};

use crate::llm::types::{Message, MessageContent, Role};

#[cfg(test)]
const AUDITED_BUILTIN_CONTRACT: &str = "0e404185c8f252da3e335c35547e52249c74dc65";
// 旧EKO制品源清单已核同一builtin参数/注册合同；不是从消息文本猜版本。
#[cfg(test)]
const AUDITED_BUILTIN_CONTRACTS: &[&str] = &[
    AUDITED_BUILTIN_CONTRACT,
    "f3c5af6443fff36dd7088dcb32a665b5fc173489",
    "69296a4dea1047be7fb91b7ab06a839ee57dda7e",
];
const THINK_SOURCE: &str = "71de9ca6f50eace5d7bddd7780203db601d0d84c08859911fd56433410c3068c";
const FINAL_SOURCE: &str = "8c81725b867972cc72c0597687a9a78149f8dcedde3aaa35e98f7ebf609ba9a8";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetiredBuiltin {
    Think,
    FinalAnswer,
}

impl RetiredBuiltin {
    fn name(self) -> &'static str {
        match self {
            Self::Think => "think",
            Self::FinalAnswer => "final_answer",
        }
    }

    fn parameter(self) -> &'static str {
        match self {
            Self::Think => "reasoning",
            Self::FinalAnswer => "answer",
        }
    }

    fn source_sha256(self) -> &'static str {
        match self {
            Self::Think => THINK_SOURCE,
            Self::FinalAnswer => FINAL_SOURCE,
        }
    }
}

/// 仅由原归档/恢复 owner 在核对真实注册来源之后构造；不能 serde 反序列化。
/// producer/blob/schema 是补充核验，不单独证明同名工具来自内置注册。
pub(crate) struct VerifiedLegacyBuiltinCall {
    pub(crate) call_id: String,
    pub(crate) builtin: RetiredBuiltin,
    pub(crate) registered_builtin_source_sha256: String,
    pub(crate) final_already_published: bool,
}

/// 私有信任边界：调用方须已核原归档中的逐call注册来源及发布receipt。
/// 合同revision标识已核实现规则，不声称是旧消息从未保存的producer字段。
/// EKO原owner可据已知默认factory合同自动构造；普通模型参数/provider_source不能构造此值。
/// 本候选没有自动从旧JSON构造证明的fallback。
#[cfg(test)]
pub(crate) struct VerifiedLegacySnapshot<'a> {
    pub(crate) runtime_state_id: &'a str,
    pub(crate) original_checkpoint_sha256: &'a str,
    pub(crate) builtin_contract_revision: &'a str,
    pub(crate) original_payload_sha256: &'a str,
    pub(crate) builtin_calls: &'a [VerifiedLegacyBuiltinCall],
}

pub(crate) struct LegacyControlProjection {
    pub(crate) messages: Vec<Message>,
    /// 仅在没有未结算真实调用且未曾发布该final时给出，继续经过原最终校验链。
    pub(crate) final_candidate: Option<String>,
    /// 只描述原消息中仍未结算的真实调用，不能据此重放已有副作用。
    pub(crate) pending_real_call_ids: Vec<String>,
}

fn unavailable(reason: impl Into<String>) -> crate::error::ReactError {
    super::invalid_checkpoint(format!(
        "旧控制工具恢复暂停，原始记录已保留：{}",
        reason.into()
    ))
}

/// 实际请求边界：仅接收已由原恢复/reconcile校验、原prepare形成的模型消息副本。
/// 无来源保原样；来源错误明确失败，不能在optional投影失败时默默丢失必需历史。
/// 不设置ContextManager、不写checkpoint、不执行或重放任何call。
pub(crate) async fn project_recovered_model_messages(
    projector: Option<&dyn crate::compression::PreModelContextProjector>,
    context: &crate::compression::ProjectionContext,
    messages: Vec<Message>,
) -> crate::error::Result<Vec<Message>> {
    let Some(projector) = projector else {
        return Ok(messages);
    };
    let sources = projector.recovered_tool_origins(context, &messages).await?;
    if sources.is_empty() {
        return Ok(messages);
    }
    let verified = resolve_legacy_origins(&messages, sources)?;
    if verified.is_empty() {
        return Ok(messages);
    }
    super::validate_tool_message_pairing(&messages).map_err(|error| {
        unavailable(format!(
            "原恢复仍有未配对调用，模型副本不能补结果或重放副作用：{error}"
        ))
    })?;
    let projected = project_verified_control_messages(&messages, &verified)?;
    if projected.final_candidate.is_some() || !projected.pending_real_call_ids.is_empty() {
        return Err(unavailable("请求历史投影不能签发旧final或启动未结算工具"));
    }
    Ok(projected.messages)
}

fn resolve_legacy_origins(
    messages: &[Message],
    sources: Vec<crate::compression::RecoveredToolOrigin>,
) -> crate::error::Result<Vec<VerifiedLegacyBuiltinCall>> {
    let calls = messages
        .iter()
        .filter_map(|message| message.tool_calls.as_ref())
        .flatten()
        .map(|call| call.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut verified = Vec::new();
    for source in sources {
        if !calls.contains(source.call_id.as_str()) || !seen.insert(source.call_id.clone()) {
            return Err(unavailable("恢复来源不在本次历史内或同一call来源重复"));
        }
        let crate::compression::RecoveredToolKind::FrameworkBuiltin {
            definition_source_sha256,
        } = source.kind
        else {
            continue;
        };
        let builtin = match definition_source_sha256.as_str() {
            THINK_SOURCE => RetiredBuiltin::Think,
            FINAL_SOURCE => RetiredBuiltin::FinalAnswer,
            _ => return Err(unavailable("未核验的旧内置工具来源定义")),
        };
        verified.push(VerifiedLegacyBuiltinCall {
            call_id: source.call_id,
            builtin,
            registered_builtin_source_sha256: definition_source_sha256,
            // 此边界只转换已配对历史，绝不从普通已完成call重新形成final候选。
            final_already_published: false,
        });
    }
    Ok(verified)
}

/// 复用原checkpoint唯一restore_payload解码；原字节用于绑定来源。
/// projection绝不能替换耐久cursor/CAS校验的原输入。
#[cfg(test)]
pub(crate) fn project_legacy_control_history(
    checkpoint: &super::AgentCheckpoint,
    proof: Option<&VerifiedLegacySnapshot<'_>>,
) -> crate::error::Result<LegacyControlProjection> {
    let Some(proof) = proof else {
        // 无证明不改历史。沿原pairing拒绝pending，错误只解释缺少何种来源，
        // 不是对新执行或用户自定义同名工具的名称拦截。
        let restored = checkpoint.restore_runtime_payload().map_err(|error| {
            unavailable(format!(
                "缺少旧producer及逐call内置注册来源，无法归一化未结算历史：{error}"
            ))
        })?;
        return Ok(LegacyControlProjection {
            messages: restored.messages,
            final_candidate: None,
            pending_real_call_ids: Vec::new(),
        });
    };
    let payload = checkpoint.restore_payload()?;
    if payload.transcript_projection.is_some() || payload.pending_transcript_projection.is_some() {
        // 合法已结算managed历史先由原完整恢复校验；没有control调用也照常通过。
        // 未结算真实调用/投递债务仍原样阻断；这里只变模型副本，绝不重写cursor。
        checkpoint.restore_runtime_payload().map_err(|error| {
            unavailable(format!("原managed恢复尚未结算，不可改写cursor：{error}"))
        })?;
    }
    let messages = &payload.messages;
    let original_payload = &checkpoint.messages_json;
    let actual_sha256 = format!("{:x}", Sha256::digest(original_payload.as_bytes()));
    let checkpoint_bytes =
        serde_json::to_vec(checkpoint).map_err(|error| unavailable(error.to_string()))?;
    let checkpoint_sha256 = format!("{:x}", Sha256::digest(checkpoint_bytes));
    if proof.runtime_state_id != checkpoint.conversation_id
        || proof.original_checkpoint_sha256 != checkpoint_sha256
        || !AUDITED_BUILTIN_CONTRACTS.contains(&proof.builtin_contract_revision)
        || proof.original_payload_sha256 != actual_sha256
    {
        return Err(unavailable(
            "已核builtin合同版本或原始快照SHA与来源适配不一致",
        ));
    }
    project_verified_control_messages(messages, proof.builtin_calls)
}

fn project_verified_control_messages(
    messages: &[Message],
    builtin_calls: &[VerifiedLegacyBuiltinCall],
) -> crate::error::Result<LegacyControlProjection> {
    let mut origins = BTreeMap::new();
    for origin in builtin_calls {
        if origin.call_id.trim().is_empty()
            || origin.registered_builtin_source_sha256 != origin.builtin.source_sha256()
            || (origin.builtin == RetiredBuiltin::Think && origin.final_already_published)
            || origins.insert(origin.call_id.as_str(), origin).is_some()
        {
            return Err(unavailable("内置调用来源重复、为空或与原blob合同不一致"));
        }
    }

    // 来源引用须唯一；无关真实/custom调用沿原“仅in-flight唯一”合同，
    // 前一结果已结算后复用call_id不能被历史适配无谓阻断。
    for id in origins.keys() {
        let count = messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .filter_map(|message| message.tool_calls.as_ref())
            .flatten()
            .filter(|call| call.id == *id)
            .count();
        if count != 1 {
            return Err(unavailable("来源引用的调用不唯一或不在原assistant消息内"));
        }
    }
    let mut pending = BTreeMap::new();
    let mut retired_ids = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut any_final_published = false;
    let mut projected = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let mut copy = message.clone();
        let mut notes = Vec::new();
        if message.role == Role::Assistant
            && let Some(calls) = message.tool_calls.as_ref()
        {
            let mut retained = Vec::new();
            for call in calls {
                if pending
                    .insert(call.id.clone(), call.function.name.clone())
                    .is_some()
                {
                    return Err(unavailable("同一in-flight调用ID重复，无法唯一关联来源"));
                }
                let Some(origin) = origins.remove(call.id.as_str()) else {
                    retained.push(call.clone());
                    continue;
                };
                if call.call_type != "function" || call.function.name != origin.builtin.name() {
                    return Err(unavailable("调用名字/type与已证明内置来源不一致"));
                }
                let args: serde_json::Value = serde_json::from_str(&call.function.arguments)
                    .map_err(|error| unavailable(format!("旧内置参数JSON无效：{error}")))?;
                let text = args
                    .as_object()
                    .and_then(|object| object.get(origin.builtin.parameter()))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| unavailable("旧内置必填字符串参数无效"))?;
                retired_ids.insert(call.id.clone());
                notes.push(format!("[历史执行说明：{}]\n{text}", origin.builtin.name()));
                if origin.builtin == RetiredBuiltin::FinalAnswer && !origin.final_already_published
                {
                    candidates.push((call.id.clone(), text.to_string(), message_index));
                }
                any_final_published |= origin.final_already_published;
            }
            copy.tool_calls = (!retained.is_empty()).then_some(retained);
        }
        if message.role == Role::Tool {
            let id = message
                .tool_call_id
                .as_deref()
                .ok_or_else(|| unavailable("工具结果缺少call_id"))?;
            let name = pending
                .remove(id)
                .ok_or_else(|| unavailable("孤儿或重复工具结果，不能补造成功记录"))?;
            if message.name.as_deref() != Some(name.as_str()) {
                return Err(unavailable("工具结果名字与原调用不符"));
            }
            if retired_ids.contains(id) {
                // 原结果也保作历史说明，不能把当时的拒绝/错误静默改成参数文本。
                // 不保留已撤tool的wire result，也不为未执行call补任何成功记录。
                let MessageContent::Text(text) = &message.content else {
                    return Err(unavailable(
                        "旧控制工具结果含非文本内容，保留原件等待明确适配",
                    ));
                };
                copy =
                    Message::assistant(format!("[历史执行说明：{} 的原始工具结果]\n{text}", name));
            }
        }
        if !notes.is_empty() {
            let mut parts = Vec::new();
            match &copy.content {
                MessageContent::Text(text) => {
                    if !text.is_empty() {
                        parts.push(text.clone());
                    }
                }
                MessageContent::Empty => {}
                MessageContent::Parts(_) => {
                    return Err(unavailable("旧控制消息含非文本内容，保留原件等待明确适配"));
                }
            }
            parts.extend(notes);
            copy.content = MessageContent::Text(parts.join("\n\n"));
            // 只追加历史说明，不把旧工具参数写成provider reasoning或native phase。
        }
        projected.push(copy);
    }
    if !origins.is_empty() {
        return Err(unavailable("来源证明包含原快照不存在的调用"));
    }
    let pending_real_call_ids = pending
        .keys()
        .filter(|id| !retired_ids.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    // 仅原恢复frontier最后一条消息中的未执行候选可进入当前校验链。
    // 已有tool结果可能是拒绝/失败/改参，不能由原参数重造最终回答；
    // 后继用户输入或模型推进也不能被旧候选越过。
    let last_message = messages.len().checked_sub(1);
    candidates.retain(|(id, _, index)| pending.contains_key(id) && Some(*index) == last_message);
    let final_candidate =
        if !any_final_published && pending_real_call_ids.is_empty() && candidates.len() == 1 {
            candidates.pop().map(|(_, text, _)| text)
        } else {
            None
        };
    if pending_real_call_ids.is_empty() {
        super::validate_tool_message_pairing(&projected)?;
    }
    Ok(LegacyControlProjection {
        messages: projected,
        final_candidate,
        pending_real_call_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{FunctionCall, ToolCall};

    struct OriginsProjector {
        origins: Vec<crate::compression::RecoveredToolOrigin>,
    }

    impl crate::compression::PreModelContextProjector for OriginsProjector {
        fn project<'a>(
            &'a self,
            _: &'a crate::compression::ProjectionContext,
        ) -> futures::future::BoxFuture<
            'a,
            crate::error::Result<Vec<crate::compression::ContextProjection>>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn recovered_tool_origins<'a>(
            &'a self,
            _: &'a crate::compression::ProjectionContext,
            _: &'a [Message],
        ) -> futures::future::BoxFuture<
            'a,
            crate::error::Result<Vec<crate::compression::RecoveredToolOrigin>>,
        > {
            Box::pin(async { Ok(self.origins.clone()) })
        }
    }

    fn projection_context() -> crate::compression::ProjectionContext {
        crate::compression::ProjectionContext {
            iteration: 0,
            agent_name: "test".to_string(),
            session_id: None,
            conversation_id: Some("original-runtime".to_string()),
            run_id: None,
            turn_id: None,
        }
    }

    fn paired_final_history() -> Vec<Message> {
        vec![
            assistant(vec![call(
                "old-final",
                "final_answer",
                serde_json::json!({"answer":"原候选"}),
            )]),
            Message::tool_result(
                "old-final".to_string(),
                "final_answer".to_string(),
                "原实际结果".to_string(),
            ),
        ]
    }

    fn final_origins() -> OriginsProjector {
        OriginsProjector {
            origins: vec![crate::compression::RecoveredToolOrigin {
                call_id: "old-final".to_string(),
                kind: crate::compression::RecoveredToolKind::FrameworkBuiltin {
                    definition_source_sha256: FINAL_SOURCE.to_string(),
                },
            }],
        }
    }

    #[tokio::test]
    async fn runtime_origin_projects_model_copy_and_preserves_the_original_messages()
    -> crate::error::Result<()> {
        let messages = paired_final_history();
        let original =
            serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?;
        let projected = project_recovered_model_messages(
            Some(&final_origins()),
            &projection_context(),
            messages.clone(),
        )
        .await?;
        assert!(
            projected
                .iter()
                .all(|message| message.tool_calls.is_none() && message.role != Role::Tool)
        );
        assert!(projected.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("原实际结果"))
        }));
        assert_eq!(
            serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?,
            original
        );
        Ok(())
    }

    #[tokio::test]
    async fn empty_or_custom_origin_keeps_same_named_history_unchanged() -> crate::error::Result<()>
    {
        let original = paired_final_history();
        let custom = OriginsProjector {
            origins: vec![crate::compression::RecoveredToolOrigin {
                call_id: "old-final".to_string(),
                kind: crate::compression::RecoveredToolKind::Custom,
            }],
        };
        for projector in [
            None,
            Some(&custom as &dyn crate::compression::PreModelContextProjector),
        ] {
            let projected = project_recovered_model_messages(
                projector,
                &projection_context(),
                original.clone(),
            )
            .await?;
            assert_eq!(
                serde_json::to_value(&projected).map_err(|error| unavailable(error.to_string()))?,
                serde_json::to_value(&original).map_err(|error| unavailable(error.to_string()))?
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn origin_from_another_call_or_unknown_definition_never_inherits_projection()
    -> crate::error::Result<()> {
        for (id, hash) in [
            ("foreign-call", FINAL_SOURCE),
            ("old-final", "unknown-source"),
        ] {
            let projector = OriginsProjector {
                origins: vec![crate::compression::RecoveredToolOrigin {
                    call_id: id.to_string(),
                    kind: crate::compression::RecoveredToolKind::FrameworkBuiltin {
                        definition_source_sha256: hash.to_string(),
                    },
                }],
            };
            assert!(
                project_recovered_model_messages(
                    Some(&projector),
                    &projection_context(),
                    paired_final_history()
                )
                .await
                .is_err()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn pending_tool_or_control_is_not_replayed_or_given_a_synthetic_result()
    -> crate::error::Result<()> {
        let messages = vec![assistant(vec![
            call(
                "old-final",
                "final_answer",
                serde_json::json!({"answer":"未结算"}),
            ),
            call(
                "effect",
                "write_file",
                serde_json::json!({"path":"original"}),
            ),
        ])];
        assert!(
            project_recovered_model_messages(
                Some(&final_origins()),
                &projection_context(),
                messages
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn unrelated_completed_real_and_custom_calls_can_reuse_their_ids()
    -> crate::error::Result<()> {
        for name in ["read_file", "custom"] {
            let mut messages = Vec::new();
            for result in ["first-result", "second-result"] {
                messages.push(assistant(vec![call(
                    "reusable",
                    name,
                    serde_json::json!({}),
                )]));
                messages.push(Message::tool_result(
                    "reusable".to_string(),
                    name.to_string(),
                    result.to_string(),
                ));
            }
            let prefix =
                serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?;
            messages.extend(paired_final_history());
            let projected = project_recovered_model_messages(
                Some(&final_origins()),
                &projection_context(),
                messages,
            )
            .await?;
            assert_eq!(
                serde_json::to_value(projected.get(..4))
                    .map_err(|error| unavailable(error.to_string()))?,
                prefix
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn real_request_uses_origin_projection_without_rewriting_durable_context()
    -> crate::error::Result<()> {
        use crate::agent::{Agent, ReactAgentBuilder};
        use crate::state::RuntimeStateStore;
        use futures::StreamExt;
        use std::sync::Arc;
        let llm = Arc::new(crate::testing::MockLlmClient::new().with_response("当前回答"));
        let directory = tempfile::tempdir()?;
        let store = Arc::new(crate::state::FileRuntimeStateStore::new(directory.path())?);
        let mut checkpoint = crate::state::AgentCheckpoint::new("original-runtime");
        checkpoint.messages_json = serde_json::to_string(&paired_final_history())
            .map_err(|error| unavailable(error.to_string()))?;
        store.save_checkpoint(&checkpoint).await?;
        let agent = ReactAgentBuilder::new()
            .llm_client(llm.clone())
            .conversation_id("original-runtime")
            .state_store(store)
            .build()?;
        agent.set_pre_model_context_projector(Some(Arc::new(final_origins())));
        let raw = agent
            .execute_stream("当前用户要求")
            .await?
            .collect::<Vec<_>>()
            .await;
        raw.into_iter().collect::<crate::error::Result<Vec<_>>>()?;
        assert_eq!(llm.call_count(), 1);
        let sent = llm
            .last_messages()
            .ok_or_else(|| unavailable("真实模型请求未触达"))?;
        assert!(sent.iter().all(|message| {
            message
                .tool_calls
                .as_ref()
                .is_none_or(|calls| calls.iter().all(|call| call.id != "old-final"))
        }));
        assert!(sent.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("历史执行说明") && text.contains("原候选"))
        }));
        let original = agent.get_messages().await;
        assert!(
            original
                .iter()
                .any(|message| message.tool_call_id.as_deref() == Some("old-final"))
        );
        Ok(())
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            provider_source: None,
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    fn assistant(calls: Vec<ToolCall>) -> Message {
        let mut message = Message::assistant("原工具前说明🙂".to_string());
        message.tool_calls = Some(calls);
        message
    }

    fn origin(id: &str, builtin: RetiredBuiltin) -> VerifiedLegacyBuiltinCall {
        VerifiedLegacyBuiltinCall {
            call_id: id.to_string(),
            builtin,
            registered_builtin_source_sha256: builtin.source_sha256().to_string(),
            final_already_published: false,
        }
    }

    fn project(
        messages: &[Message],
        calls: &[VerifiedLegacyBuiltinCall],
    ) -> crate::error::Result<LegacyControlProjection> {
        let payload =
            serde_json::to_string(messages).map_err(|error| unavailable(error.to_string()))?;
        let sha = format!("{:x}", Sha256::digest(payload.as_bytes()));
        let mut checkpoint = super::super::AgentCheckpoint::new("legacy-fixture");
        checkpoint.messages_json = payload;
        let checkpoint_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&checkpoint).map_err(|error| unavailable(error.to_string()))?
            )
        );
        project_legacy_control_history(
            &checkpoint,
            Some(&VerifiedLegacySnapshot {
                runtime_state_id: &checkpoint.conversation_id,
                original_checkpoint_sha256: &checkpoint_sha256,
                builtin_contract_revision: AUDITED_BUILTIN_CONTRACT,
                original_payload_sha256: &sha,
                builtin_calls: calls,
            }),
        )
    }

    #[test]
    fn proven_pending_think_becomes_history_note_without_result_or_reasoning()
    -> crate::error::Result<()> {
        let messages = vec![assistant(vec![call(
            "t",
            "think",
            serde_json::json!({"reasoning":"检查中文🙂"}),
        )])];
        let before =
            serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?;
        let projection = project(&messages, &[origin("t", RetiredBuiltin::Think)])?;
        let first = projection
            .messages
            .first()
            .ok_or_else(|| unavailable("缺投影"))?;
        assert!(first.tool_calls.is_none());
        assert!(
            first
                .content
                .as_text()
                .is_some_and(|text| text.contains("历史执行说明") && text.contains("检查中文🙂"))
        );
        assert!(first.reasoning_content.is_none());
        assert!(first.assistant_blocks.is_none());
        assert!(
            projection
                .messages
                .iter()
                .all(|message| message.role != Role::Tool)
        );
        assert!(projection.final_candidate.is_none());
        assert_eq!(
            serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?,
            before
        );
        Ok(())
    }

    #[test]
    fn proven_final_is_only_candidate_not_runtime_confirmation() -> crate::error::Result<()> {
        let projection = project(
            &[assistant(vec![call(
                "f",
                "final_answer",
                serde_json::json!({"answer":"待校验回答"}),
            )])],
            &[origin("f", RetiredBuiltin::FinalAnswer)],
        )?;
        assert_eq!(projection.final_candidate.as_deref(), Some("待校验回答"));
        assert!(
            projection
                .messages
                .iter()
                .all(|message| message.execution_status.is_none())
        );
        assert!(projection.pending_real_call_ids.is_empty());
        Ok(())
    }

    #[test]
    fn pending_real_call_blocks_final_candidate_and_completed_side_effect_stays_exact()
    -> crate::error::Result<()> {
        let messages = vec![
            assistant(vec![call(
                "done",
                "write_file",
                serde_json::json!({"path":"a"}),
            )]),
            Message::tool_result(
                "done".to_string(),
                "write_file".to_string(),
                "durable-result".to_string(),
            ),
            assistant(vec![
                call(
                    "f",
                    "final_answer",
                    serde_json::json!({"answer":"尚不能结束"}),
                ),
                call("pending", "read_file", serde_json::json!({"path":"a"})),
            ]),
        ];
        let projection = project(&messages, &[origin("f", RetiredBuiltin::FinalAnswer)])?;
        assert!(projection.final_candidate.is_none());
        assert_eq!(
            projection.pending_real_call_ids,
            vec!["pending".to_string()]
        );
        assert_eq!(
            serde_json::to_value(messages.get(..2))
                .map_err(|error| unavailable(error.to_string()))?,
            serde_json::to_value(projection.messages.get(..2))
                .map_err(|error| unavailable(error.to_string()))?
        );
        assert_eq!(
            projection
                .messages
                .iter()
                .filter(|message| message.role == Role::Tool)
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn completed_final_is_history_only_and_never_republished() -> crate::error::Result<()> {
        let mut published = origin("f", RetiredBuiltin::FinalAnswer);
        published.final_already_published = true;
        let projection = project(
            &[
                assistant(vec![call(
                    "f",
                    "final_answer",
                    serde_json::json!({"answer":"已发回答"}),
                )]),
                Message::tool_result(
                    "f".to_string(),
                    "final_answer".to_string(),
                    "已发回答".to_string(),
                ),
            ],
            &[published],
        )?;
        assert!(projection.final_candidate.is_none());
        assert!(
            projection
                .messages
                .iter()
                .all(|message| message.role != Role::Tool)
        );
        Ok(())
    }

    #[test]
    fn unknown_pending_control_pauses_and_completed_same_named_custom_history_is_untouched()
    -> crate::error::Result<()> {
        for name in ["think", "final_answer"] {
            let messages = vec![assistant(vec![call(
                "custom",
                name,
                serde_json::json!({"user_argument":"keep"}),
            )])];
            let payload =
                serde_json::to_string(&messages).map_err(|error| unavailable(error.to_string()))?;
            let mut checkpoint = super::super::AgentCheckpoint::new("unknown-fixture");
            checkpoint.messages_json = payload;
            let error = project_legacy_control_history(&checkpoint, None)
                .err()
                .ok_or_else(|| unavailable("未知pending意外恢复"))?;
            assert!(error.to_string().contains("逐call内置注册来源"));
            let mut completed = messages;
            completed.push(Message::tool_result(
                "custom".to_string(),
                name.to_string(),
                "用户自定义结果".to_string(),
            ));
            let raw = serde_json::to_string(&completed)
                .map_err(|error| unavailable(error.to_string()))?;
            checkpoint.messages_json = raw;
            let projection = project_legacy_control_history(&checkpoint, None)?;
            assert_eq!(
                serde_json::to_value(&projection.messages)
                    .map_err(|error| unavailable(error.to_string()))?,
                serde_json::to_value(&completed).map_err(|error| unavailable(error.to_string()))?
            );
            assert!(projection.final_candidate.is_none());
        }
        Ok(())
    }

    #[test]
    fn proof_version_payload_and_builtin_source_are_all_bound() -> crate::error::Result<()> {
        let messages = vec![assistant(vec![call(
            "f",
            "final_answer",
            serde_json::json!({"answer":"候选"}),
        )])];
        let payload =
            serde_json::to_string(&messages).map_err(|error| unavailable(error.to_string()))?;
        let sha = format!("{:x}", Sha256::digest(payload.as_bytes()));
        let origins = [origin("f", RetiredBuiltin::FinalAnswer)];
        let mut checkpoint = super::super::AgentCheckpoint::new("proof-fixture");
        checkpoint.messages_json = payload;
        let checkpoint_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&checkpoint).map_err(|error| unavailable(error.to_string()))?
            )
        );
        for (version, hash) in [
            ("unknown-version", sha.as_str()),
            (AUDITED_BUILTIN_CONTRACT, "wrong-sha"),
        ] {
            assert!(
                project_legacy_control_history(
                    &checkpoint,
                    Some(&VerifiedLegacySnapshot {
                        runtime_state_id: &checkpoint.conversation_id,
                        original_checkpoint_sha256: &checkpoint_sha256,
                        builtin_contract_revision: version,
                        original_payload_sha256: hash,
                        builtin_calls: &origins
                    })
                )
                .is_err()
            );
        }
        assert!(
            project_legacy_control_history(
                &checkpoint,
                Some(&VerifiedLegacySnapshot {
                    runtime_state_id: "foreign-runtime",
                    original_checkpoint_sha256: &checkpoint_sha256,
                    builtin_contract_revision: AUDITED_BUILTIN_CONTRACT,
                    original_payload_sha256: &sha,
                    builtin_calls: &origins,
                })
            )
            .is_err()
        );
        assert!(
            project_legacy_control_history(
                &checkpoint,
                Some(&VerifiedLegacySnapshot {
                    runtime_state_id: &checkpoint.conversation_id,
                    original_checkpoint_sha256: "wrong-checkpoint-sha",
                    builtin_contract_revision: AUDITED_BUILTIN_CONTRACT,
                    original_payload_sha256: &sha,
                    builtin_calls: &origins,
                })
            )
            .is_err()
        );
        let mut wrong = origin("f", RetiredBuiltin::FinalAnswer);
        wrong.registered_builtin_source_sha256 = THINK_SOURCE.to_string();
        assert!(project(&messages, &[wrong]).is_err());
        Ok(())
    }

    #[test]
    fn published_final_suppresses_other_old_candidates_and_keeps_control_error_note()
    -> crate::error::Result<()> {
        let messages = vec![
            assistant(vec![call(
                "early",
                "final_answer",
                serde_json::json!({"answer":"旧候选"}),
            )]),
            Message::tool_result(
                "early".to_string(),
                "final_answer".to_string(),
                "Guard refused old candidate".to_string(),
            ),
            assistant(vec![call(
                "published",
                "final_answer",
                serde_json::json!({"answer":"后来已发布"}),
            )]),
            Message::tool_result(
                "published".to_string(),
                "final_answer".to_string(),
                "后来已发布".to_string(),
            ),
        ];
        let mut published = origin("published", RetiredBuiltin::FinalAnswer);
        published.final_already_published = true;
        let projection = project(
            &messages,
            &[origin("early", RetiredBuiltin::FinalAnswer), published],
        )?;
        assert!(projection.final_candidate.is_none());
        assert!(projection.messages.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("Guard refused old candidate"))
        }));
        assert!(
            projection
                .messages
                .iter()
                .all(|message| message.role != Role::Tool)
        );
        Ok(())
    }

    #[test]
    fn completed_or_superseded_final_never_reuses_the_old_argument_as_candidate()
    -> crate::error::Result<()> {
        let pending = assistant(vec![call(
            "f",
            "final_answer",
            serde_json::json!({"answer":"原参数A"}),
        )]);
        for result in ["[Error] denied", "用户改参后的B"] {
            let messages = vec![
                pending.clone(),
                Message::tool_result(
                    "f".to_string(),
                    "final_answer".to_string(),
                    result.to_string(),
                ),
            ];
            let projection = project(&messages, &[origin("f", RetiredBuiltin::FinalAnswer)])?;
            assert!(projection.final_candidate.is_none());
            assert!(projection.messages.iter().any(|message| {
                message
                    .content
                    .as_text()
                    .is_some_and(|text| text.contains(result))
            }));
        }
        for later in [
            Message::user("用户后继约束".to_string()),
            Message::assistant("模型后继推进".to_string()),
        ] {
            let projection = project(
                &[pending.clone(), later],
                &[origin("f", RetiredBuiltin::FinalAnswer)],
            )?;
            assert!(projection.final_candidate.is_none());
        }
        Ok(())
    }

    #[test]
    fn multimodal_control_content_is_rejected_without_lossy_text_extraction()
    -> crate::error::Result<()> {
        let mut message = assistant(vec![call(
            "t",
            "think",
            serde_json::json!({"reasoning":"旧说明"}),
        )]);
        message.content = serde_json::from_value(serde_json::json!([
            {"type":"text","text":"原始可见文本"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}
        ]))
        .map_err(|error| unavailable(error.to_string()))?;
        let messages = [message];
        let before =
            serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?;
        assert!(project(&messages, &[origin("t", RetiredBuiltin::Think)]).is_err());
        assert_eq!(
            serde_json::to_value(&messages).map_err(|error| unavailable(error.to_string()))?,
            before
        );
        Ok(())
    }

    #[test]
    fn wrong_args_duplicate_call_and_orphan_result_preserve_original_error()
    -> crate::error::Result<()> {
        let wrong = [assistant(vec![call(
            "f",
            "final_answer",
            serde_json::json!({"content":"不能猜字段"}),
        )])];
        assert!(project(&wrong, &[origin("f", RetiredBuiltin::FinalAnswer)]).is_err());
        let duplicate = [assistant(vec![
            call("f", "final_answer", serde_json::json!({"answer":"一"})),
            call("f", "final_answer", serde_json::json!({"answer":"二"})),
        ])];
        assert!(project(&duplicate, &[origin("f", RetiredBuiltin::FinalAnswer)]).is_err());
        assert!(
            project(
                &[Message::tool_result(
                    "orphan".to_string(),
                    "write_file".to_string(),
                    "原件".to_string()
                )],
                &[]
            )
            .is_err()
        );
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
                .map_err(|error| unavailable(format!("自定义工具正控未结算：{error}")))?;
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
}
