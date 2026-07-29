//! Conversation persistence — concrete implementations and helper functions
//!
//! Trait definition and data types live in [`echo_core::memory::conversation`].
//! This module provides `project_messages`/`project_message` helpers.

use echo_core::error::{MemoryError, Result};
use echo_core::llm::types::{Message, MessageContent, Role, ToolCall};
pub use echo_core::memory::conversation::StoredMessage;

const MESSAGE_PROJECTION_VERSION: u8 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct MessageProjectionMeta {
    #[serde(rename = "_echo_message_version")]
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<MessageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// Project runtime Message list to persistable transcript records.
pub fn project_messages(conversation_id: &str, messages: &[Message]) -> Result<Vec<StoredMessage>> {
    messages
        .iter()
        .map(|message| project_message(conversation_id, message))
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

    // `content` is the searchable text projection. Preserve the structured
    // multimodal payload and reasoning separately so restore_message can
    // rebuild the runtime message without making plain-text records verbose.
    let structured_content = matches!(
        &message.content,
        MessageContent::Parts(_) | MessageContent::Empty
    )
    .then(|| message.content.clone());
    let attachments_json = if structured_content.is_some()
        || message.reasoning_content.is_some()
        || message.name.is_some()
        || message.tool_call_id.is_some()
    {
        Some(serde_json::to_string(&MessageProjectionMeta {
            version: MESSAGE_PROJECTION_VERSION,
            content: structured_content,
            reasoning_content: message.reasoning_content.clone(),
            name: message.name.clone(),
            tool_call_id: message.tool_call_id.clone(),
        })?)
    } else {
        None
    };

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
}
