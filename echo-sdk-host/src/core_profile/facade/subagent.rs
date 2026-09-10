//! Subagent facade handlers (plan 07, todo 3).
//!
//! `_echo_agent/subagent/dispatch|await|control` share the Session Agent's
//! own [`SubagentExecutor`] — captured before boxing (see
//! [`crate::factory::SessionAuthorityServices`]) — so RPC dispatches, the
//! in-conversation delegation tools and task execution share one control
//! plane. Control verbs are exactly the executor's real semantics
//! (message / guidance / interrupt / cancel); there is deliberately no
//! pause/resume.
//!
//! Dispatches run as controlled background attempts with Host-minted
//! [`SubagentAttemptIdentity`]s: the Subagent handle id is the execution id,
//! and every control verb addresses that exact live attempt. Results are
//! memoized per record because a background join is single-consumption.

use agent_client_protocol::{Client, ConnectionTo, Responder};
use echo_agent::agent::subagent::{DispatchRequest, SubagentAttemptIdentity, SubagentExecutor};
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    SubagentAwaitRequest, SubagentAwaitResponse, SubagentControlAction, SubagentControlRequest,
    SubagentControlResponse, SubagentDispatchRequest, SubagentDispatchResponse,
};
use echo_sdk_protocol::scalar::{WireU64, WireValue};
use std::sync::Arc;

use super::super::handler::{require_capability, require_extended};
use super::super::state::CoreProfileState;
use super::super::wire;

/// One live RPC-dispatched subagent attempt. Holds its own executor clone so
/// control and await never depend on the Session staying open for identity
/// resolution (settlement still follows the framework's own semantics).
pub(crate) struct SubagentDispatchRecord {
    pub executor: Arc<SubagentExecutor>,
    pub background: echo_agent::agent::subagent::BackgroundSubagentHandle,
    /// Control identity of the bound attempt.
    pub task_id: String,
    pub attempt: u32,
    /// Memoized terminal result (a background join is single-consumption).
    pub settled:
        tokio::sync::Mutex<Option<Result<echo_agent::agent::subagent::SubagentResult, String>>>,
    /// ACP session that owns the dispatch (records are session-scoped;
    /// session close cascades through it in the family closeout).
    pub owner_session: String,
}

fn subagent_error(
    code: ExtensionErrorCode,
    message: impl Into<String>,
    method: &str,
) -> EchoSdkError {
    wire::sdk_error(code, message, Retryability::Never, method)
}

fn record_of(
    state: &CoreProfileState,
    subagent: &WireHandle,
    method: &str,
) -> Result<Arc<SubagentDispatchRecord>, EchoSdkError> {
    state
        .handles
        .check_shape_and_generation(subagent, HandleKind::Subagent, method)?;
    state.facade.subagent_of(&subagent.id).ok_or_else(|| {
        subagent_error(
            ExtensionErrorCode::InvalidValue,
            "subagent handle was never issued by this Host",
            method,
        )
    })
}

pub(crate) async fn subagent_dispatch(
    state: Arc<CoreProfileState>,
    request: SubagentDispatchRequest,
    responder: Responder<SubagentDispatchResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/subagent/dispatch";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Subagents, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.session, HandleKind::Session, method)
    {
        fail!(error);
    }
    let acp_session_id = match state.handles.session(&request.session) {
        Ok(record) => record.acp_session_id.clone(),
        Err(error) => fail!(error),
    };
    let Some(authorities) = state.session_factory.session_services(&acp_session_id) else {
        fail!(subagent_error(
            ExtensionErrorCode::ClosedHandle,
            "session authority services are no longer live",
            method,
        ));
    };
    let spec = match request.request.clone().into_json() {
        Ok(value) => value,
        Err(error) => {
            fail!(subagent_error(
                ExtensionErrorCode::InvalidValue,
                format!("subagent request is not a lossless wire value: {error}"),
                method,
            ));
        }
    };
    let Some(agent_name) = spec
        .get("agent_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
    else {
        fail!(subagent_error(
            ExtensionErrorCode::InvalidValue,
            "subagent dispatch requires a non-empty agent_name",
            method,
        ));
    };
    let Some(task) = spec
        .get("task")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|task| !task.trim().is_empty())
    else {
        fail!(subagent_error(
            ExtensionErrorCode::InvalidValue,
            "subagent dispatch requires a non-empty task",
            method,
        ));
    };
    let constraints = spec
        .get("constraints")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();

    let identity = SubagentAttemptIdentity {
        task_id: format!("sdk-task-{}", uuid::Uuid::new_v4()),
        execution_id: format!("sdk-exec-{}", uuid::Uuid::new_v4()),
        attempt: 1,
    };
    let dispatch = DispatchRequest {
        agent_name,
        task,
        mode_override: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        parent_agent: "sdk-host".to_string(),
        parent_context: None,
        delegation_policy: Default::default(),
        runtime_context: None,
        message: None,
        prompt_payload: None,
        prompt_context: None,
        constraints,
        background: false,
    };
    // The identity moves into the dispatch; keep the task id for guidance
    // queueing before the move.
    let task_id = identity.task_id.clone();
    let executor = authorities.subagent_executor.clone();
    let record = match executor
        .dispatch_background_attempt(dispatch, identity)
        .await
    {
        Ok(background) => Arc::new(SubagentDispatchRecord {
            executor,
            task_id,
            background,
            attempt: 1,
            settled: tokio::sync::Mutex::new(None),
            owner_session: acp_session_id,
        }),
        Err(error) => {
            fail!(wire::framework_error(&error, method));
        }
    };
    let handle = WireHandle {
        id: record.background.execution_id.clone(),
        generation: WireU64::from_u64(state.handles.generation()),
        kind: HandleKind::Subagent,
    };
    // The advertised live-subagent bound is a hard limit: when the record
    // cannot be admitted, the attempt is cancelled so no dispatch runs
    // without a control identity (never a leaked background subagent).
    if let Err(error) = state
        .facade
        .register_subagent(handle.id.clone(), record.clone())
    {
        record.background.cancel();
        fail!(error);
    }
    responder.respond(SubagentDispatchResponse { subagent: handle })
}

pub(crate) async fn subagent_await(
    state: Arc<CoreProfileState>,
    request: SubagentAwaitRequest,
    responder: Responder<SubagentAwaitResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/subagent/await";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Subagents, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    let record = match record_of(&state, &request.subagent, method) {
        Ok(record) => record,
        Err(error) => fail!(error),
    };
    let timeout_secs = request
        .timeout
        .as_ref()
        .map(|duration| duration.seconds.to_u64().unwrap_or(0).saturating_add(1))
        .filter(|seconds| *seconds > 0)
        .unwrap_or(30);
    let mut settled = record.settled.lock().await;
    if settled.is_none() {
        let joined = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            record.background.join(),
        )
        .await;
        match joined {
            Ok(Ok(result)) => *settled = Some(Ok(result)),
            Ok(Err(error)) => *settled = Some(Err(error.to_string())),
            Err(_) => {
                return responder.respond(SubagentAwaitResponse {
                    settled: false,
                    result: None,
                });
            }
        }
    }
    match settled.as_ref() {
        Some(Ok(result)) => {
            let projection = serde_json::json!({
                "agent_name": result.agent_name,
                "output": result.output,
                "status": result.outcome.status.as_str(),
                "summary": result.outcome.summary,
                "iterations": result.iterations,
                "was_truncated": result.was_truncated,
                "outcome": serde_json::to_value(&result.outcome)
                    .unwrap_or(serde_json::Value::Null),
            });
            match WireValue::from_json(projection) {
                Ok(value) => responder.respond(SubagentAwaitResponse {
                    settled: true,
                    result: Some(value),
                }),
                Err(error) => {
                    fail!(subagent_error(
                        ExtensionErrorCode::SerializationViolation,
                        format!("subagent result is not a lossless wire value: {error}"),
                        method,
                    ));
                }
            }
        }
        Some(Err(message)) => {
            fail!(subagent_error(
                ExtensionErrorCode::FrameworkError,
                wire::bounded_framework_message(message),
                method,
            ));
        }
        None => responder.respond(SubagentAwaitResponse {
            settled: false,
            result: None,
        }),
    }
}

pub(crate) async fn subagent_control(
    state: Arc<CoreProfileState>,
    request: SubagentControlRequest,
    responder: Responder<SubagentControlResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/subagent/control";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Subagents, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    let record = match record_of(&state, &request.subagent, method) {
        Ok(record) => record,
        Err(error) => fail!(error),
    };
    let execution_id = record.background.execution_id.clone();
    let attempt = record.attempt;
    // Message and guidance carry text; interrupt and cancel address the
    // attempt itself and take no payload.
    let payload = match request.action {
        SubagentControlAction::Message | SubagentControlAction::Guidance => {
            match request
                .payload
                .clone()
                .filter(|text| !text.trim().is_empty())
            {
                Some(payload) => payload,
                None => fail!(subagent_error(
                    ExtensionErrorCode::InvalidValue,
                    "message and guidance control require a non-empty payload",
                    method,
                )),
            }
        }
        SubagentControlAction::Interrupt | SubagentControlAction::Cancel => String::new(),
    };
    let accepted = match request.action {
        SubagentControlAction::Message => record
            .executor
            .send_message_tracked(&execution_id, attempt, payload)
            .await
            .map(|_| true),
        SubagentControlAction::Guidance => record
            .executor
            .queue_guidance(&record.task_id, attempt.saturating_add(1), payload)
            .map(|_| true),
        SubagentControlAction::Interrupt => record
            .executor
            .interrupt_subagent(&execution_id, attempt)
            .await
            .map(|outcome| outcome.settled),
        SubagentControlAction::Cancel => {
            record.background.cancel();
            Ok(true)
        }
    };
    match accepted {
        Ok(accepted) => responder.respond(SubagentControlResponse { accepted }),
        Err(error) => {
            fail!(subagent_error(
                ExtensionErrorCode::FrameworkError,
                wire::bounded_framework_message(&error.to_string()),
                method,
            ));
        }
    }
}
