//! Typed `_echo_agent/*` core handlers.
//!
//! Every handler follows the same admission ladder:
//!
//! 1. Extended-mode gate — a connection that stayed Standard answers every
//!    `_echo_agent/*` request with the official method-not-found, so a plain
//!    Client can never half-enter the extension surface;
//! 2. capability gate — negotiated but unadvertised families answer with a
//!    typed capability mismatch;
//! 3. handle resolution — shape → kind → generation → issued/closed;
//! 4. framework work through the shared connection services and the
//!    existing framework authorities only.
//!
//! Long operations (`run/start`, `run/wait`) are spawned on the official
//! connection task instead of blocking the dispatch loop.

use agent_client_protocol::{Client, ConnectionTo, Responder};
use echo_agent::acp::{AcpSession, AcpSessionContext, RunStartSpec};
use echo_agent::agent::EventIdentity;
use echo_agent::llm::types::Message;
use echo_agent::runtime::{TurnMode, TurnRequest};
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::event::{ReplayRequest, ReplayResponse};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    AgentCloseRequest, AgentCloseResponse, AgentConfigWire, AgentCreateRequest,
    AgentCreateResponse, AgentDescribeRequest, AgentDescribeResponse, AgentSnapshotWire,
    EventAckNotification, RecoveredRunWire, RunCancelRequest, RunCancelResponse, RunGetRequest,
    RunGetResponse, RunInput, RunStartRequest, RunStartResponse, RunStatus, RunWaitRequest,
    RunWaitResponse, SessionCloseRequest, SessionCloseResponse, SessionCreateRequest,
    SessionCreateResponse, SessionLoadRequest, SessionLoadResponse,
};
use echo_sdk_protocol::scalar::WireU64;
use std::sync::Arc;
use std::time::Duration;

use super::events::StreamDelivery;
use super::handles::RunRecord;
use super::persistence::recovered_receipt_wire;
use super::state::CoreProfileState;
use super::wire;
use crate::factory::PreparedAgentDefinition;

/// Admission step 1: un-negotiated connections get the official
/// method-not-found, never a typed extension error.
async fn require_extended(
    state: &CoreProfileState,
    method: &str,
) -> std::result::Result<Arc<echo_agent::acp::AcpConnectionServices>, agent_client_protocol::Error>
{
    let services = state
        .services()
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    if !services.is_extended().await {
        return Err(agent_client_protocol::Error::method_not_found().data(method.to_string()));
    }
    if services.ensure_admission().is_err() {
        return Err(wire::into_jsonrpc_error(wire::sdk_error(
            ExtensionErrorCode::HostShuttingDown,
            "ACP Host is shutting down",
            Retryability::Never,
            method,
        )));
    }
    Ok(services)
}

/// Admission step 2: capability gate with typed mismatch errors.
fn require_capability(
    state: &CoreProfileState,
    capability: ExtensionCapability,
    method: &str,
) -> std::result::Result<(), agent_client_protocol::Error> {
    if state.advertisement.declares(capability) {
        Ok(())
    } else {
        Err(wire::into_jsonrpc_error(wire::sdk_error(
            ExtensionErrorCode::ExtensionCapabilityMismatch,
            format!("capability {} is not advertised", capability.as_str()),
            Retryability::Never,
            method,
        )))
    }
}

fn respond_error(
    responder: impl FnOnce(agent_client_protocol::Error) -> Result<(), agent_client_protocol::Error>,
    error: EchoSdkError,
) -> Result<(), agent_client_protocol::Error> {
    responder(wire::into_jsonrpc_error(error))
}

// ── Agent lifecycle ─────────────────────────────────────────────────────────

pub(crate) async fn agent_create(
    state: Arc<CoreProfileState>,
    request: AgentCreateRequest,
    responder: Responder<AgentCreateResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/agent/create";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::AgentLifecycle, method)?;
    if let Err(reason) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidConfig,
                reason,
                Retryability::Never,
                method,
            ),
        );
    }
    let fingerprint = super::handles::HandleRegistry::config_fingerprint(&request.config);
    let definition_state = state.clone();
    let outcome = match &request.idempotency_id {
        Some(idempotency_id) => {
            state
                .handles
                .agent_create_idempotent(idempotency_id, fingerprint.clone(), || {
                    build_definition(&definition_state, request.config.clone())
                })
        }
        None => build_definition(&state, request.config.clone()).and_then(
            |(definition, fingerprint)| {
                state
                    .handles
                    .register_agent(definition, fingerprint)
                    .map(|(agent, record)| (agent, record, true))
            },
        ),
    };
    match outcome {
        Ok((agent, record, _fresh)) => {
            if let Err(error) = state.register_definition(&record.definition) {
                let _ = state.handles.close_agent(&agent);
                return respond_error(
                    |error| responder.respond_with_error(error),
                    wire::sdk_error(
                        ExtensionErrorCode::PayloadTooLarge,
                        error,
                        Retryability::AfterDelay,
                        "_echo_agent/agent/create",
                    ),
                );
            }
            responder.respond(AgentCreateResponse { agent })
        }
        Err(error) => respond_error(|error| responder.respond_with_error(error), error),
    }
}

/// Build an immutable Agent definition from the typed config. The
/// `host_default` branch never transmits credentials; the explicit branch
/// resolves environment credentials at create time and fails closed on
/// unsupported configurations (enforced by the wire DTO grammar).
fn build_definition(
    state: &CoreProfileState,
    config: AgentConfigWire,
) -> std::result::Result<(Arc<PreparedAgentDefinition>, String), EchoSdkError> {
    let fingerprint = super::handles::HandleRegistry::config_fingerprint(&config);
    let definition = match config {
        AgentConfigWire::HostDefault => state.default_definition.clone(),
        AgentConfigWire::Explicit(explicit) => {
            let (framework, client) = wire::framework_from_wire(&explicit).map_err(|error| {
                wire::sdk_error(
                    ExtensionErrorCode::InvalidConfig,
                    error,
                    Retryability::Never,
                    "_echo_agent/agent/create",
                )
            })?;
            Arc::new(PreparedAgentDefinition::new(
                framework,
                client,
                Some(state.state_store.clone()),
                false,
            ))
        }
    };
    Ok((definition, fingerprint))
}

pub(crate) async fn agent_describe(
    state: Arc<CoreProfileState>,
    request: AgentDescribeRequest,
    responder: Responder<AgentDescribeResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/agent/describe";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::AgentLifecycle, method)?;
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.agent, HandleKind::Agent, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    match state.handles.agent(&request.agent) {
        Ok(record) => {
            let snapshot: AgentSnapshotWire = record.definition.snapshot();
            responder.respond(AgentDescribeResponse { snapshot })
        }
        Err(error) => respond_error(|error| responder.respond_with_error(error), error),
    }
}

pub(crate) async fn agent_close(
    state: Arc<CoreProfileState>,
    request: AgentCloseRequest,
    responder: Responder<AgentCloseResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/agent/close";
    let services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::AgentLifecycle, method)?;
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.agent, HandleKind::Agent, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let agent_record = match state.handles.agent(&request.agent) {
        Ok(record) => record,
        Err(error) if error.code == ExtensionErrorCode::ClosedHandle => {
            return responder.respond(AgentCloseResponse { released: false });
        }
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let sessions = state.handles.sessions_for_agent(&request.agent.id);
    let mut close_error = None;
    for (session_handle, acp_session_id) in &sessions {
        let session_id = agent_client_protocol::schema::v1::SessionId::new(acp_session_id.clone());
        match services.sessions().close_session(&session_id).await {
            Ok(_) => {
                let run_ids = state.handles.runs_for_session(acp_session_id);
                for run_id in run_ids {
                    let _ = services.remove_run(&run_id).await;
                    state.handles.remove_run(&run_id);
                    state.remove_delivery_for_run(&run_id);
                }
                let _ = state.handles.close_session(session_handle);
            }
            Err(error) => close_error = Some(error),
        }
    }
    let definition_id = agent_record.definition.id().to_string();
    if let Some(error) = close_error {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::framework_error(&error, method),
        );
    }
    let closed = match state.handles.close_agent(&request.agent) {
        Ok(released) => released,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    state.session_factory.remove_definition(&definition_id);
    responder.respond(AgentCloseResponse { released: closed })
}

// ── Session handles ─────────────────────────────────────────────────────────

pub(crate) async fn session_create(
    state: Arc<CoreProfileState>,
    request: SessionCreateRequest,
    responder: Responder<SessionCreateResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/session/create";
    let services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::SessionHandles, method)?;
    if let Err(reason) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason,
                Retryability::Never,
                method,
            ),
        );
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.agent, HandleKind::Agent, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let record = match state.handles.agent(&request.agent) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let working_dir = match wire::working_dir_from_wire(request.working_dir.as_ref()) {
        Ok(path) => path,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let session_id = match &request.session_id {
        Some(explicit) => explicit.clone(),
        None => format!("sess_{}", uuid::Uuid::new_v4()),
    };
    match create_session_on(
        &state,
        &services,
        record.definition.as_ref(),
        session_id,
        working_dir,
    )
    .await
    {
        Ok(acp_session_id) => {
            let cwd = services
                .sessions()
                .get(&agent_client_protocol::schema::v1::SessionId::new(
                    acp_session_id.clone(),
                ))
                .await
                .map(|session| session.context.cwd.clone());
            match state.handles.register_session_with_cwd(
                acp_session_id.clone(),
                request.agent.id.clone(),
                cwd,
            ) {
                Ok((session, session_record)) => {
                    let cwd_wire = match services
                        .sessions()
                        .get(&agent_client_protocol::schema::v1::SessionId::new(
                            acp_session_id.clone(),
                        ))
                        .await
                    {
                        Some(acp_session) => match wire::path_to_wire(&acp_session.context.cwd) {
                            Ok(path) => path,
                            Err(error) => {
                                let _ = state.handles.close_session(&session);
                                let _ = services
                                    .sessions()
                                    .close_session(
                                        &agent_client_protocol::schema::v1::SessionId::new(
                                            acp_session_id.clone(),
                                        ),
                                    )
                                    .await;
                                return respond_error(
                                    |error| responder.respond_with_error(error),
                                    wire::sdk_error(
                                        ExtensionErrorCode::InvalidValue,
                                        error,
                                        Retryability::Never,
                                        method,
                                    ),
                                );
                            }
                        },
                        None => {
                            let _ = state.handles.close_session(&session);
                            let _ = services
                                .sessions()
                                .close_session(&agent_client_protocol::schema::v1::SessionId::new(
                                    acp_session_id.clone(),
                                ))
                                .await;
                            return respond_error(
                                |error| responder.respond_with_error(error),
                                wire::sdk_error(
                                    ExtensionErrorCode::FrameworkError,
                                    "Session disappeared before persistence",
                                    Retryability::Never,
                                    method,
                                ),
                            );
                        }
                    };
                    if let Err(error) = state.persistence.record_session(
                        &acp_session_id,
                        &session_record.agent_handle_id,
                        &record.config_fingerprint,
                        cwd_wire,
                    ) {
                        let _ = state.handles.close_session(&session);
                        let _ = services
                            .sessions()
                            .close_session(&agent_client_protocol::schema::v1::SessionId::new(
                                acp_session_id.clone(),
                            ))
                            .await;
                        return respond_error(
                            |error| responder.respond_with_error(error),
                            wire::framework_error(&error, method),
                        );
                    }
                    responder.respond(SessionCreateResponse {
                        session,
                        acp_session_id,
                    })
                }
                Err(error) => {
                    let _ = services
                        .sessions()
                        .close_session(&agent_client_protocol::schema::v1::SessionId::new(
                            acp_session_id,
                        ))
                        .await;
                    respond_error(|error| responder.respond_with_error(error), error)
                }
            }
        }
        Err(error) => respond_error(|error| responder.respond_with_error(error), error),
    }
}

/// Create the ACP Session (independent Agent included) through the shared
/// registry — the exact path standard `session/new` uses.
async fn create_session_on(
    state: &CoreProfileState,
    services: &Arc<echo_agent::acp::AcpConnectionServices>,
    definition: &PreparedAgentDefinition,
    session_id: String,
    working_dir: Option<std::path::PathBuf>,
) -> std::result::Result<String, EchoSdkError> {
    let cwd = match working_dir {
        Some(path) if path.is_absolute() => path,
        Some(path) => {
            return Err(wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                format!("working_dir must be absolute: {}", path.display()),
                Retryability::Never,
                "_echo_agent/session/create",
            ));
        }
        None => std::env::current_dir().map_err(|error| {
            wire::sdk_error(
                ExtensionErrorCode::InvalidConfig,
                format!("no working directory available: {error}"),
                Retryability::Never,
                "_echo_agent/session/create",
            )
        })?,
    };
    let capabilities = services
        .sessions()
        .capabilities()
        .await
        .map_err(|error| wire::framework_error(&error, "_echo_agent/session/create"))?;
    // The definition id rides the Session context meta so the connection's
    // single session factory can build this Session from the exact Agent
    // definition the handle references (no second factory, no second map).
    let mut meta = agent_client_protocol::schema::v1::Meta::new();
    meta.insert(
        state.definition_meta_key().to_string(),
        serde_json::Value::String(definition.id().to_string()),
    );
    let context = AcpSessionContext {
        session_id: agent_client_protocol::schema::v1::SessionId::new(session_id.clone()),
        cwd,
        additional_directories: Vec::new(),
        mcp_servers: Vec::new(),
        client_capabilities: capabilities,
        meta: Some(meta),
    };
    services
        .insert_session(context)
        .await
        .map(|session_id| session_id.to_string())
        .map_err(|error| wire::framework_error(&error, "_echo_agent/session/create"))
}

pub(crate) async fn session_load(
    state: Arc<CoreProfileState>,
    request: SessionLoadRequest,
    responder: Responder<SessionLoadResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/session/load";
    let services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::SessionHandles, method)?;
    if let Err(reason) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason,
                Retryability::Never,
                method,
            ),
        );
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.agent, HandleKind::Agent, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let record = match state.handles.agent(&request.agent) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let requested_working_dir = match wire::working_dir_from_wire(request.working_dir.as_ref()) {
        Ok(path) => path,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let persisted_session = match state.persistence.load_session(&request.session_id) {
        Ok(session) => session,
        Err(error) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::framework_error(&error, method),
            );
        }
    };
    // Only sessions present in the durable session/run indexes can be loaded;
    // unknown identities fail closed instead of silently starting an empty Session.
    let recovered = state.recovered_runs_of_session(&request.session_id);
    if recovered.is_empty() && persisted_session.is_none() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "session {} is not present in the configured state root",
                    request.session_id
                ),
                Retryability::Never,
                method,
            ),
        );
    }
    if persisted_session.as_ref().is_some_and(|session| {
        !session.agent_config_fingerprint.is_empty()
            && session.agent_config_fingerprint != record.config_fingerprint
    }) || recovered.iter().any(|run| {
        !run.record.agent_config_fingerprint.is_empty()
            && run.record.agent_config_fingerprint != record.config_fingerprint
    }) {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidConfig,
                "session recovery Agent definition does not match the requested Agent handle",
                Retryability::Never,
                method,
            ),
        );
    }
    let persisted_cwd = persisted_session
        .as_ref()
        .map(|session| session.cwd.clone())
        .or_else(|| recovered.first().and_then(|run| run.record.cwd.clone()))
        .and_then(|cwd| wire::path_from_wire(&cwd).ok());
    if let (Some(requested), Some(persisted)) = (&requested_working_dir, &persisted_cwd)
        && requested != persisted
    {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidConfig,
                "session recovery working_dir does not match the persisted Session directory",
                Retryability::Never,
                method,
            ),
        );
    }
    let effective_working_dir = requested_working_dir.or(persisted_cwd);
    if effective_working_dir.is_none() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidConfig,
                "recovered Session has no persisted working_dir",
                Retryability::Never,
                method,
            ),
        );
    }
    let acp_session_id = match create_session_on(
        &state,
        &services,
        record.definition.as_ref(),
        request.session_id.clone(),
        effective_working_dir,
    )
    .await
    {
        Ok(acp_session_id) => acp_session_id,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let mut runs = Vec::with_capacity(recovered.len());
    let mut registered_run_ids = Vec::with_capacity(recovered.len());
    let mut recovered_sequence = 0u64;
    for run in &recovered {
        recovered_sequence = recovered_sequence.max(run.last_sequence);
        match state.handles.register_recovered_run(
            run.record.run_id.clone(),
            run.record.stream_id.clone(),
            run.record.session_id.clone(),
            run.status,
            run.last_sequence,
            run.terminal.clone(),
            recovered_receipt_wire(run),
        ) {
            Ok((run_handle, stream_handle, _record)) => {
                registered_run_ids.push(run.record.run_id.clone());
                runs.push(RecoveredRunWire {
                    run: run_handle,
                    stream: stream_handle,
                    status: run.status,
                    last_sequence: WireU64::from_u64(run.last_sequence),
                    terminal: run.terminal.clone(),
                });
            }
            Err(error) => {
                for run_id in registered_run_ids {
                    state.handles.remove_run(&run_id);
                }
                let _ = services
                    .sessions()
                    .close_session(&agent_client_protocol::schema::v1::SessionId::new(
                        acp_session_id.clone(),
                    ))
                    .await;
                return respond_error(|error| responder.respond_with_error(error), error);
            }
        }
    }
    match state.handles.register_session_with_cwd(
        acp_session_id.clone(),
        request.agent.id.clone(),
        services
            .sessions()
            .get(&agent_client_protocol::schema::v1::SessionId::new(
                acp_session_id.clone(),
            ))
            .await
            .map(|session| session.context.cwd.clone()),
    ) {
        Ok((session, session_record)) => {
            let cwd_wire = services
                .sessions()
                .get(&agent_client_protocol::schema::v1::SessionId::new(
                    acp_session_id.clone(),
                ))
                .await
                .ok_or_else(|| {
                    agent_client_protocol::Error::internal_error().data("Session disappeared")
                })
                .and_then(|acp_session| {
                    wire::path_to_wire(&acp_session.context.cwd)
                        .map_err(|error| agent_client_protocol::Error::internal_error().data(error))
                })?;
            state
                .persistence
                .record_session(
                    &acp_session_id,
                    &session_record.agent_handle_id,
                    &record.config_fingerprint,
                    cwd_wire,
                )
                .map_err(|error| {
                    agent_client_protocol::Error::internal_error().data(error.to_string())
                })?;
            responder.respond(SessionLoadResponse {
                session,
                acp_session_id,
                recovered_sequence: (recovered_sequence > 0)
                    .then(|| WireU64::from_u64(recovered_sequence)),
                runs,
            })
        }
        Err(error) => {
            for run_id in registered_run_ids {
                state.handles.remove_run(&run_id);
            }
            let _ = services
                .sessions()
                .close_session(&agent_client_protocol::schema::v1::SessionId::new(
                    acp_session_id,
                ))
                .await;
            respond_error(|error| responder.respond_with_error(error), error)
        }
    }
}

pub(crate) async fn session_close(
    state: Arc<CoreProfileState>,
    request: SessionCloseRequest,
    responder: Responder<SessionCloseResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/session/close";
    let services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::SessionHandles, method)?;
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.session, HandleKind::Session, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let acp_session_id = match state.handles.session(&request.session) {
        Ok(record) => record.acp_session_id.clone(),
        Err(error) if error.code == ExtensionErrorCode::ClosedHandle => {
            return responder.respond(SessionCloseResponse { released: false });
        }
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    // Close the underlying ACP Session too: cancel its active run, wait for
    // the framework receipt, then close the Session Agent.
    let close = services
        .sessions()
        .close_session(&agent_client_protocol::schema::v1::SessionId::new(
            acp_session_id.clone(),
        ))
        .await;
    if let Err(error) = close {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::framework_error(&error, method),
        );
    }
    let released = match state.handles.close_session(&request.session) {
        Ok((released, _)) => released,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let run_ids = state.handles.runs_for_session(&acp_session_id);
    for run_id in run_ids {
        let _ = services.remove_run(&run_id).await;
        state.handles.remove_run(&run_id);
        state.remove_delivery_for_run(&run_id);
    }
    responder.respond(SessionCloseResponse { released })
}

// ── Runs ────────────────────────────────────────────────────────────────────

pub(crate) async fn run_start(
    state: Arc<CoreProfileState>,
    request: RunStartRequest,
    responder: Responder<RunStartResponse>,
    connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/run/start";
    let services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Runs, method)?;
    if let Err(reason) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason,
                Retryability::Never,
                method,
            ),
        );
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.session, HandleKind::Session, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let (session, acp_session_id) = match resolve_session(&state, &request.session).await {
        Ok(resolved) => resolved,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let agent_handle_id = match state.handles.session(&request.session) {
        Ok(record) => record.agent_handle_id.clone(),
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let agent_fingerprint = match state.handles.agent_fingerprint(&wire::handle(
        agent_handle_id.clone(),
        HandleKind::Agent,
        state.handles.generation(),
    )) {
        Ok(fingerprint) => fingerprint,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let active = match session.begin_turn() {
        Ok(active) => active,
        Err(error) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::sdk_error(
                    ExtensionErrorCode::InvalidRequest,
                    error.to_string(),
                    Retryability::AfterDelay,
                    method,
                ),
            );
        }
    };
    let run_id = active.turn.id().to_string();
    let identity = match EventIdentity::for_chat(
        Some(acp_session_id.clone()),
        run_id.clone(),
        format!("{run_id}:message"),
        Some(run_id.clone()),
    ) {
        Ok(identity) => identity,
        Err(error) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::framework_error(&error, method),
            );
        }
    };
    // The stream handle id is exactly the envelope stream identity, so live
    // notifications, replay cursors and journal records all address one id.
    let stream_id = identity.stream_id.as_str().to_string();
    let turn = match &request.input {
        RunInput::Chat { text } => TurnRequest::new(identity, text.clone()),
        RunInput::Execute { task } => {
            TurnRequest::new(identity, task.clone()).mode(TurnMode::Execute)
        }
    }
    .cancel(active.turn.cancellation());
    let journal: Option<
        std::sync::Arc<
            dyn echo_agent::state::journal::EventJournal<echo_agent::agent::EventEnvelope>,
        >,
    > = match state.journal(&run_id) {
        Ok(journal) => {
            let coerced: std::sync::Arc<
                dyn echo_agent::state::journal::EventJournal<echo_agent::agent::EventEnvelope>,
            > = journal;
            Some(coerced)
        }
        Err(error) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::framework_error(&error, method),
            );
        }
    };
    let stream_handle = wire::handle(
        stream_id.clone(),
        HandleKind::Stream,
        state.handles.generation(),
    );
    let delivery = StreamDelivery::new(
        state.clone(),
        &run_id,
        connection.clone(),
        stream_handle.clone(),
        state.limits.max_outstanding_live_events,
        state
            .limits
            .max_event_bytes
            .saturating_mul(state.limits.max_outstanding_live_events),
        state.limits.max_event_bytes,
    );
    let cwd = match wire::path_to_wire(&session.context.cwd) {
        Ok(cwd) => cwd,
        Err(error) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::sdk_error(
                    ExtensionErrorCode::InvalidValue,
                    error,
                    Retryability::Never,
                    method,
                ),
            );
        }
    };
    if let Err(error) = state.persistence.record_run_started(
        &run_id,
        &stream_id,
        &acp_session_id,
        &agent_handle_id,
        &agent_fingerprint,
        Some(cwd),
        input_kind(&request.input),
    ) {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::framework_error(&error, method),
        );
    }
    let spec = RunStartSpec {
        session: session.clone(),
        active,
        run_id: run_id.clone(),
        stream_id: stream_id.clone(),
        turn,
        projector: Some(services.projector(
            agent_client_protocol::schema::v1::SessionId::new(acp_session_id.clone()),
            connection.clone(),
        )),
        journal,
        observers: vec![delivery.clone()],
    };
    let (entry, task) = match services.prepare_run(spec).await {
        Ok(pair) => pair,
        Err(error) => {
            state.remove_delivery_for_run(&run_id);
            state.remove_journal(&run_id);
            if let Err(cleanup) = state.persistence.remove_run(&run_id) {
                tracing::warn!(
                    "failed to remove run {} after prepare failure: {cleanup}",
                    run_id
                );
            }
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::framework_error(&error, method),
            );
        }
    };
    let run_handle = match state.handles.register_live_run(
        run_id.clone(),
        entry.clone(),
        Some(&request.session.id),
    ) {
        Ok((run, _record)) => run,
        Err(error) => {
            let _ = services.remove_run(&run_id).await;
            state.remove_delivery_for_run(&run_id);
            state.remove_journal(&run_id);
            if let Err(cleanup) = state.persistence.remove_run(&run_id) {
                tracing::warn!(
                    "failed to remove run {} after handle failure: {cleanup}",
                    run_id
                );
            }
            drop(task);
            return respond_error(|error| responder.respond_with_error(error), error);
        }
    };
    let stream_record = state
        .handles
        .register_live_stream(stream_id.clone(), run_id.clone());
    if let Err(error) = stream_record {
        let _ = services.remove_run(&run_id).await;
        state.handles.remove_run(&run_id);
        state.remove_delivery_for_run(&run_id);
        state.remove_journal(&run_id);
        if let Err(cleanup) = state.persistence.remove_run(&run_id) {
            tracing::warn!(
                "failed to remove run {} after stream registration failure: {cleanup}",
                run_id
            );
        }
        drop(task);
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    state.register_delivery(delivery);
    state.begin_settlement();
    if let Err(error) = connection.spawn(task) {
        state.finish_settlement(Err(format!("failed to spawn run driver: {error}")));
        let _ = services.remove_run(&run_id).await;
        state.handles.remove_run(&run_id);
        state.remove_delivery_for_run(&run_id);
        state.remove_journal(&run_id);
        if let Err(cleanup) = state.persistence.remove_run(&run_id) {
            tracing::warn!(
                "failed to remove run {} after spawn failure: {cleanup}",
                run_id
            );
        }
        return responder.respond_with_error(error);
    }
    let settle_state = state.clone();
    let settle_entry = entry.clone();
    tokio::spawn(async move {
        let receipt = settle_entry.wait_receipt().await;
        let result = wire::terminal_of(&receipt)
            .and_then(|terminal| {
                wire::receipt_wire(&receipt).map(|receipt_wire| (terminal, receipt_wire))
            })
            .and_then(|(terminal, receipt_wire)| {
                settle_state
                    .persistence
                    .record_run_settled(
                        &settle_entry.run_id,
                        terminal,
                        receipt_wire,
                        settle_entry.ledger.last_sequence(),
                    )
                    .map_err(|error| error.to_string())
            });
        settle_state.cleanup_settled_run(&settle_entry.run_id);
        settle_state.finish_settlement(result);
    });
    responder.respond(RunStartResponse {
        run: run_handle,
        stream: stream_handle,
        first_event: None,
    })
}

fn input_kind(input: &RunInput) -> &'static str {
    match input {
        RunInput::Chat { .. } => "chat",
        RunInput::Execute { .. } => "execute",
    }
}

fn persist_receipt_now(
    state: &CoreProfileState,
    entry: &echo_agent::acp::RunEntry,
    receipt: &echo_agent::runtime::TurnReceipt,
) -> std::result::Result<(), EchoSdkError> {
    let terminal = wire::terminal_of(receipt).map_err(|error| {
        wire::sdk_error(
            ExtensionErrorCode::SerializationViolation,
            error,
            Retryability::Never,
            "_echo_agent/run/wait",
        )
    })?;
    let receipt_wire = wire::receipt_wire(receipt).map_err(|error| {
        wire::sdk_error(
            ExtensionErrorCode::SerializationViolation,
            error,
            Retryability::Never,
            "_echo_agent/run/wait",
        )
    })?;
    state
        .persistence
        .record_run_settled(
            &entry.run_id,
            terminal,
            receipt_wire,
            entry.ledger.last_sequence(),
        )
        .map_err(|error| wire::framework_error(&error, "_echo_agent/run/wait"))
        .map(|_| ())
}

/// Resolve a session handle to the shared ACP Session.
async fn resolve_session(
    state: &CoreProfileState,
    handle: &WireHandle,
) -> std::result::Result<(Arc<AcpSession>, String), EchoSdkError> {
    let record = state.handles.session(handle)?;
    let services = state.services().map_err(|error| {
        wire::sdk_error(
            ExtensionErrorCode::HostShuttingDown,
            error.to_string(),
            Retryability::Never,
            "_echo_agent/session",
        )
    })?;
    let session_id =
        agent_client_protocol::schema::v1::SessionId::new(record.acp_session_id.clone());
    let session = services.sessions().get(&session_id).await.ok_or_else(|| {
        wire::handle_error(
            ExtensionErrorCode::ClosedHandle,
            "the underlying ACP Session is gone",
            "_echo_agent/run/start",
            handle,
        )
    })?;
    Ok((session, record.acp_session_id.clone()))
}

pub(crate) async fn run_get(
    state: Arc<CoreProfileState>,
    request: RunGetRequest,
    responder: Responder<RunGetResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/run/get";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Runs, method)?;
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.run, HandleKind::Run, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let record = match state.handles.run(&request.run) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let generation = state.handles.generation();
    let response = match record.as_ref() {
        RunRecord::Live { entry } => {
            let receipt = entry.receipt();
            let (terminal, receipt_wire) = match receipt.as_deref() {
                Some(receipt) => match (wire::terminal_of(receipt), wire::receipt_wire(receipt)) {
                    (Ok(terminal), Ok(receipt_wire)) => (Some(terminal), Some(receipt_wire)),
                    (Err(error), _) | (_, Err(error)) => {
                        return respond_error(
                            |error| responder.respond_with_error(error),
                            wire::sdk_error(
                                ExtensionErrorCode::SerializationViolation,
                                error,
                                Retryability::Never,
                                method,
                            ),
                        );
                    }
                },
                None => (None, None),
            };
            RunGetResponse {
                status: wire::status_of(receipt.as_deref()),
                last_sequence: WireU64::from_u64(entry.ledger.last_sequence()),
                stream: Some(wire::handle(
                    entry.stream_id.clone(),
                    HandleKind::Stream,
                    generation,
                )),
                terminal,
                receipt: receipt_wire,
            }
        }
        RunRecord::Recovered(recovered) => RunGetResponse {
            status: recovered.status,
            last_sequence: WireU64::from_u64(recovered.last_sequence),
            stream: Some(wire::handle(
                recovered.stream_id.clone(),
                HandleKind::Stream,
                generation,
            )),
            terminal: recovered.terminal.clone(),
            receipt: recovered.receipt.clone(),
        },
    };
    responder.respond(response)
}

pub(crate) async fn run_wait(
    state: Arc<CoreProfileState>,
    request: RunWaitRequest,
    responder: Responder<RunWaitResponse>,
    connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/run/wait";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Runs, method)?;
    if let Err(reason) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason,
                Retryability::Never,
                method,
            ),
        );
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.run, HandleKind::Run, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let record = match state.handles.run(&request.run) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    match record.as_ref() {
        RunRecord::Live { entry } => {
            let entry = entry.clone();
            if let Some(receipt) = entry.receipt() {
                if let Err(error) = persist_receipt_now(&state, &entry, &receipt) {
                    return respond_error(|error| responder.respond_with_error(error), error);
                }
                let terminal = match wire::terminal_of(&receipt) {
                    Ok(terminal) => terminal,
                    Err(error) => {
                        return respond_error(
                            |error| responder.respond_with_error(error),
                            wire::sdk_error(
                                ExtensionErrorCode::SerializationViolation,
                                error,
                                Retryability::Never,
                                method,
                            ),
                        );
                    }
                };
                let receipt_wire = match wire::receipt_wire(&receipt) {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        return respond_error(
                            |error| responder.respond_with_error(error),
                            wire::sdk_error(
                                ExtensionErrorCode::SerializationViolation,
                                error,
                                Retryability::Never,
                                method,
                            ),
                        );
                    }
                };
                return responder.respond(RunWaitResponse {
                    settled: true,
                    terminal: Some(terminal),
                    receipt: Some(receipt_wire),
                });
            }
            let timeout = request.timeout.as_ref().map(|duration| {
                let nanos = duration
                    .seconds
                    .to_u64()
                    .unwrap_or(u64::MAX)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(u64::from(duration.nanos));
                Duration::from_nanos(nanos)
            });
            let wait_state = state.clone();
            connection.spawn(async move {
                let waited = super::events::wait_with_timeout(entry.wait_receipt(), timeout).await;
                match waited {
                    Some(receipt) => {
                        match persist_receipt_now(&wait_state, &entry, &receipt)
                            .and_then(|_| {
                                wire::terminal_of(&receipt).map_err(|error| {
                                    wire::sdk_error(
                                        ExtensionErrorCode::SerializationViolation,
                                        error,
                                        Retryability::Never,
                                        method,
                                    )
                                })
                            })
                            .and_then(|terminal| {
                                wire::receipt_wire(&receipt)
                                    .map(|receipt_wire| (terminal, receipt_wire))
                                    .map_err(|error| {
                                        wire::sdk_error(
                                            ExtensionErrorCode::SerializationViolation,
                                            error,
                                            Retryability::Never,
                                            method,
                                        )
                                    })
                            }) {
                            Ok((terminal, receipt_wire)) => responder.respond(RunWaitResponse {
                                settled: true,
                                terminal: Some(terminal),
                                receipt: Some(receipt_wire),
                            }),
                            Err(error) => {
                                responder.respond_with_error(wire::into_jsonrpc_error(error))
                            }
                        }
                    }
                    None => responder.respond(RunWaitResponse {
                        settled: false,
                        terminal: None,
                        receipt: None,
                    }),
                }
            })
        }
        RunRecord::Recovered(recovered) => {
            let (status, terminal, receipt) = (
                recovered.status,
                recovered.terminal.clone(),
                recovered.receipt.clone(),
            );
            if status == RunStatus::Interrupted {
                // A crashed predecessor can never be waited into success.
                return respond_error(
                    |error| responder.respond_with_error(error),
                    wire::handle_error(
                        ExtensionErrorCode::HostExited,
                        "the run was interrupted when the previous Host process exited",
                        method,
                        &request.run,
                    ),
                );
            }
            responder.respond(RunWaitResponse {
                settled: true,
                terminal,
                receipt,
            })
        }
    }
}

pub(crate) async fn run_cancel(
    state: Arc<CoreProfileState>,
    request: RunCancelRequest,
    responder: Responder<RunCancelResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/run/cancel";
    let services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Runs, method)?;
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.run, HandleKind::Run, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let record = match state.handles.run(&request.run) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    match record.as_ref() {
        RunRecord::Live { entry } => {
            let initiated = entry.cancel();
            let status = wire::status_of(entry.receipt().as_deref());
            responder.respond(RunCancelResponse {
                cancellation_initiated: initiated,
                status,
            })
        }
        RunRecord::Recovered(recovered) => {
            let _ = services;
            responder.respond(RunCancelResponse {
                cancellation_initiated: false,
                status: recovered.status,
            })
        }
    }
}

pub(crate) async fn run_steer(
    state: Arc<CoreProfileState>,
    request: echo_sdk_protocol::methods::RunSteerRequest,
    responder: Responder<echo_sdk_protocol::methods::RunSteerResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/run/steer";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::Runs, method)?;
    if let Err(reason) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason,
                Retryability::Never,
                method,
            ),
        );
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.run, HandleKind::Run, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let record = match state.handles.run(&request.run) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let entry = match record.as_ref() {
        RunRecord::Live { entry } => entry.clone(),
        RunRecord::Recovered(recovered) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::handle_error(
                    ExtensionErrorCode::HostExited,
                    format!("run status {:?} cannot be steered", recovered.status),
                    method,
                    &request.run,
                ),
            );
        }
    };
    let services = state
        .services()
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
    let session = services
        .sessions()
        .get(&entry.session_id)
        .await
        .ok_or_else(|| {
            agent_client_protocol::Error::internal_error().data("run session missing")
        })?;
    let receipt = session
        .agent
        .steer_input_tracked(Some(&entry.run_id), Message::user(request.text));
    match receipt {
        Ok(receipt) => responder.respond(echo_sdk_protocol::methods::RunSteerResponse {
            accepted: true,
            steer_id: Some(receipt.steer_id().to_string()),
        }),
        Err(error) => responder
            .respond(echo_sdk_protocol::methods::RunSteerResponse {
                accepted: false,
                steer_id: None,
            })
            .map(|_| {
                tracing::debug!("steer rejected: {error}");
            }),
    }
}

// ── Replay and ACK ──────────────────────────────────────────────────────────

pub(crate) async fn run_replay(
    state: Arc<CoreProfileState>,
    request: ReplayRequest,
    responder: Responder<ReplayResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/run/replay";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::EventReplay, method)?;
    if let Err(error) = request.validate() {
        return respond_error(
            |error| responder.respond_with_error(error),
            wire::sdk_error(
                ExtensionErrorCode::InvalidValue,
                format!("invalid replay request: {error}"),
                Retryability::Never,
                method,
            ),
        );
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.stream, HandleKind::Stream, method)
    {
        return respond_error(|error| responder.respond_with_error(error), error);
    }
    let stream_record = match state.handles.stream(&request.stream) {
        Ok(record) => record,
        Err(error) => return respond_error(|error| responder.respond_with_error(error), error),
    };
    let run_id = stream_record.run_handle_id.clone();
    let journal = match state.journal(&run_id) {
        Ok(journal) => journal,
        Err(error) => {
            return respond_error(
                |error| responder.respond_with_error(error),
                wire::framework_error(&error, method),
            );
        }
    };
    let after = request.after_sequence.to_u64().unwrap_or_default();
    let requested_limit = request
        .max_events
        .as_ref()
        .and_then(|limit| limit.to_u64())
        .unwrap_or(u64::MAX);
    let limit = usize::try_from(requested_limit.min(state.limits.max_replay_events as u64))
        .unwrap_or(usize::MAX);
    match super::events::bounded_replay(
        state.limits.max_replay_bytes,
        journal.as_ref(),
        &request.stream,
        after,
        limit,
    ) {
        Ok(response) => responder.respond(response),
        Err(error) => respond_error(|error| responder.respond_with_error(error), error),
    }
}

pub(crate) async fn event_ack(
    state: Arc<CoreProfileState>,
    notification: EventAckNotification,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/event/ack";
    // Notifications never produce responses; malformed or unknown ACKs are
    // ignored (ACP notification rules) with a bounded stderr diagnostic.
    if let Err(error) = notification.ack.validate() {
        tracing::debug!("ignoring invalid {method}: {error}");
        return Ok(());
    }
    if let Err(error) = state.handles.check_shape_and_generation(
        &notification.ack.stream,
        HandleKind::Stream,
        method,
    ) {
        tracing::debug!("ignoring unmatched {method}: {}", error.message);
        return Ok(());
    }
    let Some(delivery) = state.delivery(&notification.ack.stream.id) else {
        tracing::debug!(
            "ignoring {method} for unknown stream {}",
            notification.ack.stream.id
        );
        return Ok(());
    };
    if let Err(error) = delivery.acknowledge(&notification.ack).await {
        tracing::warn!("ack resume failed: {error}");
    }
    Ok(())
}
