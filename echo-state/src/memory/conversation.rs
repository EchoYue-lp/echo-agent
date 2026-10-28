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
        Some(
            serde_json::json!({
                "tool_call_id": message.tool_call_id,
                "name": message.name,
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
/// - `tool_result_json` is present on a `tool`-role message but cannot be
///   deserialized into `{ tool_call_id, name }`.
/// - the role is anything other than `system`/`user`/`assistant`/`tool`.
///   Unknown roles are an error rather than silently demoted to `user`, so
///   callers can detect schema drift instead of silently losing the role.
pub fn restore_message(stored: &StoredMessage) -> Result<Message> {
    let text = stored.content.clone().unwrap_or_default();
    let projection = restore_projection_meta(stored)?;
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
                tool_call_id: Option<String>,
                name: Option<String>,
            }
            let meta = stored
                .tool_result_json
                .as_deref()
                .map(serde_json::from_str::<ToolResultMeta>)
                .transpose()?;
            // If the projection carried the call id/name, use them; otherwise
            // fall back to a stable placeholder so the tool message is still
            // well-formed (downstream code keys on role==Tool, not the id).
            let tool_call_id = meta
                .as_ref()
                .and_then(|value| value.tool_call_id.clone())
                .unwrap_or_else(|| "unknown_tool_call".to_string());
            let name = meta.and_then(|value| value.name).unwrap_or_default();
            Message::tool_result(tool_call_id, name, text)
        }
        "user" => Message::user(text),
        other => {
            return Err(MemoryError::SerializationError(format!(
                "cannot restore message: unknown role '{other}'"
            ))
            .into());
        }
    };
    if let Some(projection) = projection {
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
    }
    Ok(message)
}

fn restore_projection_meta(stored: &StoredMessage) -> Result<Option<MessageProjectionMeta>> {
    let Some(raw) = stored.attachments_json.as_deref() else {
        return Ok(None);
    };
    // The application also uses attachments_json for UI metadata. Only claim
    // payloads carrying our explicit marker; unrelated or legacy payloads are
    // intentionally left to the application projection.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Ok(None);
    };
    if value.get("_echo_message_version").is_none() {
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
