//! Per-iteration tool-call branch: emit `ToolCall` events, push assistant
//! message, split sequential/concurrent batches, execute, dispatch verifier
//! handoff to the normal text-candidate path after tools settle.

use super::super::TOOL_CANCELLATION_GRACE_PERIOD;
use super::super::processor::build_tool_calls_from_map;
use super::super::stream_macros::yield_event_or;
use super::{IterOutcome, ThinkOutput, with_reasoning_content};
use crate::agent::react::StepType;
use crate::agent::react::run::pipeline::ToolPipelineEvent;
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
}

/// 将原ToolResult.artifact放入同一canonical tool Message的既有typed ResourceLink。
/// 格式只定位原producer已有的显示引用，不用路径/工具名授予artifact来源。
fn tool_result_with_artifact(
    call_id: &str,
    name: &str,
    output: String,
    artifact: Option<&echo_core::tools::artifact::ToolOutputArtifactRef>,
) -> Message {
    use echo_core::llm::types::LinkedResource;
    use sha2::{Digest as _, Sha256};
    let Some(reference) = artifact else {
        return Message::tool_result(call_id.to_string(), name.to_string(), output);
    };
    let reference_value = match serde_json::to_value(reference) {
        Ok(value) => value,
        Err(error) => {
            // 保原真实result/事件/错误结算路径，不为新呈现失败丢弃已执行调用。
            tracing::warn!(%error, "canonical tool artifact link could not be encoded; original result retained");
            return Message::tool_result(call_id.to_string(), name.to_string(), output);
        }
    };
    let mut message = Message::tool_result(call_id.to_string(), name.to_string(), String::new());
    const PREFIX: &str = "\n\n[Tool output preview only: the text above is not a summary and is not the complete result. Full output artifact: ";
    let suffix = format!(
        "{PREFIX}{} ({:.1} MiB, sha256 {}). Use read_artifact with this exact path and expected_sha256 to retrieve bounded pages until complete.]",
        reference.path.display(),
        reference.payload_bytes as f64 / 1_048_576.0,
        reference.sha256
    );
    // Producer-owned exact suffix, if present. Arbitrary output path text stays untouched.
    let text_reference = output.strip_suffix(&suffix).map(|preview| {
        serde_json::json!({
            "part": 0, "start_chars": preview.chars().count(), "chars": suffix.chars().count(),
            "sha256": format!("{:x}", Sha256::digest(suffix.as_bytes())),
            "path_start_chars": PREFIX.chars().count(),
            "path_chars": reference.path.to_string_lossy().chars().count(),
        })
    });
    // Value/String/primitive members only: no fallible external Serialize here.
    let meta = serde_json::json!({ "echo_tool_output_artifact": {
        "version": 1, "call_id": call_id, "tool_name": name,
        "reference": reference_value, "text_reference": text_reference,
    }});
    message.content = MessageContent::Parts(vec![
        ContentPart::Text { text: output },
        ContentPart::ResourceLink {
            resource: Box::new(LinkedResource {
                annotations: None,
                description: Some("Complete tool output artifact".into()),
                mime_type: Some("text/plain".into()),
                name: format!("{name} output"),
                size: i64::try_from(reference.artifact_bytes).ok(),
                title: None,
                uri: format!("echo-tool-output:sha256:{}", reference.sha256),
                meta: Some(meta),
            }),
        },
    ]);
    message
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
        text: "[runtime_context:ToolResultAttachment]\nRich content returned by the preceding tool call.".to_string(),
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
) -> Result<()> {
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
            context_guard.push(tool_result_with_artifact(
                call_id,
                &name,
                output.clone(),
                result.artifact.as_ref(),
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
        }
        Err(failure) => {
            let crate::agent::snapshot::ToolCallFailure {
                name,
                error,
                result,
            } = failure;
            if matches!(&error, ReactError::ExecutionNotSettled { .. }) {
                return Err(error);
            }
            let result = match result {
                Some(result) => result,
                None => return Err(error),
            };
            let message = result.error.clone().unwrap_or_else(|| error.to_string());
            context.lock().await.push(tool_result_with_artifact(
                call_id,
                &name,
                format!("[Error] {message}"),
                result.artifact.as_ref(),
            ));
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    call_id: call_id.to_string(),
                    name,
                    result,
                }))
                .await;
        }
    }
    Ok(())
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
    let terminal = AgentEvent::from_error("tool_batch", &error)?;
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
    let _ = tx.send(Ok(terminal)).await;
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
) -> Result<PublishedWave> {
    // Refuse this wave's observation before publishing any captured result or
    // synthesizing interrupted peer results into the transcript.
    for (call_id, _, _) in calls {
        if let Some((_, Err(failure))) = completed.get(call_id) {
            crate::error::AgentFailure::try_from(&failure.error)?;
        }
    }
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
            publish_completed_call(snap, context, tx, call_id, outcome).await?;
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
                !settlement
                    .admitted_calls
                    .iter()
                    .any(|(admitted_id, _, _)| admitted_id == call_id),
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
    Ok(published)
}

#[derive(Clone, Copy)]
struct InterruptedSettlement<'a> {
    category: crate::tools::ToolFailureCategory,
    message: &'a str,
    // IDs are unique inside this proposal, not across model iterations. A
    // call absent from the original admitted wave is definitively unstarted
    // even when a previous proposal used the same ID. Keep the original wave
    // slice as the phase fact instead of adding a second started-ID store.
    admitted_calls: &'a [ToolCallSpec],
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
/// batch by approval or tool concurrency policy, and settles every result
/// before handing final-answer candidates to the run driver. The driver owns
/// steer and cancellation fences before schema, Critic, or terminal decisions.
pub(crate) async fn run_tools(
    snap: &AgentRunSnapshot,
    context: &Arc<Mutex<crate::compression::ContextManager>>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    iteration: usize,
    think: ThinkOutput,
    _label: &str,
) -> Result<IterOutcome> {
    let agent = &snap.config.agent_name;
    let pt = think.pt;
    let ct = think.ct;

    // The model has returned, but its proposed calls have not been published
    // to the transcript or tool pipeline. A correction accepted during that
    // model call can therefore discard the proposal without inventing any
    // tool result. Keep any ordinary assistant draft as context only.
    let assistant_draft = (!think.content_buffer.is_empty()).then(|| {
        let mut message = with_reasoning_content(
            Message::assistant(think.content_buffer.clone()),
            think.reasoning_buffer.clone(),
            think.reasoning_blocks.clone(),
        );
        message.assistant_blocks = Some(think.assistant_blocks.clone());
        message.approved_assistant_blocks = Some(think.approved_assistant_blocks.clone());
        message
    });
    if !super::cancellation_requested(snap)
        && snap
            .drain_steer_into_context(context, assistant_draft)
            .await
            > 0
    {
        return Ok(IterOutcome::Continue);
    }

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
    let mut assistant_message = if msg_tc.is_empty() {
        Message::assistant("(流式工具调用参数解析失败,已跳过;请重新发起工具调用)".to_string())
    } else {
        let mut message = Message::assistant_with_tools(msg_tc);
        if !think.content_buffer.is_empty() {
            message.content = MessageContent::Text(think.content_buffer);
        }
        message
    };
    assistant_message.assistant_blocks = Some(think.assistant_blocks.clone());
    assistant_message.approved_assistant_blocks = Some(think.approved_assistant_blocks.clone());
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
        // A wave admitted by this check remains owned through all execution
        // futures and stream readers. Steering never races/drops those
        // futures. Re-check only before the next wave is admitted; Stop still
        // follows the existing cancellation/physical-settlement branches.
        if !super::cancellation_requested(snap) && snap.has_pending_steer() {
            let mut completed = HashMap::new();
            let mut emitted_invocations = HashSet::new();
            let published = settle_interrupted_calls(
                snap,
                context,
                tx,
                &remaining_calls,
                &mut completed,
                &mut emitted_invocations,
                InterruptedSettlement {
                    category: crate::tools::ToolFailureCategory::Permanent,
                    message: "Not executed: steering input requires reconsideration before this tool wave starts",
                    admitted_calls: &[],
                },
            )
            .await?;
            batch_failure_count = batch_failure_count.saturating_add(published.failures);
            // The original assistant already contains every call ID. Close
            // all still-unstarted calls before inserting the correction and
            // publishing its genuine Drained receipt. Earlier wave results
            // remain intact; the next model request will reconsider the
            // ordinary tool results with the drained steering input.
            if super::cancellation_requested(snap) {
                close_cancelled_batch(snap, context, tx, &batch_tool_names, batch_success_count)
                    .await?;
                return Ok(IterOutcome::TerminalSettled {
                    outcome: crate::agent::AgentSteerTurnOutcome::Cancelled,
                });
            }
            let _ = snap.drain_steer_into_context(context, None).await;
            break;
        }
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
                let mut cancellation_grace_elapsed = false;
                let mut batch_timed_out = false;

                let mut stream_open = true;
                while !futs.is_empty() || stream_open {
                    tokio::select! {
                        biased;
                        Some(()) = futs.next(), if !futs.is_empty() => {
                            while let Ok(event) = stream_rx.try_recv() {
                                forward_pipeline_event(tx, &mut emitted_invocations, event).await;
                            }
                        },
                        _ = &mut cancellation_grace, if cancellation_observed && !cancellation_grace_elapsed => {
                            cancellation_grace_elapsed = true;
                            tracing::warn!(
                                grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                                "tool batch cancellation observation elapsed; retaining original executions"
                            );
                        }
                        _ = &mut cancel, if !cancellation_observed => {
                            cancellation_observed = true;
                            cancellation_grace.as_mut().reset(
                                tokio::time::Instant::now() + TOOL_CANCELLATION_GRACE_PERIOD,
                            );
                        },
                        _ = &mut timeout, if !cancellation_observed => {
                            // Record the first cause, signal the original scope,
                            // and keep polling the same owned futures to closure.
                            batch_timed_out = true;
                            cancellation_observed = true;
                            cancellation_grace_elapsed = true;
                            if let Some(cancel) = snap.cancel_token.as_ref() { cancel.cancel(); }
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
                        category: if batch_timed_out {
                            crate::tools::ToolFailureCategory::Timeout
                        } else if cancellation_observed {
                            crate::tools::ToolFailureCategory::Cancelled
                        } else {
                            crate::tools::ToolFailureCategory::Unavailable
                        },
                        message: if batch_timed_out {
                            "Tool batch timeout elapsed"
                        } else if cancellation_observed {
                            "Tool call cancelled before returning a result"
                        } else {
                            "Tool call finished without a result"
                        },
                        admitted_calls: &conc,
                    },
                )
                .await?;
                batch_success_count = batch_success_count.saturating_add(published.successes);
                batch_failure_count = batch_failure_count.saturating_add(published.failures);
                if batch_timed_out {
                    close_failed_batch(
                        snap,
                        context,
                        tx,
                        &batch_tool_names,
                        batch_success_count,
                        ReactError::from(crate::error::ToolError::Timeout("batch timeout".into())),
                    )
                    .await?;
                    return Ok(IterOutcome::TerminalSettled {
                        outcome: crate::agent::AgentSteerTurnOutcome::Failed,
                    });
                }
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
                            admitted_calls: &[],
                        },
                    )
                    .await?;
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
                let mut cancellation_grace_elapsed = false;
                let result = loop {
                    tokio::select! {
                        biased;
                        result = &mut execution => break result,
                        _ = &mut cancellation_grace, if cancellation_observed && !cancellation_grace_elapsed => {
                            cancellation_grace_elapsed = true;
                            tracing::warn!(
                                tool = %fname,
                                grace_ms = TOOL_CANCELLATION_GRACE_PERIOD.as_millis(),
                                "tool cancellation observation elapsed; retaining original execution"
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
                        admitted_calls: std::slice::from_ref(&current_call),
                    },
                )
                .await?;
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
    use super::{AgentRunSnapshot, ReactError, tool_result_with_artifact};
    use super::{
        InterruptedSettlement, ToolExecutionWave, build_execution_waves, project_typed_tool_result,
        publish_completed_call, settle_interrupted_calls,
    };
    use crate::llm::types::{ContentPart, MessageContent};
    use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolResultContent, ToolResultKind};
    use serde_json::Value;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    fn call(id: &str) -> (String, String, Value) {
        (id.to_string(), "tool".to_string(), Value::Null)
    }

    struct BlockRichOutput;

    struct PassRichOutput;

    impl crate::guard::Guard for PassRichOutput {
        fn name(&self) -> &str {
            "pass_rich_output"
        }

        fn check<'a>(
            &'a self,
            _content: &'a str,
            _direction: crate::guard::GuardDirection,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>>
        {
            Box::pin(async { Ok(crate::guard::GuardResult::Pass) })
        }
    }

    impl crate::guard::Guard for BlockRichOutput {
        fn name(&self) -> &str {
            "block_rich_output"
        }

        fn check<'a>(
            &'a self,
            _content: &'a str,
            direction: crate::guard::GuardDirection,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<crate::guard::GuardResult>>
        {
            Box::pin(async move {
                if direction == crate::guard::GuardDirection::ToolOutput {
                    Ok(crate::guard::GuardResult::Block {
                        reason: "rich output blocked".to_string(),
                    })
                } else {
                    Ok(crate::guard::GuardResult::Pass)
                }
            })
        }
    }

    struct RichOutputTool;

    impl Tool for RichOutputTool {
        fn name(&self) -> &str {
            "rich_output"
        }

        fn description(&self) -> &str {
            "returns text and model-visible image data"
        }

        fn parameters(&self) -> Value {
            serde_json::json!({"type": "object"})
        }

        fn execute<'a>(
            &'a self,
            _parameters: ToolParameters,
        ) -> futures::future::BoxFuture<'a, crate::error::Result<ToolResult>> {
            Box::pin(async {
                Ok(ToolResult::success_with_kind(
                    ToolResultKind::Image {
                        mime_type: "image/png".to_string(),
                    },
                    "raw image result",
                )
                .with_data(serde_json::json!({"raw": "secret"}))
                .with_model_content(ToolResultContent::ImageUrl {
                    url: "data:image/png;base64,SECRET".to_string(),
                    detail: Some("high".to_string()),
                }))
            })
        }
    }

    #[test]
    fn canonical_tool_artifact_link_uses_real_processed_output_reference()
    -> crate::error::Result<()> {
        let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
        let agent = crate::agent::ReactAgent::new(
            crate::agent::AgentConfig::new("test-model", "agent", "system").tool_output_artifacts(
                Some(
                    echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                        root.path(),
                        "conversation_or_30d",
                    )
                    .threshold_bytes(1),
                ),
            ),
        );
        let snapshot = AgentRunSnapshot::from_agent(&agent);
        let raw = "中文工具原文🙂".repeat(4000);
        let processed = snapshot.process_tool_output_for_call(raw, "call-real", "shell", None);
        let reference = processed
            .artifact
            .as_ref()
            .ok_or_else(|| ReactError::Other("real producer did not spill".into()))?;
        let message = tool_result_with_artifact(
            "call-real",
            "shell",
            processed.output.clone(),
            Some(reference),
        );
        let MessageContent::Parts(parts) = &message.content else {
            return Err(ReactError::Other("typed content missing".into()));
        };
        let resource = parts
            .iter()
            .find_map(|part| match part {
                ContentPart::ResourceLink { resource } => Some(resource),
                _ => None,
            })
            .ok_or_else(|| ReactError::Other("typed link missing".into()))?;
        let binding = resource
            .meta
            .as_ref()
            .and_then(|value| value.get("echo_tool_output_artifact"))
            .ok_or_else(|| ReactError::Other("binding missing".into()))?;
        assert_eq!(
            binding.get("call_id").and_then(Value::as_str),
            Some("call-real")
        );
        let text = binding
            .get("text_reference")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ReactError::Other(
                    "real producer suffix did not bind; verify actual newline characters".into(),
                )
            })?;
        let start = text
            .get("start_chars")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| ReactError::Other("range missing".into()))?;
        assert!(
            processed
                .output
                .chars()
                .skip(start)
                .collect::<String>()
                .starts_with("\n\n[Tool output preview only:")
        );
        let projected = crate::memory::project_message("branch", &message)?;
        let restored = crate::memory::restore_message(&projected)?;
        assert_eq!(
            serde_json::to_value(restored)?,
            serde_json::to_value(message)?
        );
        let ordinary =
            tool_result_with_artifact("call-real", "shell", processed.output.clone(), None);
        assert!(
            matches!(ordinary.content, MessageContent::Text(_)),
            "matching text is not an artifact producer"
        );
        let arbitrary = tool_result_with_artifact(
            "call-real",
            "shell",
            "ordinary output /user/example".into(),
            Some(reference),
        );
        let MessageContent::Parts(parts) = arbitrary.content else {
            return Err(ReactError::Other("typed content missing".into()));
        };
        assert!(
            matches!(parts.first(), Some(ContentPart::Text { text }) if text == "ordinary output /user/example")
        );
        Ok(())
    }

    #[tokio::test]
    async fn published_tool_result_keeps_actual_artifact_in_same_canonical_message()
    -> crate::error::Result<()> {
        for failure in [false, true] {
            let root = tempfile::tempdir().map_err(|error| ReactError::Other(error.to_string()))?;
            let agent = crate::agent::ReactAgent::new(
                crate::agent::AgentConfig::new("test-model", "agent", "system")
                    .tool_output_artifacts(Some(
                        echo_core::tools::artifact::ToolOutputArtifactConfig::new(
                            root.path(),
                            "conversation_or_30d",
                        )
                        .threshold_bytes(1),
                    )),
            );
            let snapshot = AgentRunSnapshot::from_agent(&agent);
            let processed = snapshot.process_tool_output_for_call(
                "真实工具原文🙂".repeat(4000),
                "publisher-real",
                "shell",
                None,
            );
            let reference = processed
                .artifact
                .clone()
                .ok_or_else(|| ReactError::Other("actual spill reference missing".into()))?;
            let result = if failure {
                ToolResult::error("guarded diagnostic").with_artifact(reference.clone())
            } else {
                ToolResult::success(processed.output.clone()).with_artifact(reference.clone())
            };
            let outcome = if failure {
                Err(crate::agent::snapshot::ToolCallFailure {
                    name: "shell".into(),
                    error: ReactError::Other("guarded diagnostic".into()),
                    result: Some(result),
                })
            } else {
                Ok(crate::agent::snapshot::ToolCallSuccess {
                    name: "shell".into(),
                    result,
                })
            };
            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            publish_completed_call(
                &snapshot,
                &agent.memory.context,
                &tx,
                "publisher-real",
                outcome,
            )
            .await?;
            let event = rx
                .try_recv()
                .map_err(|error| ReactError::Other(error.to_string()))??;
            let crate::agent::AgentEvent::ToolResult { result, .. } = event else {
                return Err(ReactError::Other("actual result event missing".into()));
            };
            assert_eq!(result.success, !failure);
            assert_eq!(
                serde_json::to_value(result.artifact)?,
                serde_json::to_value(Some(reference.clone()))?
            );
            let messages = agent.memory.context.lock().await.messages().to_vec();
            let tool_message = messages
                .iter()
                .find(|message| message.tool_call_id.as_deref() == Some("publisher-real"))
                .ok_or_else(|| ReactError::Other("actual canonical tool message missing".into()))?;
            let MessageContent::Parts(parts) = &tool_message.content else {
                return Err(ReactError::Other(
                    "publisher dropped actual artifact carrier".into(),
                ));
            };
            let binding = parts
                .iter()
                .find_map(|part| match part {
                    ContentPart::ResourceLink { resource } => resource
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("echo_tool_output_artifact")),
                    _ => None,
                })
                .ok_or_else(|| ReactError::Other("publisher artifact metadata missing".into()))?;
            assert_eq!(
                binding.get("reference"),
                Some(&serde_json::to_value(&reference)?)
            );
            assert_eq!(
                binding.get("call_id").and_then(Value::as_str),
                Some("publisher-real")
            );
            let expected = if failure {
                "[Error] guarded diagnostic"
            } else {
                processed.output.as_str()
            };
            assert!(matches!(parts.first(), Some(ContentPart::Text { text }) if text == expected));
            let stored = crate::memory::project_message("branch", tool_message)?;
            assert_eq!(
                serde_json::to_value(crate::memory::restore_message(&stored)?)?,
                serde_json::to_value(tool_message)?
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn guarded_tool_result_cannot_reinject_rich_content_into_context()
    -> crate::error::Result<()> {
        for guard in [
            Arc::new(BlockRichOutput) as Arc<dyn crate::guard::Guard>,
            Arc::new(PassRichOutput) as Arc<dyn crate::guard::Guard>,
        ] {
            let agent = crate::agent::ReactAgentBuilder::new()
                .model("test-model")
                .guard(guard)
                .tool(Box::new(RichOutputTool))
                .build()?;
            let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
            let outcome = snapshot
                .execute_tool_with_policy(
                    "rich-output".to_string(),
                    "rich_output",
                    &ToolParameters::new(),
                    &serde_json::json!({}),
                    None,
                )
                .await
                .map_err(|failure| failure.error)?;
            assert!(outcome.result.model_content.is_empty());
            assert!(matches!(outcome.result.kind, ToolResultKind::Text));

            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            publish_completed_call(
                &snapshot,
                &agent.memory.context,
                &tx,
                "rich-output",
                Ok(outcome),
            )
            .await?;
            let event = rx
                .try_recv()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
            assert!(matches!(
                event,
                Ok(crate::agent::AgentEvent::ToolResult { result, .. })
                    if result.model_content.is_empty()
            ));
            let messages = agent.memory.context.lock().await.messages().to_vec();
            assert!(
                messages
                    .iter()
                    .all(|message| !matches!(message.content, MessageContent::Parts(_)))
            );
            assert!(messages.iter().all(|message| {
                message
                    .content
                    .as_text()
                    .is_none_or(|text| !text.contains("SECRET"))
            }));
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_tool_transcript_uses_guarded_result_error() -> crate::error::Result<()> {
        let agent = crate::agent::ReactAgent::new(crate::agent::AgentConfig::new(
            "test-model",
            "guarded",
            "sys",
        ));
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let failure = crate::agent::snapshot::ToolCallFailure {
            name: "error_only".to_string(),
            error: crate::error::ReactError::Other("raw secret diagnostic".to_string()),
            result: Some(ToolResult::error("guarded diagnostic")),
        };
        publish_completed_call(
            &snapshot,
            &agent.memory.context,
            &tx,
            "error-only",
            Err(failure),
        )
        .await?;
        let event = rx
            .try_recv()
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert!(matches!(
            event,
            Ok(crate::agent::AgentEvent::ToolResult { result, .. })
                if result.error.as_deref() == Some("guarded diagnostic")
        ));
        let messages = agent.memory.context.lock().await.messages().to_vec();
        assert!(messages.iter().any(|message| {
            message.content.as_text().as_deref() == Some("[Error] guarded diagnostic")
        }));
        assert!(!messages.iter().any(|message| {
            message
                .content
                .as_text()
                .is_some_and(|text| text.contains("raw secret"))
        }));
        Ok(())
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
    async fn interrupted_batch_distinguishes_unstarted_calls_from_possible_effects()
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
                admitted_calls: &calls,
            },
        )
        .await?;
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
                                && failure.side_effect
                                    == if call_id == "pending-a" {
                                        ToolSideEffect::Possible
                                    } else {
                                        ToolSideEffect::None
                                    }
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
    async fn interrupted_stop_and_timeout_use_original_execution_start_fact()
    -> crate::error::Result<()> {
        use echo_core::tools::{ToolFailureCategory, ToolSideEffect};

        let agent = crate::agent::ReactAgentBuilder::new()
            .model("test-model")
            .build()?;
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent);
        for category in [ToolFailureCategory::Cancelled, ToolFailureCategory::Timeout] {
            for started in [false, true] {
                let id = format!("{category:?}-{started}");
                if started {
                    snapshot.mark_tool_execution_started(&id);
                }
                let result = snapshot
                    .settle_interrupted_tool_call(
                        &id,
                        "owned_effect",
                        &Value::Null,
                        category,
                        "interrupted at the original boundary",
                        false,
                    )
                    .await;
                let failure = result.failure.ok_or_else(|| {
                    crate::error::ReactError::Other("missing interrupted failure facts".to_string())
                })?;
                assert_eq!(failure.category, category);
                assert_eq!(
                    failure.side_effect,
                    if started {
                        ToolSideEffect::Possible
                    } else {
                        ToolSideEffect::None
                    }
                );
                assert!(failure.postcondition.as_deref().is_some_and(|condition| {
                    if started {
                        condition.contains("verify the external effect")
                    } else {
                        condition.contains("not executed")
                    }
                }));
                if started {
                    // A completed earlier attempt may reuse this model ID.
                    // The current unadmitted phase overrides that old marker
                    // without deleting history needed by physical owners.
                    let later = snapshot
                        .settle_interrupted_tool_call(
                            &id,
                            "later_unadmitted_effect",
                            &Value::Null,
                            category,
                            "this later attempt was not admitted",
                            true,
                        )
                        .await;
                    assert!(
                        later
                            .failure
                            .as_ref()
                            .is_some_and(|failure| { failure.side_effect == ToolSideEffect::None })
                    );
                }
            }
        }
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
        context.lock().await.push(crate::llm::types::Message::user(
            "exact real request".to_string(),
        ));
        context.lock().await.push(message.clone());
        assert_eq!(
            context.lock().await.latest_user_request(),
            Some("exact real request".to_string())
        );
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
