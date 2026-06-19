//! Per-iteration LLM call: fire `on_think_start`, run intervention
//! callbacks, stream LLM chunks, derive token counts and emit `ThinkEnd`.

use super::super::processor::process_stream_chunk;
use super::super::stream_macros::{try_send_or, yield_event_or};
use super::{LoopState, ThinkOutcome, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use futures::StreamExt;
use std::collections::HashMap;
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
            context.lock().await.push(Message::system(injected));
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
                        cancel_token: snap.cancel_token.clone(),
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
