use super::prompt::{MappedPrompt, map_prompt};
use super::runtime::{
    AcpConnectionProfile, AcpConnectionServices, ConnectionMode, RunObserverContext, RunStartSpec,
    StandardBridgeOutcome,
};
use super::session::{
    AcpSession, AcpSessionFactory, ActiveTurnLease, SessionRegistry, validate_session_paths,
};
use crate::agent::EventIdentity;
use crate::runtime::{TurnMode, TurnOutcome, TurnRequest};
use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{Agent as AcpRole, Client, ConnectTo, ConnectionTo, Error, Responder};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_MAX_SESSIONS: usize = 128;
const DEFAULT_MAX_PROMPT_CHARS: usize = 1_000_000;
const DEFAULT_MAX_UPDATE_CHARS: usize = 1_000_000;
const DEFAULT_MAX_UPDATES_PER_TURN: usize = 10_000;
const DEFAULT_MAX_TOTAL_UPDATE_CHARS: usize = 8_000_000;
const DEFAULT_MAX_EXTENSION_CONCURRENCY: usize = 8;
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_CHARS: usize = 512;

/// Resource and implementation metadata for one ACP Agent adapter.
#[derive(Debug, Clone)]
pub struct AcpAdapterConfig {
    /// Programmatic implementation name reported during `initialize`.
    pub name: String,
    /// Human-readable implementation title reported during `initialize`.
    pub title: String,
    /// Implementation version reported during `initialize`.
    pub version: String,
    /// Maximum live Sessions owned by one ACP connection.
    pub max_sessions: usize,
    /// Maximum Unicode scalar count accepted in one mapped Prompt.
    pub max_prompt_chars: usize,
    /// Maximum Unicode scalar count emitted in one projected update payload.
    pub max_update_chars: usize,
    /// Maximum standard `session/update` notifications emitted by one Turn.
    pub max_updates_per_turn: usize,
    /// Maximum cumulative serialized update characters emitted by one Turn.
    pub max_total_update_chars: usize,
    /// Maximum concurrently in-flight extension (reverse-callback)
    /// invocations per connection (design §12.3).
    pub max_extension_concurrency: usize,
    /// Total time allowed for connection-level Agent shutdown.
    pub shutdown_timeout: Duration,
}

impl Default for AcpAdapterConfig {
    fn default() -> Self {
        Self {
            name: "echo-agent".to_string(),
            title: "echo-agent".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            max_sessions: DEFAULT_MAX_SESSIONS,
            max_prompt_chars: DEFAULT_MAX_PROMPT_CHARS,
            max_update_chars: DEFAULT_MAX_UPDATE_CHARS,
            max_updates_per_turn: DEFAULT_MAX_UPDATES_PER_TURN,
            max_total_update_chars: DEFAULT_MAX_TOTAL_UPDATE_CHARS,
            max_extension_concurrency: DEFAULT_MAX_EXTENSION_CONCURRENCY,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

impl AcpAdapterConfig {
    /// Validate non-empty implementation metadata and positive resource bounds.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.name.trim().is_empty()
            || self.title.trim().is_empty()
            || self.version.trim().is_empty()
        {
            return Err(crate::error::ReactError::Other(
                "ACP adapter name, title, and version must not be empty".to_string(),
            ));
        }
        if self.max_sessions == 0
            || self.max_prompt_chars == 0
            || self.max_update_chars == 0
            || self.max_updates_per_turn == 0
            || self.max_total_update_chars == 0
            || self.max_extension_concurrency == 0
        {
            return Err(crate::error::ReactError::Other(
                "ACP adapter resource limits must be positive".to_string(),
            ));
        }
        if self.shutdown_timeout.is_zero() {
            return Err(crate::error::ReactError::Other(
                "ACP adapter shutdown timeout must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

/// Transport-neutral stable ACP v1 Agent role backed by framework Agents.
pub struct AcpAgentAdapter {
    factory: Arc<dyn AcpSessionFactory>,
    config: AcpAdapterConfig,
}

impl AcpAgentAdapter {
    /// Construct an adapter with default metadata and resource bounds.
    pub fn new(factory: impl AcpSessionFactory) -> Self {
        Self {
            factory: Arc::new(factory),
            config: AcpAdapterConfig::default(),
        }
    }

    /// Construct an adapter with explicitly validated metadata and bounds.
    pub fn with_config(
        factory: impl AcpSessionFactory,
        config: AcpAdapterConfig,
    ) -> crate::error::Result<Self> {
        config.validate()?;
        Ok(Self {
            factory: Arc::new(factory),
            config,
        })
    }

    /// Return the immutable adapter configuration.
    pub fn config(&self) -> &AcpAdapterConfig {
        &self.config
    }

    /// Attach a negotiated extension profile. The profile contributes its
    /// capability advertisement, hello decision, per-run observers, `_meta`
    /// bridging and typed handlers, while the official connection, standard
    /// handlers and close chain keep being built exactly once.
    pub fn with_profile<P: AcpConnectionProfile>(
        self,
        profile: P,
    ) -> AcpAgentAdapterWithProfile<P> {
        AcpAgentAdapterWithProfile {
            adapter: self,
            profile: Arc::new(profile),
        }
    }
}

/// [`AcpAgentAdapter`] with one concrete extension profile attached.
pub struct AcpAgentAdapterWithProfile<P: AcpConnectionProfile> {
    adapter: AcpAgentAdapter,
    profile: Arc<P>,
}

impl<P: AcpConnectionProfile> ConnectTo<Client> for AcpAgentAdapterWithProfile<P> {
    async fn connect_to(
        self,
        client: impl ConnectTo<AcpRole>,
    ) -> agent_client_protocol::Result<()> {
        run_connection(self.adapter, self.profile, client).await
    }
}

impl ConnectTo<Client> for AcpAgentAdapter {
    async fn connect_to(
        self,
        client: impl ConnectTo<AcpRole>,
    ) -> agent_client_protocol::Result<()> {
        run_connection(self, Arc::new(NoProfile), client).await
    }
}

/// Profile used by the standard-only path: never advertises, never
/// negotiates, contributes no handlers or observers. It exists so both
/// connection paths share one generic implementation.
#[derive(Clone, Copy, Default)]
struct NoProfile;

impl AcpConnectionProfile for NoProfile {
    fn negotiate_hello(&self, _hello: &serde_json::Value) -> std::result::Result<(), String> {
        Err("no extension profile is configured".to_string())
    }

    fn attach(
        &self,
        _services: Arc<AcpConnectionServices>,
    ) -> agent_client_protocol::Builder<
        AcpRole,
        impl agent_client_protocol::HandleDispatchFrom<Client>,
        impl agent_client_protocol::RunWithConnectionTo<Client>,
        impl agent_client_protocol::HandleConnectionClose<Client>,
        agent_client_protocol::RawConnectionContext,
    > {
        AcpRole.builder()
    }
}

async fn run_connection<P: AcpConnectionProfile>(
    adapter: AcpAgentAdapter,
    profile: Arc<P>,
    client: impl ConnectTo<AcpRole>,
) -> agent_client_protocol::Result<()> {
    adapter.config.validate().map_err(framework_error)?;
    let registry = Arc::new(SessionRegistry::new(
        adapter.factory,
        adapter.config.max_sessions,
    ));
    let config = Arc::new(adapter.config);
    let services = Arc::new(AcpConnectionServices::new(registry.clone(), config.clone()));

    // `initialize`: single registration point. The profile's advertisement
    // rides `agentCapabilities._meta`, and its hello decision promotes the
    // whole connection to Extended mode. A plain Client (no `_meta` entry)
    // and a failed negotiation both stay Standard without breaking the
    // standard initialize.
    let initialize_services = services.clone();
    let initialize_profile = profile.clone();
    let initialize_registry = registry.clone();
    let initialize_config = config.clone();
    // `new`: standard handler remains the only implementation of
    // `session/new`; extension profiles bridge their handle through `_meta`.
    let new_session_services = services.clone();
    let new_session_profile = profile.clone();
    // `session/prompt`: enters the shared run authority like any extension
    // Run, so both profiles observe the same run ids and one-active-run.
    let prompt_services = services.clone();
    let prompt_profile = profile.clone();
    let prompt_registry = registry.clone();
    let prompt_config = config.clone();
    let cancel_registry = registry.clone();
    let close_services = services.clone();
    let close_config = config.clone();
    let close_profile = profile.clone();

    let connection_result = AcpRole
        .builder()
        .name(config.name.clone())
        .on_receive_request(
            async move |request: v1::InitializeRequest,
                        responder: Responder<v1::InitializeResponse>,
                        _connection: ConnectionTo<Client>| {
                initialize_registry
                    .initialize(request.client_capabilities.clone())
                    .await;
                let mut capabilities = v1::AgentCapabilities::new();
                if let Some((key, value)) = initialize_profile.advertisement_meta() {
                    let mut meta = serde_json::Map::new();
                    meta.insert(key.clone(), value);
                    capabilities = capabilities.meta(meta);
                    let hello = request
                        .client_capabilities
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get(&key));
                    match hello {
                        None => {}
                        Some(hello) => match initialize_profile.negotiate_hello(hello) {
                            Ok(()) => {
                                initialize_services.set_mode(ConnectionMode::Extended).await;
                            }
                            Err(reason) => {
                                tracing::warn!(
                                    "echo-agent extension negotiation stayed standard: {reason}"
                                );
                            }
                        },
                    }
                }
                let response = v1::InitializeResponse::new(ProtocolVersion::V1)
                    .agent_capabilities(capabilities)
                    .agent_info(
                        v1::Implementation::new(
                            initialize_config.name.clone(),
                            initialize_config.version.clone(),
                        )
                        .title(initialize_config.title.clone()),
                    );
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: v1::NewSessionRequest,
                        responder: Responder<v1::NewSessionResponse>,
                        connection: ConnectionTo<Client>| {
                if let Err(error) = validate_session_paths(&request) {
                    return responder.respond_with_error(invalid_params(error));
                }
                if !new_session_services.admission_open() {
                    return responder.respond_with_error(
                        Error::internal_error().data("ACP Host is shutting down".to_string()),
                    );
                }
                let requested_cwd = request.cwd.clone();
                let services = new_session_services.clone();
                let profile = new_session_profile.clone();
                connection.spawn(async move {
                    match services.create_session(request).await {
                        Ok(session_id) => {
                            let session_text = session_id.to_string();
                            let mut response = v1::NewSessionResponse::new(session_id.clone());
                            if services.is_extended().await {
                                if let Err(error) =
                                    profile.register_standard_session(&session_text, &requested_cwd)
                                {
                                    let _ = services.sessions().close_session(&session_id).await;
                                    return responder.respond_with_error(framework_error(error));
                                }
                                if let Some(meta) = profile.annotate_standard(
                                    StandardBridgeOutcome::SessionCreated {
                                        session_id: session_text.as_str(),
                                    },
                                ) {
                                    response = response.meta(meta);
                                }
                            }
                            responder.respond(response)
                        }
                        Err(error) => responder.respond_with_error(framework_error(error)),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: v1::PromptRequest,
                        responder: Responder<v1::PromptResponse>,
                        connection: ConnectionTo<Client>| {
                if !prompt_services.admission_open() {
                    return responder.respond_with_error(
                        Error::internal_error().data("ACP Host is shutting down".to_string()),
                    );
                }
                let prepared = match prepare_prompt(&prompt_registry, &prompt_config, request).await
                {
                    Ok(prepared) => prepared,
                    Err(error) => return responder.respond_with_error(error),
                };
                if !prompt_services.admission_open() {
                    return responder.respond_with_error(
                        Error::internal_error().data("ACP Host is shutting down".to_string()),
                    );
                }
                let request_cancellation = responder.cancellation();
                let task_connection = connection.clone();
                let services = prompt_services.clone();
                let profile = prompt_profile.clone();
                connection.spawn(async move {
                    drive_prompt(
                        prepared,
                        responder,
                        task_connection,
                        request_cancellation,
                        services,
                        profile,
                    )
                    .await
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: v1::CancelNotification, _connection: ConnectionTo<Client>| {
                cancel_registry.cancel(&notification.session_id).await;
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_close(async move |_connection: ConnectionTo<Client>| {
            let timeout = close_config.shutdown_timeout;
            close_services.close_admission();
            tokio::time::timeout(timeout, async {
                close_services.cancel_and_wait_runs(timeout).await;
                // Extension teardown order (design §12.3): close admission,
                // cancel in-flight callbacks, await bounded settlement —
                // all before profile flush and Session Agent close.
                close_services.extensions().close_admission();
                close_services.extensions().cancel_all();
                let leaked = close_services.extensions().drain(timeout).await;
                if leaked > 0 {
                    tracing::warn!("extension teardown left {leaked} unsettled invocations");
                }
                close_profile
                    .wait_for_settlements(timeout)
                    .await
                    .map_err(framework_error)?;
                close_profile
                    .flush_before_agents()
                    .map_err(framework_error)?;
                close_services
                    .close_sessions()
                    .await
                    .map_err(framework_error)
            })
            .await
            .map_err(|_| {
                Error::internal_error().data("ACP Session shutdown timed out".to_string())
            })?
            .map_err(framework_error)
        })
        .with_connection_builder(profile.attach(services.clone()))
        .connect_to(client)
        .await;
    let cleanup_result = tokio::time::timeout(config.shutdown_timeout, async {
        services.close_admission();
        services.cancel_and_wait_runs(config.shutdown_timeout).await;
        services.extensions().close_admission();
        services.extensions().cancel_all();
        let leaked = services.extensions().drain(config.shutdown_timeout).await;
        if leaked > 0 {
            tracing::warn!("extension cleanup left {leaked} unsettled invocations");
        }
        profile
            .wait_for_settlements(config.shutdown_timeout)
            .await
            .map_err(framework_error)?;
        profile.flush_before_agents().map_err(framework_error)?;
        services.close_sessions().await.map_err(framework_error)
    })
    .await
    .map_err(|_| {
        agent_client_protocol::Error::internal_error()
            .data("ACP Session shutdown timed out".to_string())
    })?;
    match (connection_result, cleanup_result) {
        (Err(error), _) => Err(error),
        (Ok(()), result) => result,
    }
}

struct PreparedPrompt {
    session_id: v1::SessionId,
    session: Arc<AcpSession>,
    active: ActiveTurnLease,
    turn: TurnRequest,
}

async fn prepare_prompt(
    registry: &SessionRegistry,
    config: &AcpAdapterConfig,
    request: v1::PromptRequest,
) -> std::result::Result<PreparedPrompt, Error> {
    let Some(session) = registry.get(&request.session_id).await else {
        return Err(Error::invalid_params().data("unknown ACP Session ID".to_string()));
    };
    let prompt = map_prompt(request.prompt, config.max_prompt_chars).map_err(invalid_params)?;
    let active = session.begin_turn().map_err(invalid_params)?;
    let identity = match EventIdentity::for_chat(
        Some(request.session_id.to_string()),
        active.turn.id(),
        active.turn.message_id.clone(),
        None,
    ) {
        Ok(identity) => identity,
        Err(error) => {
            drop(active);
            return Err(framework_error(error));
        }
    };
    let turn = match prompt {
        MappedPrompt::Text(text) => TurnRequest::new(identity, text),
        MappedPrompt::Structured(message) => TurnRequest::from_message(identity, message),
    }
    .mode(TurnMode::Chat)
    .cancel(active.turn.cancellation());
    Ok(PreparedPrompt {
        session_id: request.session_id,
        session,
        active,
        turn,
    })
}

async fn drive_prompt<P: AcpConnectionProfile>(
    prepared: PreparedPrompt,
    responder: Responder<v1::PromptResponse>,
    connection: ConnectionTo<Client>,
    request_cancellation: agent_client_protocol::RequestCancellation,
    services: Arc<AcpConnectionServices>,
    profile: Arc<P>,
) -> agent_client_protocol::Result<()> {
    let PreparedPrompt {
        session_id,
        session,
        active,
        turn,
        ..
    } = prepared;
    let projector = services.projector(session_id.clone(), connection.clone());
    let run_id = active.turn.id().to_string();
    let stream_id = turn.identity.stream_id.as_str().to_string();
    let observers = if services.is_extended().await {
        let session_str = session_id.to_string();
        profile.run_observers(RunObserverContext {
            session_id: session_str.as_str(),
            run_id: run_id.as_str(),
            stream_id: stream_id.as_str(),
            connection: connection.clone(),
        })
    } else {
        Vec::new()
    };
    let journal = if services.is_extended().await {
        match profile.run_journal(&run_id) {
            Ok(journal) => journal,
            Err(error) => {
                profile.rollback_run(&run_id, &stream_id);
                return responder.respond_with_error(framework_error(error));
            }
        }
    } else {
        None
    };
    let spec = RunStartSpec {
        session: session.clone(),
        active,
        run_id: run_id.clone(),
        stream_id: stream_id.clone(),
        turn,
        projector: Some(projector),
        journal,
        observers,
    };
    let (entry, task) = match services.prepare_run(spec).await {
        Ok(pair) => pair,
        Err(error) => {
            profile.rollback_run(&run_id, &stream_id);
            return responder.respond_with_error(framework_error(error));
        }
    };
    if services.is_extended().await
        && let Err(error) = profile.register_standard_run(&session_id.to_string(), entry.clone())
    {
        let _ = services.remove_run(&entry.run_id).await;
        profile.rollback_run(&entry.run_id, &entry.stream_id);
        drop(task);
        return responder.respond_with_error(framework_error(error));
    }
    if let Err(error) = connection.spawn(task) {
        let _ = services.remove_run(&entry.run_id).await;
        profile.rollback_run(&entry.run_id, &entry.stream_id);
        return responder.respond_with_error(error);
    }
    profile.run_spawned(entry.clone());
    let receipt = tokio::select! {
        receipt = entry.wait_receipt() => receipt,
        () = request_cancellation.cancelled() => {
            entry.cancel();
            entry.wait_receipt().await
        }
    };
    if let Err(error) = profile.persist_run_settled(&entry, &receipt) {
        return responder.respond_with_error(framework_error(error));
    }
    let mut response = match &receipt.outcome {
        TurnOutcome::Completed => v1::PromptResponse::new(v1::StopReason::EndTurn),
        TurnOutcome::Cancelled => v1::PromptResponse::new(v1::StopReason::Cancelled),
        TurnOutcome::Failed(failure) => {
            return responder
                .respond_with_error(Error::internal_error().data(bounded(&failure.message)));
        }
    };
    if services.is_extended().await {
        let session_str = session_id.to_string();
        if let Some(meta) = profile.annotate_standard(StandardBridgeOutcome::PromptStarted {
            session_id: session_str.as_str(),
            run_id: entry.run_id.as_str(),
            stream_id: entry.stream_id.as_str(),
        }) {
            response = response.meta(meta);
        }
    }
    responder.respond(response)
}

fn invalid_params(error: crate::error::ReactError) -> Error {
    Error::invalid_params().data(bounded(&error.to_string()))
}

fn framework_error(error: impl std::fmt::Display) -> Error {
    Error::internal_error().data(bounded(&error.to_string()))
}

fn bounded(message: &str) -> String {
    message.chars().take(MAX_ERROR_CHARS).collect()
}
