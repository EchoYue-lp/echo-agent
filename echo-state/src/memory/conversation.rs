//! Conversation persistence — concrete implementations and helper functions
//!
//! Trait definition and data types live in [`echo_core::memory::conversation`].
//! This module provides `project_messages`/`project_message` helpers.

use echo_core::error::{MemoryError, Result};
use echo_core::llm::types::{Message, MessageContent, Role, ToolCall};
pub use echo_core::memory::conversation::StoredMessage;
use sha2::{Digest, Sha256};

const MESSAGE_PROJECTION_VERSION: u8 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct MessageProjectionMeta {
    #[serde(rename = "_echo_message_version")]
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<MessageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    /// 确认的是Message.content；provider原件phase/complete不能替代该运行事实。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_status: Option<echo_core::agent::ExecutionStatus>,
    /// 已通过输出校验的呈现投影；Some(empty)禁止回退raw块。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approved_assistant_blocks: Option<Vec<echo_core::agent::AssistantMessageBlock>>,
    /// 原生签名/opaque状态及消息身份须与正文一起保存，恢复时不可重新猜测。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_blocks: Option<Vec<echo_core::llm::types::ReasoningBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assistant_blocks: Option<Vec<echo_core::agent::AssistantMessageBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_result_is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_response: Option<echo_core::llm::types::ProviderResponseMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_generation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript_ordinal: Option<u64>,
}

/// Typed append identity for one product transcript message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptProjectionMeta {
    pub generation_id: String,
    pub ordinal: u64,
}

/// Stable generation-cursor fingerprint for one user-visible transcript row.
///
/// Backend ids, timestamps, and generation/ordinal metadata are excluded so
/// the active cursor can match the same logical message before and after a
/// projection batch is prepared.
pub fn transcript_projection_message_digest(message: &StoredMessage) -> Result<String> {
    let normalized = serde_json::to_vec(&(
        &message.role,
        &message.content,
        normalized_transcript_attachments(message)?,
        &message.tool_calls_json,
        &message.tool_result_json,
    ))?;
    Ok(format!("{:x}", Sha256::digest(normalized)))
}

/// Project runtime Message list to persistable transcript records.
pub fn project_messages(conversation_id: &str, messages: &[Message]) -> Result<Vec<StoredMessage>> {
    messages
        .iter()
        .map(|message| project_message(conversation_id, message))
        .collect()
}

/// Digest frontier shared by a generation-bound import and its runtime cursor.
/// Canonical round-tripping removes backend IDs and application-only metadata.
pub fn managed_import_projection_digests(
    conversation_id: &str,
    messages: &[StoredMessage],
) -> Result<Vec<String>> {
    let restored = restore_messages(messages)?;
    if restored.iter().any(|message| message.role == Role::System) {
        return Err(MemoryError::SerializationError(
            "managed import contains a system message outside the visible transcript".to_string(),
        )
        .into());
    }
    project_messages(conversation_id, &restored)?
        .iter()
        .map(transcript_projection_message_digest)
        .collect()
}

/// Project a single runtime Message to a transcript record.
pub fn project_message(conversation_id: &str, message: &Message) -> Result<StoredMessage> {
    let tool_calls_json = message
        .tool_calls
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    let tool_result_json = if message.role == Role::Tool {
        let tool_call_id = message
            .tool_call_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                MemoryError::SerializationError(
                    "tool message is missing its explicit tool_call_id".to_string(),
                )
            })?;
        let name = message
            .name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                MemoryError::SerializationError(
                    "tool message is missing its explicit name".to_string(),
                )
            })?;
        Some(
            serde_json::json!({
                "tool_call_id": tool_call_id,
                "name": name,
            })
            .to_string(),
        )
    } else {
        None
    };

    // `content` is the searchable text projection. Every canonical runtime
    // message carries the marker, including plain text, so application
    // projections can reliably distinguish an Agent-owned transcript from a
    // UI-only bootstrap record.
    let structured_content = matches!(
        &message.content,
        MessageContent::Parts(_) | MessageContent::Empty
    )
    .then(|| message.content.clone());
    let attachments_json = Some(serde_json::to_string(&MessageProjectionMeta {
        version: MESSAGE_PROJECTION_VERSION,
        content: structured_content,
        reasoning_content: message.reasoning_content.clone(),
        execution_status: message.execution_status.clone(),
        approved_assistant_blocks: message.approved_assistant_blocks.clone(),
        reasoning_blocks: message.reasoning_blocks.clone(),
        assistant_blocks: message.assistant_blocks.clone(),
        name: message.name.clone(),
        tool_call_id: message.tool_call_id.clone(),
        tool_result_is_error: message.tool_result_is_error,
        provider_response: message.provider_response.clone(),
        transcript_generation_id: None,
        transcript_ordinal: None,
    })?);

    Ok(StoredMessage {
        id: None,
        conversation_id: conversation_id.to_string(),
        role: message.role.as_str().to_string(),
        content: message.text_content(),
        attachments_json,
        tool_calls_json,
        tool_result_json,
        created_at: echo_core::utils::time::now_local().to_rfc3339(),
    })
}

/// Attach one generation-local ordinal to a canonical projected message.
///
/// Only the two append-coordinate fields are replaced. Consumer-owned JSON
/// values and explicit nulls remain intact; no application schema is decoded.
pub fn set_transcript_projection_meta(
    stored: &mut StoredMessage,
    generation_id: &str,
    ordinal: u64,
) -> Result<()> {
    // Validate the owned projection fields/version before writing, but edit
    // only our append coordinates in the original object. Re-serializing the
    // typed read model would drop opaque consumer fields and explicit nulls.
    restore_projection_meta(stored)?.ok_or_else(|| {
        MemoryError::SerializationError(
            "transcript projection metadata requires a canonical message projection".to_string(),
        )
    })?;
    let raw = stored.attachments_json.as_deref().ok_or_else(|| {
        MemoryError::SerializationError("canonical projection metadata is missing".to_string())
    })?;
    let mut payload: serde_json::Value = serde_json::from_str(raw)?;
    let object = payload.as_object_mut().ok_or_else(|| {
        MemoryError::SerializationError(
            "canonical projection metadata must be an object".to_string(),
        )
    })?;
    object.insert(
        "transcript_generation_id".to_string(),
        serde_json::json!(generation_id),
    );
    object.insert("transcript_ordinal".to_string(), serde_json::json!(ordinal));
    stored.attachments_json = Some(serde_json::to_string(&payload)?);
    Ok(())
}

/// Read a generation-local ordinal from a canonical projected message.
pub fn transcript_projection_meta(
    stored: &StoredMessage,
) -> Result<Option<TranscriptProjectionMeta>> {
    let Some(projection) = restore_projection_meta(stored)? else {
        return Ok(None);
    };
    Ok(
        match (
            projection.transcript_generation_id,
            projection.transcript_ordinal,
        ) {
            (Some(generation_id), Some(ordinal)) => Some(TranscriptProjectionMeta {
                generation_id,
                ordinal,
            }),
            (None, None) => None,
            _ => {
                return Err(MemoryError::SerializationError(
                    "transcript projection generation and ordinal must be present together"
                        .to_string(),
                )
                .into());
            }
        },
    )
}

/// Return the canonical Framework attachment projection without append coordinates.
///
/// This is the established runtime cursor/digest view, not a storage rewrite:
/// consumer-owned fields are intentionally excluded. Keep this v1 normalization
/// stable when mutating stored rows so old checkpoint hashes are not redefined.
pub fn normalized_transcript_attachments(stored: &StoredMessage) -> Result<Option<String>> {
    let Some(mut projection) = restore_projection_meta(stored)? else {
        return Ok(stored.attachments_json.clone());
    };
    projection.transcript_generation_id = None;
    projection.transcript_ordinal = None;
    Ok(Some(serde_json::to_string(&projection)?))
}

/// Restore a single persisted transcript record back into a runtime [`Message`].
///
/// This is the inverse of [`project_message`]: it parses the role + the
/// `tool_calls_json` / `tool_result_json` projections back into the structured
/// `Message` shape. Round-trips losslessly for the four canonical roles
/// (system/user/assistant/tool) and for assistant messages carrying tool calls.
///
/// # Errors
///
/// Returns an error when:
/// - `tool_calls_json` is present but cannot be deserialized as
///   `Vec<ToolCall>` (corruption — surfaced rather than silently dropped).
/// - 工具结果缺失或损坏显式 `{ tool_call_id, name }`，或与canonical投影身份不同。
/// - the role is anything other than `system`/`user`/`assistant`/`tool`.
///   Unknown roles are an error rather than silently demoted to `user`, so
///   callers can detect schema drift instead of silently losing the role.
pub fn restore_message(stored: &StoredMessage) -> Result<Message> {
    let text = stored.content.clone().unwrap_or_default();
    let projection = restore_projection_meta(stored)?.ok_or_else(|| {
        MemoryError::SerializationError(
            "runtime message is missing current canonical projection metadata".to_string(),
        )
    })?;
    let mut message = match stored.role.as_str() {
        "system" => Message::system(text),
        "assistant" => {
            let calls = stored
                .tool_calls_json
                .as_deref()
                .map(serde_json::from_str::<Vec<ToolCall>>)
                .transpose()?;
            match calls {
                Some(calls) => {
                    let mut message = Message::assistant_with_tools(calls);
                    if !text.is_empty() {
                        message.content = MessageContent::Text(text);
                    }
                    message
                }
                None => Message::assistant(text),
            }
        }
        "tool" => {
            #[derive(serde::Deserialize)]
            struct ToolResultMeta {
                tool_call_id: String,
                name: String,
            }
            let raw = stored.tool_result_json.as_deref().ok_or_else(|| {
                MemoryError::SerializationError(
                    "tool result is missing explicit metadata".to_string(),
                )
            })?;
            let meta: ToolResultMeta = serde_json::from_str(raw)?;
            if meta.tool_call_id.trim().is_empty() || meta.name.trim().is_empty() {
                return Err(MemoryError::SerializationError(
                    "tool result has empty identity metadata".to_string(),
                )
                .into());
            }
            Message::tool_result(meta.tool_call_id, meta.name, text)
        }
        "user" => Message::user(text),
        other => {
            return Err(MemoryError::SerializationError(format!(
                "cannot restore message: unknown role '{other}'"
            ))
            .into());
        }
    };
    if message.role == Role::Tool
        && (projection.tool_call_id != message.tool_call_id || projection.name != message.name)
    {
        return Err(MemoryError::SerializationError(
            "tool result identity differs from its canonical projection".to_string(),
        )
        .into());
    }
    if let Some(content) = projection.content {
        message.content = content;
    }
    message.reasoning_content = projection.reasoning_content;
    message.execution_status = projection.execution_status;
    message.approved_assistant_blocks = projection.approved_assistant_blocks;
    message.reasoning_blocks = projection.reasoning_blocks;
    message.assistant_blocks = projection.assistant_blocks;
    message.name = projection.name;
    message.tool_call_id = projection.tool_call_id;
    message.tool_result_is_error = projection.tool_result_is_error;
    message.provider_response = projection.provider_response;
    Ok(message)
}

fn restore_projection_meta(stored: &StoredMessage) -> Result<Option<MessageProjectionMeta>> {
    // 这里只分类框架自有metadata；raw/display原件不被认领，也不因此取得runtime身份。
    // restore_message必须显式要求Some；metadata/digest查询继续保留raw记录能力。
    let Some(raw) = stored.attachments_json.as_deref() else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Ok(None);
    };
    if !value.is_object() || value.get("_echo_message_version").is_none() {
        return Ok(None);
    }
    let projection: MessageProjectionMeta = serde_json::from_value(value)?;
    if projection.version != MESSAGE_PROJECTION_VERSION {
        return Err(MemoryError::SerializationError(format!(
            "cannot restore message projection version {}",
            projection.version
        ))
        .into());
    }
    Ok(Some(projection))
}

/// Restore a list of persisted transcript records into runtime [`Message`]s.
///
/// Batch form of [`restore_message`]; fails on the first record that cannot be
/// restored (matching `project_messages` fail-fast semantics).
pub fn restore_messages(stored: &[StoredMessage]) -> Result<Vec<Message>> {
    stored.iter().map(restore_message).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips_empty_content_and_message_identity() -> Result<()> {
        let mut original = Message::assistant(String::new());
        original.content = MessageContent::Empty;
        original.name = Some("assistant-name".to_string());
        original.tool_call_id = Some("carried-call-id".to_string());

        let stored = project_message("conversation", &original)?;
        let restored = restore_message(&stored)?;

        assert!(matches!(restored.content, MessageContent::Empty));
        assert_eq!(restored.name.as_deref(), Some("assistant-name"));
        assert_eq!(restored.tool_call_id.as_deref(), Some("carried-call-id"));
        Ok(())
    }

    #[test]
    fn plain_text_projection_carries_canonical_marker() -> Result<()> {
        let stored = project_message("conversation", &Message::user("hello".to_string()))?;
        let projection = stored.attachments_json.as_deref().ok_or_else(|| {
            MemoryError::SerializationError("missing canonical projection marker".to_string())
        })?;

        assert!(projection.contains("_echo_message_version"));
        assert_eq!(
            restore_message(&stored)?.text_content().as_deref(),
            Some("hello")
        );
        Ok(())
    }
    #[test]
    fn current_tool_projection_requires_explicit_identity_without_guessing() -> Result<()> {
        let original = Message::tool_result(
            "call-current".to_string(),
            "read_file".to_string(),
            "结果😀".to_string(),
        );
        let row = project_message("current", &original)?;
        assert_eq!(
            serde_json::to_value(restore_message(&row)?)?,
            serde_json::to_value(&original)?
        );
        for raw in [
            None,
            Some("{}"),
            Some("{\"tool_call_id\":null,\"name\":\"read_file\"}"),
            Some("{\"tool_call_id\":\"call-current\"}"),
            Some("{\"tool_call_id\":\"\",\"name\":\"read_file\"}"),
            Some("{\"tool_call_id\":\"other-call\",\"name\":\"read_file\"}"),
        ] {
            let mut invalid = row.clone();
            invalid.tool_result_json = raw.map(str::to_string);
            let before = serde_json::to_vec(&invalid)?;
            assert!(restore_message(&invalid).is_err());
            assert_eq!(serde_json::to_vec(&invalid)?, before);
        }
        let mut annotated = row.clone();
        let mut metadata: serde_json::Value =
            serde_json::from_str(annotated.attachments_json.as_deref().ok_or_else(|| {
                MemoryError::SerializationError("canonical marker missing".into())
            })?)?;
        metadata
            .as_object_mut()
            .ok_or_else(|| MemoryError::SerializationError("canonical metadata not object".into()))?
            .insert(
                "consumer_extension".into(),
                serde_json::json!({"nested":null}),
            );
        annotated.attachments_json = Some(serde_json::to_string(&metadata)?);
        assert_eq!(
            restore_message(&annotated)?.tool_call_id.as_deref(),
            Some("call-current")
        );
        let mut invalid_message = original;
        invalid_message.tool_call_id = None;
        assert!(project_message("current", &invalid_message).is_err());
        let mut corrupt = row;
        corrupt.attachments_json = Some("{".to_string());
        assert!(restore_message(&corrupt).is_err());
        Ok(())
    }
    #[test]
    fn current_plain_projection_requires_marker_and_keeps_display_data_immutable() -> Result<()> {
        let row = project_message("current-plain", &Message::user("当前原件😀".to_string()))?;
        assert_eq!(
            restore_message(&row)?.text_content().as_deref(),
            Some("当前原件😀")
        );
        for raw in [
            None,
            Some("{"),
            Some("[]"),
            Some("null"),
            Some("{}"),
            Some("{\"consumer_extension\":null}"),
            Some("{\"_echo_message_version\":2}"),
        ] {
            let mut display_only = row.clone();
            display_only.attachments_json = raw.map(str::to_string);
            let before = serde_json::to_vec(&display_only)?;
            assert!(restore_message(&display_only).is_err());
            if raw != Some("{\"_echo_message_version\":2}") {
                assert_eq!(transcript_projection_meta(&display_only)?, None);
                assert_eq!(
                    normalized_transcript_attachments(&display_only)?,
                    display_only.attachments_json
                );
                let digest = transcript_projection_message_digest(&display_only)?;
                assert_eq!(digest.len(), 64);
                assert!(
                    set_transcript_projection_meta(&mut display_only, "current-generation", 0)
                        .is_err()
                );
            }
            assert_eq!(serde_json::to_vec(&display_only)?, before);
        }
        Ok(())
    }
}

#[cfg(test)]
mod native_history_metadata_tests {
    use super::*;
    use crate::memory::FileConversationStore;
    use echo_core::agent::{
        AgentPhase, AssistantMessageBlock, AssistantPhase, ExecutionStatus, ExecutionStatusKind,
    };
    use echo_core::llm::LlmApiProtocol;
    use echo_core::llm::types::{ContentPart, ImageUrl, ProviderOutputSource, ReasoningBlock};
    use echo_core::memory::conversation::{
        ConversationStore, EnsureConversationProjectionRequest, ManagedConversationImport,
        NewConversation, TranscriptProjectionApplyStatus, TranscriptProjectionBatch,
    };
    use serde_json::{Value, json};

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn required_payload(
        row: &StoredMessage,
    ) -> std::result::Result<Value, Box<dyn std::error::Error>> {
        let raw = row
            .attachments_json
            .as_deref()
            .ok_or_else(|| std::io::Error::other("canonical attachment metadata is missing"))?;
        Ok(serde_json::from_str(raw)?)
    }

    fn native_message(approved: Option<Vec<AssistantMessageBlock>>) -> Message {
        let signed = ReasoningBlock::Signed {
            thinking: "原生推理😀".to_string(),
            signature: "signature-逐字节-🧪".to_string(),
        };
        let block = AssistantMessageBlock {
            provider_source: Some(ProviderOutputSource {
                protocol: LlmApiProtocol::Anthropic,
                request_id: "request-native".to_string(),
                item_id: "item-native".to_string(),
                output_index: 2,
                content_index: None,
                content_kind: Some("thinking".to_string()),
                phase: None,
            }),
            block_id: "item-native".to_string(),
            request_id: "request-native".to_string(),
            sequence: 4,
            phase: AssistantPhase::Reasoning,
            delta: "原生推理😀".to_string(),
            complete: true,
            provider: Some("anthropic".to_string()),
            reasoning: Some(signed.clone()),
        };
        let mut message = Message::assistant("已确认正文".to_string());
        message.content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "多模态中文😀".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AA==".to_string(),
                    detail: Some("low".to_string()),
                },
            },
            ContentPart::File {
                name: "附件🧪.txt".to_string(),
                content: "5Lit5paH".to_string(),
            },
        ]);
        message.reasoning_content = Some("原生推理😀".to_string());
        message.reasoning_blocks = Some(vec![
            signed,
            ReasoningBlock::Redacted {
                data: "redacted-原件".to_string(),
            },
            ReasoningBlock::Opaque {
                provider: "openai_responses".to_string(),
                id: "rs-opaque".to_string(),
                data: "encrypted-逐字节".to_string(),
                summary: vec!["公开摘要".to_string()],
            },
        ]);
        message.assistant_blocks = Some(vec![block]);
        message.approved_assistant_blocks = approved;
        message.execution_status = Some(ExecutionStatus {
            adapter_generated: false,
            execution_id: "runtime-owner".to_string(),
            request_id: Some("request-native".to_string()),
            sequence: 7,
            phase: AgentPhase::Terminal,
            status: ExecutionStatusKind::Settled,
            reason: None,
        });
        message
    }

    #[tokio::test]
    async fn native_history_metadata_managed_import_cold_roundtrip() -> TestResult {
        let base =
            std::env::temp_dir().join(format!("native-history-metadata-{}", uuid::Uuid::new_v4()));
        let conversation_id = "native-metadata-import";
        let generation_id = "generation-原文";
        let with_approved = native_message(None).assistant_blocks;
        let originals = vec![
            native_message(None),
            native_message(Some(Vec::new())),
            native_message(with_approved),
        ];
        let mut rows = project_messages(conversation_id, &originals)?;
        let canonical_digests = rows
            .iter()
            .map(transcript_projection_message_digest)
            .collect::<Result<Vec<_>>>()?;
        let mut expected = Vec::new();
        for (offset, row) in rows.iter_mut().enumerate() {
            let mut payload = required_payload(row)?;
            let object = payload
                .as_object_mut()
                .ok_or_else(|| std::io::Error::other("metadata must be an object"))?;
            // Generic consumer-owned data. The Framework must retain the JSON
            // values without learning an application's rendering schema.
            object.insert("consumer_extension".to_string(), json!({"owner":"本地消费者😀","nested":[null,false,true,{"text":"长原文末尾🧪"}],"provenance":{"source":"source-a","read_only":true}}));
            object.insert("consumer_null".to_string(), Value::Null);
            object.insert("consumer_false".to_string(), json!(false));
            object.insert("consumer_true".to_string(), json!(true));
            object.insert("consumer_text".to_string(), json!("中文😀🧪"));
            // Explicit null for a known optional field must also survive a
            // coordinate-only edit instead of being normalized out of storage.
            object.insert("name".to_string(), Value::Null);
            row.attachments_json = Some(serde_json::to_string(&payload)?);
            let ordinal = u64::try_from(offset)?;
            let object = payload
                .as_object_mut()
                .ok_or_else(|| std::io::Error::other("metadata must remain an object"))?;
            object.insert("transcript_generation_id".to_string(), json!(generation_id));
            object.insert("transcript_ordinal".to_string(), json!(ordinal));
            expected.push(payload);
            set_transcript_projection_meta(row, generation_id, ordinal)?;
        }
        let (import, receipt) = {
            let store = FileConversationStore::new(&base)?;
            let authority = store
                .ensure_projection_epoch(EnsureConversationProjectionRequest {
                    conversation: NewConversation {
                        conversation_id: conversation_id.to_string(),
                        user_id: "test".to_string(),
                        agent_type: None,
                        title: None,
                    },
                    expected_tombstone_epoch: None,
                })
                .await?
                .authority;
            let import = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                authority.epoch,
                authority.revision,
                generation_id,
                rows,
            )?;
            let receipt = store.import_managed_messages(import.clone()).await?;
            assert_eq!(receipt.status, TranscriptProjectionApplyStatus::Applied);
            (import, receipt)
        };
        // Close every store handle before reading the committed file log.
        {
            let reopened = FileConversationStore::new(&base)?;
            let cold = reopened.get_messages(conversation_id).await?;
            assert_eq!(cold.len(), expected.len());
            for (offset, (row, wanted)) in cold.iter().zip(&expected).enumerate() {
                assert_eq!(required_payload(row)?, *wanted);
                assert_eq!(
                    transcript_projection_meta(row)?,
                    Some(TranscriptProjectionMeta {
                        generation_id: generation_id.to_string(),
                        ordinal: u64::try_from(offset)?
                    })
                );
            }
            assert_eq!(
                serde_json::to_value(restore_messages(&cold)?)?,
                serde_json::to_value(&originals)?
            );
            assert_eq!(
                cold.iter()
                    .map(transcript_projection_message_digest)
                    .collect::<Result<Vec<_>>>()?,
                canonical_digests
            );
            assert_eq!(
                managed_import_projection_digests(conversation_id, &cold)?,
                canonical_digests
            );
            let locator = reopened
                .get_latest_managed_import(conversation_id)
                .await?
                .ok_or_else(|| std::io::Error::other("cold managed import locator missing"))?;
            assert!(locator.matches(&import));
            assert_eq!(
                reopened
                    .import_managed_messages(import.clone())
                    .await?
                    .status,
                TranscriptProjectionApplyStatus::AlreadyApplied
            );
            // Store ordinal identity is the exact StoredMessage digest. Replay
            // the immutable original rows, never the backend-assigned ids.
            let batch = TranscriptProjectionBatch::prepare(
                conversation_id,
                receipt.authority.epoch,
                generation_id,
                0,
                import.messages.clone(),
            )?;
            assert_eq!(
                batch
                    .items
                    .iter()
                    .map(|item| item.digest.clone())
                    .collect::<Vec<_>>(),
                import.ordinal_digests()?
            );
            // Existing Store frontier semantics reject a new operation at an
            // already occupied ordinal even when the row digest is identical.
            assert!(matches!(reopened.apply_transcript_projection(batch).await?.status, TranscriptProjectionApplyStatus::Conflict { kind: echo_core::memory::conversation::TranscriptProjectionConflictKind::OrdinalDigest, .. }));
        }
        std::fs::remove_dir_all(base)?;
        Ok(())
    }

    #[tokio::test]
    async fn native_history_metadata_digest_and_exact_import_identity_remain_distinct() -> TestResult
    {
        let base =
            std::env::temp_dir().join(format!("native-metadata-digest-{}", uuid::Uuid::new_v4()));
        let conversation_id = "metadata-digest";
        let mut row = project_message(conversation_id, &Message::user("历史😀".to_string()))?;
        let normalized = normalized_transcript_attachments(&row)?;
        let legacy_digest = transcript_projection_message_digest(&row)?;
        // Existing v1 cursor bytes. Opaque application fields must not silently
        // change an old runtime cursor/checkpoint's interpretation.
        assert_eq!(normalized.as_deref(), Some("{\"_echo_message_version\":1}"));
        assert_eq!(
            legacy_digest,
            "f9f16bd580bd4a0ca556ab4721bac7dbb3cafd6248e0da66b12e9305b159eac7"
        );
        let mut payload = required_payload(&row)?;
        payload
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("metadata not object"))?
            .insert(
                "consumer_extension".to_string(),
                json!({"value":null,"enabled":false,"text":"原文😀"}),
            );
        row.attachments_json = Some(serde_json::to_string(&payload)?);
        set_transcript_projection_meta(&mut row, "generation-a", 0)?;
        assert_eq!(normalized_transcript_attachments(&row)?, normalized);
        assert_eq!(transcript_projection_message_digest(&row)?, legacy_digest);
        let (request, first_receipt) = {
            let store = FileConversationStore::new(&base)?;
            let authority = store
                .ensure_projection_epoch(EnsureConversationProjectionRequest {
                    conversation: NewConversation {
                        conversation_id: conversation_id.to_string(),
                        user_id: "test".to_string(),
                        agent_type: None,
                        title: None,
                    },
                    expected_tombstone_epoch: None,
                })
                .await?
                .authority;
            let request = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                authority.epoch,
                authority.revision,
                "generation-a",
                vec![row],
            )?;
            let receipt = store.import_managed_messages(request.clone()).await?;
            (request, receipt)
        };
        {
            let reopened = FileConversationStore::new(&base)?;
            let mut cold = reopened
                .get_messages(conversation_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| std::io::Error::other("cold row missing"))?;
            let cold_payload = required_payload(&cold)?;
            assert_eq!(
                cold_payload.get("consumer_extension"),
                payload.get("consumer_extension")
            );
            let mut changed = cold_payload;
            changed
                .as_object_mut()
                .ok_or_else(|| std::io::Error::other("metadata not object"))?
                .insert(
                    "consumer_extension".to_string(),
                    json!({"value":null,"enabled":true,"text":"更新😀"}),
                );
            cold.id = None;
            cold.attachments_json = Some(serde_json::to_string(&changed)?);
            // Hold epoch/revision/generation/ordinal and every canonical field
            // fixed: the opaque consumer change alone changes full identity.
            let metadata_only = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                request.expected_epoch,
                request.expected_revision,
                "generation-a",
                vec![cold.clone()],
            )?;
            assert_ne!(metadata_only.payload_digest, request.payload_digest);
            assert_ne!(metadata_only.ordinal_digests()?, request.ordinal_digests()?);
            let mut metadata_tamper = request.clone();
            metadata_tamper.messages = metadata_only.messages;
            assert!(metadata_tamper.validate().is_err());
            assert!(
                reopened
                    .import_managed_messages(metadata_tamper)
                    .await
                    .is_err()
            );
            set_transcript_projection_meta(&mut cold, "generation-b", 0)?;
            assert_eq!(normalized_transcript_attachments(&cold)?, normalized);
            assert_eq!(transcript_projection_message_digest(&cold)?, legacy_digest);
            assert_eq!(
                managed_import_projection_digests(conversation_id, std::slice::from_ref(&cold))?,
                vec![legacy_digest]
            );
            // Full import identities still cover the consumer extension. The
            // stable runtime digest is not permission to tamper an old request.
            let mut tampered = request.clone();
            tampered.messages = vec![cold.clone()];
            assert!(tampered.validate().is_err());
            assert!(reopened.import_managed_messages(tampered).await.is_err());
            let next = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                first_receipt.authority.epoch,
                first_receipt.authority.revision,
                "generation-b",
                vec![cold],
            )?;
            assert_ne!(next.payload_digest, request.payload_digest);
            assert_ne!(next.ordinal_digests()?, request.ordinal_digests()?);
            assert_eq!(
                reopened.import_managed_messages(next).await?.status,
                TranscriptProjectionApplyStatus::Applied
            );
        }
        {
            let reopened = FileConversationStore::new(&base)?;
            let final_row = reopened
                .get_messages(conversation_id)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| std::io::Error::other("second cold row missing"))?;
            assert_eq!(
                required_payload(&final_row)?.get("consumer_extension"),
                Some(&json!({"value":null,"enabled":true,"text":"更新😀"}))
            );
            assert_eq!(
                transcript_projection_meta(&final_row)?,
                Some(TranscriptProjectionMeta {
                    generation_id: "generation-b".to_string(),
                    ordinal: 0
                })
            );
        }
        std::fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn native_history_metadata_invalid_projection_is_not_rewritten() -> TestResult {
        for raw in [
            "{",
            "null",
            "false",
            "[]",
            "{\"consumer_extension\":null}",
            "{\"_echo_message_version\":2,\"consumer_extension\":false}",
            "{\"_echo_message_version\":1,\"transcript_ordinal\":\"bad\",\"consumer_extension\":null}",
        ] {
            let mut row = project_message("invalid", &Message::user("内容".to_string()))?;
            row.attachments_json = Some(raw.to_string());
            let before = row.attachments_json.clone();
            assert!(set_transcript_projection_meta(&mut row, "generation", 0).is_err());
            assert_eq!(row.attachments_json, before);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tool_result_observation_tests {
    use super::*;

    #[test]
    fn file_projection_retains_false_true_and_unknown_error_observation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        for observation in [None, Some(false), Some(true)] {
            let mut wire = serde_json::json!({"role":"tool", "name":"inspect", "tool_call_id":"call-1", "content":[{"type":"text","text":"观察🙂"},{"type":"file","name":"保留.txt","content":"5Lit5paH"}]});
            if let Some(is_error) = observation {
                wire.as_object_mut()
                    .ok_or("message is not object")?
                    .insert("tool_result_is_error".into(), is_error.into());
            }
            let message: Message = serde_json::from_value(wire.clone())?;
            let projected = project_message("public-contract", &message)?;
            let cold: StoredMessage = serde_json::from_slice(&serde_json::to_vec(&projected)?)?;
            let restored = restore_message(&cold)?;
            assert_eq!(serde_json::to_value(restored)?, wire);
            assert_eq!(
                managed_import_projection_digests("public-contract", std::slice::from_ref(&cold))?,
                vec![transcript_projection_message_digest(&cold)?]
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tool_result_cold_import_tests {
    use super::*;
    use crate::memory::FileConversationStore;
    use echo_core::memory::conversation::{
        ConversationStore, EnsureConversationProjectionRequest, ManagedConversationImport,
        NewConversation, TranscriptProjectionApplyStatus,
    };

    #[tokio::test]
    async fn tool_result_error_and_parts_survive_managed_import_then_cold_reopen()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let base =
            std::env::temp_dir().join(format!("tool-contract-cold-{}", uuid::Uuid::new_v4()));
        let conversation_id = "tool-contract";
        let mut originals = Vec::new();
        for observation in [None, Some(false), Some(true)] {
            let mut wire = serde_json::json!({"role":"tool", "name":"inspect", "tool_call_id":format!("call-{}", originals.len()), "content":[{"type":"text","text":"保留错误观察🙂"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}},{"type":"file","name":"保留.txt","content":"5Lit5paH"}]});
            if let Some(is_error) = observation {
                wire.as_object_mut()
                    .ok_or("expected object")?
                    .insert("tool_result_is_error".into(), is_error.into());
            }
            originals.push(wire);
        }
        let messages = originals
            .iter()
            .cloned()
            .map(serde_json::from_value::<Message>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut rows = project_messages(conversation_id, &messages)?;
        for (index, row) in rows.iter_mut().enumerate() {
            set_transcript_projection_meta(row, "tool-generation", u64::try_from(index)?)?;
        }
        {
            let store = FileConversationStore::new(&base)?;
            let authority = store
                .ensure_projection_epoch(EnsureConversationProjectionRequest {
                    conversation: NewConversation {
                        conversation_id: conversation_id.into(),
                        user_id: "test".into(),
                        agent_type: None,
                        title: None,
                    },
                    expected_tombstone_epoch: None,
                })
                .await?
                .authority;
            let request = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                authority.epoch,
                authority.revision,
                "tool-generation",
                rows,
            )?;
            assert_eq!(
                store.import_managed_messages(request).await?.status,
                TranscriptProjectionApplyStatus::Applied
            );
        }
        {
            let reopened = FileConversationStore::new(&base)?;
            let stored = reopened.get_messages(conversation_id).await?;
            let cold = restore_messages(&stored)?;
            assert_eq!(serde_json::to_value(cold)?, serde_json::json!(originals));
            assert_eq!(
                managed_import_projection_digests(conversation_id, &stored)?.len(),
                3
            );
        }
        std::fs::remove_dir_all(base)?;
        Ok(())
    }
}

// 旧API可表达的JSON损失测试片段：附到echo-state/src/memory/conversation.rs。
// batch owner将TMPDIR绑定到任务主根；不新建store权威，不声明metadata类型。
#[cfg(test)]
mod provider_response_cold_contract_tests {
    use super::{
        managed_import_projection_digests, project_messages, restore_messages,
        set_transcript_projection_meta,
    };
    use crate::memory::FileConversationStore;
    use echo_core::llm::types::Message;
    use echo_core::memory::conversation::{
        ConversationStore, EnsureConversationProjectionRequest, ManagedConversationImport,
        NewConversation, TranscriptProjectionApplyStatus,
    };
    use serde_json::{Value, json};

    #[tokio::test]
    async fn native_response_snapshot_preserves_presence_and_usage_after_managed_cold_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory =
            std::env::temp_dir().join(format!("response-metadata-{}", uuid::Uuid::new_v4()));
        let conversation_id = "response-metadata";
        let mut expected = Vec::new();
        for stop in [None, Some(Value::Null), Some(json!("END-终止"))] {
            let mut metadata = json!({"protocol":"anthropic", "provider":"configured-provider",
                "request_id":"msg-source", "model":"native-model", "stop_reason":"end_turn",
                "usage":{"input_tokens":5,"output_tokens":10,
                    "output_tokens_details":{"reasoning_tokens":7},
                    "nested_extension":{"counter":9007199254740993_u64,"value":null}}});
            if let Some(value) = stop {
                metadata
                    .as_object_mut()
                    .ok_or("expected metadata object")?
                    .insert("stop_sequence".into(), value);
            }
            expected.push(
                json!({"role":"assistant", "content":"耐久观察🙂", "provider_response":metadata}),
            );
        }
        let messages = expected
            .iter()
            .cloned()
            .map(serde_json::from_value::<Message>)
            .collect::<Result<Vec<_>, _>>()?;
        let mut rows = project_messages(conversation_id, &messages)?;
        for (offset, row) in rows.iter_mut().enumerate() {
            set_transcript_projection_meta(row, "metadata-generation", u64::try_from(offset)?)?;
        }
        {
            let store = FileConversationStore::new(&directory)?;
            let authority = store
                .ensure_projection_epoch(EnsureConversationProjectionRequest {
                    conversation: NewConversation {
                        conversation_id: conversation_id.into(),
                        user_id: "test".into(),
                        agent_type: None,
                        title: None,
                    },
                    expected_tombstone_epoch: None,
                })
                .await?
                .authority;
            let request = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                authority.epoch,
                authority.revision,
                "metadata-generation",
                rows,
            )?;
            assert_eq!(
                store.import_managed_messages(request).await?.status,
                TranscriptProjectionApplyStatus::Applied
            );
        }
        {
            let reopened = FileConversationStore::new(&directory)?;
            let cold_rows = reopened.get_messages(conversation_id).await?;
            let restored = restore_messages(&cold_rows)?;
            assert_eq!(serde_json::to_value(restored)?, json!(expected));
            let digests = managed_import_projection_digests(conversation_id, &cold_rows)?;
            assert_eq!(
                digests
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                3,
                "absence, null and string change the native observation identity"
            );
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}

// POST_API：构造器+真实File持久化合同；不是ReAct collector已实际执行或工具调用次数的证据。
#[cfg(test)]
mod observation_only_cold_contract_tests {
    use super::{project_messages, restore_messages, set_transcript_projection_meta};
    use crate::memory::FileConversationStore;
    use echo_core::llm::types::{Message, ProviderResponseMetadata};
    use echo_core::memory::conversation::{
        ConversationStore, EnsureConversationProjectionRequest, ManagedConversationImport,
        NewConversation, TranscriptProjectionApplyStatus,
    };
    use serde_json::json;

    #[tokio::test]
    async fn metadata_only_failure_record_keeps_filter_identity_after_managed_cold_reopen()
    -> Result<(), Box<dyn std::error::Error>> {
        let metadata: ProviderResponseMetadata = serde_json::from_value(json!({
            "protocol":"responses","provider":"configured-responses","request_id":"resp-incomplete",
            "status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},
            "usage":{"output_tokens":10,"nested":{"counter":9007199254740993_u64,"null":null}}
        }))?;
        let message = Message::provider_response_observation(metadata.clone());
        let expected = serde_json::to_value(&message)?;
        let conversation_id = "observation-only";
        let directory =
            std::env::temp_dir().join(format!("observation-only-{}", uuid::Uuid::new_v4()));
        let mut rows = project_messages(conversation_id, &[message])?;
        let row = rows.first_mut().ok_or("observation projection missing")?;
        set_transcript_projection_meta(row, "observation-generation", 0)?;
        {
            let store = FileConversationStore::new(&directory)?;
            let authority = store
                .ensure_projection_epoch(EnsureConversationProjectionRequest {
                    conversation: NewConversation {
                        conversation_id: conversation_id.into(),
                        user_id: "test".into(),
                        agent_type: None,
                        title: None,
                    },
                    expected_tombstone_epoch: None,
                })
                .await?
                .authority;
            let request = ManagedConversationImport::prepare_for_generation(
                conversation_id,
                authority.epoch,
                authority.revision,
                "observation-generation",
                rows,
            )?;
            assert_eq!(
                store.import_managed_messages(request).await?.status,
                TranscriptProjectionApplyStatus::Applied
            );
        }
        {
            let reopened = FileConversationStore::new(&directory)?;
            let rows = reopened.get_messages(conversation_id).await?;
            let messages = restore_messages(&rows)?;
            assert_eq!(messages.len(), 1);
            let restored = messages.first().ok_or("cold observation missing")?;
            assert_eq!(serde_json::to_value(restored)?, expected);
            assert!(restored.is_provider_response_observation_only());
            assert_eq!(restored.provider_response.as_ref(), Some(&metadata));
            assert!(restored.tool_calls.is_none());
            assert!(restored.reasoning_blocks.is_none());
            assert!(restored.execution_status.is_none());
            assert!(
                restored
                    .approved_assistant_blocks
                    .as_ref()
                    .is_some_and(Vec::is_empty)
            );
        }
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
