//! Conversation persistence — concrete implementations and helper functions
//!
//! Trait definition and data types live in [`echo_core::memory::conversation`].
//! This module provides `project_messages`/`project_message` helpers.

use chrono::Utc;
use echo_core::error::Result;
use echo_core::llm::types::Message;
pub use echo_core::memory::conversation::StoredMessage;

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

    let tool_result_json = if message.role == "tool" {
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

    Ok(StoredMessage {
        id: None,
        conversation_id: conversation_id.to_string(),
        role: message.role.clone(),
        content: message.text_content(),
        attachments_json: None,
        tool_calls_json,
        tool_result_json,
        created_at: Utc::now().to_rfc3339(),
    })
}
