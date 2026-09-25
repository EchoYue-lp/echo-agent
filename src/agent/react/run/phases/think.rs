//! Per-iteration LLM call: fire `on_think_start`, run intervention
//! callbacks, stream LLM chunks, derive token counts and emit `ThinkEnd`.

use super::super::processor::process_stream_chunk;
use super::super::stream_macros::yield_event_or;
use super::{ThinkOutcome, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::Result;
use crate::llm::types::{ContentPart, Message, ReasoningBlock, Role, ToolDefinition};
use echo_core::tokenizer::{CalibratedTokenizer, Tokenizer};
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
/// - [`ThinkOutcome::TerminalSettled`] when this phase already persisted and
///   published the single terminal outcome.
pub(crate) async fn run_think(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
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
            super::finalize::settle_terminal_projection(
                snap,
                context,
                Some("Agent execution cancelled by intervention at think".to_string()),
                tx,
            )
            .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Cancelled,
                None,
                Some("Agent execution cancelled by intervention at think"),
            )
            .await;
            let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
            });
        }
        if result.block {
            let reason = result
                .block_reason
                .unwrap_or_else(|| "blocked by intervention at think".into());
            super::finalize::settle_terminal_projection(
                snap,
                context,
                Some(format!("Think blocked by intervention: {reason}")),
                tx,
            )
            .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&format!("Think blocked by intervention: {reason}")),
            )
            .await;
            let _ = tx
                .send(Ok(AgentEvent::error_message(
                    "intervention",
                    format!("Think blocked by intervention: {reason}"),
                )))
                .await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
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

    let request = build_llm_request(snap, messages, final_only);
    let request_estimate = estimate_request_prompt(&request, snap.calibrated_tokenizer.as_ref())
        .and_then(|estimate| {
            validate_request_budget(snap, &estimate)?;
            Ok(estimate)
        });
    let request_estimate = match request_estimate {
        Ok(estimate) => estimate,
        Err(error) => {
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx
                .send(Ok(AgentEvent::from_error("react_loop", &error)))
                .await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
    };
    let estimated_context_tokens = request_estimate.message_tokens;
    let context_breakdown = crate::trace::LlmContextBreakdown::estimate(
        &request.messages,
        snap.calibrated_tokenizer.as_ref(),
    );
    let cache_fingerprint = cache_fingerprint(&request.messages, request.tools.as_deref());
    let message_count = request.messages.len();
    let (protected_message_count, protected_context_tokens) = {
        let context = context.lock().await;
        (
            context.protected_message_count(),
            context.protected_token_estimate(),
        )
    };
    let llm_started = Instant::now();
    let mut llm_stream = match create_llm_stream(snap, request).await {
        Ok(stream) => Box::pin(stream),
        Err(error) => {
            if snap
                .cancel_token
                .as_ref()
                .is_some_and(crate::agent::CancellationToken::is_cancelled)
            {
                let reason = "Agent execution cancelled before model response";
                super::finalize::settle_terminal_projection(
                    snap,
                    context,
                    Some(reason.to_string()),
                    tx,
                )
                .await?;
                snap.finalize_run(crate::trace::RunStatus::Cancelled, None, Some(reason))
                    .await;
                let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                return Ok(ThinkOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                });
            }
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            let _ = tx
                .send(Ok(AgentEvent::from_error("react_loop", &error)))
                .await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
    };
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
                super::finalize::settle_terminal_projection(
                    snap,
                    context,
                    Some("Agent execution cancelled during model response".to_string()),
                    tx,
                ).await?;
                snap.finalize_run(
                    crate::trace::RunStatus::Cancelled,
                    None,
                    Some("Agent execution cancelled during model response"),
                ).await;
                let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                return Ok(ThinkOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                });
            }
            next = llm_stream.next() => next,
        };
        let Some(cr) = next else {
            break;
        };
        let chunk = match cr {
            Ok(chunk) => chunk,
            Err(error) => {
                if snap
                    .cancel_token
                    .as_ref()
                    .is_some_and(crate::agent::CancellationToken::is_cancelled)
                {
                    super::finalize::settle_terminal_projection(
                        snap,
                        context,
                        Some("Agent execution cancelled during model response".to_string()),
                        tx,
                    )
                    .await?;
                    snap.finalize_run(
                        crate::trace::RunStatus::Cancelled,
                        None,
                        Some("Agent execution cancelled during model response"),
                    )
                    .await;
                    let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
                    return Ok(ThinkOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                    });
                }
                super::finalize::settle_terminal_projection(
                    snap,
                    context,
                    Some(error.to_string()),
                    tx,
                )
                .await?;
                snap.finalize_run(
                    crate::trace::RunStatus::Failed,
                    None,
                    Some(&error.to_string()),
                )
                .await;
                emit_partial_content_before_failure(snap, tx, &content_buffer).await;
                let _ = tx
                    .send(Ok(AgentEvent::from_error("react_loop", &error)))
                    .await;
                return Ok(ThinkOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Failed,
                });
            }
        };
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
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            emit_partial_content_before_failure(snap, tx, &content_buffer).await;
            let _ = tx.send(Err(error)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
        }
        None => {
            let error =
                crate::error::ReactError::Llm(Box::new(crate::error::LlmError::InvalidResponse(
                    "model stream ended without a finish reason; response may be truncated"
                        .to_string(),
                )));
            super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx)
                .await?;
            snap.finalize_run(
                crate::trace::RunStatus::Failed,
                None,
                Some(&error.to_string()),
            )
            .await;
            emit_partial_content_before_failure(snap, tx, &content_buffer).await;
            let _ = tx.send(Err(error)).await;
            return Ok(ThinkOutcome::TerminalSettled {
                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
            });
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
        messages: message_count,
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

    // Only comparable provider prompt usage may update the factor. Provider
    // file fallback and reasoning replay have no portable token estimate.
    if request_estimate.calibratable
        && let Some(actual) = last_usage
            .as_ref()
            .filter(|usage| usage.prompt_tokens.is_some())
            .map(|usage| usage.effective_prompt_tokens())
            .filter(|actual| *actual > 0)
    {
        snap.calibrated_tokenizer
            .calibrate(request_estimate.raw_tokens, actual);
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

    if !content_buffer.is_empty() && !tool_call_map.is_empty() {
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

async fn emit_partial_content_before_failure(
    snap: &AgentRunSnapshot,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    content: &str,
) {
    if !content.is_empty()
        && let Ok(content) = snap.check_final_answer_guard(content).await
    {
        let _ = tx.send(Ok(AgentEvent::Token(content))).await;
    }
}

fn build_llm_request(
    snap: &AgentRunSnapshot,
    messages: Vec<Message>,
    final_only: bool,
) -> crate::llm::ChatRequest {
    let tools = tools_for_request(snap, final_only);
    let layout = echo_core::llm::cache::PromptCacheLayout::from_messages(
        &messages,
        tools.as_deref().unwrap_or(&[]),
    );
    let prefix_hash = echo_core::llm::cache::diagnostic::stable_prefix_hash(
        layout.system,
        layout.canonical,
        layout.tools,
        layout.history,
    );
    let segments = layout.segment_ranges();
    let response_format = match snap.config.response_format.as_ref() {
        Some(crate::llm::ResponseFormat::Text) | None => None,
        format => format.cloned(),
    };
    crate::llm::ChatRequest {
        messages,
        temperature: snap.config.temperature,
        max_tokens: snap.config.max_tokens,
        tools,
        tool_choice: (final_only && snap.config.supports_tool_choice_none)
            .then(|| "none".to_string()),
        response_format,
        thinking: snap.thinking.clone(),
        cancel_token: snap.cancel_token.clone(),
        timeouts: None,
        user_id: snap.config.cache_user_id.clone(),
        cache_hints: Some(echo_core::llm::cache::CacheHints {
            breakpoints: vec![],
            stable_prefix_hash: Some(prefix_hash),
            segments,
        }),
    }
}

/// Create a streaming LLM call wrapped in retry / circuit-breaker policy.
pub(crate) async fn create_llm_stream(
    snap: &AgentRunSnapshot,
    request: crate::llm::ChatRequest,
) -> Result<
    std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>> + Send>,
    >,
> {
    if request
        .response_format
        .as_ref()
        .is_some_and(crate::llm::ResponseFormat::is_json)
        && !snap.config.supports_structured_output
    {
        return Err(crate::error::ConfigError::UnMatchConfigError(
            snap.config.model_name.clone(),
            "configured response_format requires a fresh structured-output model capability"
                .to_string(),
        )
        .into());
    }
    log_prompt_cache_shape(&request.messages, request.tools.as_deref());

    // ── Trait path: when an LlmClient trait object is attached (production
    // OpenAiClient / test MockLlmClient), route through it. This avoids the
    // per-call model resolution, which keeps the core loop testable with a
    // mock and avoids coupling execution to a global model registry.
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
                let request = request.clone();
                async move {
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

    Err(crate::error::ReactError::Other(format!(
        "agent '{}' has no LlmClient; inject an explicit LlmConfig or LlmClient",
        snap.config.agent_name
    )))
}

struct PromptTokenEstimate {
    raw_tokens: usize,
    message_tokens: usize,
    tool_tokens: usize,
    format_tokens: usize,
    calibratable: bool,
}

struct RequestFactorTokenizer<'a> {
    base: &'a dyn Tokenizer,
    factor: f64,
}

impl Tokenizer for RequestFactorTokenizer<'_> {
    fn count_tokens(&self, text: &str) -> usize {
        (self.base.count_tokens(text) as f64 * self.factor).round() as usize
    }
}

fn serialized_base_tokens<T: serde::Serialize + ?Sized>(
    value: &T,
    base: &dyn Tokenizer,
) -> Result<usize> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
    Ok(base.count_tokens(&encoded))
}

fn estimate_request_prompt(
    request: &crate::llm::ChatRequest,
    tokenizer: &CalibratedTokenizer,
) -> Result<PromptTokenEstimate> {
    let base = tokenizer.base_tokenizer();
    let factor = tokenizer.calibration_factor();
    let adjusted = RequestFactorTokenizer { base, factor };
    let adjust = |tokens: usize| (tokens as f64 * factor).round() as usize;
    let mut raw_messages = 0usize;
    let mut message_tokens = 0usize;
    let mut calibratable = true;
    for message in &request.messages {
        if message.content.parts().is_some_and(|parts| {
            parts.iter().any(|part| {
                matches!(
                    part,
                    ContentPart::ImageUrl { .. } | ContentPart::File { .. }
                )
            })
        }) {
            // Image prices are provider-specific and file bytes may be
            // replaced with extracted text or a name-only fallback.
            calibratable = false;
        }
        let mut tokens = message
            .content
            .estimated_tokens(base)
            .saturating_add(base.count_tokens(message.role.as_str()));
        let mut adjusted_tokens = message
            .content
            .estimated_tokens(&adjusted)
            .saturating_add(adjusted.count_tokens(message.role.as_str()));
        if let Some(tool_calls) = &message.tool_calls {
            let tool_call_tokens = serialized_base_tokens(tool_calls, base)?;
            tokens = tokens.saturating_add(tool_call_tokens);
            adjusted_tokens = adjusted_tokens.saturating_add(adjust(tool_call_tokens));
        }
        for text in [message.name.as_deref(), message.tool_call_id.as_deref()]
            .into_iter()
            .flatten()
        {
            tokens = tokens.saturating_add(base.count_tokens(text));
            adjusted_tokens = adjusted_tokens.saturating_add(adjusted.count_tokens(text));
        }
        if message.reasoning_content.is_some() {
            calibratable = false;
        }
        if let Some(blocks) = &message.reasoning_blocks {
            // Some providers replay signed text while others drop the blocks
            // and send reasoning_content instead. Do not learn across them.
            calibratable = false;
            for block in blocks {
                match block {
                    ReasoningBlock::Signed { thinking, .. } => {
                        tokens = tokens.saturating_add(base.count_tokens(thinking));
                        adjusted_tokens =
                            adjusted_tokens.saturating_add(adjusted.count_tokens(thinking));
                    }
                    ReasoningBlock::Redacted { .. } | ReasoningBlock::Opaque { .. } => {}
                }
            }
        } else if let Some(reasoning_content) = &message.reasoning_content {
            tokens = tokens.saturating_add(base.count_tokens(reasoning_content));
            adjusted_tokens =
                adjusted_tokens.saturating_add(adjusted.count_tokens(reasoning_content));
        }
        raw_messages = raw_messages.saturating_add(tokens);
        message_tokens = message_tokens.saturating_add(adjusted_tokens);
    }
    let raw_tools = request
        .tools
        .as_ref()
        .map(|tools| serialized_base_tokens(tools, base))
        .transpose()?
        .unwrap_or(0);
    let raw_format = request
        .response_format
        .as_ref()
        .map(|format| serialized_base_tokens(format, base))
        .transpose()?
        .unwrap_or(0);
    let raw_tokens = raw_messages
        .saturating_add(raw_tools)
        .saturating_add(raw_format);

    Ok(PromptTokenEstimate {
        raw_tokens,
        message_tokens,
        tool_tokens: adjust(raw_tools),
        format_tokens: adjust(raw_format),
        calibratable,
    })
}

/// Reserve the current tool and format schema before ContextManager prepares
/// messages. The later immutable ChatRequest remains the final budget authority
/// if intervention or tool visibility changes after this estimate.
pub(super) fn estimate_pre_compaction_overhead(snap: &AgentRunSnapshot) -> Result<usize> {
    let request = build_llm_request(snap, Vec::new(), false);
    let estimate = estimate_request_prompt(&request, snap.calibrated_tokenizer.as_ref())?;
    Ok(estimate.tool_tokens.saturating_add(estimate.format_tokens))
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

fn validate_request_budget(snap: &AgentRunSnapshot, estimate: &PromptTokenEstimate) -> Result<()> {
    if let Some(error) = &snap.config.token_budget_error {
        return Err(crate::error::ReactError::Other(error.clone()));
    }
    let window = snap.config.token_limit;
    if window == usize::MAX {
        return Ok(());
    }
    let message_tokens = estimate.message_tokens;
    let tool_tokens = estimate.tool_tokens;
    let format_tokens = estimate.format_tokens;
    let default_output = window / 10;
    let output_tokens = snap
        .config
        .max_tokens
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default_output);
    let safety_tokens = window / 20;
    let required = message_tokens
        .saturating_add(tool_tokens)
        .saturating_add(format_tokens)
        .saturating_add(output_tokens)
        .saturating_add(safety_tokens);
    if required > window {
        return Err(crate::error::AgentError::ContextLimitExceeded(format!(
            "request requires approximately {required} tokens (messages {message_tokens}, tools {tool_tokens}, format {format_tokens}, output {output_tokens}, safety {safety_tokens}) but the model window is {window}"
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
    use crate::agent::react::builder::ReactAgentBuilder;
    use crate::llm::types::{ImageUrl, ResponseFormat, Usage};
    use crate::testing::{MockLlmClient, MockTool};
    use echo_core::tokenizer::HeuristicTokenizer;

    struct BlockPartialOutput;

    impl crate::guard::Guard for BlockPartialOutput {
        fn name(&self) -> &str {
            "block_partial_output"
        }

        fn check<'a>(
            &'a self,
            _content: &'a str,
            direction: crate::guard::GuardDirection,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>>
        {
            Box::pin(async move {
                if direction == crate::guard::GuardDirection::Output {
                    Ok(crate::guard::GuardResult::Block {
                        reason: "partial content rejected".to_string(),
                    })
                } else {
                    Ok(crate::guard::GuardResult::Pass)
                }
            })
        }
    }

    #[tokio::test]
    async fn failed_provider_partial_output_cannot_bypass_output_guard() {
        let mut agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "test-model",
            "guarded",
            "sys",
        ));
        agent.set_guard_manager(echo_core::guard::GuardManager::from_guards(vec![
            std::sync::Arc::new(BlockPartialOutput),
        ]));
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = mpsc::channel::<Result<AgentEvent>>(4);
        emit_partial_content_before_failure(&snapshot, &tx, "unfiltered partial").await;
        assert!(rx.try_recv().is_err());
    }

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

    #[tokio::test]
    async fn production_feedback_converges_from_entire_request_base_estimate() -> Result<()> {
        let mut agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new()))
            .tool(Box::new(MockTool::new("calibration_probe")))
            .build()?;
        let messages = vec![Message::user(
            "measure this text prompt across repeated requests".repeat(20),
        )];
        let tools = AgentRunSnapshot::from_agent(&agent).tools.tools_for_llm();
        assert!(!tools.is_empty());
        let base = HeuristicTokenizer;
        let raw_message_tokens = messages.iter().fold(0usize, |total, message| {
            total
                .saturating_add(message.content.estimated_tokens(&base))
                .saturating_add(base.count_tokens(message.role.as_str()))
        });
        let tool_schema = serde_json::to_string(&tools)
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let raw_prompt_tokens = raw_message_tokens.saturating_add(base.count_tokens(&tool_schema));
        let actual_prompt_tokens =
            u32::try_from(raw_prompt_tokens.saturating_mul(2)).unwrap_or(u32::MAX);
        let usage = Usage {
            prompt_tokens: Some(actual_prompt_tokens.saturating_sub(20)),
            cache_read_input_tokens: Some(20),
            ..Usage::default()
        };
        let llm = Arc::new((0..12).fold(MockLlmClient::new(), |llm, _| {
            llm.with_response_usage("done", usage.clone())
        }));
        agent.set_llm_client(llm.clone());
        let snapshot = AgentRunSnapshot::from_agent(&agent);

        for _ in 0..12 {
            let (tx, _rx) = mpsc::channel(16);
            let outcome =
                run_think(&snapshot, agent.context(), &tx, messages.clone(), false).await?;
            assert!(matches!(outcome, ThinkOutcome::Continue(_)));
        }

        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 12);
        assert_eq!(llm.all_tool_counts(), vec![tools.len(); 12]);
        let factor = snapshot.calibrated_tokenizer.calibration_factor();
        assert!(
            (factor - 2.0).abs() < 0.08,
            "factor {factor} did not converge"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_prompt_usage_does_not_change_calibration() -> Result<()> {
        let llm = MockLlmClient::new()
            .with_response("usage absent")
            .with_response_usage(
                "completion only",
                Usage {
                    completion_tokens: Some(10),
                    total_tokens: Some(10),
                    ..Usage::default()
                },
            )
            .with_response_usage(
                "cache only",
                Usage {
                    cache_read_input_tokens: Some(40),
                    ..Usage::default()
                },
            );
        let agent = ReactAgentBuilder::new().llm_client(Arc::new(llm)).build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let messages = vec![Message::user(
            "prompt without reported input usage".to_string(),
        )];

        for _ in 0..3 {
            let (tx, _rx) = mpsc::channel(16);
            let outcome =
                run_think(&snapshot, agent.context(), &tx, messages.clone(), false).await?;
            assert!(matches!(outcome, ThinkOutcome::Continue(_)));
        }

        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 0);
        assert_eq!(snapshot.calibrated_tokenizer.calibration_factor(), 1.0);
        Ok(())
    }

    #[test]
    fn response_format_schema_is_counted_in_request_budget() -> Result<()> {
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new()))
            .build()?;
        let mut snapshot = AgentRunSnapshot::from_agent(&agent);
        Arc::make_mut(&mut snapshot.config).token_limit = 200;
        let mut request = crate::llm::ChatRequest::new(vec![Message::user("short".to_string())]);
        let without_format = estimate_request_prompt(&request, &snapshot.calibrated_tokenizer)?;
        assert!(validate_request_budget(&snapshot, &without_format).is_ok());

        request.response_format = Some(ResponseFormat::json_schema(
            "large",
            serde_json::json!({"type":"string","enum":vec!["option"; 300]}),
        ));
        let with_format = estimate_request_prompt(&request, &snapshot.calibrated_tokenizer)?;
        assert!(with_format.format_tokens > 0);
        assert_eq!(
            with_format.raw_tokens,
            without_format
                .raw_tokens
                .saturating_add(with_format.format_tokens)
        );
        assert!(validate_request_budget(&snapshot, &with_format).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn provider_specific_replay_is_not_used_as_calibration_sample() -> Result<()> {
        let usage = Usage {
            prompt_tokens: Some(200),
            ..Usage::default()
        };
        let llm = MockLlmClient::new()
            .with_response_usage("file", usage.clone())
            .with_response_usage("reasoning blocks", usage.clone())
            .with_response_usage("reasoning text", usage);
        let agent = ReactAgentBuilder::new().llm_client(Arc::new(llm)).build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let file = Message::user_multimodal(vec![ContentPart::File {
            name: "report.pdf".to_string(),
            content: "base64-data".to_string(),
        }]);
        let mut reasoning = Message::assistant("answer".to_string());
        reasoning.reasoning_blocks = Some(vec![ReasoningBlock::Signed {
            thinking: "private reasoning".to_string(),
            signature: "signature".to_string(),
        }]);
        let mut reasoning_text = Message::assistant("answer".to_string());
        reasoning_text.reasoning_content = Some("private reasoning".to_string());

        for message in [file, reasoning, reasoning_text] {
            let (tx, _rx) = mpsc::channel(16);
            let outcome = run_think(&snapshot, agent.context(), &tx, vec![message], false).await?;
            assert!(matches!(outcome, ThinkOutcome::Continue(_)));
        }

        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 0);
        assert_eq!(snapshot.calibrated_tokenizer.calibration_factor(), 1.0);
        Ok(())
    }

    #[tokio::test]
    async fn image_usage_does_not_distort_text_feedback_across_requests() -> Result<()> {
        let text = "plain text whose provider token ratio is two ".repeat(20);
        let base = HeuristicTokenizer;
        let raw_text = base
            .count_tokens(&text)
            .saturating_add(base.count_tokens("user"));
        let text_usage = Usage {
            prompt_tokens: Some(u32::try_from(raw_text.saturating_mul(2)).unwrap_or(u32::MAX)),
            ..Usage::default()
        };
        let image_usage = Usage {
            // A provider may charge a fixed vision cost independent of text
            // tokenization; this is not twice the local 1,100-token estimate.
            prompt_tokens: Some(
                u32::try_from(raw_text.saturating_mul(2).saturating_add(300)).unwrap_or(u32::MAX),
            ),
            ..Usage::default()
        };
        let llm = MockLlmClient::new()
            .with_response_usage("text first", text_usage.clone())
            .with_response_usage("image", image_usage)
            .with_response_usage("text again", text_usage);
        let agent = ReactAgentBuilder::new().llm_client(Arc::new(llm)).build()?;
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let text_message = Message::user(text.clone());
        let image_message = Message::user_multimodal(vec![
            ContentPart::Text { text },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.invalid/image.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ]);

        let (tx, _rx) = mpsc::channel(16);
        assert!(matches!(
            run_think(
                &snapshot,
                agent.context(),
                &tx,
                vec![text_message.clone()],
                false
            )
            .await?,
            ThinkOutcome::Continue(_)
        ));
        let first_factor = snapshot.calibrated_tokenizer.calibration_factor();
        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 1);

        let (tx, _rx) = mpsc::channel(16);
        assert!(matches!(
            run_think(&snapshot, agent.context(), &tx, vec![image_message], false).await?,
            ThinkOutcome::Continue(_)
        ));
        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 1);
        assert_eq!(
            snapshot.calibrated_tokenizer.calibration_factor(),
            first_factor
        );

        let (tx, _rx) = mpsc::channel(16);
        assert!(matches!(
            run_think(&snapshot, agent.context(), &tx, vec![text_message], false).await?,
            ThinkOutcome::Continue(_)
        ));
        assert_eq!(snapshot.calibrated_tokenizer.sample_count(), 2);
        assert!(snapshot.calibrated_tokenizer.calibration_factor() > first_factor);
        Ok(())
    }

    #[test]
    fn image_budget_keeps_fixed_cost_after_text_calibration() -> Result<()> {
        let tokenizer = CalibratedTokenizer::new(Arc::new(HeuristicTokenizer));
        for _ in 0..12 {
            tokenizer.calibrate(100, 200);
        }
        let text = "model-visible text";
        let request = crate::llm::ChatRequest::new(vec![Message::user_multimodal(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.invalid/image.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ])]);
        let estimate = estimate_request_prompt(&request, &tokenizer)?;
        let expected = tokenizer
            .count_tokens(text)
            .saturating_add(tokenizer.count_tokens("user"))
            .saturating_add(1_100);
        assert_eq!(estimate.message_tokens, expected);
        assert!(!estimate.calibratable);
        Ok(())
    }

    #[test]
    fn pre_compaction_tool_reservation_does_not_freeze_final_visibility() -> Result<()> {
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(MockLlmClient::new()))
            .tool(Box::new(MockTool::new("late_visible_tool")))
            .build()?;
        let mut snapshot = AgentRunSnapshot::from_agent(&agent);
        let tool_name = "late_visible_tool".to_string();
        let visibility = Arc::new(echo_core::tools::ToolVisibilityState::new(
            std::collections::HashSet::from([tool_name.clone()]),
            std::collections::HashSet::new(),
        ));
        Arc::make_mut(&mut snapshot.tools).visibility = Some(visibility.clone());

        let reserved = estimate_pre_compaction_overhead(&snapshot)?;
        assert!(
            build_llm_request(&snapshot, Vec::new(), false)
                .tools
                .is_none()
        );
        visibility.activate([tool_name]);
        let final_request = build_llm_request(&snapshot, Vec::new(), false);
        let final_estimate =
            estimate_request_prompt(&final_request, snapshot.calibrated_tokenizer.as_ref())?;
        assert!(final_estimate.tool_tokens > reserved);
        assert_eq!(final_request.tools.as_ref().map(Vec::len), Some(1));
        Ok(())
    }
}
