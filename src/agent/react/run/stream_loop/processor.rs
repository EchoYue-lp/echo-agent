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
            events.push(AgentEvent::Token(content.clone()));
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
        let args: Value = serde_json::from_str(args_str).unwrap_or(Value::Object(Default::default()));

        msg_tool_calls.push(LlmToolCall {
            id: id.clone(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.clone(),
                arguments: args_str.clone(),
            },
        });
        steps.push((id.clone(), name.clone(), args));
    }

    (msg_tool_calls, steps)
}
