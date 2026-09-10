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
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::{WireDuration, WireField, WireU64, WireValue};
use futures::StreamExt as _;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::handles::HandleRegistry;
use super::super::wire;
use crate::factory::SessionAuthorityServices;

const METHOD: &str = "_echo_agent/workflow/op";

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
    pub graph: Arc<echo_agent::workflow::Graph>,
    pub cancel: tokio_util::sync::CancellationToken,
}

/// One standalone SharedState resource (owner-checked like graphs).
pub(crate) struct WorkflowStateRecord {
    pub state: echo_agent::workflow::SharedState,
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

pub(crate) fn workflow_event_value(
    event: echo_agent::workflow::WorkflowEvent,
) -> Result<WireValue, EchoSdkError> {
    let type_id = "echo_orchestration::workflow::WorkflowEvent".to_string();
    let string = |name: &str, value: String| WireField {
        name: name.to_string(),
        value: WireValue::String(value),
    };
    let number = |name: &str, value: usize| -> Result<WireField, EchoSdkError> {
        let value = u64::try_from(value)
            .map_err(|_| invalid(format!("workflow event {name} exceeds WireU64")))?;
        Ok(WireField {
            name: name.to_string(),
            value: WireValue::U64(WireU64::from_u64(value)),
        })
    };
    let duration = |name: &str, value: std::time::Duration| WireField {
        name: name.to_string(),
        value: WireValue::Duration(WireDuration {
            seconds: WireU64::from_u64(value.as_secs()),
            nanos: value.subsec_nanos(),
        }),
    };
    let (variant, fields) = match event {
        echo_agent::workflow::WorkflowEvent::NodeStart {
            node_name,
            step_index,
        } => (
            "node_start",
            vec![
                string("node_name", node_name),
                number("step_index", step_index)?,
            ],
        ),
        echo_agent::workflow::WorkflowEvent::NodeEnd {
            node_name,
            step_index,
            elapsed,
        } => (
            "node_end",
            vec![
                string("node_name", node_name),
                number("step_index", step_index)?,
                duration("elapsed", elapsed),
            ],
        ),
        echo_agent::workflow::WorkflowEvent::Token { node_name, token } => (
            "token",
            vec![string("node_name", node_name), string("token", token)],
        ),
        echo_agent::workflow::WorkflowEvent::NodeError { node_name, error } => (
            "node_error",
            vec![string("node_name", node_name), string("error", error)],
        ),
        echo_agent::workflow::WorkflowEvent::Completed {
            result,
            total_steps,
            elapsed,
        } => (
            "completed",
            vec![
                string("result", result),
                number("total_steps", total_steps)?,
                duration("elapsed", elapsed),
            ],
        ),
    };
    Ok(WireValue::Variant {
        type_id,
        variant: variant.to_string(),
        fields,
    })
}

/// The graph resource behind one issued [`WireHandle`], resolved through
/// the unified handle authority (shape/kind/generation/issued + owner) and
/// then the business map; a graph built by one Session is invisible to
/// every other.
fn graph_record(
    handles: &HandleRegistry,
    records: &std::sync::Mutex<HashMap<String, Arc<WorkflowGraphRecord>>>,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<WorkflowGraphRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    records
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown workflow graph resource {}", resource.id)))
}

/// Dispatch one workflow family operation. `owner` is the requesting
/// session's ACP id; every resource access is owner-checked against it.
/// Resource and page bounds come from the advertised Host limits (see
/// [`super::FacadeFamilyLimits`]), never from private magic constants.
// The dispatcher receives each existing Rust authority explicitly so workflow
// adapters cannot hide a second state, checkpoint, or stream owner in context.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
    handles: &HandleRegistry,
    graphs: &std::sync::Mutex<HashMap<String, Arc<WorkflowGraphRecord>>>,
    states: &std::sync::Mutex<HashMap<String, Arc<WorkflowStateRecord>>>,
    streams: &super::stream::FacadeStreamRuntime,
    authorities: &Arc<SessionAuthorityServices>,
    checkpoint_store: Option<Arc<dyn echo_agent::workflow::CheckpointStore>>,
    owner: &str,
    request: &FeatureOperationRequest,
    limits: super::FacadeFamilyLimits,
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
            let graph = match checkpoint_store.clone() {
                Some(store) => graph.with_checkpoint_store(store),
                None => graph,
            };
            // One shared token: the record's cancel() reaches the graph's
            // node boundaries through the same clone the engine holds.
            let cancel = tokio_util::sync::CancellationToken::new();
            let graph = Arc::new(graph.with_cancel_token(cancel.clone()));
            // The unified handle authority mints the id and enforces the
            // advertised global resource bound.
            let (resource, _record) = handles.register_facade_resource(
                limits.max_resources,
                "workflow",
                "workflow.graph",
                Some(owner),
                METHOD,
            )?;
            graphs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(WorkflowGraphRecord { graph, cancel }),
                );
            wire(serde_json::json!({
                "resource": resource,
                "name": name,
                "nodes": node_count,
                "edges": edge_count,
            }))
        }
        "workflow.graph.run" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let values = object_at(&arguments, 1)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
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
        "workflow.graph.run_stream" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let values = object_at(&arguments, 1)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            let producer = streams.open(handles, &resource, owner, &request.operation)?;
            let sender = producer.sender.clone();
            let cancel = producer.cancel.clone();
            let graph = record.graph.clone();
            let state = shared_state_of(values);
            let background = tokio::spawn(async move {
                let source = graph.run_stream(state).await;
                let mut source = match source {
                    Ok(source) => source,
                    Err(error) => {
                        let _ = sender.send(Err(framework(error))).await;
                        return;
                    }
                };
                loop {
                    let item = tokio::select! {
                        () = cancel.cancelled() => break,
                        item = source.next() => item,
                    };
                    let Some(event) = item else { break };
                    let item = event.map_err(framework).and_then(workflow_event_value);
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        result = sender.send(item) => {
                            if result.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            streams.attach_background(&producer, background);
            Ok(WireValue::Handle(producer.handle))
        }
        "workflow.graph.run_until_interrupt" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let values = object_at(&arguments, 1)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            let result = record
                .graph
                .run_until_interrupt(shared_state_of(values))
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.resume" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
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
            let record = graph_record(handles, graphs, &resource, owner)?;
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
        "workflow.graph.resume_exact" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let checkpoint = arguments
                .get(1)
                .cloned()
                .ok_or_else(|| invalid("workflow.graph.resume_exact requires a checkpoint"))?;
            let checkpoint: echo_agent::workflow::Checkpoint =
                serde_json::from_value(checkpoint)
                    .map_err(|error| invalid(format!("checkpoint is malformed: {error}")))?;
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
            let record = graph_record(handles, graphs, &resource, owner)?;
            let result = record
                .graph
                .resume(checkpoint, decision)
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.resume_with_state" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let checkpoint = arguments
                .get(1)
                .cloned()
                .ok_or_else(|| invalid("workflow.graph.resume_with_state requires a checkpoint"))?;
            let checkpoint: echo_agent::workflow::Checkpoint =
                serde_json::from_value(checkpoint)
                    .map_err(|error| invalid(format!("checkpoint is malformed: {error}")))?;
            let updates = object_at(&arguments, 2)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            let result = record
                .graph
                .resume_with_state(checkpoint, updates)
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.branch" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let updates = object_at(&arguments, 2)?;
            let branch_name = arguments.get(3).and_then(serde_json::Value::as_str);
            let record = graph_record(handles, graphs, &resource, owner)?;
            let result = record
                .graph
                .branch_from(&checkpoint_id, updates, branch_name.map(str::to_string))
                .await
                .map_err(framework)?;
            wire(run_outcome_json(result)?)
        }
        "workflow.graph.tag_checkpoint" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
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
            let record = graph_record(handles, graphs, &resource, owner)?;
            let tag_refs: Vec<&str> = tags.iter().map(String::as_str).collect();
            record
                .graph
                .tag_checkpoint(&checkpoint_id, label, tag_refs)
                .await
                .map_err(framework)?;
            wire(serde_json::json!({"ok": true}))
        }
        "workflow.graph.list_checkpoints" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            let infos = record.graph.list_checkpoints().await.map_err(framework)?;
            let page: Vec<serde_json::Value> = infos
                .iter()
                .take(limits.page)
                .map(|info| serde_json::to_value(info).unwrap_or_else(|_| serde_json::Value::Null))
                .collect();
            wire(serde_json::json!({
                "checkpoints": page,
                "truncated": infos.len() > limits.page,
            }))
        }
        "workflow.graph.list_checkpoints_by_graph" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            let infos = record
                .graph
                .list_checkpoints_by_graph()
                .await
                .map_err(framework)?;
            wire(serde_json::to_value(infos).map_err(|error| invalid(error.to_string()))?)
        }
        "workflow.graph.load_checkpoint" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            let checkpoint = record
                .graph
                .load_checkpoint(&checkpoint_id)
                .await
                .map_err(framework)?;
            wire(serde_json::to_value(checkpoint).map_err(|error| invalid(error.to_string()))?)
        }
        "workflow.graph.restore" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let checkpoint_id = string_at(&arguments, 1, "a checkpoint id")?;
            let record = graph_record(handles, graphs, &resource, owner)?;
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
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow graph resource", METHOD)?;
            let record = graph_record(handles, graphs, &resource, owner)?;
            record.cancel.cancel();
            wire(serde_json::json!({"cancelled": true}))
        }
        "workflow.state.new" => {
            let values = object_at(&arguments, 0)?;
            let (resource, _record) = handles.register_facade_resource(
                limits.max_resources,
                "workflow",
                "workflow.state",
                Some(owner),
                METHOD,
            )?;
            states
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(WorkflowStateRecord {
                        state: shared_state_of(values),
                    }),
                );
            wire(serde_json::json!({"resource": resource}))
        }
        "workflow.state.get" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow state resource", METHOD)?;
            let key = string_at(&arguments, 1, "a key")?;
            let record = state_record(handles, states, &resource, owner)?;
            let value = record
                .state
                .get_raw(&key)
                .unwrap_or(serde_json::Value::Null);
            wire(serde_json::json!({"value": value}))
        }
        "workflow.state.set" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow state resource", METHOD)?;
            let key = string_at(&arguments, 1, "a key")?;
            let value = arguments
                .get(2)
                .cloned()
                .ok_or_else(|| invalid("workflow.state.set requires a value at argument 2"))?;
            let record = state_record(handles, states, &resource, owner)?;
            record
                .state
                .set(&key, value)
                .map_err(|error| invalid(format!("workflow state update failed: {error}")))?;
            wire(serde_json::json!({"ok": true}))
        }
        "workflow.state.keys" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow state resource", METHOD)?;
            let record = state_record(handles, states, &resource, owner)?;
            let mut keys = record.state.keys();
            keys.truncate(limits.page);
            wire(serde_json::json!({"keys": keys}))
        }
        "workflow.state.snapshot" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a workflow state resource", METHOD)?;
            let record = state_record(handles, states, &resource, owner)?;
            wire(state_json(&record.state)?)
        }
        other => Err(invalid(format!(
            "unknown workflow operation {other}; the family surface is closed"
        ))),
    }
}

fn state_record(
    handles: &HandleRegistry,
    records: &std::sync::Mutex<HashMap<String, Arc<WorkflowStateRecord>>>,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<WorkflowStateRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    records
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown workflow state resource {}", resource.id)))
}

/// Drop every workflow resource whose handle the unified authority just
/// closed (session close or connection teardown cascade).
pub(crate) fn drop_session_resources(
    graphs: &std::sync::Mutex<HashMap<String, Arc<WorkflowGraphRecord>>>,
    states: &std::sync::Mutex<HashMap<String, Arc<WorkflowStateRecord>>>,
    closed: &[String],
) {
    graphs
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|id, record| {
            if closed.iter().any(|closed_id| closed_id == id) {
                record.cancel.cancel();
                false
            } else {
                true
            }
        });
    states
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
}
