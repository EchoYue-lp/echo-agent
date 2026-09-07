//! Task graph facade handlers (plan 07, todo 3).
//!
//! `_echo_agent/task/create|update|list` bind to the Session Agent's own
//! [`echo_agent::tasks::TaskRevisionService`] — the exact instance the
//! in-conversation `task_*` tools operate through (captured before the
//! concrete agent is boxed, see
//! [`crate::factory::SessionAuthorityServices`]). The Host never rebuilds
//! the graph, recomputes revisions or owns task state: it translates the
//! typed RPC onto the framework authority and maps results back.
//!
//! TaskRun identity is the session-scoped graph id: the handle id equals
//! the ACP session id, which is the `conversation_id` the framework scope
//! policy resolves for this Session. PlanTask handles address framework
//! `TaskId`s.

use agent_client_protocol::{Client, ConnectionTo, Responder};
use echo_agent::tasks::{TaskRevisionService, TaskStatus as FrameworkTaskStatus};
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    TaskCreateRequest, TaskCreateResponse, TaskListRequest, TaskListResponse, TaskSummary,
    TaskUpdateRequest, TaskUpdateResponse, WireTaskStatus,
};
use echo_sdk_protocol::scalar::WireU64;
use std::sync::Arc;

use super::super::handler::{require_capability, require_extended};
use super::super::state::CoreProfileState;
use super::super::wire;
use crate::factory::SessionAuthorityServices;

fn task_error(error: echo_agent::tasks::TaskRevisionError, method: &str) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&error.to_string()),
        Retryability::Never,
        method,
    )
}

/// Resolve the session authority services for one TaskRun handle. The
/// handle id is the ACP session id (the framework graph scope); a session
/// that is no longer open fails through the handle ladder.
pub(crate) fn session_services(
    state: &CoreProfileState,
    task_run: &WireHandle,
    method: &str,
) -> Result<Arc<SessionAuthorityServices>, EchoSdkError> {
    state
        .handles
        .check_shape_and_generation(task_run, HandleKind::TaskRun, method)?;
    let session_handle = state
        .handles
        .session_handle_for_acp(&task_run.id)
        .ok_or_else(|| {
            wire::handle_error(
                ExtensionErrorCode::InvalidValue,
                "task run does not address an open session scope",
                method,
                task_run,
            )
        })?;
    let acp_session_id = state
        .handles
        .session(&session_handle)
        .map_err(|error| wire::handle_error(error.code, error.message, method, task_run))?
        .acp_session_id
        .clone();
    state
        .session_factory
        .session_services(&acp_session_id)
        .ok_or_else(|| {
            wire::handle_error(
                ExtensionErrorCode::ClosedHandle,
                "session authority services are no longer live",
                method,
                task_run,
            )
        })
}

/// ToolContext the framework scope policy resolves for this session's
/// graph: `conversation_id` is the session identity, exactly what the
/// in-conversation tool calls carry.
fn scope_context(acp_session_id: &str) -> echo_agent::tools::ToolContext {
    echo_agent::tools::ToolContext {
        conversation_id: Some(acp_session_id.to_string()),
        ..echo_agent::tools::ToolContext::default()
    }
}

fn plan_task_handle(task_run: &WireHandle, task_id: &str) -> WireHandle {
    WireHandle {
        id: task_id.to_string(),
        generation: task_run.generation.clone(),
        kind: HandleKind::PlanTask,
    }
}

fn wire_status(status: &FrameworkTaskStatus) -> WireTaskStatus {
    match status {
        FrameworkTaskStatus::Pending => WireTaskStatus::Pending,
        FrameworkTaskStatus::Running => WireTaskStatus::Running,
        FrameworkTaskStatus::Blocked(reason) => WireTaskStatus::Blocked {
            reason: reason.clone(),
        },
        FrameworkTaskStatus::Completed => WireTaskStatus::Completed,
        FrameworkTaskStatus::Failed(error) => WireTaskStatus::Failed {
            error: error.clone(),
        },
        FrameworkTaskStatus::Skipped => WireTaskStatus::Skipped,
        FrameworkTaskStatus::Cancelled => WireTaskStatus::Cancelled,
        FrameworkTaskStatus::TimedOut { error } => WireTaskStatus::TimedOut {
            error: error.clone(),
        },
        FrameworkTaskStatus::Retrying {
            attempt,
            last_error,
        } => WireTaskStatus::Retrying {
            attempt: *attempt,
            last_error: last_error.clone(),
        },
        FrameworkTaskStatus::Paused(reason) => WireTaskStatus::Paused {
            reason: reason.clone(),
        },
    }
}

/// The spec `WireValue` is the framework `task_create` payload; parsing
/// reuses the tool grammar so RPC and in-conversation creation share one
/// input contract.
fn create_input_from_wire(
    service: &TaskRevisionService,
    spec: &echo_sdk_protocol::scalar::WireValue,
    method: &str,
) -> Result<echo_agent::tasks::TaskCreateInput, EchoSdkError> {
    let value = spec.clone().into_json().map_err(|error| {
        wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            format!("task spec is not a lossless wire value: {error}"),
            Retryability::Never,
            method,
        )
    })?;
    let parameters = match value {
        serde_json::Value::Object(object) => object.into_iter().collect(),
        _ => {
            return Err(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                "task spec must be an object",
                Retryability::Never,
                method,
            ));
        }
    };
    echo_agent::tasks::parse_task_create_input(&parameters, &service.task_input_schema_extensions())
        .map_err(|message| {
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                wire::bounded_framework_message(&message),
                Retryability::Never,
                method,
            )
        })
}

pub(crate) async fn task_create(
    state: Arc<CoreProfileState>,
    request: TaskCreateRequest,
    responder: Responder<TaskCreateResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/task/create";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::TaskGraph, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    if let Err(error) = request.validate() {
        fail!(wire::sdk_error(
            ExtensionErrorCode::InvalidValue,
            error,
            Retryability::Never,
            method,
        ));
    }
    let authorities = match session_services(&state, &request.task_run, method) {
        Ok(authorities) => authorities,
        Err(error) => fail!(error),
    };
    let service = authorities.task_revision_service.clone();
    let input = match create_input_from_wire(&service, &request.spec, method) {
        Ok(input) => input,
        Err(error) => fail!(error),
    };
    let context = scope_context(&request.task_run.id);
    let outcome = match service.create_from_tool(input, &context).await {
        Ok(outcome) => outcome,
        Err(error) => fail!(task_error(error, method)),
    };
    responder.respond(TaskCreateResponse {
        tasks: outcome
            .graph
            .snapshot
            .tasks
            .iter()
            .map(|task| plan_task_handle(&request.task_run, task.execution.task_id.as_str()))
            .collect(),
        revision: WireU64::from_u64(outcome.graph.snapshot.revision),
    })
}

pub(crate) async fn task_update(
    state: Arc<CoreProfileState>,
    request: TaskUpdateRequest,
    responder: Responder<TaskUpdateResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/task/update";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::TaskGraph, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    let authorities = match session_services(&state, &request.task_run, method) {
        Ok(authorities) => authorities,
        Err(error) => fail!(error),
    };
    let service = authorities.task_revision_service.clone();
    let value = match request.patch.clone().into_json() {
        Ok(value) => value,
        Err(error) => {
            fail!(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                format!("task patch is not a lossless wire value: {error}"),
                Retryability::Never,
                method,
            ));
        }
    };
    let parameters = match &value {
        serde_json::Value::Object(object) => object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<echo_agent::tools::ToolParameters>(),
        _ => {
            fail!(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                "task patch must be an object",
                Retryability::Never,
                method,
            ));
        }
    };
    let input = match echo_agent::tasks::parse_task_update_input(
        &parameters,
        &service.task_input_schema_extensions(),
        service.allow_manual_progress_updates(),
    ) {
        Ok(input) => input,
        Err(message) => {
            fail!(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                wire::bounded_framework_message(&message),
                Retryability::Never,
                method,
            ));
        }
    };
    let context = scope_context(&request.task_run.id);
    let graph = match service.update_from_tool(input, &context).await {
        Ok(graph) => graph,
        Err(error) => fail!(task_error(error, method)),
    };
    responder.respond(TaskUpdateResponse {
        revision: WireU64::from_u64(graph.snapshot.revision),
        updated: graph
            .snapshot
            .tasks
            .iter()
            .map(|task| plan_task_handle(&request.task_run, task.execution.task_id.as_str()))
            .collect(),
    })
}

pub(crate) async fn task_list(
    state: Arc<CoreProfileState>,
    request: TaskListRequest,
    responder: Responder<TaskListResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/task/list";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::TaskGraph, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    let authorities = match session_services(&state, &request.task_run, method) {
        Ok(authorities) => authorities,
        Err(error) => fail!(error),
    };
    let service = authorities.task_revision_service.clone();
    let graph = match service.load(&request.task_run.id).await {
        Ok(Some(graph)) => graph,
        Ok(None) => {
            fail!(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                "task run has no committed graph yet",
                Retryability::Never,
                method,
            ));
        }
        Err(error) => fail!(task_error(error, method)),
    };
    responder.respond(TaskListResponse {
        tasks: graph
            .snapshot
            .tasks
            .iter()
            .map(|task| TaskSummary {
                task: plan_task_handle(&request.task_run, task.execution.task_id.as_str()),
                status: wire_status(&task.execution.status),
                revision: WireU64::from_u64(graph.snapshot.revision),
            })
            .collect(),
    })
}
