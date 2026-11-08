//! 当前SSE碎片累积：复用原文本/推理/工具来源，不再产生旧事件副本。
//!
//! These are static methods (no `&self`) — pure functions that transform chunk data.

use crate::llm::types::{ChatCompletionChunk, FunctionCall, ToolCall as LlmToolCall};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// 原工具碎片累积器附带同一 provider 的来源坐标，不创建第二套调用权威。
pub(crate) type ToolCallFragment = (
    String,
    String,
    String,
    Option<crate::llm::types::ProviderOutputSource>,
);
pub(crate) type ToolCallFragments = HashMap<u32, ToolCallFragment>;

/// 仅累积原provider的正文、推理与工具碎片；当前原生块由同一模型步骤发布。
/// 不再生成Token/Think副本，也不拥有阶段或执行终态。
pub(crate) fn process_stream_chunk(
    chunk: &ChatCompletionChunk,
    content_buffer: &mut String,
    reasoning_buffer: &mut String,
    tool_call_map: &mut ToolCallFragments,
) {
    if let Some(choice) = chunk.choices.first() {
        if let Some(reasoning) = &choice.delta.reasoning_content
            && !reasoning.is_empty()
        {
            reasoning_buffer.push_str(reasoning);
        }
        if let Some(content) = &choice.delta.content
            && !content.is_empty()
        {
            content_buffer.push_str(content);
        }
        if let Some(delta_calls) = &choice.delta.tool_calls {
            for dc in delta_calls {
                let entry = tool_call_map
                    .entry(dc.index)
                    .or_insert_with(|| (String::new(), String::new(), String::new(), None));
                if let Some(source) = &dc.provider_source {
                    entry.3 = Some(source.clone());
                }
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
pub(crate) type ParsedToolStep = (String, String, Value);

pub(crate) fn build_tool_calls_from_map(
    tool_call_map: &ToolCallFragments,
) -> Result<(Vec<LlmToolCall>, Vec<ParsedToolStep>), String> {
    let mut sorted_indices: Vec<u32> = tool_call_map.keys().cloned().collect();
    sorted_indices.sort();

    let mut msg_tool_calls: Vec<LlmToolCall> = Vec::new();
    let mut steps: Vec<ParsedToolStep> = Vec::new();
    let mut seen_ids = HashSet::new();

    for idx in &sorted_indices {
        let Some((id, name, args_str, provider_source)) = tool_call_map.get(idx) else {
            continue;
        };
        let canonical_id = if id.trim().is_empty() {
            format!("call_{}", uuid::Uuid::new_v4())
        } else {
            id.clone()
        };
        if !seen_ids.insert(canonical_id.clone()) {
            return Err(format!(
                "duplicate tool call id '{}' in one model batch",
                canonical_id
            ));
        }
        match parse_tool_args(args_str) {
            Ok((args, canonical_args)) => {
                // Send the CANONICAL (repaired) args back to the provider so
                // the assistant message always carries valid JSON.
                msg_tool_calls.push(LlmToolCall {
                    provider_source: provider_source.clone(),
                    id: canonical_id.clone(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: canonical_args,
                    },
                });
                steps.push((canonical_id, name.clone(), args));
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

    Ok((msg_tool_calls, steps))
}

#[cfg(test)]
mod tests {
    use super::{build_tool_calls_from_map, parse_tool_args, process_stream_chunk};
    use crate::llm::types::ChatCompletionChunk;
    use std::collections::HashMap;

    #[test]
    fn parse_tool_args_valid_json_passes_through() -> Result<(), serde_json::Error> {
        let raw = r#"{"task": {"agent_role": "explorer"}}"#;
        let (val, echo) = parse_tool_args(raw)?;
        assert_eq!(val["task"]["agent_role"], "explorer");
        // Echoed args are the canonical re-serialized form.
        assert_eq!(echo, val.to_string());
        Ok(())
    }

    #[test]
    fn parse_tool_args_repairs_deepseek_extra_trailing_brace() -> Result<(), serde_json::Error> {
        // DeepSeek "fake-stream" artifact: one extra `}` at the tail.
        // See vLLM issue #42878.
        let raw = r#"{"task": {"agent_role": "explorer"}}}"#;
        let (val, _echo) = parse_tool_args(raw)?;
        assert_eq!(val["task"]["agent_role"], "explorer");
        Ok(())
    }

    #[test]
    fn parse_tool_args_repairs_trailing_whitespace_after_brace() -> Result<(), serde_json::Error> {
        // DeepSeek variant: valid JSON followed by trailing whitespace.
        let raw = "{\"task\": {\"agent_role\": \"explorer\"}}   \n";
        let (val, _echo) = parse_tool_args(raw)?;
        assert_eq!(val["task"]["agent_role"], "explorer");
        Ok(())
    }

    #[test]
    fn parse_tool_args_repairs_multiple_extra_braces() -> Result<(), serde_json::Error> {
        // Worst case: several extra trailing `}`.
        let raw = r#"{"task": {"agent_role": "explorer"}}}}}"#;
        let (val, _echo) = parse_tool_args(raw)?;
        assert_eq!(val["task"]["agent_role"], "explorer");
        Ok(())
    }

    #[test]
    fn parse_tool_args_genuinely_broken_returns_err() {
        // Not a JSON object at all — repair cannot help.
        let raw = "not json at all";
        assert!(parse_tool_args(raw).is_err());
    }

    #[test]
    fn parse_tool_args_empty_object() -> Result<(), serde_json::Error> {
        let (val, echo) = parse_tool_args("{}")?;
        assert!(val.is_object());
        assert_eq!(echo, "{}");
        Ok(())
    }

    #[test]
    fn tool_call_ids_are_non_empty_and_shared_across_projections() -> Result<(), String> {
        let calls = HashMap::from([(
            0,
            (String::new(), "Read".to_string(), "{}".to_string(), None),
        )]);
        let (messages, steps) = build_tool_calls_from_map(&calls)?;
        let message_id = messages.first().map(|call| call.id.as_str());
        let step_id = steps.first().map(|(id, _, _)| id.as_str());
        assert!(message_id.is_some_and(|id| !id.is_empty()));
        assert_eq!(message_id, step_id);
        Ok(())
    }

    #[test]
    fn duplicate_tool_call_ids_reject_the_entire_batch() {
        let calls = HashMap::from([
            (
                0,
                (
                    "duplicate".to_string(),
                    "Read".to_string(),
                    "{}".to_string(),
                    None,
                ),
            ),
            (
                1,
                (
                    "duplicate".to_string(),
                    "Read".to_string(),
                    "{}".to_string(),
                    None,
                ),
            ),
        ]);
        assert!(build_tool_calls_from_map(&calls).is_err());
    }

    #[test]
    fn process_stream_chunk_accumulates_reasoning_for_protocol_replay()
    -> Result<(), serde_json::Error> {
        let chunk: ChatCompletionChunk = serde_json::from_value(serde_json::json!({
            "choices": [{
                "index": 0,
                "delta": {"reasoning_content": "先分析，再调用工具。"}
            }]
        }))?;
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut calls = HashMap::new();

        process_stream_chunk(&chunk, &mut content, &mut reasoning, &mut calls);

        assert_eq!(reasoning, "先分析，再调用工具。");
        Ok(())
    }

    #[test]
    fn native_message_tool_fragments_preserve_provider_source_for_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = crate::llm::types::ProviderOutputSource {
            protocol: echo_core::llm::LlmApiProtocol::Responses,
            request_id: "response-original".into(),
            item_id: "function-item".into(),
            output_index: 3,
            content_index: None,
            content_kind: Some("function_call".into()),
            phase: None,
        };
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut calls = HashMap::new();
        for delta in [
            serde_json::json!({"tool_calls":[{"index":3,"id":"call-original","provider_source":source,"function":{"name":"read_file","arguments":"{"}}]}),
            serde_json::json!({"tool_calls":[{"index":3,"function":{"arguments":"}"}}]}),
        ] {
            let chunk: ChatCompletionChunk = serde_json::from_value(serde_json::json!({
                "choices":[{"index":0,"delta":delta}]
            }))?;
            process_stream_chunk(&chunk, &mut content, &mut reasoning, &mut calls);
        }
        let (tool_calls, steps) = build_tool_calls_from_map(&calls)?;
        let call = tool_calls.first().ok_or("assembled tool call missing")?;
        assert_eq!(call.provider_source.as_ref(), Some(&source));
        assert_eq!(call.id, "call-original");
        assert_eq!(call.function.arguments, "{}");
        assert_eq!(steps.len(), 1);
        let decoded: crate::llm::types::ToolCall =
            serde_json::from_value(serde_json::to_value(call)?)?;
        assert_eq!(decoded.provider_source.as_ref(), Some(&source));
        Ok(())
    }
}
