use super::projection::AcpEventProjector;
use super::prompt::{MappedPrompt, map_prompt};
use super::session::{
    AcpSession, AcpSessionFactory, ActiveTurnLease, SessionRegistry, validate_session_paths,
};
use crate::agent::EventIdentity;
use crate::runtime::{AgentTurnDriver, TurnMode, TurnOutcome, TurnRequest};
use agent_client_protocol::schema::{ProtocolVersion, v1};
use agent_client_protocol::{Agent as AcpRole, Client, ConnectTo, ConnectionTo, Error, Responder};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_MAX_SESSIONS: usize = 128;
const DEFAULT_MAX_PROMPT_CHARS: usize = 1_000_000;
const DEFAULT_MAX_UPDATE_CHARS: usize = 1_000_000;
const DEFAULT_MAX_UPDATES_PER_TURN: usize = 10_000;
const DEFAULT_MAX_TOTAL_UPDATE_CHARS: usize = 8_000_000;
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
}

impl ConnectTo<Client> for AcpAgentAdapter {
    async fn connect_to(
        self,
        client: impl ConnectTo<AcpRole>,
    ) -> agent_client_protocol::Result<()> {
        self.config.validate().map_err(framework_error)?;
        let registry = Arc::new(SessionRegistry::new(self.factory, self.config.max_sessions));
        let config = Arc::new(self.config);

        let connection_result = AcpRole
            .builder()
            .name(config.name.clone())
            .on_receive_request(
                {
                    let registry = registry.clone();
                    let config = config.clone();
                    async move |request: v1::InitializeRequest,
                                responder: Responder<v1::InitializeResponse>,
                                _connection: ConnectionTo<Client>| {
                        registry
                            .initialize(request.client_capabilities.clone())
                            .await;
                        let response = v1::InitializeResponse::new(ProtocolVersion::V1)
                            .agent_capabilities(v1::AgentCapabilities::new())
                            .agent_info(
                                v1::Implementation::new(
                                    config.name.clone(),
                                    config.version.clone(),
                                )
                                .title(config.title.clone()),
                            );
                        responder.respond(response)
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let registry = registry.clone();
                    async move |request: v1::NewSessionRequest,
                                responder: Responder<v1::NewSessionResponse>,
                                connection: ConnectionTo<Client>| {
                        if let Err(error) = validate_session_paths(&request) {
                            return responder.respond_with_error(invalid_params(error));
                        }
                        let registry = registry.clone();
                        connection.spawn(async move {
                            match registry.create(request).await {
                                Ok(session_id) => {
                                    responder.respond(v1::NewSessionResponse::new(session_id))
                                }
                                Err(error) => responder.respond_with_error(framework_error(error)),
                            }
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let registry = registry.clone();
                    let config = config.clone();
                    async move |request: v1::PromptRequest,
                                responder: Responder<v1::PromptResponse>,
                                connection: ConnectionTo<Client>| {
                        let prepared = match prepare_prompt(&registry, &config, request).await {
                            Ok(prepared) => prepared,
                            Err(error) => return responder.respond_with_error(error),
                        };
                        let request_cancellation = responder.cancellation();
                        let task_connection = connection.clone();
                        connection.spawn(async move {
                            drive_prompt(prepared, responder, task_connection, request_cancellation)
                                .await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                {
                    let registry = registry.clone();
                    async move |notification: v1::CancelNotification,
                                _connection: ConnectionTo<Client>| {
                        registry.cancel(&notification.session_id).await;
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_close({
                let registry = registry.clone();
                let config = config.clone();
                async move |_connection: ConnectionTo<Client>| {
                    close_registry(&registry, config.shutdown_timeout).await
                }
            })
            .connect_to(client)
            .await;
        let cleanup_result = close_registry(&registry, config.shutdown_timeout).await;
        match (connection_result, cleanup_result) {
            (Err(error), _) => Err(error),
            (Ok(()), result) => result,
        }
    }
}

async fn close_registry(
    registry: &SessionRegistry,
    timeout: Duration,
) -> agent_client_protocol::Result<()> {
    tokio::time::timeout(timeout, registry.close_all())
        .await
        .map_err(|_| Error::internal_error().data("ACP Session shutdown timed out".to_string()))?
        .map_err(framework_error)
}

struct PreparedPrompt {
    session_id: v1::SessionId,
    session: Arc<AcpSession>,
    active: ActiveTurnLease,
    turn: TurnRequest,
    max_update_chars: usize,
    max_updates_per_turn: usize,
    max_total_update_chars: usize,
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
        active.turn.id.clone(),
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
    .cancel(active.turn.cancel.clone());
    Ok(PreparedPrompt {
        session_id: request.session_id,
        session,
        active,
        turn,
        max_update_chars: config.max_update_chars,
        max_updates_per_turn: config.max_updates_per_turn,
        max_total_update_chars: config.max_total_update_chars,
    })
}

async fn drive_prompt(
    prepared: PreparedPrompt,
    responder: Responder<v1::PromptResponse>,
    connection: ConnectionTo<Client>,
    request_cancellation: agent_client_protocol::RequestCancellation,
) -> agent_client_protocol::Result<()> {
    let PreparedPrompt {
        session_id,
        session,
        active,
        turn,
        max_update_chars,
        max_updates_per_turn,
        max_total_update_chars,
    } = prepared;
    let sink = AcpEventProjector::new(
        session_id,
        connection,
        max_update_chars,
        max_updates_per_turn,
        max_total_update_chars,
    );
    let drive = AgentTurnDriver.drive(session.agent.as_ref(), turn, &sink);
    tokio::pin!(drive);
    let receipt = tokio::select! {
        receipt = &mut drive => receipt,
        () = request_cancellation.cancelled() => {
            active.turn.cancel.cancel();
            drive.await
        }
    };
    drop(active);
    match receipt.outcome {
        TurnOutcome::Completed => {
            responder.respond(v1::PromptResponse::new(v1::StopReason::EndTurn))
        }
        TurnOutcome::Cancelled => {
            responder.respond(v1::PromptResponse::new(v1::StopReason::Cancelled))
        }
        TurnOutcome::Failed(failure) => {
            responder.respond_with_error(Error::internal_error().data(bounded(&failure.message)))
        }
    }
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
