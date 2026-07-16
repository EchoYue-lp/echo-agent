//! Per-iteration tool-call branch: emit `ToolCall` events, push assistant
//! message, split approval/concurrent batches, execute, dispatch verifier
//! handoff to `finalize_completed_run` when `final_answer` is accepted.

use super::super::processor::build_tool_calls_from_map;
use super::super::stream_macros::{try_send_or, yield_event_or, yield_final_event_or};
use super::verify::verify_answer;
use super::{IterOutcome, LoopState, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::react::{StepType, TOOL_FINAL_ANSWER};
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tracing::{Instrument, info_span};

/// Tool-call branch of one iteration. Emits the `ToolBatchStart` /
/// `ToolCall` events, pushes the assistant-with-tools message, splits the
/// batch by approval requirement, runs both sub-batches, and short-circuits
/// with [`IterOutcome::Finish`] the moment a `final_answer` tool call is
/// verifier-accepted.
///
/// On verifier rejection of a `final_answer`, increments
/// `state.verifier_retry_count` and continues processing remaining results
/// before returning [`IterOutcome::Continue`].
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_tools(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    state: &mut LoopState,
    iteration: usize,
    think: ThinkOutput,
    _label: &str,
) -> Result<IterOutcome> {
    let agent = &snap.config.agent_name;
    let pt = think.pt;
    let ct = think.ct;

    let (msg_tc, steps) = build_tool_calls_from_map(&think.tool_call_map);
    yield_event_or!(
        tx,
        AgentEvent::ToolBatchStart {
            tool_count: steps.len()
        },
        IterOutcome::Abandoned
    );
    for (id, name, args) in &steps {
        yield_event_or!(
            tx,
            AgentEvent::ToolCall {
                call_id: id.clone(),
                name: name.clone(),
                args: args.clone(),
            },
            IterOutcome::Abandoned
        );
    }
    {
        let ts: Vec<StepType> = steps
            .iter()
            .map(|(id, n, a)| StepType::Call {
                tool_call_id: id.clone(),
                function_name: n.clone(),
                arguments: a.clone(),
            })
            .collect();
        for cb in snap.config.callbacks.iter() {
            cb.on_think_end(agent, &ts, pt, ct).await;
        }
    }
    // Push the assistant turn into history. When ALL tool calls were dropped
    // (e.g. every args failed JSON parsing after repair), `msg_tc` is empty —
    // pushing an assistant_with_tools([]) with empty content makes providers
    // reject the next request with HTTP 400 ("content or tool_calls must be
    // set"). Fall back to a content-bearing assistant message so the turn is
    // structurally valid and the model can retry the call.
    if msg_tc.is_empty() {
        context.lock().await.push(Message::assistant(
            "(流式工具调用参数解析失败,已跳过;请重新发起工具调用)".to_string(),
        ));
    } else {
        context
            .lock()
            .await
            .push(Message::assistant_with_tools(msg_tc));
    }

    #[cfg(feature = "human-loop")]
    let (appr, conc) = {
        let mut a = vec![];
        let mut c = vec![];
        for s in steps {
            if snap.tool_needs_approval(&s.1).await {
                a.push(s);
            } else {
                c.push(s);
            }
        }
        (a, c)
    };
    #[cfg(not(feature = "human-loop"))]
    #[allow(clippy::type_complexity)]
    let (appr, conc): (Vec<(String, String, Value)>, Vec<(String, String, Value)>) =
        (vec![], steps);

    let mut finish_output = None;

    if !conc.is_empty() {
        let mc = snap.tools.tool_manager.max_concurrency();
        let snapshot = snap.clone();
        let tool_count = conc.len();
        let (stream_tx, mut stream_rx) = mpsc::channel(64);
        let mut futs = FuturesUnordered::new();
        for (id, name, args) in conc {
            let snapshot = snapshot.clone();
            let event_tx = stream_tx.clone();
            futs.push(
                async move {
                    let params = if let Value::Object(m) = &args {
                        m.clone().into_iter().collect()
                    } else {
                        HashMap::new()
                    };
                    let result = snapshot
                        .execute_tool_with_policy(id.clone(), &name, &params, &args, Some(event_tx))
                        .await;
                    (id, name, result)
                }
                .instrument(info_span!("tool")),
            );
        }
        drop(stream_tx);
        let bt = super::super::retry::compute_concurrent_tool_batch_timeout(
            &snap.config.tool_execution,
            tool_count,
            mc,
        );
        let cancel = async {
            match snap.cancel_token.as_ref() {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let timeout = async {
            match bt {
                Some(duration) => tokio::time::sleep(duration).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(cancel);
        tokio::pin!(timeout);

        let mut stream_open = true;
        while !futs.is_empty() || stream_open {
            tokio::select! {
                biased;
                event = stream_rx.recv(), if stream_open => {
                    match event {
                        Some((call_id, name, event)) => {
                            yield_final_event_or!(
                                tx,
                                AgentEvent::ToolStream { call_id, name, event },
                                IterOutcome::Abandoned
                            );
                        }
                        None => stream_open = false,
                    }
                }
                Some((id, fname, result)) = futs.next(), if !futs.is_empty() => {
                    while let Ok((call_id, name, event)) = stream_rx.try_recv() {
                        yield_final_event_or!(
                            tx,
                            AgentEvent::ToolStream { call_id, name, event },
                            IterOutcome::Abandoned
                        );
                    }
                    match result {
                        Ok(output) => {
                            yield_event_or!(
                                tx,
                                AgentEvent::ToolResult {
                                    call_id: id.clone(),
                                    name: fname.clone(),
                                    output: output.clone(),
                                },
                                IterOutcome::Abandoned
                            );
                            context.lock().await.push(Message::tool_result(
                                id,
                                fname.clone(),
                                output.clone(),
                            ));
                            if fname == TOOL_FINAL_ANSWER {
                                // Verify answer before accepting
                                if verify_answer(snap, context, &output, state.verifier_retry_count).await {
                                    finish_output = Some(output);
                                } else {
                                    // Verifier failed — continue loop for self-correction
                                    state.verifier_retry_count += 1;
                                }
                            }
                        }
                        Err(error) => {
                            yield_event_or!(
                                tx,
                                AgentEvent::ToolError {
                                    call_id: id.clone(),
                                    name: fname.clone(),
                                    error: error.error.to_string(),
                                    failure: error.failure.clone(),
                                },
                                IterOutcome::Abandoned
                            );
                            context.lock().await.push(Message::tool_result(
                                id,
                                fname.clone(),
                                format!("[Error] {}", error.error),
                            ));
                            // Checkpoint on tool error for recovery
                            snap.save_runtime_checkpoint(
                                context,
                                Some(format!("Tool error: {fname}")),
                            )
                            .await;
                        }
                    }
                },
                _ = &mut cancel => return Ok(IterOutcome::Abandoned),
                _ = &mut timeout => {
                    try_send_or!(
                        tx,
                        Err(ReactError::from(crate::error::ToolError::Timeout(
                            "batch timeout".into()
                        ))),
                        IterOutcome::Abandoned
                    )
                }
            }
        }
    }

    for (id, fname, args) in appr {
        let params = if let Value::Object(m) = &args {
            m.clone().into_iter().collect()
        } else {
            HashMap::new()
        };
        let (stream_tx, mut stream_rx) = mpsc::channel(64);
        let execution =
            snap.execute_tool_with_policy(id.clone(), &fname, &params, &args, Some(stream_tx));
        tokio::pin!(execution);
        let result = loop {
            tokio::select! {
                biased;
                Some((call_id, name, event)) = stream_rx.recv() => {
                    yield_final_event_or!(
                        tx,
                        AgentEvent::ToolStream { call_id, name, event },
                        IterOutcome::Abandoned
                    );
                }
                result = &mut execution => break result,
                _ = async {
                    match snap.cancel_token.as_ref() {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => return Ok(IterOutcome::Abandoned),
            }
        };
        while let Some((call_id, name, event)) = stream_rx.recv().await {
            yield_final_event_or!(
                tx,
                AgentEvent::ToolStream {
                    call_id,
                    name,
                    event
                },
                IterOutcome::Abandoned
            );
        }
        match result {
            Ok(truncated) => {
                yield_event_or!(
                    tx,
                    AgentEvent::ToolResult {
                        call_id: id.clone(),
                        name: fname.clone(),
                        output: truncated.clone(),
                    },
                    IterOutcome::Abandoned
                );
                context.lock().await.push(Message::tool_result(
                    id,
                    fname.clone(),
                    truncated.clone(),
                ));
                if fname == TOOL_FINAL_ANSWER {
                    // Verify answer before accepting
                    if verify_answer(snap, context, &truncated, state.verifier_retry_count).await {
                        finish_output = Some(truncated);
                    } else {
                        // Verifier failed — continue loop for self-correction
                        state.verifier_retry_count += 1;
                    }
                }
            }
            Err(error) => {
                yield_event_or!(
                    tx,
                    AgentEvent::ToolError {
                        call_id: id.clone(),
                        name: fname.clone(),
                        error: error.error.to_string(),
                        failure: error.failure.clone(),
                    },
                    IterOutcome::Abandoned
                );
                context.lock().await.push(Message::tool_result(
                    id,
                    fname.clone(),
                    format!("[Error] {}", error.error),
                ));
                // Checkpoint on tool error for recovery
                snap.save_runtime_checkpoint(context, Some(format!("Tool error: {fname}")))
                    .await;
            }
        }
    }

    // This is the first point where every assistant tool call in the batch has
    // a matching result. Persist it regardless of the periodic interval so a
    // restart never loses an already completed write/dangerous tool outcome.
    snap.save_runtime_checkpoint(context, None).await;
    yield_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
    if let Some(output) = finish_output {
        return Ok(IterOutcome::Finish { output });
    }
    snap.auto_snapshot(context, iteration).await;

    // Periodic runtime checkpoint based on configured interval
    let interval = snap.config.react_checkpoint_interval;
    if interval > 0 && (iteration + 1).is_multiple_of(interval) {
        snap.save_runtime_checkpoint(context, None).await;
    }

    Ok(IterOutcome::Continue)
}
