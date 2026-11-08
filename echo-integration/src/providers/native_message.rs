//! Thin provider-to-Framework assistant block conversions.
//!
//! Provider adapters retain vendor-native ids and phase values at their wire
//! boundary. This module only projects already-decoded assistant text and
//! reasoning into the shared `AssistantMessageBlock` contract; execution and
//! final settlement remain in the Framework runtime.

use echo_core::agent::{AssistantMessageBlock, AssistantPhase};
use echo_core::llm::LlmApiProtocol;
use echo_core::llm::types::{Message, ProviderOutputSource, ReasoningBlock};

/// Project a provider-neutral assistant message into canonical blocks.
///
/// Chat Completions and Anthropic do not provide a portable message phase, so
/// their text blocks intentionally use `Unknown`. Reasoning remains explicitly
/// `Reasoning`, and provider signatures/opaque payloads are retained unchanged.
pub(crate) fn blocks_from_message(
    message: &Message,
    request_id: &str,
    provider: &str,
    complete: bool,
) -> Vec<AssistantMessageBlock> {
    let mut blocks = Vec::new();
    let mut sequence = 0_u64;
    if let Some(reasoning) = message
        .reasoning_content
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        blocks.push(AssistantMessageBlock {
            provider_source: Some(output_source(
                LlmApiProtocol::ChatCompletions,
                request_id,
                &format!("{request_id}:choice:0"),
                0,
                None,
                "reasoning_content",
                None,
            )),
            block_id: format!("{request_id}:reasoning"),
            request_id: request_id.to_string(),
            sequence,
            phase: AssistantPhase::Reasoning,
            delta: reasoning.to_string(),
            complete,
            provider: Some(provider.to_string()),
            reasoning: None,
        });
        sequence = sequence.saturating_add(1);
    }
    for (index, reasoning) in message
        .reasoning_blocks
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let block_id = match reasoning {
            ReasoningBlock::Signed { .. } => format!("{request_id}:reasoning:signed:{index}"),
            ReasoningBlock::Redacted { .. } | ReasoningBlock::Opaque { .. } => {
                format!("{request_id}:reasoning:{index}")
            }
        };
        blocks.push(AssistantMessageBlock {
            provider_source: Some(output_source(
                LlmApiProtocol::ChatCompletions,
                request_id,
                &format!("{request_id}:choice:0"),
                0,
                u32::try_from(index).ok(),
                "reasoning_blocks",
                None,
            )),
            block_id,
            request_id: request_id.to_string(),
            sequence,
            phase: AssistantPhase::Reasoning,
            delta: String::new(),
            complete,
            provider: Some(provider.to_string()),
            reasoning: Some(reasoning.clone()),
        });
        sequence = sequence.saturating_add(1);
    }
    if let Some(text) = message.content.as_text().filter(|text| !text.is_empty()) {
        blocks.push(AssistantMessageBlock {
            provider_source: Some(output_source(
                LlmApiProtocol::ChatCompletions,
                request_id,
                &format!("{request_id}:choice:0"),
                0,
                None,
                "content",
                None,
            )),
            block_id: format!("{request_id}:assistant"),
            request_id: request_id.to_string(),
            sequence,
            phase: AssistantPhase::Unknown,
            delta: text,
            complete,
            provider: Some(provider.to_string()),
            reasoning: None,
        });
    }
    blocks
}

/// A local fallback belongs to one response only; it never aliases different
/// requests when a compatible endpoint omits its own response id.
pub(crate) fn fallback_request_id(protocol: &str) -> String {
    format!("{protocol}:{:032x}", rand::random::<u128>())
}

pub(crate) fn validate_completion(
    message: &Message,
    finish: Option<&str>,
) -> echo_core::error::Result<()> {
    use echo_core::error::LlmError;
    if !matches!(finish, Some("stop" | "tool_calls" | "function_call")) {
        return Err(LlmError::InvalidResponse(format!(
            "Chat Completions incomplete or unknown finish reason: {finish:?}"
        ))
        .into());
    }
    let tools = message.tool_calls.as_deref().unwrap_or_default();
    if message
        .content
        .as_text()
        .is_none_or(|text| text.trim().is_empty())
        && tools.is_empty()
    {
        return Err(LlmError::EmptyResponse.into());
    }
    if matches!(finish, Some("tool_calls" | "function_call")) && tools.is_empty() {
        return Err(LlmError::InvalidResponse("tool finish without tool calls".into()).into());
    }
    let mut ids = std::collections::HashSet::new();
    for call in tools {
        if call.id.is_empty() || call.function.name.is_empty() || !ids.insert(&call.id) {
            return Err(
                LlmError::InvalidResponse("invalid or duplicate tool identity".into()).into(),
            );
        }
        let args: serde_json::Value =
            serde_json::from_str(&call.function.arguments).map_err(|error| {
                LlmError::InvalidResponse(format!("invalid tool arguments: {error}"))
            })?;
        if !args.is_object() {
            return Err(
                LlmError::InvalidResponse("tool arguments must be an object".into()).into(),
            );
        }
    }
    Ok(())
}

/// A field-only projection of native protocol coordinates, shared by tool and
/// assistant observations. It grants no runtime completion authority.
pub(crate) fn output_source(
    protocol: LlmApiProtocol,
    request_id: &str,
    item_id: &str,
    output_index: u32,
    content_index: Option<u32>,
    content_kind: &str,
    phase: Option<&str>,
) -> ProviderOutputSource {
    ProviderOutputSource {
        protocol,
        request_id: request_id.to_string(),
        item_id: item_id.to_string(),
        output_index,
        content_index,
        content_kind: Some(content_kind.to_string()),
        phase: phase.map(ToString::to_string),
    }
}
