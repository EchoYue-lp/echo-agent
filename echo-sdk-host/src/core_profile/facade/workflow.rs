//! Workflow family adapter (plan 07, todo 4).
//!
//! `_echo_agent/workflow/op` routes declarative graph workflows onto the
//! framework's own [`echo_agent::workflow`] engine. A client ships a
//! `WorkflowDefinition` (the same declarative JSON/YAML surface the loader
//! parses); the Host builds the graph through
//! [`WorkflowDefinition::build_graph_with_llm_config`] — agent nodes reuse
//! the Session Agent's exact LLM configuration — and keeps the compiled
//! graph as a facade resource.
//!
//! The Host owns addressing and lifecycle only (resource ids, the cancel
//! token, per-session cleanup); the graph engine itself — routing,
//! fan-out, interrupts, checkpoints, claim/resume CAS — stays the single
//! authority in `echo_orchestration::workflow`. Closures and callbacks are
//! NOT remotely constructible: the family surface is the closed
//! declarative set (`agent`/`router` nodes, fixed/conditional/parallel
//! edges), matching the loader's own limits.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::wire;
use crate::factory::SessionAuthorityServices;

const METHOD: &str = "_echo_agent/workflow/op";
/// Upper bound on compiled graph resources per connection; a graph holds
/// its agents, so this stays far below the subagent record bound.
const MAX_GRAPHS: usize = 128;
/// Upper bound on standalone SharedState resources per connection.
const MAX_STATES: usize = 512;

fn invalid(message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        METHOD,
    )
}

fn framework(error: echo_agent::error::ReactError) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&error.to_string()),
        Retryability::Never,
        METHOD,
    )
}

/// One compiled graph resource: the graph itself, its cooperative cancel
/// token and the owning ACP session. Cancelling the token makes every
/// in-flight node boundary fail, exactly like an in-process caller's
/// `with_cancel_token`.
pub(crate) struct WorkflowGraphRecord {
    pub graph: echo_agent::workflow::Graph,
    pub cancel: tokio_util::sync::CancellationToken,
    pub owner: String,
}

/// One standalone SharedState resource (owner-checked like graphs).
pub(crate) struct WorkflowStateRecord {
    pub state: echo_agent::workflow::SharedState,
    pub owner: String,
}

fn json_of(value: &WireValue, position: usize) -> Result<serde_json::Value, EchoSdkError> {
    value.clone().into_json().map_err(|error| {
        invalid(format!(
            "workflow argument {position} is not a lossless wire value: {error}"
        ))
    })
}

fn string_at(
    arguments: &[serde_json::Value],
    position: usize,
    what: &str,
) -> Result<String, EchoSdkError> {
    arguments
        .get(position)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            invalid(format!(
                "workflow operation requires {what} at argument {position}"
            ))
        })
}

/// Optional object argument at `position` (defaults to an empty map).
fn object_at(
    arguments: &[serde_json::Value],
    position: usize,
) -> Result<HashMap<String, serde_json::Value>, EchoSdkError> {
    match arguments.get(position) {
        None | Some(serde_json::Value::Null) => Ok(HashMap::new()),
        Some(value @ serde_json::Value::Object(_)) => serde_json::from_value(value.clone())
            .map_err(|error| invalid(format!("workflow argument {position}: {error}"))),
        Some(_) => Err(invalid(format!(
            "workflow argument {position} must be an object"
        ))),
    }
}

fn state_json(
    state: &echo_agent::workflow::SharedState,
) -> Result<serde_json::Value, EchoSdkError> {
    state
        .to_json_value()
        .map_err(|error| invalid(format!("workflow state serialization failed: {error}")))
}

fn run_outcome_json(
    result: echo_agent::workflow::RunUntilInterruptResult,
) -> Result<serde_json::Value, EchoSdkError> {
    use echo_agent::workflow::RunUntilInterruptResult;
    match result {
        RunUntilInterruptResult::Completed(result) => Ok(serde_json::json!({
            "outcome": "completed",
            "path": result.path,
            "steps": result.steps,
            "state": state_json(&result.state)?,
        })),
        RunUntilInterruptResult::Interrupted(interrupt) => {
            let checkpoint = serde_json::to_value(&interrupt.checkpoint)
                .map_err(|error| invalid(format!("checkpoint encoding failed: {error}")))?;
            Ok(serde_json::json!({
                "outcome": "interrupted",
                "pending_node": interrupt.pending_node,
                "prompt": interrupt.prompt,
                "checkpoint": checkpoint,
            }))
        }
        RunUntilInterruptResult::Deferred(interrupt) => {
            let checkpoint = serde_json::to_value(&interrupt.checkpoint)
                .map_err(|error| invalid(format!("checkpoint encoding failed: {error}")))?;
            Ok(serde_json::json!({
                "outcome": "deferred",
                "pending_node": interrupt.pending_node,
                "checkpoint": checkpoint,
            }))
        }
        RunUntilInterruptResult::Rejected {
            state,
            path,
            steps,
            reason,
        } => Ok(serde_json::json!({
            "outcome": "rejected",
            "path": path,
            "steps": steps,
            "reason": reason,
            "state": state_json(&state)?,
        })),
    }
}

fn shared_state_of(
    values: HashMap<String, serde_json::Value>,
) -> echo_agent::workflow::SharedState {
    echo_agent::workflow::SharedState::from_values(values)
}

/// The graph resource for `graph_id`, owner-checked against the requesting
/// session; a graph built by one Session is invisible to every other.
fn graph_record(
    records: &std::sync::Mutex<HashMap<String, Arc<WorkflowGraphRecord>>>,
    graph_id: &str,
    owner: &str,
) -> Result<Arc<WorkflowGraphRecord>, EchoSdkError> {
    let record = records
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(graph_id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown workflow graph resource {graph_id}")))?;
    if record.owner != owner {
        return Err(invalid(format!(
            "workflow graph {graph_id} belongs to another session"
        )));
    }
    Ok(record)
}

/// Dispatch one workflow family operation. `owner` is the requesting
/// session's ACP id; every resource access is owner-checked against it.
pub(crate) async fn dispatch(
    graphs: &std::sync::Mutex<HashMap<String, Arc<WorkflowGraphRecord>>>,
    states: &std::sync::Mutex<HashMap<String, Arc<WorkflowStateRecord>>>,
    authorities: &Arc<SessionAuthorityServices>,
    owner: &str,
    request: &FeatureOperationRequest,
    page_limit: usize,
) -> Result<WireValue, EchoSdkError> {
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(value, position))
        .collect::<Result<_, _>>()?;
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(error.to_string()))
    };
    match request.operation.as_str() {
        "workflow.graph.build" => {
            let definition_json = string_at(&arguments, 0, "a workflow definition JSON string")?;
            let definition =
                echo_agent::workflow::WorkflowDefinition::from_json_str(&definition_json)
                    .map_err(framework)?;
            let node_count = definition.nodes.len();
            let edge_count = definition.edges.len();
            let name = definition.name.clone();
            // Agent nodes reuse the Session Agent's own provider: the
            // explicit client first (how the Host injects providers), the
            // LLM config as fallback for config-only agents.
            let client = authorities.llm_client.clone();
            let graph = if client.is_some() {
                definition
                    .build_graph_with_client(client)
                    .map_err(framework)?
            } else {
                definition
                    .build_graph_with_llm_config(authorities.llm_config.as_ref())
                    .map_err(framework)?
            };
            // One shared token: the record's cancel() reaches the graph's
            // node boundaries through the same clone the engine holds.
            let cancel = tokio_util::sync::CancellationToken::new();
            let graph = graph.with_cancel_token(cancel.clone());
            let graph_id = format!("wfg-{}", uuid::Uuid::new_v4());
            let mut records = graphs.lock().unwrap_or_else(|error| error.into_inner());
            if records.len() >= MAX_GRAPHS {
                return Err(invalid(format!(
                    "workflow graph resource limit {MAX_GRAPHS} reached"
                )));
            }
            records.insert(
                graph_id.clone(),
                Arc::new(WorkflowGraphRecord {
                    graph,
                    cancel,
                    owner: owner.to_string(),
                }),
            );
            wire(serde_json::json!({
                "graph_id": graph_id,
                "name": name,
                "nodes": node_count,
                "edges": edge_count,
            }))
        }
        "workflow.graph.run" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let values = object_at(&arguments, 1)?;
            let record = graph_record(graphs, &graph_id, owner)?;
            let result = record
                .graph
                .run(shared_state_of(values))
                .await
                .map_err(framework)?;
            wire(serde_json::json!({
                "outcome": "completed",
                "path": result.path,
                "steps": result.steps,
                "state": state_json(&result.state)?,
            }))
        }
        "workflow.graph.run_until_interrupt" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let values = object_at(&arguments, 1)?;
            let record = graph_record(graphs, &graph_id, owner)?;
            let result = record
                .graph
                .run_until_interrupt(shared_state_of(values))
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.resume" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let decision = string_at(&arguments, 2, "a decision (approve|reject|defer)")?;
            let reason = arguments.get(3).and_then(serde_json::Value::as_str);
            let decision = match decision.as_str() {
                "approve" => echo_agent::workflow::ApprovalDecision::Approved,
                "reject" => echo_agent::workflow::ApprovalDecision::Rejected {
                    reason: reason.map(str::to_string),
                },
                "defer" => echo_agent::workflow::ApprovalDecision::Deferred,
                other => {
                    return Err(invalid(format!(
                        "unknown resume decision {other}; expected approve|reject|defer"
                    )));
                }
            };
            let record = graph_record(graphs, &graph_id, owner)?;
            let checkpoint = record
                .graph
                .load_checkpoint(&checkpoint_id)
                .await
                .map_err(framework)?
                .ok_or_else(|| invalid(format!("unknown checkpoint {checkpoint_id}")))?;
            let result = record
                .graph
                .resume(checkpoint, decision)
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.branch" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let updates = object_at(&arguments, 2)?;
            let branch_name = arguments.get(3).and_then(serde_json::Value::as_str);
            let record = graph_record(graphs, &graph_id, owner)?;
            let result = record
                .graph
                .branch_from(&checkpoint_id, updates, branch_name.map(str::to_string))
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.tag_checkpoint" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let label = arguments.get(2).and_then(serde_json::Value::as_str);
            let tags: Vec<String> = arguments
                .get(3)
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let record = graph_record(graphs, &graph_id, owner)?;
            let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
            record
                .graph
                .tag_checkpoint(&checkpoint_id, label, tag_refs)
                .await
                .map_err(framework)?;
            wire(serde_json::json!({"ok": true}))
        }
        "workflow.graph.list_checkpoints" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let record = graph_record(graphs, &graph_id, owner)?;
            let infos = record.graph.list_checkpoints().await.map_err(framework)?;
            let page: Vec<serde_json::Value> = infos
                .iter()
                .take(page_limit)
                .map(|info| serde_json::to_value(info).unwrap_or_else(|_| serde_json::Value::Null))
                .collect();
            wire(serde_json::json!({
                "checkpoints": page,
                "truncated": infos.len() > page_limit,
            }))
        }
        "workflow.graph.restore" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let record = graph_record(graphs, &graph_id, owner)?;
            let (state, checkpoint) = record
                .graph
                .restore_to_checkpoint(&checkpoint_id)
                .await
                .map_err(framework)?;
            let checkpoint = serde_json::to_value(checkpoint)
                .map_err(|error| invalid(format!("checkpoint encoding failed: {error}")))?;
            wire(serde_json::json!({
                "state": state_json(&state)?,
                "checkpoint": checkpoint,
            }))
        }
        "workflow.graph.cancel" => {
            let graph_id = string_at(&arguments, 0, "a graph id")?;
            let record = graph_record(graphs, &graph_id, owner)?;
            record.cancel.cancel();
            wire(serde_json::json!({"cancelled": true}))
        }
        "workflow.state.new" => {
            let values = object_at(&arguments, 0)?;
            let state_id = format!("wfs-{}", uuid::Uuid::new_v4());
            let mut records = states.lock().unwrap_or_else(|error| error.into_inner());
            if records.len() >= MAX_STATES {
                return Err(invalid(format!(
                    "workflow state resource limit {MAX_STATES} reached"
                )));
            }
            records.insert(
                state_id.clone(),
                Arc::new(WorkflowStateRecord {
                    state: shared_state_of(values),
                    owner: owner.to_string(),
                }),
            );
            wire(serde_json::json!({"state_id": state_id}))
        }
        "workflow.state.get" => {
            let state_id = string_at(&arguments, 0, "a state id")?;
            let key = string_at(&arguments, 1, "a key")?;
            let record = state_record(states, &state_id, owner)?;
            let value = record
                .state
                .get_raw(&key)
                .unwrap_or(serde_json::Value::Null);
            wire(serde_json::json!({"value": value}))
        }
        "workflow.state.set" => {
            let state_id = string_at(&arguments, 0, "a state id")?;
            let key = string_at(&arguments, 1, "a key")?;
            let value = arguments
                .get(2)
                .cloned()
                .ok_or_else(|| invalid("workflow.state.set requires a value at argument 2"))?;
            let record = state_record(states, &state_id, owner)?;
            record
                .state
                .set(&key, value)
                .map_err(|error| invalid(format!("workflow state update failed: {error}")))?;
            wire(serde_json::json!({"ok": true}))
        }
        "workflow.state.keys" => {
            let state_id = string_at(&arguments, 0, "a state id")?;
            let record = state_record(states, &state_id, owner)?;
            let mut keys = record.state.keys();
            keys.truncate(page_limit);
            wire(serde_json::json!({"keys": keys}))
        }
        "workflow.state.snapshot" => {
            let state_id = string_at(&arguments, 0, "a state id")?;
            let record = state_record(states, &state_id, owner)?;
            wire(state_json(&record.state)?)
        }
        other => Err(invalid(format!(
            "unknown workflow operation {other}; the family surface is closed"
        ))),
    }
}

fn state_record(
    records: &std::sync::Mutex<HashMap<String, Arc<WorkflowStateRecord>>>,
    state_id: &str,
    owner: &str,
) -> Result<Arc<WorkflowStateRecord>, EchoSdkError> {
    let record = records
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(state_id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown workflow state resource {state_id}")))?;
    if record.owner != owner {
        return Err(invalid(format!(
            "workflow state {state_id} belongs to another session"
        )));
    }
    Ok(record)
}

/// Drop every workflow resource owned by one session (session close).
pub(crate) fn drop_session_resources(
    graphs: &std::sync::Mutex<HashMap<String, Arc<WorkflowGraphRecord>>>,
    states: &std::sync::Mutex<HashMap<String, Arc<WorkflowStateRecord>>>,
    owner: &str,
) {
    graphs
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|_, record| {
            if record.owner == owner {
                record.cancel.cancel();
                false
            } else {
                true
            }
        });
    states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|_, record| record.owner != owner);
}
