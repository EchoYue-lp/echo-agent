//! SSE chunk processing — convert raw LLM streaming chunks into AgentEvent sequences.
//!
//! These are static methods (no `&self`) — pure functions that transform chunk data.

use crate::agent::AgentEvent;
use crate::llm::types::{ChatCompletionChunk, FunctionCall, ToolCall as LlmToolCall};
use serde_json::Value;
use std::collections::HashMap;

/// Process streaming response chunk, collect content and return events.
///
/// `in_reasoning` tracks whether reasoning_content is being output (Qwen3/DeepSeek thinking process).
/// ThinkStart is emitted when reasoning_content is first encountered,
/// ThinkEnd is emitted when content or tool_calls is first encountered after reasoning ends.
#[allow(clippy::type_complexity)]
pub(crate) fn process_stream_chunk(
    chunk: &ChatCompletionChunk,
    content_buffer: &mut String,
    tool_call_map: &mut HashMap<u32, (String, String, String)>,
    in_reasoning: &mut bool,
    emit_content_tokens: bool,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();

    if let Some(choice) = chunk.choices.first() {
        // Handle reasoning_content (Qwen3/DeepSeek thinking process)
        if let Some(reasoning) = &choice.delta.reasoning_content
            && !reasoning.is_empty()
        {
            if !*in_reasoning {
                *in_reasoning = true;
                events.push(AgentEvent::ThinkStart);
            }
            events.push(AgentEvent::Token(reasoning.clone()));
        }

        // When content is first encountered after reasoning ends, close the thinking block
        if let Some(content) = &choice.delta.content
            && !content.is_empty()
        {
            if *in_reasoning {
                *in_reasoning = false;
                events.push(AgentEvent::ThinkEnd {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                });
            }
            content_buffer.push_str(content);
            if emit_content_tokens {
                events.push(AgentEvent::Token(content.clone()));
            }
        }

        if let Some(delta_calls) = &choice.delta.tool_calls {
            if *in_reasoning {
                *in_reasoning = false;
                events.push(AgentEvent::ThinkEnd {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                });
            }
            for dc in delta_calls {
                let entry = tool_call_map
                    .entry(dc.index)
                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                if let Some(id) = &dc.id
                    && !id.is_empty()
                {
                    entry.0 = id.clone();
                }
                if let Some(f) = &dc.function {
                    if let Some(name) = &f.name
                        && !name.is_empty()
                    {
                        entry.1 = name.clone();
                    }
                    if let Some(args) = &f.arguments {
                        entry.2.push_str(args);
                    }
                }
            }
        }
    }

    events
}

/// Parse streaming tool-call arguments, tolerating the trailing-character
/// corruption that some providers (notably DeepSeek "fake-stream") inject when
/// reassembling argument fragments. See vLLM issue #42878: streamed fragments
/// do not always concatenate back to the same JSON the non-streaming path
/// would produce — common symptoms are extra trailing `}` / `]` or whitespace.
///
/// Strategy:
///   1. Parse as-is.
///   2. On failure, trim trailing `}`, `]`, `,` and whitespace and retry.
///   3. Give up (return `Err`) only if repair also fails.
///
/// On success returns `(parsed_value, args_to_echo)`: `args_to_echo` is the
/// canonical string we send back to the provider in the assistant message —
/// it is the repaired/re-serialized form so the next turn sees valid JSON.
fn parse_tool_args(args_str: &str) -> Result<(Value, String), serde_json::Error> {
    match serde_json::from_str::<Value>(args_str) {
        Ok(v) => Ok((v.clone(), v.to_string())),
        Err(original_err) => {
            // Repair attempt: strip trailing junk one char at a time (up to a
            // small bound) and retry. This catches the DeepSeek extra-`}` case.
            let trimmed = args_str.trim();
            let mut attempt = trimmed.to_string();
            for _ in 0..8 {
                let last = attempt.chars().last();
                if matches!(
                    last,
                    Some('}') | Some(']') | Some(',') | Some(' ' | '\n' | '\t' | '\r')
                ) {
                    attempt.pop();
                    if let Ok(v) = serde_json::from_str::<Value>(&attempt) {
                        tracing::warn!(
                            raw_args = args_str,
                            repaired_args = %v,
                            "Repaired streaming tool-call arguments (stripped trailing characters from a provider fake-stream artifact)"
                        );
                        return Ok((v.clone(), v.to_string()));
                    }
                } else {
                    break;
                }
            }
            Err(original_err)
        }
    }
}

/// Convert the collected tool_call_map into structured tool call lists.
pub(crate) fn build_tool_calls_from_map(
    tool_call_map: &HashMap<u32, (String, String, String)>,
) -> (Vec<LlmToolCall>, Vec<(String, String, Value)>) {
    let mut sorted_indices: Vec<u32> = tool_call_map.keys().cloned().collect();
    sorted_indices.sort();

    let mut msg_tool_calls: Vec<LlmToolCall> = Vec::new();
    let mut steps: Vec<(String, String, Value)> = Vec::new();

    for idx in &sorted_indices {
        let (id, name, args_str) = &tool_call_map[idx];
        match parse_tool_args(args_str) {
            Ok((args, canonical_args)) => {
                // Send the CANONICAL (repaired) args back to the provider so
                // the assistant message always carries valid JSON.
                msg_tool_calls.push(LlmToolCall {
                    id: id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: canonical_args,
                    },
                });
                steps.push((id.clone(), name.clone(), args));
            }
            Err(e) => {
                tracing::warn!(
                    tool_name = %name,
                    tool_call_id = %id,
                    raw_args = %args_str,
                    error = %e,
                    "Failed to parse streaming tool-call arguments even after repair; dropping this tool call"
                );
                // CRITICAL: drop the tool call entirely — do NOT echo it back
                // to the provider (corrupt args cause HTTP 400 "Invalid
                // assistant message"), and do NOT emit a half tool_call with no
                // matching tool_result (that also breaks the next turn).
                // Dropping it lets the model retry the call cleanly next turn.
            }
        }
    }

    (msg_tool_calls, steps)
}

#[cfg(test)]
mod tests {
    use super::parse_tool_args;

    #[test]
    fn parse_tool_args_valid_json_passes_through() {
        let raw = r#"{"task": {"agent_role": "explorer"}}"#;
        let (val, echo) = parse_tool_args(raw).expect("valid JSON should parse");
        assert_eq!(val["task"]["agent_role"], "explorer");
        // Echoed args are the canonical re-serialized form.
        assert_eq!(echo, val.to_string());
    }

    #[test]
    fn parse_tool_args_repairs_deepseek_extra_trailing_brace() {
        // DeepSeek "fake-stream" artifact: one extra `}` at the tail.
        // See vLLM issue #42878.
        let raw = r#"{"task": {"agent_role": "explorer"}}}"#;
        let (val, _echo) = parse_tool_args(raw).expect("repair should recover valid JSON");
        assert_eq!(val["task"]["agent_role"], "explorer");
    }

    #[test]
    fn parse_tool_args_repairs_trailing_whitespace_after_brace() {
        // DeepSeek variant: valid JSON followed by trailing whitespace.
        let raw = "{\"task\": {\"agent_role\": \"explorer\"}}   \n";
        let (val, _echo) = parse_tool_args(raw).expect("repair should strip trailing whitespace");
        assert_eq!(val["task"]["agent_role"], "explorer");
    }

    #[test]
    fn parse_tool_args_repairs_multiple_extra_braces() {
        // Worst case: several extra trailing `}`.
        let raw = r#"{"task": {"agent_role": "explorer"}}}}}"#;
        let (val, _echo) = parse_tool_args(raw).expect("repair should recover valid JSON");
        assert_eq!(val["task"]["agent_role"], "explorer");
    }

    #[test]
    fn parse_tool_args_genuinely_broken_returns_err() {
        // Not a JSON object at all — repair cannot help.
        let raw = "not json at all";
        assert!(parse_tool_args(raw).is_err());
    }

    #[test]
    fn parse_tool_args_empty_object() {
        let (val, echo) = parse_tool_args("{}").expect("empty object is valid");
        assert!(val.is_object());
        assert_eq!(echo, "{}");
    }
}
