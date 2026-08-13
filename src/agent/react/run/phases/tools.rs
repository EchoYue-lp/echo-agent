//! Per-iteration tool-call branch: emit `ToolCall` events, push assistant
//! message, split sequential/concurrent batches, execute, dispatch verifier
//! handoff to `finalize_completed_run` when `final_answer` is accepted.

use super::super::processor::build_tool_calls_from_map;
use super::super::stream_macros::{yield_event_or, yield_final_event_or};
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
type ToolCallSpec = (String, String, Value);

enum ToolExecutionWave {
    Concurrent(Vec<ToolCallSpec>),
    Sequential(ToolCallSpec),
}

async fn close_cancelled_batch(
    snap: &AgentRunSnapshot,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    tool_names: &[String],
    success_count: usize,
) {
    let failure_count = tool_names.len().saturating_sub(success_count);
    snap.fire_post_tool_batch(tool_names, success_count, failure_count)
        .await;
    snap.finalize_run(
        crate::trace::RunStatus::Cancelled,
        None,
        Some("Tool batch cancelled"),
    )
    .await;
    let _ = tx.send(Ok(AgentEvent::ToolBatchEnd)).await;
    let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
}

async fn close_failed_batch(
    snap: &AgentRunSnapshot,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    tool_names: &[String],
    success_count: usize,
    error: ReactError,
) {
    let failure_count = tool_names.len().saturating_sub(success_count);
    snap.fire_post_tool_batch(tool_names, success_count, failure_count)
        .await;
    snap.finalize_run(
        crate::trace::RunStatus::Failed,
        None,
        Some(&error.to_string()),
    )
    .await;
    let _ = tx.send(Ok(AgentEvent::ToolBatchEnd)).await;
    let _ = tx
        .send(Ok(AgentEvent::from_error("tool_batch", &error)))
        .await;
}

fn build_execution_waves(
    steps: Vec<ToolCallSpec>,
    sequential_call_ids: &std::collections::HashSet<String>,
) -> Vec<ToolExecutionWave> {
    let mut waves = Vec::new();
    let mut concurrent = Vec::new();
    for step in steps {
        if sequential_call_ids.contains(&step.0) {
            if !concurrent.is_empty() {
                waves.push(ToolExecutionWave::Concurrent(std::mem::take(
                    &mut concurrent,
                )));
            }
            waves.push(ToolExecutionWave::Sequential(step));
        } else {
            concurrent.push(step);
        }
    }
    if !concurrent.is_empty() {
        waves.push(ToolExecutionWave::Concurrent(concurrent));
    }
    waves
}

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

    let (msg_tc, steps) =
        build_tool_calls_from_map(&think.tool_call_map).map_err(ReactError::Other)?;
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
        think.reasoning_blocks,
    ));

    let mut sequential_call_ids = std::collections::HashSet::new();
    for (id, name, _) in &steps {
        if requires_sequential_execution(snap, name).await {
            sequential_call_ids.insert(id.clone());
        }
    }
    let waves = build_execution_waves(steps, &sequential_call_ids);

    let mut finish_output = None;
    let mut batch_success_count = 0usize;
    let mut batch_failure_count = 0usize;
    let batch_tool_names: Vec<String> = waves
        .iter()
        .flat_map(|wave| match wave {
            ToolExecutionWave::Concurrent(calls) => calls
                .iter()
                .map(|(_, name, _)| name.clone())
                .collect::<Vec<_>>(),
            ToolExecutionWave::Sequential((_, name, _)) => vec![name.clone()],
        })
        .collect();
    for wave in waves {
        match wave {
            ToolExecutionWave::Concurrent(conc) => {
                // Results are keyed by call id and projected in call order.
                let mut completed: HashMap<
                    String,
                    (
                        String,
                        std::result::Result<String, crate::agent::snapshot::ToolCallFailure>,
                    ),
                > = HashMap::new();
                if conc.is_empty() {
                    continue;
                }
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
                // Clone to keep `conc` for the call-order emission loop below.
                for (id, name, args) in conc.clone() {
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
                                .execute_tool_with_policy(
                                    id.clone(),
                                    &name,
                                    &params,
                                    &args,
                                    Some(event_tx),
                                )
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
                            close_cancelled_batch(
                                snap,
                                tx,
                                &batch_tool_names,
                                batch_success_count,
                            ).await;
                            return Ok(IterOutcome::Abandoned);
                        }
                        _ = &mut cancel, if !cancellation_observed => {
                            cancellation_observed = true;
                            cancellation_grace.as_mut().reset(
                                tokio::time::Instant::now() + TOOL_CANCELLATION_GRACE_PERIOD,
                            );
                        },
                        _ = &mut timeout, if !cancellation_observed => {
                            let error = ReactError::from(crate::error::ToolError::Timeout(
                                "batch timeout".into()
                            ));
                            close_failed_batch(
                                snap,
                                tx,
                                &batch_tool_names,
                                batch_success_count,
                                error,
                            ).await;
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
                            completed.insert(id, (fname, result));
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
                    }
                }
                if cancellation_observed {
                    close_cancelled_batch(snap, tx, &batch_tool_names, batch_success_count).await;
                    return Ok(IterOutcome::Abandoned);
                }

                // Emit results and push them into context in call order (`conc`
                // order), not completion order — the assistant message already
                // carries the tool calls in call order, and strict providers reject
                // misordered tool results with HTTP 400 (F-RCT-04-P1-01).
                for (id, _fname, _args) in &conc {
                    let Some((fname, result)) = completed.remove(id) else {
                        continue;
                    };
                    match result {
                        Ok(output) => {
                            batch_success_count = batch_success_count.saturating_add(1);
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
                                id.clone(),
                                fname.clone(),
                                output.clone(),
                            ));
                            if fname == TOOL_FINAL_ANSWER {
                                // Verify answer before accepting
                                if verify_answer(snap, context, &output, state.verifier_retry_count)
                                    .await
                                {
                                    finish_output = Some(output);
                                } else {
                                    // Verifier failed — continue loop for self-correction
                                    state.verifier_retry_count += 1;
                                }
                            }
                        }
                        Err(error) => {
                            batch_failure_count = batch_failure_count.saturating_add(1);
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
                                id.clone(),
                                fname.clone(),
                                format!("[Error] {}", error.error),
                            ));
                        }
                    }
                }
            }
            ToolExecutionWave::Sequential((id, fname, args)) => {
                if snap
                    .cancel_token
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
                {
                    close_cancelled_batch(snap, tx, &batch_tool_names, batch_success_count).await;
                    return Ok(IterOutcome::Abandoned);
                }
                let params = if let Value::Object(m) = &args {
                    m.clone().into_iter().collect()
                } else {
                    HashMap::new()
                };
                let (stream_tx, mut stream_rx) = mpsc::channel(64);
                let execution = snap.execute_tool_with_policy(
                    id.clone(),
                    &fname,
                    &params,
                    &args,
                    Some(stream_tx),
                );
                tokio::pin!(execution);
                let cancellation_grace = tokio::time::sleep(Duration::ZERO);
                tokio::pin!(cancellation_grace);
                let mut cancellation_observed = false;
                let result = loop {
                    tokio::select! {
                        biased;
                        _ = &mut cancellation_grace, if cancellation_observed => {
                            tracing::warn!(
                                tool = %fname,
                                grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                                "tool cancellation grace period elapsed"
                            );
                            close_cancelled_batch(
                                snap,
                                tx,
                                &batch_tool_names,
                                batch_success_count,
                            ).await;
                            return Ok(IterOutcome::Abandoned);
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
                        result = &mut execution => break result,
                        Some((call_id, name, event)) = stream_rx.recv() => {
                            yield_final_event_or!(
                                tx,
                                AgentEvent::ToolStream { call_id, name, event },
                                IterOutcome::Abandoned
                            );
                        }
                    }
                };
                while let Ok((call_id, name, event)) = stream_rx.try_recv() {
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
                        batch_success_count = batch_success_count.saturating_add(1);
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
                            if verify_answer(snap, context, &truncated, state.verifier_retry_count)
                                .await
                            {
                                finish_output = Some(truncated);
                            } else {
                                // Verifier failed — continue loop for self-correction
                                state.verifier_retry_count += 1;
                            }
                        }
                    }
                    Err(error) => {
                        batch_failure_count = batch_failure_count.saturating_add(1);
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
                    }
                }
                if cancellation_observed {
                    close_cancelled_batch(snap, tx, &batch_tool_names, batch_success_count).await;
                    return Ok(IterOutcome::Abandoned);
                }
            }
        }
    }

    // This is the first point where every assistant tool call in the batch has
    // a matching result. Persist it regardless of the periodic interval so a
    // restart never loses an already completed write/dangerous tool outcome.
    snap.save_runtime_checkpoint(context, None).await?;
    yield_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
    snap.fire_post_tool_batch(&batch_tool_names, batch_success_count, batch_failure_count)
        .await;
    if let Some(output) = finish_output {
        return Ok(IterOutcome::Finish { output });
    }
    snap.auto_snapshot(context, iteration).await;

    // Periodic runtime checkpoint based on configured interval
    let interval = snap.config.react_checkpoint_interval;
    if interval > 0 && (iteration + 1).is_multiple_of(interval) {
        snap.save_runtime_checkpoint(context, None).await?;
    }

    Ok(IterOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::{ToolExecutionWave, build_execution_waves};
    use serde_json::Value;
    use std::collections::HashSet;

    fn call(id: &str) -> (String, String, Value) {
        (id.to_string(), "tool".to_string(), Value::Null)
    }

    #[test]
    fn sequential_calls_remain_ordered_barriers() {
        let waves = build_execution_waves(
            vec![call("a"), call("barrier"), call("b"), call("c")],
            &HashSet::from(["barrier".to_string()]),
        );
        assert_eq!(waves.len(), 3);
        assert!(matches!(
            waves.first(),
            Some(ToolExecutionWave::Concurrent(calls))
                if calls.first().is_some_and(|call| call.0 == "a")
        ));
        assert!(matches!(
            waves.get(1),
            Some(ToolExecutionWave::Sequential(call)) if call.0 == "barrier"
        ));
        assert!(matches!(
            waves.get(2),
            Some(ToolExecutionWave::Concurrent(calls))
                if calls.iter().map(|call| call.0.as_str()).eq(["b", "c"])
        ));
    }
}
