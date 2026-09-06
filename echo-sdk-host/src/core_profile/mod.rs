//! Negotiated `_echo_agent/*` core profile over the shared ACP runtime.
//!
//! The profile is the only place extension semantics live: advertisement,
//! hello negotiation, typed handlers, generation-fenced handles, ACK-gated
//! live events, durable replay and recovery. It attaches to the root
//! adapter through [`echo_agent::acp::AcpConnectionProfile`], so the
//! official connection, standard handlers and close chain are still built
//! exactly once — `with_connection_builder` merges this profile's typed
//! handlers after the standard ones on the same dispatch loop.

pub(crate) mod events;
#[cfg(feature = "sdk-extension-bridge")]
pub(crate) mod extension_bridge;
pub(crate) mod handler;
pub(crate) mod handles;
pub(crate) mod persistence;
pub(crate) mod state;
pub(crate) mod wire;

use std::sync::Arc;

use agent_client_protocol::{Agent as AcpRole, Client, ConnectionTo, RawConnectionContext};
use echo_agent::acp::{
    AcpConnectionProfile, AcpConnectionServices, RunEventObserver, RunObserverContext,
    StandardBridgeOutcome,
};
use echo_sdk_protocol::capability::{ECHO_AGENT_META_KEY, EchoAgentClientHello};
use echo_sdk_protocol::event::{EventAckNotification, ReplayRequest};
use echo_sdk_protocol::handle::HandleKind;
use echo_sdk_protocol::methods::{
    AgentCloseRequest, AgentCreateRequest, AgentDescribeRequest, RunCancelRequest, RunGetRequest,
    RunStartRequest, RunSteerRequest, RunWaitRequest, SessionCloseRequest, SessionCreateRequest,
    SessionLoadRequest,
};

use crate::HostError;
use handles::HandleRegistry;
use state::CoreProfileState;

/// The negotiated core profile: bind with
/// [`crate::run_stdio_with_profile`] (or construct directly for tests) and
/// attach through [`echo_agent::acp::AcpAgentAdapter::with_profile`].
pub struct SdkCoreProfile {
    state: Arc<CoreProfileState>,
}

impl SdkCoreProfile {
    /// Install the profile for one Host process: open the state root,
    /// advance the generation and prepare the default Agent definition
    /// (which carries the state store for checkpoint/resume).
    pub fn install(
        state_root: &std::path::Path,
        limits: crate::config::SdkProfileLimits,
        framework: echo_agent::config::FrameworkConfig,
        llm_client: std::sync::Arc<dyn echo_agent::llm::LlmClient>,
    ) -> Result<Self, HostError> {
        let state = CoreProfileState::install(state_root, limits, framework, llm_client)?;
        Ok(Self { state })
    }

    /// Handle generation of this Host incarnation.
    pub fn generation(&self) -> u64 {
        self.state.handles.generation()
    }

    pub(crate) fn session_factory(&self) -> crate::factory::CoreProfileSessionFactory {
        self.state.session_factory.clone()
    }

    pub(crate) fn register_standard_session(
        &self,
        session_id: &str,
        cwd: &std::path::Path,
    ) -> Result<(), String> {
        let (session_handle, session_record) = self
            .state
            .handles
            .register_session_with_cwd(
                session_id.to_string(),
                self.state.default_agent_handle.id.clone(),
                Some(cwd.to_path_buf()),
            )
            .map_err(|error| error.message.clone())?;
        let fingerprint = HandleRegistry::config_fingerprint(
            &echo_sdk_protocol::methods::AgentConfigWire::HostDefault,
        );
        let cwd_wire = match wire::path_to_wire(cwd) {
            Ok(path) => path,
            Err(error) => {
                let _ = self.state.handles.close_session(&session_handle);
                return Err(error);
            }
        };
        if let Err(error) = self
            .state
            .persistence
            .record_session(
                session_id,
                &session_record.agent_handle_id,
                &fingerprint,
                cwd_wire,
            )
            .map_err(|error| error.to_string())
        {
            let _ = self.state.handles.close_session(&session_handle);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn register_standard_run(
        &self,
        _session_id: &str,
        entry: Arc<echo_agent::acp::RunEntry>,
    ) -> Result<(), String> {
        let Some(session_handle_id) = self
            .state
            .handles
            .session_handle_for_acp(entry.session_id.to_string().as_str())
        else {
            self.state.remove_delivery_for_run(&entry.run_id);
            self.state.remove_journal(&entry.run_id);
            return Err("standard Session handle is not registered".to_string());
        };
        if let Err(error) = self.state.handles.register_live_run(
            entry.run_id.clone(),
            entry.clone(),
            Some(&session_handle_id.id),
        ) {
            self.state.remove_delivery_for_run(&entry.run_id);
            return Err(error.message.clone());
        }
        if let Err(error) = self
            .state
            .handles
            .register_live_stream(entry.stream_id.clone(), entry.run_id.clone())
        {
            self.state.handles.remove_run(&entry.run_id);
            self.state.remove_delivery_for_run(&entry.run_id);
            return Err(error.message.clone());
        }
        let agent_handle_id = match self
            .state
            .handles
            .session(&session_handle_id)
            .map(|record| record.agent_handle_id.clone())
        {
            Ok(agent_handle_id) => agent_handle_id,
            Err(error) => {
                self.state.handles.remove_run(&entry.run_id);
                self.state.remove_delivery_for_run(&entry.run_id);
                return Err(error.message.clone());
            }
        };
        let fingerprint = match self
            .state
            .handles
            .agent(&wire::handle(
                agent_handle_id.clone(),
                HandleKind::Agent,
                self.state.handles.generation(),
            ))
            .map(|record| record.config_fingerprint.clone())
        {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                self.state.handles.remove_run(&entry.run_id);
                self.state.remove_delivery_for_run(&entry.run_id);
                return Err(error.message.clone());
            }
        };
        let cwd = self
            .state
            .handles
            .session_handle_for_acp(entry.session_id.to_string().as_str())
            .and_then(|handle| self.state.handles.session(&handle).ok())
            .and_then(|record| record.cwd.clone())
            .map(|path| wire::path_to_wire(&path))
            .transpose();
        let cwd = match cwd {
            Ok(cwd) => cwd,
            Err(error) => {
                self.state.handles.remove_run(&entry.run_id);
                self.state.remove_delivery_for_run(&entry.run_id);
                return Err(error);
            }
        };
        if let Err(error) = self
            .state
            .persistence
            .record_run_started(
                &entry.run_id,
                &entry.stream_id,
                &entry.session_id.to_string(),
                &agent_handle_id,
                &fingerprint,
                cwd,
                "chat",
            )
            .map_err(|error| error.to_string())
        {
            self.state.handles.remove_run(&entry.run_id);
            self.state.remove_delivery_for_run(&entry.run_id);
            return Err(error);
        }
        Ok(())
    }

    /// Recovered runs visible after a restart on this state root.
    pub fn recovered_run_count(&self) -> usize {
        self.state.recovered_runs.len()
    }

    /// Bounded shutdown flush for callers that own the Host lifecycle.
    pub fn flush(&self) -> Result<(), String> {
        self.state.flush_journals()
    }

    fn persist_receipt(
        &self,
        entry: &echo_agent::acp::RunEntry,
        receipt: &echo_agent::runtime::TurnReceipt,
    ) -> Result<(), String> {
        let terminal = wire::terminal_of(receipt)?;
        let receipt_wire = wire::receipt_wire(receipt)?;
        if let Err(error) = self.state.persistence.record_run_settled(
            &entry.run_id,
            terminal,
            receipt_wire,
            entry.ledger.last_sequence(),
        ) {
            return Err(error.to_string());
        }
        self.state.cleanup_settled_run(&entry.run_id);
        Ok(())
    }

    fn stream_buffer_limit(&self) -> usize {
        self.state
            .limits
            .max_event_bytes
            .saturating_mul(self.state.limits.max_outstanding_live_events)
    }
}

impl AcpConnectionProfile for SdkCoreProfile {
    fn advertisement_meta(&self) -> Option<(String, serde_json::Value)> {
        state::advertisement_meta(&self.state.advertisement)
    }

    fn negotiate_hello(&self, hello: &serde_json::Value) -> std::result::Result<(), String> {
        let parsed = EchoAgentClientHello::from_meta_value(hello)?;
        let shape = parsed.validate_shape();
        if !shape.is_empty() {
            return Err(format!(
                "client hello failed shape validation: {}",
                shape.join("; ")
            ));
        }
        let problems = self.state.advertisement.negotiate_hello(&parsed);
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn annotate_standard(
        &self,
        outcome: StandardBridgeOutcome<'_>,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        // Bridge standard responses with the same generation-fenced handles
        // the extension methods mint, so both entry points reference one
        // object (design §10.4).
        let mut meta = serde_json::Map::new();
        match outcome {
            StandardBridgeOutcome::SessionCreated { session_id } => {
                let handle = self.state.handles.session_handle_for_acp(session_id)?;
                meta.insert("session".to_string(), serde_json::to_value(handle).ok()?);
            }
            StandardBridgeOutcome::PromptStarted {
                session_id: _,
                run_id,
                stream_id,
            } => {
                let run = self.state.handles.run_handle_for_id(run_id)?;
                let stream = self.state.handles.stream_handle_for_id(stream_id)?;
                meta.insert("run".to_string(), serde_json::to_value(run).ok()?);
                meta.insert("stream".to_string(), serde_json::to_value(stream).ok()?);
            }
        }
        let mut wrapper = serde_json::Map::new();
        wrapper.insert(
            ECHO_AGENT_META_KEY.to_string(),
            serde_json::Value::Object(meta),
        );
        Some(wrapper)
    }

    fn register_standard_session(
        &self,
        session_id: &str,
        cwd: &std::path::Path,
    ) -> std::result::Result<(), String> {
        SdkCoreProfile::register_standard_session(self, session_id, cwd)
    }

    fn register_standard_run(
        &self,
        session_id: &str,
        entry: Arc<echo_agent::acp::RunEntry>,
    ) -> std::result::Result<(), String> {
        SdkCoreProfile::register_standard_run(self, session_id, entry)
    }

    fn run_observers(&self, context: RunObserverContext<'_>) -> Vec<Arc<dyn RunEventObserver>> {
        // Live delivery also exists for standard Prompts in Extended mode:
        // both views come from the same committed envelopes.
        let stream_handle = wire::handle(
            context.stream_id,
            HandleKind::Stream,
            self.state.handles.generation(),
        );
        let delivery = events::StreamDelivery::new(
            self.state.clone(),
            context.run_id,
            context.connection,
            stream_handle.clone(),
            self.state.limits.max_outstanding_live_events,
            self.stream_buffer_limit(),
            self.state.limits.max_event_bytes,
        );
        self.state.register_delivery(delivery.clone());
        vec![delivery]
    }

    fn attach(
        &self,
        services: Arc<AcpConnectionServices>,
    ) -> agent_client_protocol::Builder<
        AcpRole,
        impl agent_client_protocol::HandleDispatchFrom<Client>,
        impl agent_client_protocol::RunWithConnectionTo<Client>,
        impl agent_client_protocol::HandleConnectionClose<Client>,
        RawConnectionContext,
    > {
        self.state.bind_services(services);
        let ack_state = self.state.clone();
        #[cfg(feature = "sdk-extension-bridge")]
        let stream_state = self.state.clone();
        let builder = AcpRole
            .builder()
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: AgentCreateRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::agent_create(state.clone(), request, responder, connection).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: AgentDescribeRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::agent_describe(state.clone(), request, responder, connection).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: AgentCloseRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::agent_close(task_state, request, responder, connection).await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: SessionCreateRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::session_create(task_state, request, responder, connection)
                                .await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: SessionLoadRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::session_load(task_state, request, responder, connection).await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: SessionCloseRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::session_close(task_state, request, responder, connection).await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: RunStartRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::run_start(task_state, request, responder, connection).await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: RunGetRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::run_get(state.clone(), request, responder, connection).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: RunWaitRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::run_wait(task_state, request, responder, connection).await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: RunCancelRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::run_cancel(state.clone(), request, responder, connection).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: RunSteerRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::run_steer(state.clone(), request, responder, connection).await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: ReplayRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        let task_connection = connection.clone();
                        let task_state = state.clone();
                        task_connection.spawn(async move {
                            handler::run_replay(task_state, request, responder, connection).await
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |notification: EventAckNotification,
                            connection: ConnectionTo<Client>| {
                    let state = ack_state.clone();
                    let task_connection = connection.clone();
                    task_connection.spawn(async move {
                        handler::event_ack(state, notification, connection).await
                    })
                },
                agent_client_protocol::on_receive_notification!(),
            );
        // ── Extension bridge ────────────────────────────────────────────
        // Compiled only with the feature: without it the typed handlers are
        // absent and the official dispatch answers method-not-found, which
        // is exactly the fail-closed contract. Statement-level `let`
        // rebinding keeps the chain one official builder composition.
        #[cfg(feature = "sdk-extension-bridge")]
        let builder = builder
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: echo_sdk_protocol::methods::ExtensionRegisterRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::extension_register(state.clone(), request, responder, connection)
                            .await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let state = self.state.clone();
                    async move |request: echo_sdk_protocol::methods::ExtensionUnregisterRequest,
                                responder,
                                connection: ConnectionTo<Client>| {
                        handler::extension_unregister(state.clone(), request, responder, connection)
                            .await
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |notification: echo_sdk_protocol::methods::ExtensionStreamEvent,
                            connection: ConnectionTo<Client>| {
                    eprintln!("[host] stream notification handler entered");
                    let state = stream_state.clone();
                    let task_connection = connection.clone();
                    task_connection.spawn(async move {
                        if let Err(error) = handler::extension_stream(state, notification).await {
                            tracing::warn!("extension stream delivery failed: {error}");
                        }
                        Ok(())
                    })
                },
                agent_client_protocol::on_receive_notification!(),
            );
        #[cfg(not(feature = "sdk-extension-bridge"))]
        let builder = builder;
        builder
    }

    fn run_journal(
        &self,
        run_id: &str,
    ) -> std::result::Result<
        Option<Arc<dyn echo_agent::state::journal::EventJournal<echo_agent::agent::EventEnvelope>>>,
        String,
    > {
        self.state
            .journal(run_id)
            .map(|journal| {
                Some(
                    journal
                        as Arc<
                            dyn echo_agent::state::journal::EventJournal<
                                    echo_agent::agent::EventEnvelope,
                                >,
                        >,
                )
            })
            .map_err(|error| error.to_string())
    }

    fn persist_run_settled(
        &self,
        entry: &echo_agent::acp::RunEntry,
        receipt: &echo_agent::runtime::TurnReceipt,
    ) -> std::result::Result<(), String> {
        self.persist_receipt(entry, receipt)
    }

    fn rollback_run(&self, run_id: &str, stream_id: &str) {
        self.state.handles.remove_run(run_id);
        self.state.remove_delivery(stream_id);
        self.state.remove_delivery_for_run(run_id);
        self.state.remove_journal(run_id);
        if let Err(error) = self.state.persistence.remove_run(run_id) {
            tracing::warn!("failed to rollback durable run {run_id}: {error}");
        }
    }

    fn run_spawned(&self, entry: Arc<echo_agent::acp::RunEntry>) {
        self.state.begin_settlement();
        let settlement_state = self.state.clone();
        tokio::spawn(async move {
            let receipt = entry.wait_receipt().await;
            let result = wire::terminal_of(&receipt)
                .and_then(|terminal| {
                    wire::receipt_wire(&receipt).map(|receipt| (terminal, receipt))
                })
                .and_then(|(terminal, receipt)| {
                    settlement_state
                        .persistence
                        .record_run_settled(
                            &entry.run_id,
                            terminal,
                            receipt,
                            entry.ledger.last_sequence(),
                        )
                        .map_err(|error| error.to_string())
                });
            settlement_state.cleanup_settled_run(&entry.run_id);
            settlement_state.finish_settlement(result);
        });
    }

    fn flush_before_agents(&self) -> std::result::Result<(), String> {
        self.state.flush_journals()
    }

    fn release_after_agents(&self) {
        // Connection teardown order ends here: extension registrations are
        // connection-owned and never survive the connection (design §12.1).
        #[cfg(feature = "sdk-extension-bridge")]
        self.state.handles.close_all_extensions();
    }

    fn wait_for_settlements(
        &self,
        timeout: std::time::Duration,
    ) -> futures::future::BoxFuture<'static, std::result::Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move { state.wait_settlements(timeout).await })
    }
}
