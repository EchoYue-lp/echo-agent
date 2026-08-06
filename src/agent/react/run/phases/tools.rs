//! Per-iteration tool-call branch: emit `ToolCall` events, push assistant
//! message, split sequential/concurrent batches, execute, dispatch verifier
//! handoff to `finalize_completed_run` when `final_answer` is accepted.

use super::super::processor::build_tool_calls_from_map;
use super::super::stream_macros::{try_send_or, yield_event_or, yield_final_event_or};
use super::verify::verify_answer;
use super::{IterOutcome, LoopState, ThinkOutput, with_reasoning_content};
use crate::agent::AgentEvent;
use crate::agent::react::{StepType, TOOL_FINAL_ANSWER};
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{ReactError, Result};
use crate::llm::types::{Message, MessageContent};
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tracing::{Instrument, info_span};

const TOOL_CANCELLATION_GRACE_PERIOD: Duration = Duration::from_secs(5);

async fn requires_sequential_execution(snap: &AgentRunSnapshot, tool_name: &str) -> bool {
    let tool_disallows_parallel_execution = snap
        .tools
        .tool_manager
        .get_tool(tool_name)
        .is_some_and(|tool| !tool.value().allows_parallel_batch_execution());
    #[cfg(feature = "human-loop")]
    {
        tool_disallows_parallel_execution || snap.tool_needs_approval(tool_name).await
    }
    #[cfg(not(feature = "human-loop"))]
    {
        tool_disallows_parallel_execution
    }
}

/// Tool-call branch of one iteration. Emits the `ToolBatchStart` /
/// `ToolCall` events, pushes the assistant-with-tools message, splits the
/// batch by approval or tool concurrency policy, runs both sub-batches, and short-circuits
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
    let assistant_message = if msg_tc.is_empty() {
        Message::assistant("(流式工具调用参数解析失败,已跳过;请重新发起工具调用)".to_string())
    } else {
        let mut message = Message::assistant_with_tools(msg_tc);
        if !think.content_buffer.is_empty() {
            message.content = MessageContent::Text(think.content_buffer);
        }
        message
    };
    context.lock().await.push(with_reasoning_content(
        assistant_message,
        think.reasoning_buffer,
    ));

    let (serial, conc) = {
        let mut serial = vec![];
        let mut concurrent = vec![];
        for s in steps {
            if requires_sequential_execution(snap, &s.1).await {
                serial.push(s);
            } else {
                concurrent.push(s);
            }
        }
        (serial, concurrent)
    };

    let mut finish_output = None;

    if !conc.is_empty() {
        let mc = snap.tools.tool_manager.max_concurrency();
        let snapshot = snap.clone();
        let has_timeout_exempt_tool = conc.iter().any(|(_, name, _)| {
            snap.tools
                .tool_manager
                .get_tool(name)
                .map(|tool| tool.value().exempt_from_batch_timeout())
                .unwrap_or(false)
        });
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
        // A timeout-exempt tool owns its execution deadline. Disable the outer
        // batch timer for the mixed batch; ordinary peers remain protected by
        // ToolManager's per-tool timeout, while the long-running tool can wait
        // for its internal Subagent deadline instead of being cancelled at the
        // ordinary 120-second ceiling.
        let bt = if has_timeout_exempt_tool {
            None
        } else {
            super::super::retry::compute_concurrent_tool_batch_timeout(
                &snap.config.tool_execution,
                tool_count,
                mc,
            )
        };
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
        let cancellation_grace = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(cancellation_grace);
        let mut cancellation_observed = false;

        let mut stream_open = true;
        while !futs.is_empty() || stream_open {
            tokio::select! {
                biased;
                _ = &mut cancellation_grace, if cancellation_observed => {
                    tracing::warn!(
                        grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                        "tool batch cancellation grace period elapsed"
                    );
                    snap.save_runtime_checkpoint(context, Some("Tool batch cancelled".to_string())).await;
                    yield_final_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
                    return Ok(IterOutcome::Abandoned);
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
                _ = &mut cancel, if !cancellation_observed => {
                    cancellation_observed = true;
                    cancellation_grace.as_mut().reset(
                        tokio::time::Instant::now() + TOOL_CANCELLATION_GRACE_PERIOD,
                    );
                },
                _ = &mut timeout, if !cancellation_observed => {
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
        if cancellation_observed {
            snap.save_runtime_checkpoint(context, Some("Tool batch cancelled".to_string()))
                .await;
            yield_final_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
            return Ok(IterOutcome::Abandoned);
        }
    }

    for (id, fname, args) in serial {
        if snap
            .cancel_token
            .as_ref()
            .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
        {
            snap.save_runtime_checkpoint(context, Some("Tool batch cancelled".to_string()))
                .await;
            yield_final_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
            return Ok(IterOutcome::Abandoned);
        }
        let params = if let Value::Object(m) = &args {
            m.clone().into_iter().collect()
        } else {
            HashMap::new()
        };
        let (stream_tx, mut stream_rx) = mpsc::channel(64);
        let execution =
            snap.execute_tool_with_policy(id.clone(), &fname, &params, &args, Some(stream_tx));
        tokio::pin!(execution);
        let cancellation_grace = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(cancellation_grace);
        let mut cancellation_observed = false;
        let result = loop {
            tokio::select! {
                biased;
                result = &mut execution => break result,
                _ = &mut cancellation_grace, if cancellation_observed => {
                    tracing::warn!(
                        tool = %fname,
                        grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                        "tool cancellation grace period elapsed"
                    );
                    snap.save_runtime_checkpoint(context, Some(format!("Tool cancelled: {fname}"))).await;
                    yield_final_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
                    return Ok(IterOutcome::Abandoned);
                }
                Some((call_id, name, event)) = stream_rx.recv() => {
                    yield_final_event_or!(
                        tx,
                        AgentEvent::ToolStream { call_id, name, event },
                        IterOutcome::Abandoned
                    );
                }
                _ = async {
                    match snap.cancel_token.as_ref() {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                }, if !cancellation_observed => {
                    cancellation_observed = true;
                    cancellation_grace.as_mut().reset(
                        tokio::time::Instant::now() + TOOL_CANCELLATION_GRACE_PERIOD,
                    );
                },
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
        if cancellation_observed {
            snap.save_runtime_checkpoint(context, Some("Tool batch cancelled".to_string()))
                .await;
            yield_final_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
            return Ok(IterOutcome::Abandoned);
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
