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
) -> Result<impl futures::Stream<Item = Result<crate::llm::types::ChatCompletionChunk>>> {
    let tools = if snap.config.enable_tool {
        let t = snap.tools.tool_manager.get_openai_tools();
        if t.is_empty() { None } else { Some(t) }
    } else {
        None
    };
    let cancel = snap.cancel_token.clone();
    super::super::retry::retry_llm_call(
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
                crate::llm::stream_chat(
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
                .await
            }
        },
    )
    .await
}
