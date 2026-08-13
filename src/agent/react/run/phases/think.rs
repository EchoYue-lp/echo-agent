//! Per-iteration LLM call: fire `on_think_start`, run intervention
//! callbacks, stream LLM chunks, derive token counts and emit `ThinkEnd`.

use super::super::processor::process_stream_chunk;
use super::super::stream_macros::{try_send_or, yield_event_or};
use super::{LoopState, ThinkOutcome, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use crate::llm::types::{Message, Role, ToolDefinition};
use echo_core::tokenizer::Tokenizer;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
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
    final_only: bool,
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
            snap.finalize_run(
                crate::trace::RunStatus::Cancelled,
                None,
                Some("Agent execution cancelled by intervention at think"),
            )
            .await;
            let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
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
            let _ = tx
                .send(Ok(AgentEvent::error_message(
                    "intervention",
                    format!("Think blocked by intervention: {reason}"),
                )))
                .await;
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

    let estimated_context_tokens = messages.iter().fold(0usize, |total, message| {
        total.saturating_add(
            message
                .content
                .estimated_tokens(snap.calibrated_tokenizer.as_ref()),
        )
    });
    let context_breakdown =
        crate::trace::LlmContextBreakdown::estimate(&messages, snap.calibrated_tokenizer.as_ref());
    let request_tools = tools_for_request(snap, final_only);
    try_send_or!(
        tx,
        validate_request_budget(
            snap,
            estimated_context_tokens,
            request_tools.as_deref().unwrap_or_default(),
        ),
        ThinkOutcome::Failed
    );
    let cache_fingerprint = cache_fingerprint(&messages, request_tools.as_deref());
    let (protected_message_count, protected_context_tokens) = {
        let context = context.lock().await;
        (
            context.protected_message_count(),
            context.protected_token_estimate(),
        )
    };
    let llm_started = Instant::now();
    let mut llm_stream = Box::pin(try_send_or!(
        tx,
        create_llm_stream(snap, messages.clone(), final_only).await,
        ThinkOutcome::Failed
    ));
    let mut content_buffer = String::new();
    let mut reasoning_buffer = String::new();
    let mut reasoning_blocks = Vec::new();
    let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
    let mut last_usage = None;
    let mut finish_reason = None::<String>;
    let mut in_reasoning = false;

    loop {
        let next = tokio::select! {
            biased;
            _ = async {
                match snap.cancel_token.as_ref() {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => {
                snap.finalize_run(
                    crate::trace::RunStatus::Cancelled,
                    None,
                    Some("Agent execution cancelled during model response"),
                ).await;
                let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                return Ok(ThinkOutcome::Cancelled);
            }
            next = llm_stream.next() => next,
        };
        let Some(cr) = next else {
            break;
        };
        let chunk = try_send_or!(tx, cr, ThinkOutcome::Failed);
        for reason in chunk
            .choices
            .iter()
            .filter_map(|choice| choice.finish_reason.as_ref())
        {
            finish_reason = Some(reason.clone());
        }
        if chunk.usage.is_some() {
            last_usage = chunk.usage.clone();
        }
        for blocks in chunk
            .choices
            .iter()
            .filter_map(|choice| choice.delta.reasoning_blocks.as_ref())
        {
            reasoning_blocks.extend(blocks.iter().cloned());
        }
        for event in process_stream_chunk(
            &chunk,
            &mut content_buffer,
            &mut reasoning_buffer,
            &mut tool_call_map,
            &mut in_reasoning,
            false,
        ) {
            yield_event_or!(tx, event, ThinkOutcome::Abandoned);
        }
    }

    match finish_reason.as_deref() {
        Some("stop" | "tool_calls" | "function_call") => {}
        Some(reason) => {
            let error =
                crate::error::ReactError::Llm(Box::new(crate::error::LlmError::InvalidResponse(
                    format!("model stream ended with non-success finish reason '{reason}'"),
                )));
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx.send(Err(error)).await;
            return Ok(ThinkOutcome::Failed);
        }
        None => {
            let error =
                crate::error::ReactError::Llm(Box::new(crate::error::LlmError::InvalidResponse(
                    "model stream ended without a finish reason; response may be truncated"
                        .to_string(),
                )));
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx.send(Err(error)).await;
            return Ok(ThinkOutcome::Failed);
        }
    }

    let pt = last_usage
        .as_ref()
        .map(|usage| usage.effective_prompt_tokens())
        .unwrap_or(0) as usize;
    let ct = last_usage
        .as_ref()
        .and_then(|u| u.completion_tokens)
        .unwrap_or(0) as usize;
    let total_tokens = last_usage
        .as_ref()
        .map(|usage| usage.effective_total_tokens() as usize)
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

    snap.record_event(crate::trace::RunEvent::LlmCall {
        messages: messages.len(),
        prompt_tokens: u32::try_from(pt).unwrap_or(u32::MAX),
        completion_tokens: u32::try_from(ct).unwrap_or(u32::MAX),
        cached_prompt_tokens: u32::try_from(cached_prompt_tokens).unwrap_or(u32::MAX),
        cache_creation_prompt_tokens: u32::try_from(cache_creation_prompt_tokens)
            .unwrap_or(u32::MAX),
        usage_reported,
        estimated_context_tokens,
        protected_context_tokens,
        protected_message_count,
        context_limit_tokens: snap.config.token_limit,
        context_breakdown,
        cache_fingerprint,
        duration_ms: u64::try_from(llm_started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
    .await;

    // Feed the actual prompt-token count back into the CalibratedTokenizer so
    // future context-window / compression estimates converge to the model's
    // real tokenization. Without this the calibration factor stays at 1.0 and
    // the tokenizer is no more accurate than the raw heuristic.
    if pt > 0 {
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
    #[cfg(feature = "telemetry")]
    {
        let provider = snap.config.provider.as_deref().unwrap_or("unknown");
        let status = if content_buffer.is_empty() && tool_call_map.is_empty() {
            "empty"
        } else {
            "success"
        };
        crate::telemetry::Metrics::record_llm_call(provider, &snap.config.model_name, status);
        crate::telemetry::Metrics::record_llm_latency(
            provider,
            &snap.config.model_name,
            llm_started.elapsed().as_secs_f64() * 1000.0,
        );
        crate::telemetry::Metrics::record_llm_tokens(
            provider,
            &snap.config.model_name,
            "input",
            u64::try_from(pt).unwrap_or(u64::MAX),
        );
        crate::telemetry::Metrics::record_llm_tokens(
            provider,
            &snap.config.model_name,
            "output",
            u64::try_from(ct).unwrap_or(u64::MAX),
        );
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
        content_buffer,
        reasoning_buffer,
        reasoning_blocks,
        tool_call_map,
        pt,
        ct,
        usage_reported,
    }))
}

/// Create a streaming LLM call wrapped in retry / circuit-breaker policy.
pub(crate) async fn create_llm_stream(
    snap: &AgentRunSnapshot,
    messages: Vec<Message>,
    final_only: bool,
) -> Result<
    std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
    >,
> {
    let tools = tools_for_request(snap, final_only);
    log_prompt_cache_shape(&messages, tools.as_deref());
    let cancel = snap.cancel_token.clone();

    // ── Trait path: when an LlmClient trait object is attached (production
    // OpenAiClient / test MockLlmClient), route through it. This avoids the
    // per-call model-resolve (Config::get_model) of the legacy reqwest path,
    // which is what makes the core loop testable with a mock and removes the
    // NotFindModelError dependency on echo-agent-models.yaml.
    // tracing::info!(
    //     agent = %snap.config.agent_name,
    //     model = %snap.config.model_name,
    //     has_llm_client = snap.llm_client.is_some(),
    //     "think: LLM call path selection"
    // );
    if let Some(llm_client) = snap.llm_client.clone() {
        type ChunkStream = std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
        >;
        let stream: ChunkStream = super::super::retry::retry_llm_call(
            &snap.config.agent_name,
            snap.config.llm_max_retries,
            snap.config.llm_retry_delay_ms,
            &snap.guard.circuit_breaker,
            snap.cancel_token.as_ref(),
            || {
                let llm_client = llm_client.clone();
                let ms = messages.clone();
                let t = tools.clone();
                let temp = snap.config.temperature;
                let max_tokens = snap.config.max_tokens;
                async move {
                    // Build cache hints before moving ownership into ChatRequest.
                    let tools_ref: &[ToolDefinition] = t.as_deref().unwrap_or(&[]);
                    let layout =
                        echo_core::llm::cache::PromptCacheLayout::from_messages(&ms, tools_ref);
                    let prefix_hash = echo_core::llm::cache::diagnostic::stable_prefix_hash(
                        layout.system,
                        layout.canonical,
                        layout.tools,
                        layout.history,
                    );
                    let segments = layout.segment_ranges();
                    let request = crate::llm::ChatRequest {
                        messages: ms,
                        temperature: temp,
                        max_tokens,
                        tools: t,
                        tool_choice: (final_only && snap.config.supports_tool_choice_none)
                            .then(|| "none".to_string()),
                        response_format: None,
                        thinking: snap.thinking.clone(),
                        cancel_token: snap.cancel_token.clone(),
                        user_id: snap.config.cache_user_id.clone(),
                        cache_hints: Some(echo_core::llm::cache::CacheHints {
                            breakpoints: vec![],
                            stable_prefix_hash: Some(prefix_hash),
                            segments,
                        }),
                    };
                    let inner = llm_client.chat_stream(request).await?;
                    // Adapt the trait's flattened ChatChunk back into the
                    // ChatCompletionChunk shape consumed by the think phase.
                    // Both originate from the same stream, so no information is lost.
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
        snap.cancel_token.as_ref(),
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
                    (final_only && snap.config.supports_tool_choice_none)
                        .then(|| "none".to_string()),
                    None,
                    ct,
                    None,
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

fn tools_for_request(snap: &AgentRunSnapshot, final_only: bool) -> Option<Vec<ToolDefinition>> {
    if !snap.config.enable_tool || final_only {
        return None;
    }
    let tools = snap.tools.tools_for_llm();
    if tools.is_empty() {
        return None;
    }
    if let Ok(stats) = echo_execution::tools::ToolManager::schema_stats_for(&tools) {
        snap.tools.tool_manager.record_schema_stats(&stats);
        tracing::info!(
            target: "echo_agent::tool_budget",
            tool_count = stats.tool_count,
            schema_bytes = stats.schema_bytes,
            schema_estimated_tokens = stats.estimated_tokens,
            "model tool schema budget"
        );
    }
    Some(tools)
}

fn validate_request_budget(
    snap: &AgentRunSnapshot,
    message_tokens: usize,
    tools: &[ToolDefinition],
) -> Result<()> {
    if let Some(error) = &snap.config.token_budget_error {
        return Err(crate::error::ReactError::Other(error.clone()));
    }
    let window = snap.config.token_limit;
    if window == usize::MAX {
        return Ok(());
    }
    let tool_tokens = serde_json::to_string(tools)
        .map_err(|error| crate::error::ReactError::Other(error.to_string()))
        .map(|schema| snap.calibrated_tokenizer.count_tokens(&schema))?;
    let default_output = window / 10;
    let output_tokens = snap
        .config
        .max_tokens
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default_output);
    let safety_tokens = window / 20;
    let required = message_tokens
        .saturating_add(tool_tokens)
        .saturating_add(output_tokens)
        .saturating_add(safety_tokens);
    if required > window {
        return Err(crate::error::AgentError::ContextLimitExceeded(format!(
            "request requires approximately {required} tokens (messages {message_tokens}, tools {tool_tokens}, output {output_tokens}, safety {safety_tokens}) but the model window is {window}"
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn cache_fingerprint(
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) -> echo_core::llm::cache::PromptCacheFingerprint {
    let layout =
        echo_core::llm::cache::PromptCacheLayout::from_messages(messages, tools.unwrap_or(&[]));
    echo_core::llm::cache::prompt_cache_fingerprint(
        layout.system,
        layout.canonical,
        layout.tools,
        layout.history,
    )
}

fn log_prompt_cache_shape(messages: &[Message], tools: Option<&[ToolDefinition]>) {
    let fingerprint = cache_fingerprint(messages, tools);
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
    tracing::debug!(
        target: "echo_agent::prompt_cache",
        prefix_hash = %fingerprint.stable_prefix_hash,
        system_prefix_hash = %fingerprint.system_prefix_hash,
        tools_schema_hash = %fingerprint.tools_schema_hash,
        message_count = messages.len(),
        leading_system_messages,
        cwd_system_messages,
        memory_system_messages,
        tool_count = fingerprint.tool_count,
        "LLM prompt cache shape"
    );
    if cwd_system_messages > 1 {
        tracing::warn!(
            target: "echo_agent::prompt_cache",
            cwd_system_messages,
            "Multiple cwd system messages found; prompt-cache prefix is likely unstable"
        );
    }
    if memory_system_messages > 0 {
        tracing::warn!(
            target: "echo_agent::prompt_cache",
            memory_system_messages,
            "Dynamic memory context is present as a system message; prompt-cache prefix is likely unstable"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_cache_fingerprint_isolates_stable_system_from_history() {
        let first = vec![
            Message::system("stable system".to_string()),
            Message::user("first request".to_string()),
        ];
        let second = vec![
            Message::system("stable system".to_string()),
            Message::user("second request".to_string()),
        ];

        let first_shape = cache_fingerprint(&first, None);
        let second_shape = cache_fingerprint(&second, None);

        assert_eq!(
            first_shape.system_prefix_hash,
            second_shape.system_prefix_hash
        );
        assert_ne!(
            first_shape.stable_prefix_hash,
            second_shape.stable_prefix_hash
        );
    }

    #[test]
    fn duplicate_system_messages_change_system_component_hash() {
        let duplicate = vec![
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::user("hello".to_string()),
        ];
        let single = vec![
            Message::system("Current working directory: /tmp/a".to_string()),
            Message::user("hello".to_string()),
        ];

        let duplicate = cache_fingerprint(&duplicate, None);
        let single = cache_fingerprint(&single, None);

        assert_ne!(duplicate.system_prefix_hash, single.system_prefix_hash);
    }
}
