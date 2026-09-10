//! Observability family adapters: trace, eval, improve (plan 07, todo 4).
//!
//! `_echo_agent/trace/op` resource-izes the framework's
//! [`echo_agent::trace::RunStore`] (in-memory or JSONL): save/load/list are
//! the store's real verbs and run shapes are the framework's serialized
//! `Run`/`RunSummary` — no second trace model.
//!
//! `_echo_agent/eval/op` (feature `eval`) runs the framework's constraint
//! evaluation over a completed run trace and builds `EvalReport`s from
//! `EvalResult`s — the same types the eval runner produces.
//!
//! `_echo_agent/improve/op` (feature `improve`) exports ShareGPT
//! trajectories; with `eval` also compiled it exposes the run analyzer's
//! real critique. Neither family runs an optimization loop remotely — the
//! improvement loop needs a host-language agent factory and stays with the
//! ExtensionBridge.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::collections::HashMap;
use std::sync::Arc;

use super::super::handles::HandleRegistry;
use super::super::wire;

const TRACE_METHOD: &str = "_echo_agent/trace/op";
#[cfg(feature = "framework-eval")]
const EVAL_METHOD: &str = "_echo_agent/eval/op";
#[cfg(feature = "framework-improve")]
const IMPROVE_METHOD: &str = "_echo_agent/improve/op";

/// One run-store resource: the framework store plus the owning session.
pub(crate) struct TraceStoreRecord {
    pub store: Arc<dyn echo_agent::trace::RunStore>,
}

fn invalid(method: &'static str, message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        method,
    )
}

fn framework(method: &'static str, error: echo_agent::error::ReactError) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&error.to_string()),
        Retryability::Never,
        method,
    )
}

fn json_of(
    method: &'static str,
    value: &WireValue,
    position: usize,
) -> Result<serde_json::Value, EchoSdkError> {
    value.clone().into_json().map_err(|error| {
        invalid(
            method,
            format!("argument {position} is not a lossless wire value: {error}"),
        )
    })
}

fn string_at(
    method: &'static str,
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
            invalid(
                method,
                format!("operation requires {what} at argument {position}"),
            )
        })
}

fn arguments_of(
    method: &'static str,
    request: &FeatureOperationRequest,
) -> Result<Vec<serde_json::Value>, EchoSdkError> {
    request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(method, value, position))
        .collect()
}

// ── Trace family ────────────────────────────────────────────────────────────

fn store_record(
    handles: &HandleRegistry,
    stores: &std::sync::Mutex<HashMap<String, Arc<TraceStoreRecord>>>,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<TraceStoreRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, TRACE_METHOD)?;
    stores
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| {
            invalid(
                TRACE_METHOD,
                format!("unknown trace store resource {}", resource.id),
            )
        })
}

/// Dispatch one trace family operation.
pub(crate) async fn dispatch_trace(
    handles: &HandleRegistry,
    stores: &std::sync::Mutex<HashMap<String, Arc<TraceStoreRecord>>>,
    owner: &str,
    request: &FeatureOperationRequest,
    limits: super::FacadeFamilyLimits,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(TRACE_METHOD, error.to_string()))
    };
    let arguments = arguments_of(TRACE_METHOD, request)?;
    match request.operation.as_str() {
        "trace.store.open" => {
            let mode = string_at(TRACE_METHOD, &arguments, 0, "a store mode (memory|jsonl)")?;
            let path = arguments.get(1).and_then(serde_json::Value::as_str);
            let (resource, _record) = handles.register_facade_resource(
                limits.max_resources,
                "trace",
                "trace.store",
                Some(owner),
                TRACE_METHOD,
            )?;
            let store: Arc<dyn echo_agent::trace::RunStore> = match (mode.as_str(), path) {
                ("memory", _) => Arc::new(echo_agent::trace::InMemoryRunStore::new()),
                ("jsonl", Some(path)) => {
                    let store = echo_agent::trace::JsonlRunStore::new(path)
                        .map_err(|error| framework(TRACE_METHOD, error))?;
                    Arc::new(store)
                }
                ("jsonl", None) => {
                    return Err(invalid(
                        TRACE_METHOD,
                        "jsonl trace stores require a directory path at argument 1",
                    ));
                }
                (other, _) => {
                    return Err(invalid(
                        TRACE_METHOD,
                        format!("unknown trace store mode {other}; expected memory|jsonl"),
                    ));
                }
            };
            stores
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(resource.id.clone(), Arc::new(TraceStoreRecord { store }));
            wire(serde_json::json!({"resource": resource}))
        }
        "trace.run.save" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a trace store resource", TRACE_METHOD)?;
            let run_json = arguments.get(1).ok_or_else(|| {
                invalid(TRACE_METHOD, "trace.run.save requires a run at argument 1")
            })?;
            let run: echo_agent::trace::Run =
                serde_json::from_value(run_json.clone()).map_err(|error| {
                    invalid(TRACE_METHOD, format!("run payload malformed: {error}"))
                })?;
            let record = store_record(handles, stores, &resource, owner)?;
            record
                .store
                .save(run)
                .await
                .map_err(|error| framework(TRACE_METHOD, error))?;
            wire(serde_json::json!({"ok": true}))
        }
        "trace.run.load" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a trace store resource", TRACE_METHOD)?;
            let run_id = string_at(TRACE_METHOD, &arguments, 1, "a run id")?;
            let record = store_record(handles, stores, &resource, owner)?;
            let run = record
                .store
                .load(&run_id)
                .await
                .map_err(|error| framework(TRACE_METHOD, error))?;
            let run = run
                .map(|run| serde_json::to_value(&run).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null);
            wire(serde_json::json!({"run": run}))
        }
        "trace.run.list_session" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a trace store resource", TRACE_METHOD)?;
            let session_id = string_at(TRACE_METHOD, &arguments, 1, "a session id")?;
            let record = store_record(handles, stores, &resource, owner)?;
            let summaries = record
                .store
                .list_by_session(&session_id)
                .await
                .map_err(|error| framework(TRACE_METHOD, error))?;
            let page: Vec<serde_json::Value> = summaries
                .iter()
                .take(limits.page)
                .map(|summary| serde_json::to_value(summary).unwrap_or_default())
                .collect();
            wire(serde_json::json!({"summaries": page}))
        }
        "trace.run.list_recent" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a trace store resource", TRACE_METHOD)?;
            let limit = arguments
                .get(1)
                .and_then(serde_json::Value::as_u64)
                .map(|limit| limit.min(limits.page as u64) as usize)
                .unwrap_or(limits.page);
            let record = store_record(handles, stores, &resource, owner)?;
            let summaries = record
                .store
                .list_all(limit)
                .await
                .map_err(|error| framework(TRACE_METHOD, error))?;
            let page: Vec<serde_json::Value> = summaries
                .iter()
                .take(limit)
                .map(|summary| serde_json::to_value(summary).unwrap_or_default())
                .collect();
            wire(serde_json::json!({"summaries": page}))
        }
        other => Err(invalid(
            TRACE_METHOD,
            format!("unknown trace operation {other}; the family surface is closed"),
        )),
    }
}

// ── Eval family ─────────────────────────────────────────────────────────────

/// Dispatch one eval family operation (feature `eval`).
#[cfg(feature = "framework-eval")]
pub(crate) async fn dispatch_eval(
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(EVAL_METHOD, error.to_string()))
    };
    let arguments = arguments_of(EVAL_METHOD, request)?;
    match request.operation.as_str() {
        "eval.constraints.run" => {
            let constraints_json = arguments.first().ok_or_else(|| {
                invalid(
                    EVAL_METHOD,
                    "eval.constraints.run requires constraints at argument 0",
                )
            })?;
            let run_json = arguments.get(1).ok_or_else(|| {
                invalid(
                    EVAL_METHOD,
                    "eval.constraints.run requires a run at argument 1",
                )
            })?;
            let constraints: echo_agent::eval::EvalConstraints =
                serde_json::from_value(constraints_json.clone()).map_err(|error| {
                    invalid(EVAL_METHOD, format!("constraints malformed: {error}"))
                })?;
            let run: echo_agent::trace::Run = serde_json::from_value(run_json.clone())
                .map_err(|error| invalid(EVAL_METHOD, format!("run payload malformed: {error}")))?;
            let runner = echo_agent::eval::EvalRunner::new(std::env::temp_dir());
            let violations = runner.evaluate_run_constraints(&constraints, &run);
            wire(serde_json::json!({"violations": violations}))
        }
        "eval.report.build" => {
            let results_json = arguments
                .first()
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    invalid(
                        EVAL_METHOD,
                        "eval.report.build requires a results array at argument 0",
                    )
                })?;
            let mut results = Vec::with_capacity(results_json.len());
            for item in results_json {
                results.push(
                    serde_json::from_value::<echo_agent::eval::EvalResult>(item.clone()).map_err(
                        |error| invalid(EVAL_METHOD, format!("eval result malformed: {error}")),
                    )?,
                );
            }
            let report = echo_agent::eval::EvalReport::new(results);
            wire(serde_json::to_value(&report).unwrap_or_default())
        }
        other => Err(invalid(
            EVAL_METHOD,
            format!("unknown eval operation {other}; the family surface is closed"),
        )),
    }
}

// ── Improve family ──────────────────────────────────────────────────────────

/// Dispatch one improve family operation (feature `improve`).
#[cfg(feature = "framework-improve")]
pub(crate) async fn dispatch_improve(
    request: &FeatureOperationRequest,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(IMPROVE_METHOD, error.to_string()))
    };
    let arguments = arguments_of(IMPROVE_METHOD, request)?;
    let run_json = arguments.first().ok_or_else(|| {
        invalid(
            IMPROVE_METHOD,
            "improve operations require a run at argument 0",
        )
    })?;
    let run: echo_agent::trace::Run = serde_json::from_value(run_json.clone())
        .map_err(|error| invalid(IMPROVE_METHOD, format!("run payload malformed: {error}")))?;
    match request.operation.as_str() {
        "improve.trajectory.sharegpt" => {
            let messages = echo_agent::improve::TrajectorySaver::convert_run_to_sharegpt(&run);
            let messages: Vec<serde_json::Value> = messages
                .iter()
                .map(|message| serde_json::to_value(message).unwrap_or_default())
                .collect();
            wire(serde_json::json!({"messages": messages}))
        }
        "improve.run.analyze" => {
            // The analyzer's critique type is compiled with the eval
            // feature; without it the operation is honestly unavailable.
            #[cfg(feature = "framework-eval")]
            {
                let critique = echo_agent::improve::Analyzer::analyze(&run);
                wire(serde_json::to_value(&critique).unwrap_or_default())
            }
            #[cfg(not(feature = "framework-eval"))]
            {
                let mut error = EchoSdkError::new(
                    ExtensionErrorCode::FeatureUnavailable,
                    "improve.run.analyze requires the eval feature",
                    Retryability::Never,
                );
                error.details = Some(echo_sdk_protocol::error::ErrorDetails {
                    fields: None,
                    facade: Some(echo_sdk_protocol::error::FacadeFailureDetail {
                        required_feature: Some("eval".to_string()),
                        ..Default::default()
                    }),
                });
                Err(error)
            }
        }
        other => Err(invalid(
            IMPROVE_METHOD,
            format!("unknown improve operation {other}; the family surface is closed"),
        )),
    }
}

/// Drop every trace store owned by one session (session close).
pub(crate) fn drop_session_stores(
    stores: &std::sync::Mutex<HashMap<String, Arc<TraceStoreRecord>>>,
    closed: &[String],
) {
    stores
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
}
