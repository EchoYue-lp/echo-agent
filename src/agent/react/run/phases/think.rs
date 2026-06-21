//! Per-iteration LLM call: fire `on_think_start`, run intervention
//! callbacks, stream LLM chunks, derive token counts and emit `ThinkEnd`.

use super::super::processor::process_stream_chunk;
use super::super::stream_macros::{try_send_or, yield_event_or};
use super::{LoopState, ThinkOutcome, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{ReactError, Result};
use crate::llm::types::{Message, Role, ToolDefinition};
use futures::StreamExt;
use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// LLM-call phase: stream chunks, derive content / tool calls / token counts.
///
/// Returns:
/// - [`ThinkOutcome::Continue`] with the assembled [`ThinkOutput`].
/// - [`ThinkOutcome::Abandoned`] when the channel was closed mid-stream.
/// - [`ThinkOutcome::Cancelled`] / [`ThinkOutcome::Blocked`] when an
///   intervention callback aborted the turn (the error has already been
///   forwarded to the channel and the TaskNode status updated).
pub(crate) async fn run_think(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    state: &mut LoopState,
    messages: Vec<Message>,
) -> Result<ThinkOutcome> {
    let agent = &snap.config.agent_name;
    for cb in snap.config.callbacks.iter() {
        cb.on_think_start(agent, &messages).await;
    }

    // ── Intervention callbacks for think (streaming path) ──
    for intervention in &snap.tools.intervention_callbacks {
        let result = intervention.on_think_start(agent, &messages).await;
        if result.cancel {
            if let Some(ref node_id) = state.task_node_id {
                snap.update_node_status(node_id, crate::state::TaskNodeStatus::Failed)
                    .await;
            }
            let _ = tx.try_send(Err(ReactError::Other(
                "Agent execution cancelled by intervention at think".into(),
            )));
            return Ok(ThinkOutcome::Cancelled);
        }
        if result.block {
            let reason = result
                .block_reason
                .unwrap_or_else(|| "blocked by intervention at think".into());
            if let Some(ref node_id) = state.task_node_id {
                snap.update_node_status(
                    node_id,
                    crate::state::TaskNodeStatus::Blocked {
                        reason: reason.clone(),
                    },
                )
                .await;
            }
            let _ = tx.try_send(Err(ReactError::Other(format!(
                "Think blocked by intervention: {}",
                reason
            ))));
            return Ok(ThinkOutcome::Blocked);
        }
        if let Some(injected) = result.injected_context {
            super::super::context::push_runtime_context_note(
                context,
                "Intervention:ThinkStart",
                &injected,
            )
            .await;
        }
    }

    let mut llm_stream = Box::pin(try_send_or!(
        tx,
        create_llm_stream(snap, messages.clone()).await,
        ThinkOutcome::Abandoned
    ));
    let mut content_buffer = String::new();
    let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
    let mut last_usage = None;
    let mut in_reasoning = false;

    while let Some(cr) = llm_stream.next().await {
        let chunk = try_send_or!(tx, cr, ThinkOutcome::Abandoned);
        if chunk.usage.is_some() {
            last_usage = chunk.usage.clone();
        }
        for event in process_stream_chunk(
            &chunk,
            &mut content_buffer,
            &mut tool_call_map,
            &mut in_reasoning,
            false,
        ) {
            yield_event_or!(tx, event, ThinkOutcome::Abandoned);
        }
    }

    let pt = last_usage
        .as_ref()
        .and_then(|u| u.prompt_tokens)
        .unwrap_or(0) as usize;
    let ct = last_usage
        .as_ref()
        .and_then(|u| u.completion_tokens)
        .unwrap_or(0) as usize;
    let total_tokens = last_usage
        .as_ref()
        .and_then(|u| u.total_tokens)
        .map(|t| t as usize)
        .unwrap_or_else(|| pt.saturating_add(ct));
    let cached_prompt_tokens = last_usage
        .as_ref()
        .map(|u| u.cached_prompt_tokens() as usize)
        .unwrap_or(0);
    let cache_creation_prompt_tokens = last_usage
        .as_ref()
        .map(|u| u.cache_creation_prompt_tokens() as usize)
        .unwrap_or(0);
    let usage_reported = last_usage.is_some();

    // Feed the actual prompt-token count back into the CalibratedTokenizer so
    // future context-window / compression estimates converge to the model's
    // real tokenization. Without this the calibration factor stays at 1.0 and
    // the tokenizer is no more accurate than the raw heuristic.
    if pt > 0 {
        use echo_core::tokenizer::Tokenizer;
        let estimated: usize = messages
            .iter()
            .filter_map(|m| m.text_content())
            .map(|t| snap.calibrated_tokenizer.count_tokens(&t))
            .sum();
        if estimated > 0 {
            snap.calibrated_tokenizer.calibrate(estimated, pt as u32);
        }
    }

    // Record usage in the token tracker for cumulative tracking
    if let Some(ref u) = last_usage {
        snap.token_tracker.record_usage(u);
    }
    tracing::debug!(
        target: "echo_agent::llm_usage",
        agent = %snap.config.agent_name,
        model = %snap.config.model_name,
        prompt_tokens = pt,
        completion_tokens = ct,
        total_tokens = total_tokens,
        cached_prompt_tokens = cached_prompt_tokens,
        cache_creation_prompt_tokens = cache_creation_prompt_tokens,
        usage_reported = usage_reported,
        "LLM usage recorded"
    );
    yield_event_or!(
        tx,
        AgentEvent::LlmUsage {
            model: snap.config.model_name.clone(),
            prompt_tokens: pt,
            completion_tokens: ct,
            total_tokens,
            cached_prompt_tokens,
            cache_creation_prompt_tokens,
            usage_reported,
        },
        ThinkOutcome::Abandoned
    );

    if in_reasoning {
        yield_event_or!(
            tx,
            AgentEvent::ThinkEnd {
                prompt_tokens: pt,
                completion_tokens: ct,
            },
            ThinkOutcome::Abandoned
        );
    }

    if !content_buffer.is_empty() {
        if !tool_call_map.is_empty() {
            yield_event_or!(tx, AgentEvent::ThinkStart, ThinkOutcome::Abandoned);
            yield_event_or!(
                tx,
                AgentEvent::Token(content_buffer.clone()),
                ThinkOutcome::Abandoned
            );
            yield_event_or!(
                tx,
                AgentEvent::ThinkEnd {
                    prompt_tokens: pt,
                    completion_tokens: ct,
                },
                ThinkOutcome::Abandoned
            );
        } else {
            yield_event_or!(
                tx,
                AgentEvent::Token(content_buffer.clone()),
                ThinkOutcome::Abandoned
            );
        }
    }

    Ok(ThinkOutcome::Continue(ThinkOutput {
        messages,
        content_buffer,
        tool_call_map,
        pt,
        ct,
    }))
}

/// Create a streaming LLM call wrapped in retry / circuit-breaker policy.
pub(crate) async fn create_llm_stream(
    snap: &AgentRunSnapshot,
    messages: Vec<Message>,
) -> Result<
    std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
    >,
> {
    let tools = if snap.config.enable_tool {
        let t = snap.tools.tool_manager.get_openai_tools();
        if t.is_empty() { None } else { Some(t) }
    } else {
        None
    };
    log_prompt_cache_shape(&messages, tools.as_deref());
    let cancel = snap.cancel_token.clone();

    // ── Trait path: when an LlmClient trait object is attached (production
    // OpenAiClient / test MockLlmClient), route through it. This avoids the
    // per-call model-resolve (Config::get_model) of the legacy reqwest path,
    // which is what makes the core loop testable with a mock and removes the
    // NotFindModelError dependency on echo-agent-models.yaml.
    if let Some(llm_client) = snap.llm_client.clone() {
        type ChunkStream = std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
        >;
        let stream: ChunkStream = super::super::retry::retry_llm_call(
            &snap.config.agent_name,
            snap.config.llm_max_retries,
            snap.config.llm_retry_delay_ms,
            &snap.guard.circuit_breaker,
            || {
                let llm_client = llm_client.clone();
                let ms = messages.clone();
                let t = tools.clone();
                let temp = snap.config.temperature;
                let max_tokens = snap.config.max_tokens;
                async move {
                    let request = crate::llm::ChatRequest {
                        messages: ms,
                        temperature: temp,
                        max_tokens,
                        tools: t,
                        tool_choice: None,
                        response_format: None,
                        thinking: snap.thinking.clone(),
                        cancel_token: snap.cancel_token.clone(),
                        user_id: snap.config.cache_user_id.clone(),
                    };
                    let inner = llm_client.chat_stream(request).await?;
                    // Adapt the trait's flattened ChatChunk back into the
                    // ChatCompletionChunk shape consumed downstream (think
                    // phase, direct_answer_stream). Both originate from the
                    // same OpenAI stream, so no information is lost.
                    let mapped = inner.map(|chunk_result| {
                        chunk_result.map(|c| crate::llm::types::ChatCompletionChunk {
                            id: String::new(),
                            choices: vec![crate::llm::types::ChunkChoice {
                                delta: c.delta,
                                finish_reason: c.finish_reason,
                                index: 0,
                            }],
                            usage: c.usage,
                        })
                    });
                    Ok(Box::pin(mapped) as ChunkStream)
                }
            },
        )
        .await?;
        return Ok(stream);
    }

    // ── Legacy reqwest fallback (no LlmClient injected) ──
    let stream = super::super::retry::retry_llm_call(
        &snap.config.agent_name,
        snap.config.llm_max_retries,
        snap.config.llm_retry_delay_ms,
        &snap.guard.circuit_breaker,
        || {
            let c = snap.client.clone();
            let m = snap.config.model_name.clone();
            let ms = messages.clone();
            let t = tools.clone();
            let ct = cancel.clone();
            async move {
                let s = crate::llm::stream_chat(
                    c,
                    &m,
                    ms,
                    snap.config.temperature,
                    snap.config.max_tokens,
                    t,
                    None,
                    None,
                    ct,
                )
                .await?;
                Ok(Box::pin(s)
                    as std::pin::Pin<
                        Box<
                            dyn futures::Stream<
                                    Item = Result<crate::llm::types::ChatCompletionChunk>,
                                > + Send,
                        >,
                    >)
            }
        },
    )
    .await?;
    Ok(stream)
}

fn log_prompt_cache_shape(messages: &[Message], tools: Option<&[ToolDefinition]>) {
    let shape = PromptCacheShape::from_messages(messages, tools);
    tracing::debug!(
        target: "echo_agent::prompt_cache",
        prefix_hash = %shape.prefix_hash,
        message_count = shape.message_count,
        leading_system_messages = shape.leading_system_messages,
        cwd_system_messages = shape.cwd_system_messages,
        memory_system_messages = shape.memory_system_messages,
        tool_count = shape.tool_count,
        "LLM prompt cache shape"
    );
    if shape.cwd_system_messages > 1 {
        tracing::warn!(
            target: "echo_agent::prompt_cache",
            cwd_system_messages = shape.cwd_system_messages,
            "Multiple cwd system messages found; prompt-cache prefix is likely unstable"
        );
    }
    if shape.memory_system_messages > 0 {
        tracing::warn!(
            target: "echo_agent::prompt_cache",
            memory_system_messages = shape.memory_system_messages,
            "Dynamic memory context is present as a system message; prompt-cache prefix is likely unstable"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptCacheShape {
    prefix_hash: String,
    message_count: usize,
    leading_system_messages: usize,
    cwd_system_messages: usize,
    memory_system_messages: usize,
    tool_count: usize,
}

impl PromptCacheShape {
    fn from_messages(messages: &[Message], tools: Option<&[ToolDefinition]>) -> Self {
        let leading_system_messages = messages
            .iter()
            .take_while(|message| matches!(message.role, Role::System))
            .count();
        let cwd_system_messages = messages
            .iter()
            .filter(|message| {
                matches!(message.role, Role::System)
                    && message
                        .text_content()
                        .is_some_and(|text| text.contains("Current working directory:"))
            })
            .count();
        let memory_system_messages = messages
            .iter()
            .filter(|message| {
                matches!(message.role, Role::System)
                    && message
                        .text_content()
                        .is_some_and(|text| text.contains("[memory_context]"))
            })
            .count();
        let tool_count = tools.map(|defs| defs.len()).unwrap_or(0);
        let mut hasher = StableFnv64::new();
        for message in messages.iter().take(leading_system_messages) {
            hasher.write_str(message.role.as_str());
            if let Some(text) = message.text_content() {
                hasher.write_str(&text);
            }
        }
        if let Some(defs) = tools
            && let Ok(serialized) = serde_json::to_string(defs)
        {
            hasher.write_str(&serialized);
        }
        Self {
            prefix_hash: format!("{:016x}", hasher.finish()),
            message_count: messages.len(),
            leading_system_messages,
            cwd_system_messages,
            memory_system_messages,
            tool_count,
        }
    }
}

struct StableFnv64(u64);

impl StableFnv64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn write_str(&mut self, value: &str) {
        self.write(value.as_bytes());
    }
}

impl Hasher for StableFnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_cache_shape_hash_ignores_non_prefix_user_turns() {
        let first = vec![
            Message::system("stable system".to_string()),
            Message::user("first request".to_string()),
        ];
        let second = vec![
            Message::system("stable system".to_string()),
            Message::user("second request".to_string()),
        ];

        let first_shape = PromptCacheShape::from_messages(&first, None);
        let second_shape = PromptCacheShape::from_messages(&second, None);

        assert_eq!(first_shape.prefix_hash, second_shape.prefix_hash);
        assert_eq!(first_shape.leading_system_messages, 1);
        assert_eq!(second_shape.leading_system_messages, 1);
    }

    #[test]
    fn prompt_cache_shape_counts_duplicate_cwd_system_messages() {
        let messages = vec![
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::user("hello".to_string()),
        ];

        let shape = PromptCacheShape::from_messages(&messages, None);

        assert_eq!(shape.leading_system_messages, 2);
        assert_eq!(shape.cwd_system_messages, 2);
    }
}
