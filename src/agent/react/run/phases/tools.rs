//! Per-iteration tool-call branch: emit `ToolCall` events, push assistant
//! message, split approval/concurrent batches, execute, dispatch verifier
//! handoff to `finalize_completed_run` when `final_answer` is accepted.

use super::super::processor::build_tool_calls_from_map;
use super::super::stream_macros::{try_send_or, yield_event_or};
use super::verify::verify_answer;
use super::{IterOutcome, LoopState, ThinkOutput};
use crate::agent::AgentEvent;
use crate::agent::react::{StepType, TOOL_FINAL_ANSWER};
use crate::agent::snapshot::AgentRunSnapshot;
use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use futures::future::join_all;
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
    for (_, name, args) in &steps {
        yield_event_or!(
            tx,
            AgentEvent::ToolCall {
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
    context
        .lock()
        .await
        .push(Message::assistant_with_tools(msg_tc));

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

    if !conc.is_empty() {
        let mc = snap.tools.tool_manager.max_concurrency();
        let snapshot = snap.clone();
        let futs: Vec<_> = conc
            .iter()
            .map(|(_, n, a)| {
                let snapshot = snapshot.clone();
                let name = n.clone();
                let args = a.clone();
                async move {
                    let params = if let Value::Object(m) = &args {
                        m.clone().into_iter().collect()
                    } else {
                        HashMap::new()
                    };
                    snapshot
                        .execute_tool_with_policy(&name, &params, &args)
                        .await
                }
                .instrument(info_span!("tool", tool.name = %n))
            })
            .collect();
        let bt = super::super::retry::compute_concurrent_tool_batch_timeout(
            &snap.config.tool_execution,
            futs.len(),
            mc,
        );
        let results: Vec<std::result::Result<String, ReactError>> = if let Some(to) = bt {
            try_send_or!(
                tx,
                tokio::time::timeout(to, join_all(futs)).await.map_err(|_| {
                    ReactError::from(crate::error::ToolError::Timeout("batch timeout".into()))
                }),
                IterOutcome::Abandoned
            )
        } else {
            join_all(futs).await
        };

        for ((id, fname, _), result) in conc.into_iter().zip(results) {
            match result {
                Ok(output) => {
                    yield_event_or!(
                        tx,
                        AgentEvent::ToolResult {
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
                            return Ok(IterOutcome::Finish { output });
                        }
                        // Verifier failed — continue loop for self-correction
                        state.verifier_retry_count += 1;
                    }
                }
                Err(error) => {
                    yield_event_or!(
                        tx,
                        AgentEvent::ToolError {
                            name: fname.clone(),
                            error: error.to_string(),
                        },
                        IterOutcome::Abandoned
                    );
                    context.lock().await.push(Message::tool_result(
                        id,
                        fname.clone(),
                        format!("[Error] {error}"),
                    ));
                    // Checkpoint on tool error for recovery
                    snap.save_runtime_checkpoint(context, Some(format!("Tool error: {fname}")))
                        .await;
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
        match snap.execute_tool_with_policy(&fname, &params, &args).await {
            Ok(truncated) => {
                yield_event_or!(
                    tx,
                    AgentEvent::ToolResult {
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
                        return Ok(IterOutcome::Finish { output: truncated });
                    }
                    // Verifier failed — continue loop for self-correction
                    state.verifier_retry_count += 1;
                }
            }
            Err(error) => {
                yield_event_or!(
                    tx,
                    AgentEvent::ToolError {
                        name: fname.clone(),
                        error: error.to_string(),
                    },
                    IterOutcome::Abandoned
                );
                context.lock().await.push(Message::tool_result(
                    id,
                    fname.clone(),
                    format!("[Error] {error}"),
                ));
                // Checkpoint on tool error for recovery
                snap.save_runtime_checkpoint(context, Some(format!("Tool error: {fname}")))
                    .await;
            }
        }
    }

    yield_event_or!(tx, AgentEvent::ToolBatchEnd, IterOutcome::Abandoned);
    snap.auto_snapshot(context, iteration).await;

    // Periodic runtime checkpoint based on configured interval
    let interval = snap.config.react_checkpoint_interval;
    if interval > 0 && (iteration + 1).is_multiple_of(interval) {
        snap.save_runtime_checkpoint(context, None).await;
    }

    Ok(IterOutcome::Continue)
}
