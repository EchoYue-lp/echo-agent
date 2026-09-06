//! Generation-fenced handle registry for the core profile.
//!
//! Validation order is fixed by the contract (design §10.4): shape → kind →
//! generation → issued/closed. A well-shaped id that was never issued at the
//! current generation is `invalid_value`; an old-generation handle is
//! `stale_handle`; a released handle is `closed_handle`. Handles never
//! rebind: ids are minted once and tombstoned on close, and the tombstone
//! ring is bounded by the configured handle limit so a chatty client cannot
//! grow the registry without end.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::{HandleKind, WireHandle};
use echo_sdk_protocol::methods::{
    AgentConfigWire, ExtensionDescriptor, ExtensionKind, MAX_EXTENSION_DESCRIPTOR_BYTES,
};
use echo_sdk_protocol::scalar::WireDuration;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::core_profile::wire::{handle, handle_error, sdk_error};
use crate::factory::PreparedAgentDefinition;

pub(crate) struct AgentRecord {
    pub definition: Arc<PreparedAgentDefinition>,
    /// Canonical config JSON used for idempotent create comparison.
    pub config_fingerprint: String,
}

pub(crate) struct SessionRecord {
    /// ACP Session identity shared with the standard profile.
    pub acp_session_id: String,
    pub agent_handle_id: String,
    #[allow(dead_code)]
    pub cwd: Option<std::path::PathBuf>,
}

/// One run known to the handle registry. Live runs carry the shared
/// [`echo_agent::acp::RunEntry`]; recovered runs only carry their durable
/// snapshot and never revive a driver.
/// Durable snapshot facts of a run recovered from a previous Host process.
pub(crate) struct RecoveredRunRecord {
    /// The durable envelope stream identity; the stream handle for a
    /// recovered run must address exactly this id so journal replay
    /// cursors line up.
    pub stream_id: String,
    pub session_id: String,
    pub status: echo_sdk_protocol::methods::RunStatus,
    pub last_sequence: u64,
    pub terminal: Option<echo_sdk_protocol::methods::RunTerminal>,
    pub receipt: Option<echo_sdk_protocol::methods::RunReceiptWire>,
}

pub(crate) enum RunRecord {
    Live {
        entry: Arc<echo_agent::acp::RunEntry>,
    },
    Recovered(Box<RecoveredRunRecord>),
}

pub(crate) struct StreamRecord {
    pub run_handle_id: String,
}

/// One registered extension implementation. The record is connection-owned:
/// it exists only while the registering connection lives and is never
/// persisted or restored across a Host restart (design §12.1).
pub(crate) struct ExtensionRecord {
    pub kind: ExtensionKind,
    pub implementation_id: String,
    pub descriptor: ExtensionDescriptor,
    /// Canonical descriptor fingerprint for idempotent re-registration.
    pub descriptor_fingerprint: String,
    /// Per-registration default invocation deadline.
    pub timeout: Option<WireDuration>,
}

struct HandleInner {
    generation: u64,
    max_handles: usize,
    agents: HashMap<String, Arc<AgentRecord>>,
    sessions: HashMap<String, Arc<SessionRecord>>,
    runs: HashMap<String, Arc<RunRecord>>,
    streams: HashMap<String, Arc<StreamRecord>>,
    extensions: HashMap<String, Arc<ExtensionRecord>>,
    /// `(kind, implementation_id)` identity → extension handle id, for
    /// idempotent re-registration and typed conflicts.
    extension_index: HashMap<String, String>,
    tombstones: VecDeque<(HandleKind, String)>,
    idempotency: HashMap<String, Arc<AgentCreateOutcome>>,
    next_agent_id: u64,
}

/// Result of an idempotent `agent/create` invocation.
pub(crate) struct AgentCreateOutcome {
    pub agent: WireHandle,
}

/// Errors of handle resolution, mapped to typed extension errors by
/// [`HandleRegistry::resolve_error`].
pub(crate) enum ResolveError {
    /// Malformed handle (empty id, over-long id).
    InvalidShape(&'static str),
    /// Handle shape is valid but addresses a different object family.
    WrongKind,
    /// Handle predates the current Host generation.
    Stale,
    /// Well-formed, current generation, but never issued.
    Unknown,
    /// Handle was issued and explicitly released in this generation.
    Closed,
}

pub(crate) struct HandleRegistry {
    inner: Mutex<HandleInner>,
}

impl HandleRegistry {
    pub fn new(generation: u64, max_handles: usize) -> Self {
        Self {
            inner: Mutex::new(HandleInner {
                generation,
                max_handles,
                agents: HashMap::new(),
                sessions: HashMap::new(),
                runs: HashMap::new(),
                streams: HashMap::new(),
                extensions: HashMap::new(),
                extension_index: HashMap::new(),
                tombstones: VecDeque::new(),
                idempotency: HashMap::new(),
                next_agent_id: 0,
            }),
        }
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HandleInner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn enforce_budget(
        &self,
        inner: &mut HandleInner,
        additional: usize,
    ) -> Result<(), EchoSdkError> {
        let open = inner
            .agents
            .len()
            .saturating_add(inner.sessions.len())
            .saturating_add(inner.runs.len())
            .saturating_add(inner.streams.len())
            .saturating_add(inner.extensions.len());
        if open.saturating_add(additional) > inner.max_handles {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                format!("open handle limit {} reached", inner.max_handles),
                Retryability::AfterDelay,
                "handle-registry",
            ));
        }
        Ok(())
    }

    fn mint_id(inner: &mut HandleInner, kind: HandleKind) -> Result<String, EchoSdkError> {
        match kind {
            HandleKind::Agent => {
                inner.next_agent_id = inner.next_agent_id.checked_add(1).ok_or_else(|| {
                    sdk_error(
                        ExtensionErrorCode::PayloadTooLarge,
                        "Agent handle id counter exhausted",
                        Retryability::Never,
                        "handle-registry",
                    )
                })?;
                Ok(format!("agent-{}", inner.next_agent_id))
            }
            _ => Ok(format!("{}-{}", kind.as_str(), uuid::Uuid::new_v4())),
        }
    }

    fn insert(
        &self,
        inner: &mut HandleInner,
        kind: HandleKind,
        id: String,
    ) -> Result<WireHandle, EchoSdkError> {
        Ok(handle(id, kind, inner.generation))
    }

    /// Register a new Agent definition handle.
    pub fn register_agent(
        &self,
        definition: Arc<PreparedAgentDefinition>,
        config_fingerprint: String,
    ) -> Result<(WireHandle, Arc<AgentRecord>), EchoSdkError> {
        let mut inner = self.lock();
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::Agent)?;
        let record = Arc::new(AgentRecord {
            definition,
            config_fingerprint,
        });
        inner.agents.insert(id.clone(), record.clone());
        let agent = self.insert(&mut inner, HandleKind::Agent, id)?;
        Ok((agent, record))
    }

    /// Idempotent create: same id + same canonical config returns the same
    /// handle; same id + different config is a typed conflict.
    pub fn agent_create_idempotent(
        &self,
        idempotency_id: &str,
        config_fingerprint: String,
        create: impl FnOnce() -> Result<(Arc<PreparedAgentDefinition>, String), EchoSdkError>,
    ) -> Result<(WireHandle, Arc<AgentRecord>, bool), EchoSdkError> {
        {
            let inner = self.lock();
            if let Some(previous) = inner.idempotency.get(idempotency_id) {
                let record = inner.agents.get(&previous.agent.id).ok_or_else(|| {
                    handle_error(
                        ExtensionErrorCode::ClosedHandle,
                        "idempotent create refers to a closed agent",
                        "_echo_agent/agent/create",
                        &previous.agent,
                    )
                })?;
                if record.config_fingerprint == config_fingerprint {
                    return Ok((previous.agent.clone(), record.clone(), false));
                }
                return Err(handle_error(
                    ExtensionErrorCode::InvalidRequest,
                    "idempotency id was already used with a different config",
                    "_echo_agent/agent/create",
                    &previous.agent,
                ));
            }
        }
        let (definition, fingerprint) = create()?;
        let (agent, record) = self.register_agent(definition, fingerprint)?;
        let mut inner = self.lock();
        if let Some(previous) = inner.idempotency.get(idempotency_id).cloned() {
            let previous_record =
                inner
                    .agents
                    .get(&previous.agent.id)
                    .cloned()
                    .ok_or_else(|| {
                        handle_error(
                            ExtensionErrorCode::ClosedHandle,
                            "idempotent create refers to a closed agent",
                            "_echo_agent/agent/create",
                            &previous.agent,
                        )
                    })?;
            inner.agents.remove(&agent.id);
            if previous_record.config_fingerprint == config_fingerprint {
                return Ok((previous.agent.clone(), previous_record, false));
            }
            return Err(handle_error(
                ExtensionErrorCode::InvalidRequest,
                "idempotency id was already used with a different config",
                "_echo_agent/agent/create",
                &previous.agent,
            ));
        }
        if inner.idempotency.len() >= inner.max_handles
            && let Some(oldest) = inner.idempotency.keys().next().cloned()
        {
            inner.idempotency.remove(&oldest);
        }
        inner.idempotency.insert(
            idempotency_id.to_string(),
            Arc::new(AgentCreateOutcome {
                agent: agent.clone(),
            }),
        );
        Ok((agent, record, true))
    }

    /// Register a Session handle over an existing ACP Session.
    pub fn register_session_with_cwd(
        &self,
        acp_session_id: String,
        agent_handle_id: String,
        cwd: Option<std::path::PathBuf>,
    ) -> Result<(WireHandle, Arc<SessionRecord>), EchoSdkError> {
        let mut inner = self.lock();
        if !inner.agents.contains_key(&agent_handle_id) {
            let agent = handle(agent_handle_id.clone(), HandleKind::Agent, inner.generation);
            drop(inner);
            return Err(self.resolve_error(
                &agent,
                HandleKind::Agent,
                "_echo_agent/session/create",
            ));
        }
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::Session)?;
        let record = Arc::new(SessionRecord {
            acp_session_id,
            agent_handle_id,
            cwd,
        });
        inner.sessions.insert(id.clone(), record.clone());
        let session = self.insert(&mut inner, HandleKind::Session, id)?;
        Ok((session, record))
    }

    /// Register a Run handle over a live run entry.
    pub fn register_live_run(
        &self,
        run_id: String,
        entry: Arc<echo_agent::acp::RunEntry>,
        session_handle_id: Option<&str>,
    ) -> Result<(WireHandle, Arc<RunRecord>), EchoSdkError> {
        let mut inner = self.lock();
        if let Some(session_handle_id) = session_handle_id {
            let Some(session) = inner.sessions.get(session_handle_id) else {
                let session = handle(
                    session_handle_id.to_string(),
                    HandleKind::Session,
                    inner.generation,
                );
                drop(inner);
                return Err(self.resolve_error(
                    &session,
                    HandleKind::Session,
                    "_echo_agent/run/start",
                ));
            };
            if session.acp_session_id != entry.session_id.to_string() {
                return Err(sdk_error(
                    ExtensionErrorCode::InvalidRequest,
                    "run entry does not belong to the requested Session",
                    Retryability::Never,
                    "_echo_agent/run/start",
                ));
            }
        }
        self.enforce_budget(&mut inner, 1)?;
        if inner.runs.contains_key(&run_id) {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidRequest,
                "run id is already registered",
                Retryability::AfterDelay,
                "_echo_agent/run/start",
            ));
        }
        let record = Arc::new(RunRecord::Live { entry });
        inner.runs.insert(run_id.clone(), record.clone());
        let run = self.insert(&mut inner, HandleKind::Run, run_id)?;
        Ok((run, record))
    }

    /// Register a recovered (post-restart) run with fresh-generation
    /// handles. The stream handle reuses the durable envelope stream id.
    #[allow(clippy::too_many_arguments)]
    pub fn register_recovered_run(
        &self,
        run_id: String,
        stream_id: String,
        session_id: String,
        status: echo_sdk_protocol::methods::RunStatus,
        last_sequence: u64,
        terminal: Option<echo_sdk_protocol::methods::RunTerminal>,
        receipt: Option<echo_sdk_protocol::methods::RunReceiptWire>,
    ) -> Result<(WireHandle, WireHandle, Arc<RunRecord>), EchoSdkError> {
        let mut inner = self.lock();
        self.enforce_budget(&mut inner, 2)?;
        if inner.runs.contains_key(&run_id) || inner.streams.contains_key(&stream_id) {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidRequest,
                "recovered run or stream id is already registered",
                Retryability::AfterDelay,
                "_echo_agent/session/load",
            ));
        }
        let record = Arc::new(RunRecord::Recovered(Box::new(RecoveredRunRecord {
            stream_id: stream_id.clone(),
            session_id,
            status,
            last_sequence,
            terminal,
            receipt,
        })));
        inner.runs.insert(run_id.clone(), record.clone());
        inner.streams.insert(
            stream_id.clone(),
            Arc::new(StreamRecord {
                run_handle_id: run_id.clone(),
            }),
        );
        let run = self.insert(&mut inner, HandleKind::Run, run_id)?;
        let stream = self.insert(&mut inner, HandleKind::Stream, stream_id)?;
        Ok((run, stream, record))
    }

    /// Register the live event stream of an already-registered run.
    pub fn register_live_stream(
        &self,
        stream_id: String,
        run_handle_id: String,
    ) -> Result<WireHandle, EchoSdkError> {
        let mut inner = self.lock();
        self.enforce_budget(&mut inner, 1)?;
        if inner.streams.contains_key(&stream_id) {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidRequest,
                "stream id is already registered",
                Retryability::AfterDelay,
                "_echo_agent/run/start",
            ));
        }
        inner
            .streams
            .insert(stream_id.clone(), Arc::new(StreamRecord { run_handle_id }));
        self.insert(&mut inner, HandleKind::Stream, stream_id)
    }

    // ── Extension registrations ─────────────────────────────────────────

    /// Register one extension implementation. Registration is idempotent per
    /// `(kind, implementation_id)`: the same identity with the same
    /// descriptor fingerprint returns the same handle; a different descriptor
    /// is a typed conflict. Records are connection-owned and never persist.
    pub fn register_extension(
        &self,
        max_extensions: usize,
        max_descriptor_bytes: usize,
        kind: ExtensionKind,
        implementation_id: &str,
        descriptor: ExtensionDescriptor,
        timeout: Option<WireDuration>,
    ) -> Result<(WireHandle, Arc<ExtensionRecord>), EchoSdkError> {
        const OPERATION: &str = "_echo_agent/extension/register";
        if descriptor.kind() != kind {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                "descriptor kind does not match the registration kind",
                Retryability::Never,
                OPERATION,
            ));
        }
        let fingerprint = descriptor.fingerprint();
        let encoded_len = serde_json::to_vec(&descriptor)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if encoded_len > max_descriptor_bytes || encoded_len > MAX_EXTENSION_DESCRIPTOR_BYTES {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                "extension descriptor exceeds the serialized descriptor bound",
                Retryability::Never,
                OPERATION,
            ));
        }
        let identity = format!("{}/{}", kind.as_str(), implementation_id);
        let mut inner = self.lock();
        if let Some(existing_id) = inner.extension_index.get(&identity).cloned()
            && let Some(existing) = inner.extensions.get(&existing_id).cloned()
        {
            if existing.descriptor_fingerprint == fingerprint {
                let generation = inner.generation;
                return Ok((
                    handle(existing_id, HandleKind::Extension, generation),
                    existing,
                ));
            }
            let prior = handle(existing_id, HandleKind::Extension, inner.generation);
            drop(inner);
            return Err(handle_error(
                ExtensionErrorCode::ExtensionConflict,
                format!(
                    "implementation {identity} is already registered with a different descriptor"
                ),
                OPERATION,
                &prior,
            ));
        }
        if inner.extensions.len() >= max_extensions {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                format!("registered extension limit {max_extensions} reached"),
                Retryability::AfterDelay,
                OPERATION,
            ));
        }
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::Extension)?;
        let record = Arc::new(ExtensionRecord {
            kind,
            implementation_id: implementation_id.to_string(),
            descriptor,
            descriptor_fingerprint: fingerprint,
            timeout,
        });
        inner.extensions.insert(id.clone(), record.clone());
        inner.extension_index.insert(identity, id.clone());
        let extension = handle(id, HandleKind::Extension, inner.generation);
        Ok((extension, record))
    }

    /// Resolve one extension registration through the fixed ladder.
    pub fn extension(&self, handle: &WireHandle) -> Result<Arc<ExtensionRecord>, EchoSdkError> {
        let found = self.lock().extensions.get(&handle.id).cloned();
        found.ok_or_else(|| {
            self.resolve_error(handle, HandleKind::Extension, "_echo_agent/extension")
        })
    }

    /// Release one extension registration; idempotent. Returns false when it
    /// was already released, true when this call released it.
    pub fn close_extension(&self, handle: &WireHandle) -> Result<bool, EchoSdkError> {
        const OPERATION: &str = "_echo_agent/extension/unregister";
        let removed = {
            let mut inner = self.lock();
            let record = inner.extensions.remove(&handle.id);
            if let Some(record) = &record {
                let identity = format!("{}/{}", record.kind.as_str(), record.implementation_id);
                inner.extension_index.remove(&identity);
                // Cascade: drop callback streams minted for this extension.
                inner
                    .streams
                    .retain(|_, stream| stream.run_handle_id != handle.id);
            }
            record
        };
        let Some(record) = removed else {
            return if self.is_closed(handle) {
                Ok(false)
            } else {
                Err(self.resolve_error(handle, HandleKind::Extension, OPERATION))
            };
        };
        let _ = record;
        let mut inner = self.lock();
        inner
            .tombstones
            .push_back((HandleKind::Extension, handle.id.clone()));
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
        Ok(true)
    }

    /// Release every extension registration (connection teardown).
    pub fn close_all_extensions(&self) {
        let mut inner = self.lock();
        let drained: Vec<(String, String)> = inner
            .extensions
            .drain()
            .map(|(id, record)| {
                (
                    format!("{}/{}", record.kind.as_str(), record.implementation_id),
                    id,
                )
            })
            .collect();
        for (identity, id) in drained {
            inner.extension_index.remove(&identity);
            inner.tombstones.push_back((HandleKind::Extension, id));
        }
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
    }

    /// All live registrations of one kind, ordered by handle id.
    pub fn extensions_of_kind(
        &self,
        kind: ExtensionKind,
    ) -> Vec<(WireHandle, Arc<ExtensionRecord>)> {
        let inner = self.lock();
        let mut found: Vec<(WireHandle, Arc<ExtensionRecord>)> = inner
            .extensions
            .iter()
            .filter(|(_, record)| record.kind == kind)
            .map(|(id, record)| {
                (
                    handle(id.clone(), HandleKind::Extension, inner.generation),
                    record.clone(),
                )
            })
            .collect();
        found.sort_by(|left, right| left.0.id.cmp(&right.0.id));
        found
    }

    /// Mint one callback stream handle owned by an extension registration.
    /// Used by streaming reverse invocations; the SDK must echo this exact
    /// handle, so stream identities stay Host-minted and generation-fenced.
    pub fn register_extension_stream(
        &self,
        extension_id: &str,
    ) -> Result<WireHandle, EchoSdkError> {
        let mut inner = self.lock();
        if !inner.extensions.contains_key(extension_id) {
            return Err(sdk_error(
                ExtensionErrorCode::ClosedHandle,
                "extension registration is no longer live",
                Retryability::Never,
                "_echo_agent/extension/invoke",
            ));
        }
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::Stream)?;
        inner.streams.insert(
            id.clone(),
            Arc::new(StreamRecord {
                run_handle_id: extension_id.to_string(),
            }),
        );
        Ok(handle(id, HandleKind::Stream, inner.generation))
    }

    /// Release one callback stream handle after its single terminal.
    pub fn remove_extension_stream(&self, stream_id: &str) {
        let mut inner = self.lock();
        if inner.streams.remove(stream_id).is_some() {
            inner
                .tombstones
                .push_back((HandleKind::Stream, stream_id.to_string()));
            while inner.tombstones.len() > inner.max_handles {
                inner.tombstones.pop_front();
            }
        }
    }

    pub fn agent(&self, handle: &WireHandle) -> Result<Arc<AgentRecord>, EchoSdkError> {
        // Release the registry guard before error mapping: resolve_error
        // takes the same non-reentrant lock.
        let found = self.lock().agents.get(&handle.id).cloned();
        found.ok_or_else(|| self.resolve_error(handle, HandleKind::Agent, "_echo_agent/agent"))
    }

    pub fn agent_fingerprint(&self, handle: &WireHandle) -> Result<String, EchoSdkError> {
        Ok(self.agent(handle)?.config_fingerprint.clone())
    }

    pub fn session(&self, handle: &WireHandle) -> Result<Arc<SessionRecord>, EchoSdkError> {
        let found = self.lock().sessions.get(&handle.id).cloned();
        found.ok_or_else(|| self.resolve_error(handle, HandleKind::Session, "_echo_agent/session"))
    }

    pub fn run(&self, handle: &WireHandle) -> Result<Arc<RunRecord>, EchoSdkError> {
        let found = self.lock().runs.get(&handle.id).cloned();
        found.ok_or_else(|| self.resolve_error(handle, HandleKind::Run, "_echo_agent/run"))
    }

    pub fn stream(&self, handle: &WireHandle) -> Result<Arc<StreamRecord>, EchoSdkError> {
        let found = self.lock().streams.get(&handle.id).cloned();
        found
            .ok_or_else(|| self.resolve_error(handle, HandleKind::Stream, "_echo_agent/run/replay"))
    }

    pub fn session_handle_for_acp(&self, acp_session_id: &str) -> Option<WireHandle> {
        let inner = self.lock();
        inner.sessions.iter().find_map(|(id, record)| {
            (record.acp_session_id == acp_session_id)
                .then(|| handle(id.clone(), HandleKind::Session, inner.generation))
        })
    }

    pub fn run_handle_for_id(&self, run_id: &str) -> Option<WireHandle> {
        let inner = self.lock();
        inner
            .runs
            .contains_key(run_id)
            .then(|| handle(run_id.to_string(), HandleKind::Run, inner.generation))
    }

    pub fn stream_handle_for_id(&self, stream_id: &str) -> Option<WireHandle> {
        let inner = self.lock();
        inner
            .streams
            .contains_key(stream_id)
            .then(|| handle(stream_id.to_string(), HandleKind::Stream, inner.generation))
    }

    pub fn sessions_for_agent(&self, agent_handle_id: &str) -> Vec<(WireHandle, String)> {
        let inner = self.lock();
        inner
            .sessions
            .iter()
            .filter(|(_, record)| record.agent_handle_id == agent_handle_id)
            .map(|(id, record)| {
                (
                    handle(id.clone(), HandleKind::Session, inner.generation),
                    record.acp_session_id.clone(),
                )
            })
            .collect()
    }

    pub fn runs_for_session(&self, session_id: &str) -> Vec<String> {
        let inner = self.lock();
        inner
            .runs
            .iter()
            .filter_map(|(run_id, record)| {
                let matches = match record.as_ref() {
                    RunRecord::Live { entry } => entry.session_id.to_string() == session_id,
                    RunRecord::Recovered(recovered) => recovered.session_id == session_id,
                };
                matches.then(|| run_id.clone())
            })
            .collect()
    }

    pub fn remove_run(&self, run_id: &str) {
        let mut inner = self.lock();
        inner.runs.remove(run_id);
        inner
            .tombstones
            .push_back((HandleKind::Run, run_id.to_string()));
        if let Some(stream_id) = inner.streams.iter().find_map(|(stream_id, record)| {
            (record.run_handle_id == run_id).then(|| stream_id.clone())
        }) {
            inner.streams.remove(&stream_id);
            inner.tombstones.push_back((HandleKind::Stream, stream_id));
        }
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
    }

    /// Close an Agent handle. Idempotent: returns false when it was already
    /// closed, true when this call released it.
    pub fn close_agent(&self, handle: &WireHandle) -> Result<bool, EchoSdkError> {
        let removed = {
            let mut inner = self.lock();
            inner.agents.remove(&handle.id)
        };
        // The guard is dropped before error mapping (non-reentrant lock).
        let Some(record) = removed else {
            return if self.is_closed(handle) {
                Ok(false)
            } else {
                Err(self.resolve_error(handle, HandleKind::Agent, "_echo_agent/agent/close"))
            };
        };
        {
            let mut inner = self.lock();
            inner
                .tombstones
                .push_back((HandleKind::Agent, handle.id.clone()));
            while inner.tombstones.len() > inner.max_handles {
                inner.tombstones.pop_front();
            }
        }
        drop(record);
        Ok(true)
    }

    /// Close a Session handle (the Session's agent close is the caller's job
    /// through the shared registry).
    pub fn close_session(&self, handle: &WireHandle) -> Result<(bool, String), EchoSdkError> {
        let removed = {
            let mut inner = self.lock();
            inner.sessions.remove(&handle.id)
        };
        let Some(record) = removed else {
            return if self.is_closed(handle) {
                Ok((false, handle.id.clone()))
            } else {
                Err(self.resolve_error(handle, HandleKind::Session, "_echo_agent/session/close"))
            };
        };
        {
            let mut inner = self.lock();
            inner
                .tombstones
                .push_back((HandleKind::Session, handle.id.clone()));
            while inner.tombstones.len() > inner.max_handles {
                inner.tombstones.pop_front();
            }
        }
        let acp_session_id = record.acp_session_id.clone();
        Ok((true, acp_session_id))
    }

    /// Canonical fingerprint of a config payload for idempotency.
    pub fn config_fingerprint(config: &AgentConfigWire) -> String {
        let mut value = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);
        if let serde_json::Value::Object(root) = &mut value
            && root.get("variant").and_then(serde_json::Value::as_str) == Some("explicit")
            && let Some(serde_json::Value::Object(model)) = root.get_mut("model")
            && let Some(serde_json::Value::Object(credential)) = model.get_mut("credential")
            && credential.get("source").and_then(serde_json::Value::as_str) == Some("inline")
            && let Some(token) = credential.get_mut("token")
            && let Some(token_text) = token.as_str()
        {
            use sha2::{Digest as _, Sha256};
            let digest = Sha256::digest(token_text.as_bytes());
            *token = serde_json::Value::String(format!("sha256:{digest:x}"));
        }
        serde_json::to_string(&value).unwrap_or_else(|_| "<unencodable>".to_string())
    }

    /// Map a failed lookup to the fixed typed error ladder. The caller
    /// must NOT hold the registry lock (non-reentrant std Mutex).
    fn resolve_error(
        &self,
        handle: &WireHandle,
        expected: HandleKind,
        operation: &str,
    ) -> EchoSdkError {
        match Self::classify(&self.lock(), handle, expected) {
            ResolveError::InvalidShape(reason) => sdk_error(
                ExtensionErrorCode::InvalidValue,
                reason.to_string(),
                Retryability::Never,
                operation,
            ),
            ResolveError::Stale => handle_error(
                ExtensionErrorCode::StaleHandle,
                format!(
                    "handle generation {} predates Host generation {}",
                    handle.generation.to_u64().unwrap_or_default(),
                    self.lock().generation
                ),
                operation,
                handle,
            ),
            ResolveError::WrongKind => handle_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "handle kind {} does not address {}",
                    handle.kind.as_str(),
                    expected.as_str()
                ),
                operation,
                handle,
            ),
            ResolveError::Unknown => handle_error(
                ExtensionErrorCode::InvalidValue,
                "handle was never issued by this Host generation",
                operation,
                handle,
            ),
            ResolveError::Closed => handle_error(
                ExtensionErrorCode::ClosedHandle,
                "handle was already released by this Host generation",
                operation,
                handle,
            ),
        }
    }

    fn is_closed(&self, handle: &WireHandle) -> bool {
        let inner = self.lock();
        handle.generation.to_u64() == Some(inner.generation)
            && inner
                .tombstones
                .iter()
                .any(|(kind, id)| *kind == handle.kind && id == &handle.id)
    }

    fn classify(inner: &HandleInner, handle: &WireHandle, expected: HandleKind) -> ResolveError {
        if handle.validate().is_err() {
            return ResolveError::InvalidShape("handle id must be non-empty and bounded");
        }
        if handle.kind != expected {
            return ResolveError::WrongKind;
        }
        if handle.generation.to_u64().is_none() {
            return ResolveError::InvalidShape("handle generation is not a valid integer");
        }
        match handle.generation.to_u64() {
            Some(generation) if generation < inner.generation => return ResolveError::Stale,
            Some(generation) if generation > inner.generation => {
                return ResolveError::InvalidShape("handle generation is from the future");
            }
            _ => {}
        }
        if inner
            .tombstones
            .iter()
            .any(|(kind, id)| *kind == handle.kind && id == &handle.id)
        {
            return ResolveError::Closed;
        }
        ResolveError::Unknown
    }

    /// Shared generation/kind gate used before a family lookup.
    pub fn check_shape_and_generation(
        &self,
        handle: &WireHandle,
        expected: HandleKind,
        operation: &str,
    ) -> Result<(), EchoSdkError> {
        if handle.validate().is_err() {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                "handle id must be non-empty and bounded",
                Retryability::Never,
                operation,
            ));
        }
        if handle.kind != expected {
            return Err(handle_error(
                ExtensionErrorCode::InvalidValue,
                format!(
                    "handle kind {} does not address {}",
                    handle.kind.as_str(),
                    expected.as_str()
                ),
                operation,
                handle,
            ));
        }
        let generation = self.generation();
        match handle.generation.to_u64() {
            None => {
                return Err(sdk_error(
                    ExtensionErrorCode::InvalidValue,
                    "handle generation is not a valid integer",
                    Retryability::Never,
                    operation,
                ));
            }
            Some(present) if present < generation => {
                return Err(handle_error(
                    ExtensionErrorCode::StaleHandle,
                    format!("handle generation {present} predates Host generation {generation}"),
                    operation,
                    handle,
                ));
            }
            Some(present) if present > generation => {
                return Err(handle_error(
                    ExtensionErrorCode::InvalidValue,
                    format!(
                        "handle generation {present} is from a future Host generation {generation}"
                    ),
                    operation,
                    handle,
                ));
            }
            Some(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_sdk_protocol::methods::{
        AgentConfigExplicitWire, AgentSettingsWire, CredentialSourceWire, LlmApiProtocolWire,
        ModelConfigWire,
    };
    use echo_sdk_protocol::scalar::WireU64;

    fn registry() -> HandleRegistry {
        HandleRegistry::new(7, 16)
    }

    fn run_handle(id: &str, generation: u64, kind: HandleKind) -> WireHandle {
        WireHandle {
            id: id.to_string(),
            generation: WireU64::from_u64(generation),
            kind,
        }
    }

    #[test]
    fn unknown_current_generation_ids_are_invalid_not_stale() {
        let registry = registry();
        let handle = run_handle("agent-never", 7, HandleKind::Agent);
        let code = registry.agent(&handle).err().map(|error| error.code);
        assert_eq!(code, Some(ExtensionErrorCode::InvalidValue));
    }

    #[test]
    fn stale_generation_is_reported_before_unknown_ids() {
        let registry = registry();
        let handle = run_handle("agent-1", 6, HandleKind::Agent);
        assert!(
            registry
                .check_shape_and_generation(&handle, HandleKind::Agent, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::StaleHandle)
        );
    }

    #[test]
    fn wrong_kind_is_a_typed_invalid_value() {
        let registry = registry();
        let handle = run_handle("sess-1", 7, HandleKind::Session);
        assert!(
            registry
                .check_shape_and_generation(&handle, HandleKind::Run, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::InvalidValue)
        );
    }

    #[test]
    fn empty_ids_fail_shape_first() {
        let registry = registry();
        let handle = run_handle("  ", 7, HandleKind::Run);
        assert!(
            registry
                .check_shape_and_generation(&handle, HandleKind::Run, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::InvalidValue)
        );
    }

    #[test]
    fn config_fingerprint_redacts_inline_credentials() {
        let config = AgentConfigWire::Explicit(Box::new(AgentConfigExplicitWire {
            config_version: 1,
            model: ModelConfigWire {
                provider: "local".to_string(),
                name: "model".to_string(),
                base_url: "http://127.0.0.1".to_string(),
                api_protocol: LlmApiProtocolWire::ChatCompletions,
                credential: Some(CredentialSourceWire::Inline {
                    token: "secret-token".to_string(),
                }),
                max_tokens: None,
                temperature: None,
                context_window: None,
            },
            agent: AgentSettingsWire {
                name: "agent".to_string(),
                system_prompt: "system".to_string(),
                max_iterations: 1,
            },
        }));
        let fingerprint = HandleRegistry::config_fingerprint(&config);
        assert!(!fingerprint.contains("secret-token"));
        assert!(fingerprint.contains("sha256:"));
    }
}
