//! Per-iteration tool-call branch: emit `ToolCall` events, push assistant
//! message, split sequential/concurrent batches, execute, dispatch verifier
//! handoff to `finalize_completed_run` when `final_answer` is accepted.

use super::super::TOOL_CANCELLATION_GRACE_PERIOD;
use super::super::processor::build_tool_calls_from_map;
use super::super::stream_macros::yield_event_or;
use super::verify::verify_answer;
use super::{IterOutcome, LoopState, ThinkOutput, with_reasoning_content};
use crate::agent::react::run::pipeline::ToolPipelineEvent;
use crate::agent::react::{StepType, TOOL_FINAL_ANSWER};
use crate::agent::snapshot::AgentRunSnapshot;
use crate::agent::{AgentEvent, ToolInvocation};
use crate::error::{ReactError, Result};
use crate::llm::types::{ContentPart, ImageUrl, Message, MessageContent};
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tracing::{Instrument, info_span};

type ToolCallSpec = (String, String, Value);
type ToolCallOutcome = std::result::Result<
    crate::agent::snapshot::ToolCallSuccess,
    crate::agent::snapshot::ToolCallFailure,
>;
type CompletedToolCalls = HashMap<String, (String, ToolCallOutcome)>;

#[derive(Default)]
struct PublishedWave {
    successes: usize,
    failures: usize,
    final_answers: Vec<String>,
}

async fn project_typed_tool_result(
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    result: &crate::tools::ToolResult,
    accepts_images: bool,
) -> Option<Message> {
    if !result.success {
        return None;
    }
    if let echo_core::tools::ToolResultKind::SkillActivation { name } = &result.kind {
        crate::agent::react::capabilities::project_skill_activation(
            context,
            name,
            result.output.as_str(),
        )
        .await;
    }

    if result.model_content.is_empty() || !accepts_images {
        return None;
    }
    let mut parts = vec![ContentPart::Text {
        text: "Rich content returned by the preceding tool call.".to_string(),
    }];
    parts.extend(result.model_content.iter().map(|content| match content {
        echo_core::tools::ToolResultContent::ImageUrl { url, detail } => ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: url.clone(),
                detail: detail.clone(),
            },
        },
    }));
    Some(Message::user_multimodal(parts))
}

enum ToolExecutionWave {
    Concurrent(Vec<ToolCallSpec>),
    Sequential(ToolCallSpec),
}

fn agent_event(event: ToolPipelineEvent) -> AgentEvent {
    match event {
        ToolPipelineEvent::Invocation {
            call_id,
            invocation,
        } => AgentEvent::ToolCall {
            call_id,
            invocation,
        },
        ToolPipelineEvent::Stream {
            call_id,
            name,
            event,
        } => AgentEvent::ToolStream {
            call_id,
            name,
            event,
        },
    }
}

async fn forward_pipeline_event(
    tx: &mpsc::Sender<Result<AgentEvent>>,
    emitted_invocations: &mut HashSet<String>,
    event: ToolPipelineEvent,
) {
    if let ToolPipelineEvent::Invocation { call_id, .. } = &event {
        emitted_invocations.insert(call_id.clone());
    }
    let _ = tx.send(Ok(agent_event(event))).await;
}

async fn publish_completed_call(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    call_id: &str,
    outcome: ToolCallOutcome,
) -> Option<String> {
    match outcome {
        Ok(execution) => {
            let name = execution.name;
            let mut result = execution.result;
            let output = result.output.clone();
            let model_message = project_typed_tool_result(
                context,
                &result,
                snap.config
                    .input_modalities
                    .as_ref()
                    .is_none_or(|modalities| {
                        modalities.contains(&echo_core::llm::ModelInputModality::Image)
                    }),
            )
            .await;
            result.model_content.clear();
            let mut context_guard = context.lock().await;
            context_guard.push(Message::tool_result(
                call_id.to_string(),
                name.clone(),
                output.clone(),
            ));
            if let Some(message) = model_message {
                context_guard.push(message);
            }
            drop(context_guard);
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    call_id: call_id.to_string(),
                    name: name.clone(),
                    result,
                }))
                .await;
            (name == TOOL_FINAL_ANSWER).then_some(output)
        }
        Err(error) => {
            let name = error.name;
            let message = error
                .result
                .error
                .clone()
                .unwrap_or_else(|| error.error.to_string());
            context.lock().await.push(Message::tool_result(
                call_id.to_string(),
                name.clone(),
                format!("[Error] {message}"),
            ));
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    call_id: call_id.to_string(),
                    name,
                    result: error.result,
                }))
                .await;
            None
        }
    }
}

async fn close_cancelled_batch(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    tool_names: &[String],
    success_count: usize,
) -> Result<()> {
    let failure_count = tool_names.len().saturating_sub(success_count);
    snap.fire_post_tool_batch(tool_names, success_count, failure_count)
        .await;
    super::finalize::settle_terminal_projection(
        snap,
        context,
        Some("Tool batch cancelled".to_string()),
        tx,
    )
    .await?;
    snap.finalize_run(
        crate::trace::RunStatus::Cancelled,
        None,
        Some("Tool batch cancelled"),
    )
    .await;
    let _ = tx.send(Ok(AgentEvent::ToolBatchEnd)).await;
    let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
    Ok(())
}

async fn close_failed_batch(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    tool_names: &[String],
    success_count: usize,
    error: ReactError,
) -> Result<()> {
    let failure_count = tool_names.len().saturating_sub(success_count);
    snap.fire_post_tool_batch(tool_names, success_count, failure_count)
        .await;
    super::finalize::settle_terminal_projection(snap, context, Some(error.to_string()), tx).await?;
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
    Ok(())
}

async fn settle_interrupted_calls(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    calls: &[ToolCallSpec],
    completed: &mut CompletedToolCalls,
    emitted_invocations: &mut HashSet<String>,
    settlement: InterruptedSettlement<'_>,
) -> PublishedWave {
    let mut published = PublishedWave::default();
    for (call_id, name, input) in calls {
        if emitted_invocations.insert(call_id.clone()) {
            let _ = tx
                .send(Ok(AgentEvent::ToolCall {
                    call_id: call_id.clone(),
                    invocation: ToolInvocation {
                        requested_name: name.clone(),
                        requested_args: input.clone(),
                        name: name.clone(),
                        args: input.clone(),
                        rewrites: Vec::new(),
                    },
                }))
                .await;
        }
        if let Some((_, outcome)) = completed.remove(call_id) {
            let succeeded = outcome.is_ok();
            if let Some(answer) = publish_completed_call(snap, context, tx, call_id, outcome).await
            {
                published.final_answers.push(answer);
            }
            if succeeded {
                published.successes = published.successes.saturating_add(1);
            } else {
                published.failures = published.failures.saturating_add(1);
            }
            continue;
        }
        let result = snap
            .settle_interrupted_tool_call(
                call_id,
                name,
                input,
                settlement.category,
                settlement.message,
            )
            .await;
        context.lock().await.push(Message::tool_result(
            call_id.clone(),
            name.clone(),
            format!("[Error] {}", settlement.message),
        ));
        let _ = tx
            .send(Ok(AgentEvent::ToolResult {
                call_id: call_id.clone(),
                name: name.clone(),
                result,
            }))
            .await;
        published.failures = published.failures.saturating_add(1);
    }
    published
}

#[derive(Clone, Copy)]
struct InterruptedSettlement<'a> {
    category: crate::tools::ToolFailureCategory,
    message: &'a str,
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
        .is_some_and(|tool| !tool.allows_parallel_batch_execution());
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
    let mut remaining_calls: Vec<ToolCallSpec> = waves
        .iter()
        .flat_map(|wave| match wave {
            ToolExecutionWave::Concurrent(calls) => calls.clone(),
            ToolExecutionWave::Sequential(call) => vec![call.clone()],
        })
        .collect();
    for wave in waves {
        match wave {
            ToolExecutionWave::Concurrent(conc) => {
                // Results are keyed by call id and projected in call order.
                let completed: Arc<std::sync::Mutex<CompletedToolCalls>> =
                    Arc::new(std::sync::Mutex::new(HashMap::new()));
                let mut emitted_invocations = HashSet::new();
                if conc.is_empty() {
                    continue;
                }
                let mc = snap.tools.tool_manager.max_concurrency();
                let snapshot = snap.clone();
                let has_timeout_exempt_tool = conc.iter().any(|(_, name, _)| {
                    snap.tools
                        .tool_manager
                        .get_tool(name)
                        .map(|tool| tool.exempt_from_batch_timeout())
                        .unwrap_or(false)
                });
                let tool_count = conc.len();
                let (stream_tx, mut stream_rx) = mpsc::channel(64);
                let mut futs = FuturesUnordered::new();
                // Clone to keep `conc` for the call-order emission loop below.
                for (id, name, args) in conc.clone() {
                    let snapshot = snapshot.clone();
                    let event_tx = stream_tx.clone();
                    let completed = completed.clone();
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
                            completed
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .insert(id, (name, result));
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
                        Some(()) = futs.next(), if !futs.is_empty() => {
                            while let Ok(event) = stream_rx.try_recv() {
                                forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                            }
                        },
                        _ = &mut cancellation_grace, if cancellation_observed => {
                            tracing::warn!(
                                grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                                "tool batch cancellation grace period elapsed"
                            );
                            while let Ok(event) = stream_rx.try_recv() {
                                forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                            }
                            let mut ready = std::mem::take(
                                &mut *completed.lock().unwrap_or_else(|error| error.into_inner()),
                            );
                            let published = settle_interrupted_calls(
                                snap,
                                context,
                                tx,
                                &remaining_calls,
                                &mut ready,
                                &mut emitted_invocations,
                                InterruptedSettlement {
                                    category: crate::tools::ToolFailureCategory::Cancelled,
                                    message: "Tool call cancellation grace period elapsed",
                                },
                            ).await;
                            batch_success_count = batch_success_count.saturating_add(published.successes);
                            close_cancelled_batch(
                                snap,
                                context,
                                tx,
                                &batch_tool_names,
                                batch_success_count,
                            ).await?;
                            return Ok(IterOutcome::TerminalSettled {
                                outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                            });
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
                            while let Ok(event) = stream_rx.try_recv() {
                                forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                            }
                            let mut ready = std::mem::take(
                                &mut *completed.lock().unwrap_or_else(|error| error.into_inner()),
                            );
                            let published = settle_interrupted_calls(
                                snap,
                                context,
                                tx,
                                &remaining_calls,
                                &mut ready,
                                &mut emitted_invocations,
                                InterruptedSettlement {
                                    category: crate::tools::ToolFailureCategory::Timeout,
                                    message: "Tool batch timeout elapsed",
                                },
                            ).await;
                            batch_success_count = batch_success_count.saturating_add(published.successes);
                            close_failed_batch(
                                snap,
                                context,
                                tx,
                                &batch_tool_names,
                                batch_success_count,
                                error,
                            ).await?;
                            return Ok(IterOutcome::TerminalSettled {
                                outcome: crate::agent::AgentSteerTurnOutcome::Failed,
                            });
                        }
                        event = stream_rx.recv(), if stream_open => {
                            match event {
                                Some(event) => {
                                    forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                                }
                                None => stream_open = false,
                            }
                        }
                    }
                }
                // Both normal completion and cancellation publish the captured
                // results in assistant call order before transcript settlement.
                let mut ready = std::mem::take(
                    &mut *completed.lock().unwrap_or_else(|error| error.into_inner()),
                );
                let published = settle_interrupted_calls(
                    snap,
                    context,
                    tx,
                    if cancellation_observed {
                        &remaining_calls
                    } else {
                        &conc
                    },
                    &mut ready,
                    &mut emitted_invocations,
                    InterruptedSettlement {
                        category: if cancellation_observed {
                            crate::tools::ToolFailureCategory::Cancelled
                        } else {
                            crate::tools::ToolFailureCategory::Unavailable
                        },
                        message: if cancellation_observed {
                            "Tool call cancelled before returning a result"
                        } else {
                            "Tool call finished without a result"
                        },
                    },
                )
                .await;
                batch_success_count = batch_success_count.saturating_add(published.successes);
                batch_failure_count = batch_failure_count.saturating_add(published.failures);
                if cancellation_observed {
                    close_cancelled_batch(
                        snap,
                        context,
                        tx,
                        &batch_tool_names,
                        batch_success_count,
                    )
                    .await?;
                    return Ok(IterOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                    });
                }
                for output in published.final_answers {
                    if verify_answer(snap, context, &output, state.verifier_retry_count).await {
                        finish_output = Some(output);
                    } else {
                        state.verifier_retry_count = state.verifier_retry_count.saturating_add(1);
                    }
                }
                remaining_calls.drain(..conc.len().min(remaining_calls.len()));
            }
            ToolExecutionWave::Sequential((id, fname, args)) => {
                let mut emitted_invocations = HashSet::new();
                if snap
                    .cancel_token
                    .as_ref()
                    .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
                {
                    let mut completed = HashMap::new();
                    let published = settle_interrupted_calls(
                        snap,
                        context,
                        tx,
                        &remaining_calls,
                        &mut completed,
                        &mut emitted_invocations,
                        InterruptedSettlement {
                            category: crate::tools::ToolFailureCategory::Cancelled,
                            message: "Tool batch cancelled before this call started",
                        },
                    )
                    .await;
                    batch_success_count = batch_success_count.saturating_add(published.successes);
                    close_cancelled_batch(
                        snap,
                        context,
                        tx,
                        &batch_tool_names,
                        batch_success_count,
                    )
                    .await?;
                    return Ok(IterOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                    });
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
                        result = &mut execution => break result,
                        _ = &mut cancellation_grace, if cancellation_observed => {
                            tracing::warn!(
                                tool = %fname,
                                grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                                "tool cancellation grace period elapsed"
                            );
                            while let Ok(event) = stream_rx.try_recv() {
                                forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                            }
                            let mut completed = HashMap::new();
                            let published = settle_interrupted_calls(
                                snap,
                                context,
                                tx,
                                &remaining_calls,
                                &mut completed,
                                &mut emitted_invocations,
                                InterruptedSettlement {
                                    category: crate::tools::ToolFailureCategory::Cancelled,
                                    message: "Tool call cancellation grace period elapsed",
                                },
                            ).await;
                            batch_success_count = batch_success_count.saturating_add(published.successes);
                            close_cancelled_batch(
                                snap,
                                context,
                                tx,
                                &batch_tool_names,
                                batch_success_count,
                            ).await?;
                            return Ok(IterOutcome::TerminalSettled {
                                outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                            });
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
                        Some(event) = stream_rx.recv() => {
                            forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                        }
                    }
                };
                while let Ok(event) = stream_rx.try_recv() {
                    forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                }
                let mut completed = HashMap::from([(id.clone(), (fname.clone(), result))]);
                let current_call = (id.clone(), fname.clone(), args.clone());
                let published = settle_interrupted_calls(
                    snap,
                    context,
                    tx,
                    if cancellation_observed {
                        remaining_calls.as_slice()
                    } else {
                        std::slice::from_ref(&current_call)
                    },
                    &mut completed,
                    &mut emitted_invocations,
                    InterruptedSettlement {
                        category: crate::tools::ToolFailureCategory::Cancelled,
                        message: "Tool batch cancelled after this call completed",
                    },
                )
                .await;
                batch_success_count = batch_success_count.saturating_add(published.successes);
                batch_failure_count = batch_failure_count.saturating_add(published.failures);
                if cancellation_observed {
                    close_cancelled_batch(
                        snap,
                        context,
                        tx,
                        &batch_tool_names,
                        batch_success_count,
                    )
                    .await?;
                    return Ok(IterOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                    });
                }
                for output in published.final_answers {
                    if verify_answer(snap, context, &output, state.verifier_retry_count).await {
                        finish_output = Some(output);
                    } else {
                        state.verifier_retry_count = state.verifier_retry_count.saturating_add(1);
                    }
                }
                remaining_calls.drain(..1.min(remaining_calls.len()));
            }
        }
    }

    // This is the first point where every assistant tool call in the batch has
    // a matching result. Persist it regardless of the periodic interval so a
    // restart never loses an already completed write/dangerous tool outcome.
    let settlement = snap.save_transcript_projection(context, None).await?;
    if snap.conversation_store.is_some() {
        snap.mark_transcript_settlement_observed();
        yield_event_or!(
            tx,
            AgentEvent::TranscriptProjectionSettlement(settlement.clone()),
            IterOutcome::Abandoned
        );
    }
    if settlement.status != crate::memory::TranscriptProjectionSettlementStatus::Settled {
        return Err(crate::agent::snapshot::transcript_settlement_admission_error(&settlement));
    }
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
        let settlement = snap.save_transcript_projection(context, None).await?;
        if snap.conversation_store.is_some() {
            snap.mark_transcript_settlement_observed();
            yield_event_or!(
                tx,
                AgentEvent::TranscriptProjectionSettlement(settlement.clone()),
                IterOutcome::Abandoned
            );
        }
        if settlement.status != crate::memory::TranscriptProjectionSettlementStatus::Settled {
            return Err(crate::agent::snapshot::transcript_settlement_admission_error(&settlement));
        }
    }

    Ok(IterOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::{
        InterruptedSettlement, ToolExecutionWave, build_execution_waves, project_typed_tool_result,
        settle_interrupted_calls,
    };
    use crate::llm::types::{ContentPart, MessageContent};
    use echo_core::tools::{ToolResult, ToolResultContent, ToolResultKind};
    use serde_json::Value;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    fn call(id: &str) -> (String, String, Value) {
        (id.to_string(), "tool".to_string(), Value::Null)
    }

    #[derive(Default)]
    struct InterruptedCallback {
        interrupted: AtomicUsize,
        ordinary_errors: AtomicUsize,
        inputs: std::sync::Mutex<Vec<Value>>,
    }

    impl echo_core::agent::AgentCallback for InterruptedCallback {
        fn on_tool_error_with_id<'a>(
            &'a self,
            _agent: &'a str,
            _call_id: &'a str,
            _tool: &'a str,
            _error: &'a crate::error::ReactError,
        ) -> futures::future::BoxFuture<'a, ()> {
            Box::pin(async {
                self.ordinary_errors.fetch_add(1, Ordering::SeqCst);
            })
        }

        fn on_tool_interrupted_with_id<'a>(
            &'a self,
            _agent: &'a str,
            _call_id: &'a str,
            _tool: &'a str,
            input: &'a Value,
            _error: &'a crate::error::ReactError,
        ) -> futures::future::BoxFuture<'a, ()> {
            Box::pin(async move {
                self.interrupted.fetch_add(1, Ordering::SeqCst);
                self.inputs
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(input.clone());
            })
        }
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

    #[tokio::test]
    async fn interrupted_batch_settles_only_unfinished_calls_with_possible_effects()
    -> crate::error::Result<()> {
        use crate::agent::AgentEvent;
        use crate::trace::{InMemoryRunStore, RunEvent, RunStore};
        use echo_core::tools::{ToolFailureCategory, ToolSideEffect};

        let store = Arc::new(InMemoryRunStore::new());
        let callback = Arc::new(InterruptedCallback::default());
        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .with_run_store(store.clone())
            .callback(callback.clone())
            .build()?;
        let legacy = agent.capture_legacy_external_context();
        let run_id = agent
            .start_legacy_trace_run("interrupted batch", &legacy)
            .await
            .ok_or_else(|| crate::error::ReactError::Other("trace did not start".to_string()))?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        snapshot
            .record_event(RunEvent::new_tool_call(
                "pending-a".to_string(),
                "shell".to_string(),
                Some(Value::Null),
                None,
                0,
            ))
            .await;
        snapshot.mark_tool_execution_started("pending-a");
        snapshot
            .record_event(RunEvent::new_tool_call(
                "completed".to_string(),
                "shell".to_string(),
                Some(Value::Null),
                None,
                0,
            ))
            .await;
        let calls = vec![
            ("completed".to_string(), "shell".to_string(), Value::Null),
            ("pending-a".to_string(), "shell".to_string(), Value::Null),
            (
                "pending-b".to_string(),
                "write_file".to_string(),
                Value::Null,
            ),
        ];
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut completed = HashMap::from([(
            "completed".to_string(),
            (
                "shell".to_string(),
                Ok(crate::agent::snapshot::ToolCallSuccess {
                    name: "shell".to_string(),
                    result: ToolResult::success("completed output"),
                }),
            ),
        )]);
        let mut emitted = HashSet::from(["completed".to_string(), "pending-a".to_string()]);
        let published = settle_interrupted_calls(
            &snapshot,
            &agent.memory.context,
            &tx,
            &calls,
            &mut completed,
            &mut emitted,
            InterruptedSettlement {
                category: ToolFailureCategory::Timeout,
                message: "batch timeout",
            },
        )
        .await;
        assert_eq!((published.successes, published.failures), (1, 2));
        assert_eq!(callback.interrupted.load(Ordering::SeqCst), 2);
        assert_eq!(callback.ordinary_errors.load(Ordering::SeqCst), 0);
        assert_eq!(
            callback
                .inputs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            2
        );
        drop(tx);

        let mut observed = Vec::new();
        while let Some(event) = rx.recv().await {
            match event? {
                AgentEvent::ToolCall { call_id, .. } => {
                    assert_eq!(call_id, "pending-b");
                    observed.push((call_id, true));
                }
                AgentEvent::ToolResult {
                    call_id, result, ..
                } => {
                    if call_id == "completed" {
                        assert!(result.success);
                        assert_eq!(result.output, "completed output");
                    } else {
                        assert!(result.failure.as_ref().is_some_and(|failure| {
                            failure.category == ToolFailureCategory::Timeout
                                && failure.side_effect == ToolSideEffect::Possible
                        }));
                    }
                    observed.push((call_id, false));
                }
                other => {
                    return Err(crate::error::ReactError::Other(format!(
                        "unexpected event: {other:?}"
                    )));
                }
            }
        }
        assert_eq!(
            observed,
            [
                ("completed".to_string(), false),
                ("pending-a".to_string(), false),
                ("pending-b".to_string(), true),
                ("pending-b".to_string(), false),
            ]
        );
        let run = store
            .load(&run_id)
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("trace missing".to_string()))?;
        for id in ["pending-a", "pending-b"] {
            assert_eq!(
                run.events
                    .iter()
                    .filter(
                        |event| matches!(event, RunEvent::ToolCall { call_id, .. } if call_id == id)
                    )
                    .count(),
                1
            );
            assert_eq!(
                run.events
                    .iter()
                    .filter(|event| matches!(event, RunEvent::ToolResult { call_id, success: false, .. } if call_id == id))
                    .count(),
                1
            );
            assert_eq!(
                run.events
                    .iter()
                    .filter(|event| matches!(event, RunEvent::ToolError { call_id, .. } if call_id == id))
                    .count(),
                1
            );
        }
        assert_eq!(
            run.events
                .iter()
                .filter(|event| matches!(
                    event,
                    RunEvent::ToolExecutionSkipped { call_id, .. }
                        if call_id == "pending-b"
                ))
                .count(),
            1
        );
        assert!(!run.events.iter().any(|event| matches!(
            event,
            RunEvent::ToolExecutionSkipped { call_id, .. }
                if call_id == "completed" || call_id == "pending-a"
        )));
        assert!(!run.events.iter().any(|event| matches!(event,
            RunEvent::ToolResult { call_id, .. } | RunEvent::ToolError { call_id, .. }
            if call_id == "completed"
        )));
        Ok(())
    }

    #[tokio::test]
    async fn image_result_projects_multimodal_message() -> crate::error::Result<()> {
        let context = Arc::new(Mutex::new(
            crate::compression::ContextManager::builder(4096).build(),
        ));
        let result = ToolResult::success_with_kind(
            ToolResultKind::Image {
                mime_type: "image/png".to_string(),
            },
            "loaded",
        )
        .with_model_content(ToolResultContent::ImageUrl {
            url: "data:image/png;base64,AAAA".to_string(),
            detail: Some("high".to_string()),
        });

        let Some(message) = project_typed_tool_result(&context, &result, true).await else {
            return Err(std::io::Error::other("image result should create a model message").into());
        };
        let MessageContent::Parts(parts) = message.content else {
            return Err(std::io::Error::other("image result should be multimodal").into());
        };
        assert!(matches!(parts.first(), Some(ContentPart::Text { .. })));
        assert!(matches!(
            parts.get(1),
            Some(ContentPart::ImageUrl { image_url })
                if image_url.url == "data:image/png;base64,AAAA"
                    && image_url.detail.as_deref() == Some("high")
        ));
        Ok(())
    }

    #[tokio::test]
    async fn image_result_is_not_projected_to_text_only_model() {
        let context = Arc::new(Mutex::new(
            crate::compression::ContextManager::builder(4096).build(),
        ));
        let result = ToolResult::success_with_kind(
            ToolResultKind::Image {
                mime_type: "image/png".to_string(),
            },
            "loaded",
        )
        .with_model_content(ToolResultContent::ImageUrl {
            url: "data:image/png;base64,AAAA".to_string(),
            detail: None,
        });

        assert!(
            project_typed_tool_result(&context, &result, false)
                .await
                .is_none()
        );
    }
}
