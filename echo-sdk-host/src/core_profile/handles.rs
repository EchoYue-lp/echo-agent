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
    /// Connection-owned TaskRun handle issued atomically with this Session.
    pub task_run_handle_id: String,
}

/// One connection-owned task graph scope. The opaque handle id is distinct
/// from the framework scope id so a client cannot manufacture a TaskRun by
/// copying an ACP Session id.
pub(crate) struct TaskRunRecord {
    #[allow(dead_code)]
    pub session_handle_id: String,
    #[allow(dead_code)]
    pub acp_session_id: String,
}

/// One issued PlanTask address within a TaskRun graph.
pub(crate) struct PlanTaskRecord {
    pub task_run_handle_id: String,
    pub task_id: String,
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

#[allow(dead_code)]
pub(crate) struct StreamRecord {
    pub run_handle_id: String,
    /// Facade streams are owned by a Session/resource and share this record
    /// with ACP run and extension streams. The optional fields are `None` for
    /// those existing stream kinds; a facade stream is identified by
    /// `facade == true` and is always resolved through this registry.
    pub owner_session: Option<String>,
    pub resource_id: Option<String>,
    pub facade: bool,
    state: Mutex<StreamState>,
}

#[derive(Debug, Default)]
#[allow(dead_code)]
struct StreamState {
    last_sequence: u64,
    cancelled: bool,
    terminal: bool,
}

/// One open facade resource (memory namespace, workflow, journal, ledger,
/// run store, MCP/A2A client, …). The record carries addressing only —
/// the business object stays with the owning Rust service (plan 07 todo 2).
#[allow(dead_code)]
pub(crate) struct FacadeResourceRecord {
    /// Family the resource was opened through.
    pub family: String,
    /// Family-defined canonical resource type (e.g. `memory.namespace`).
    pub resource_type: String,
    /// Owning session handle id, when the resource is session-scoped.
    pub owner_session: Option<String>,
}

/// One registered extension implementation. The record is connection-owned:
/// it exists only while the registering connection lives and is never
/// persisted or restored across a Host restart (design §12.1).
#[allow(dead_code)]
pub(crate) struct ExtensionRecord {
    pub kind: ExtensionKind,
    pub implementation_id: String,
    pub descriptor: ExtensionDescriptor,
    /// Canonical descriptor fingerprint for idempotent re-registration.
    pub descriptor_fingerprint: String,
    /// Per-registration default invocation deadline.
    pub timeout: Option<WireDuration>,
    /// Monotonic registration order used when a kind has one active winner.
    pub registration_order: u64,
    /// Factory-created CustomAgent instances are invocation-scoped and do not
    /// participate in the direct-registration logical-name namespace.
    pub factory_instance: bool,
}

struct HandleInner {
    generation: u64,
    max_handles: usize,
    agents: HashMap<String, Arc<AgentRecord>>,
    sessions: HashMap<String, Arc<SessionRecord>>,
    task_runs: HashMap<String, Arc<TaskRunRecord>>,
    plan_tasks: HashMap<String, Arc<PlanTaskRecord>>,
    plan_task_index: HashMap<(String, String), String>,
    runs: HashMap<String, Arc<RunRecord>>,
    streams: HashMap<String, Arc<StreamRecord>>,
    extensions: HashMap<String, Arc<ExtensionRecord>>,
    facade_resources: HashMap<String, Arc<FacadeResourceRecord>>,
    /// `(kind, implementation_id)` identity → extension handle id, for
    /// idempotent re-registration and typed conflicts.
    #[allow(dead_code)]
    extension_index: HashMap<String, String>,
    tombstones: VecDeque<(HandleKind, String)>,
    idempotency: HashMap<String, Arc<AgentCreateOutcome>>,
    next_agent_id: u64,
    next_extension_order: u64,
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

#[derive(Clone, Copy)]
pub(crate) struct ExtensionRegistrationLimits {
    pub max_extensions: usize,
    pub max_descriptor_bytes: usize,
}

fn extension_semantic_identity_conflicts(
    existing: &ExtensionDescriptor,
    candidate: &ExtensionDescriptor,
) -> bool {
    match (existing, candidate) {
        (
            ExtensionDescriptor::Tool { name: existing, .. },
            ExtensionDescriptor::Tool {
                name: candidate, ..
            },
        )
        | (
            ExtensionDescriptor::CustomAgent { name: existing, .. },
            ExtensionDescriptor::CustomAgent {
                name: candidate, ..
            },
        ) => existing == candidate,
        (
            ExtensionDescriptor::ChannelPlugin(existing),
            ExtensionDescriptor::ChannelPlugin(candidate),
        ) => existing.channel_id == candidate.channel_id,
        (
            ExtensionDescriptor::ChannelMessageHandler(existing),
            ExtensionDescriptor::ChannelMessageHandler(candidate),
        ) => existing.handler_id == candidate.handler_id,
        (ExtensionDescriptor::Store { .. }, ExtensionDescriptor::Store { .. })
        | (
            ExtensionDescriptor::HumanLoopProvider { .. },
            ExtensionDescriptor::HumanLoopProvider { .. },
        ) => true,
        _ => false,
    }
}

impl HandleRegistry {
    pub fn new(generation: u64, max_handles: usize) -> Self {
        Self {
            inner: Mutex::new(HandleInner {
                generation,
                max_handles,
                agents: HashMap::new(),
                sessions: HashMap::new(),
                task_runs: HashMap::new(),
                plan_tasks: HashMap::new(),
                plan_task_index: HashMap::new(),
                runs: HashMap::new(),
                streams: HashMap::new(),
                extensions: HashMap::new(),
                facade_resources: HashMap::new(),
                extension_index: HashMap::new(),
                tombstones: VecDeque::new(),
                idempotency: HashMap::new(),
                next_agent_id: 0,
                next_extension_order: 0,
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
            .saturating_add(inner.task_runs.len())
            .saturating_add(inner.plan_tasks.len())
            .saturating_add(inner.runs.len())
            .saturating_add(inner.streams.len())
            .saturating_add(inner.extensions.len())
            .saturating_add(inner.facade_resources.len());
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

    /// Register a Session handle and its one TaskRun handle over an existing
    /// ACP Session. Both are committed under one registry lock.
    pub fn register_session_with_cwd(
        &self,
        acp_session_id: String,
        agent_handle_id: String,
        cwd: Option<std::path::PathBuf>,
    ) -> Result<(WireHandle, WireHandle, Arc<SessionRecord>), EchoSdkError> {
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
        self.enforce_budget(&mut inner, 2)?;
        let id = Self::mint_id(&mut inner, HandleKind::Session)?;
        let task_run_id = Self::mint_id(&mut inner, HandleKind::TaskRun)?;
        let record = Arc::new(SessionRecord {
            acp_session_id: acp_session_id.clone(),
            agent_handle_id,
            cwd,
            task_run_handle_id: task_run_id.clone(),
        });
        inner.sessions.insert(id.clone(), record.clone());
        inner.task_runs.insert(
            task_run_id.clone(),
            Arc::new(TaskRunRecord {
                session_handle_id: id.clone(),
                acp_session_id,
            }),
        );
        let session = self.insert(&mut inner, HandleKind::Session, id)?;
        let task_run = self.insert(&mut inner, HandleKind::TaskRun, task_run_id)?;
        Ok((session, task_run, record))
    }

    /// Resolve an issued TaskRun through the fixed handle ladder.
    #[allow(dead_code)]
    pub fn task_run(&self, handle: &WireHandle) -> Result<Arc<TaskRunRecord>, EchoSdkError> {
        self.check_shape_and_generation(handle, HandleKind::TaskRun, "_echo_agent/task")?;
        let found = self.lock().task_runs.get(&handle.id).cloned();
        found.ok_or_else(|| self.resolve_error(handle, HandleKind::TaskRun, "_echo_agent/task"))
    }

    /// Return the already-issued TaskRun belonging to a Session.
    #[allow(dead_code)]
    pub fn task_run_for_session(&self, session: &WireHandle) -> Result<WireHandle, EchoSdkError> {
        let record = self.session(session)?;
        Ok(handle(
            record.task_run_handle_id.clone(),
            HandleKind::TaskRun,
            self.generation(),
        ))
    }

    /// Issue or reuse the canonical PlanTask handle for one framework task
    /// identity. Re-listing a graph never creates a second address.
    #[allow(dead_code)]
    pub fn register_plan_task(
        &self,
        task_run: &WireHandle,
        task_id: &str,
        operation: &str,
    ) -> Result<WireHandle, EchoSdkError> {
        let task_run_record = self.task_run(task_run)?;
        if task_id.trim().is_empty() {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                "PlanTask identity must be non-empty",
                Retryability::Never,
                operation,
            ));
        }
        let mut inner = self.lock();
        let key = (task_run.id.clone(), task_id.to_string());
        if let Some(existing_id) = inner.plan_task_index.get(&key).cloned() {
            return Ok(handle(existing_id, HandleKind::PlanTask, inner.generation));
        }
        if !inner.task_runs.contains_key(&task_run.id) {
            drop(inner);
            return Err(self.resolve_error(task_run, HandleKind::TaskRun, operation));
        }
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::PlanTask)?;
        inner.plan_tasks.insert(
            id.clone(),
            Arc::new(PlanTaskRecord {
                task_run_handle_id: task_run.id.clone(),
                task_id: task_id.to_string(),
            }),
        );
        inner.plan_task_index.insert(key, id.clone());
        drop(task_run_record);
        Ok(handle(id, HandleKind::PlanTask, inner.generation))
    }

    /// Resolve a PlanTask and prove it belongs to the supplied TaskRun.
    #[allow(dead_code)]
    pub fn plan_task_for_run(
        &self,
        handle: &WireHandle,
        task_run: &WireHandle,
        operation: &str,
    ) -> Result<Arc<PlanTaskRecord>, EchoSdkError> {
        self.check_shape_and_generation(handle, HandleKind::PlanTask, operation)?;
        self.check_shape_and_generation(task_run, HandleKind::TaskRun, operation)?;
        let found = self.lock().plan_tasks.get(&handle.id).cloned();
        let record =
            found.ok_or_else(|| self.resolve_error(handle, HandleKind::PlanTask, operation))?;
        if record.task_run_handle_id != task_run.id {
            return Err(handle_error(
                ExtensionErrorCode::InvalidValue,
                "PlanTask belongs to another TaskRun",
                operation,
                handle,
            ));
        }
        Ok(record)
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
                owner_session: None,
                resource_id: None,
                facade: false,
                state: Mutex::new(StreamState {
                    last_sequence,
                    cancelled: false,
                    terminal: false,
                }),
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
        inner.streams.insert(
            stream_id.clone(),
            Arc::new(StreamRecord {
                run_handle_id,
                owner_session: None,
                resource_id: None,
                facade: false,
                state: Mutex::new(StreamState::default()),
            }),
        );
        self.insert(&mut inner, HandleKind::Stream, stream_id)
    }

    // ── Extension registrations ─────────────────────────────────────────

    /// Register one extension implementation. Registration is idempotent per
    /// `(kind, implementation_id)`: the same identity with the same
    /// registration fingerprint (descriptor plus default timeout) returns the
    /// same handle; a different snapshot is a typed conflict. Records are
    /// connection-owned and never persist.
    #[allow(dead_code)]
    pub fn register_extension(
        &self,
        limits: ExtensionRegistrationLimits,
        kind: ExtensionKind,
        implementation_id: &str,
        descriptor: ExtensionDescriptor,
        timeout: Option<WireDuration>,
        factory_instance: bool,
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
        let fingerprint = serde_json::to_string(&serde_json::json!({
            "descriptor": &descriptor,
            "timeout": &timeout,
        }))
        .unwrap_or_else(|_| "<unencodable>".to_string());
        let encoded_len = serde_json::to_vec(&descriptor)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if encoded_len > limits.max_descriptor_bytes || encoded_len > MAX_EXTENSION_DESCRIPTOR_BYTES
        {
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
        if !factory_instance
            && let Some((existing_id, existing)) = inner.extensions.iter().find(|(_, existing)| {
                !existing.factory_instance
                    && extension_semantic_identity_conflicts(&existing.descriptor, &descriptor)
            })
        {
            let prior = handle(existing_id.clone(), HandleKind::Extension, inner.generation);
            let existing_identity =
                format!("{}/{}", existing.kind.as_str(), existing.implementation_id);
            drop(inner);
            return Err(handle_error(
                ExtensionErrorCode::ExtensionConflict,
                format!("extension semantic identity is already owned by {existing_identity}"),
                OPERATION,
                &prior,
            ));
        }
        if inner.extensions.len() >= limits.max_extensions {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                format!(
                    "registered extension limit {} reached",
                    limits.max_extensions
                ),
                Retryability::AfterDelay,
                OPERATION,
            ));
        }
        self.enforce_budget(&mut inner, 1)?;
        inner.next_extension_order =
            inner.next_extension_order.checked_add(1).ok_or_else(|| {
                sdk_error(
                    ExtensionErrorCode::PayloadTooLarge,
                    "extension registration order exhausted",
                    Retryability::Never,
                    OPERATION,
                )
            })?;
        let registration_order = inner.next_extension_order;
        let id = Self::mint_id(&mut inner, HandleKind::Extension)?;
        let record = Arc::new(ExtensionRecord {
            kind,
            implementation_id: implementation_id.to_string(),
            descriptor,
            descriptor_fingerprint: fingerprint,
            timeout,
            registration_order,
            factory_instance,
        });
        inner.extensions.insert(id.clone(), record.clone());
        inner.extension_index.insert(identity, id.clone());
        let extension = handle(id, HandleKind::Extension, inner.generation);
        Ok((extension, record))
    }

    /// Resolve one extension registration through the fixed ladder.
    #[allow(dead_code)]
    pub fn extension(&self, handle: &WireHandle) -> Result<Arc<ExtensionRecord>, EchoSdkError> {
        let found = self.lock().extensions.get(&handle.id).cloned();
        found.ok_or_else(|| {
            self.resolve_error(handle, HandleKind::Extension, "_echo_agent/extension")
        })
    }

    /// Release one extension registration; idempotent. Returns false when it
    /// was already released, true when this call released it.
    #[allow(dead_code)]
    pub fn close_extension(&self, handle: &WireHandle) -> Result<bool, EchoSdkError> {
        const OPERATION: &str = "_echo_agent/extension/unregister";
        let removed = {
            let mut inner = self.lock();
            let record = inner.extensions.remove(&handle.id);
            if let Some(record) = &record {
                let identity = format!("{}/{}", record.kind.as_str(), record.implementation_id);
                inner.extension_index.remove(&identity);
                // Cascade: drop callback streams minted for this extension.
                let stream_ids = inner
                    .streams
                    .iter()
                    .filter(|(_, stream)| stream.run_handle_id == handle.id)
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>();
                for stream_id in stream_ids {
                    inner.streams.remove(&stream_id);
                    inner.tombstones.push_back((HandleKind::Stream, stream_id));
                }
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
    #[allow(dead_code)]
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
        let extension_ids = drained
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<std::collections::HashSet<_>>();
        for (identity, id) in drained {
            inner.extension_index.remove(&identity);
            inner.tombstones.push_back((HandleKind::Extension, id));
        }
        let stream_ids = inner
            .streams
            .iter()
            .filter(|(_, stream)| extension_ids.contains(&stream.run_handle_id))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for stream_id in stream_ids {
            inner.streams.remove(&stream_id);
            inner.tombstones.push_back((HandleKind::Stream, stream_id));
        }
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
    }

    /// All live registrations of one kind, ordered by monotonic registration
    /// order. This remains deterministic even though handle ids are opaque.
    #[allow(dead_code)]
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
        found.sort_by_key(|(_, record)| record.registration_order);
        found
    }

    /// Registrations eligible for injection into newly constructed Session
    /// Agents. Factory-created CustomAgent instances are owned by one
    /// invocation and must never become globally visible to another Session.
    #[allow(dead_code)]
    pub fn session_extensions_of_kind(
        &self,
        kind: ExtensionKind,
    ) -> Vec<(WireHandle, Arc<ExtensionRecord>)> {
        self.extensions_of_kind(kind)
            .into_iter()
            .filter(|(_, record)| !record.factory_instance)
            .collect()
    }

    /// Mint one callback stream handle owned by an extension registration.
    /// Used by streaming reverse invocations; the SDK must echo this exact
    /// handle, so stream identities stay Host-minted and generation-fenced.
    #[allow(dead_code)]
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
                owner_session: None,
                resource_id: None,
                facade: false,
                state: Mutex::new(StreamState::default()),
            }),
        );
        Ok(handle(id, HandleKind::Stream, inner.generation))
    }

    /// Release one callback stream handle after its single terminal.
    #[allow(dead_code)]
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

    // ── Facade resources ───────────────────────────────────────────────
    // Family handlers (todos 3-5) open and close every resource through
    // this single ladder; the admission ladder itself never allocates
    // resources. One global map and one advertised bound
    // (`max_facade_resources`) means the total open resource count across
    // all families can never exceed the `initialize` advertisement.
    /// Open one facade resource handle (workflow graph, ledger, run store,
    /// MCP manager, …). The id is minted once, generation-fenced and never
    /// rebound; the business object stays with the owning service.
    #[allow(dead_code)]
    pub fn register_facade_resource(
        &self,
        max_facade_resources: usize,
        family: &str,
        resource_type: &str,
        owner_session: Option<&str>,
        operation: &str,
    ) -> Result<(WireHandle, Arc<FacadeResourceRecord>), EchoSdkError> {
        let mut inner = self.lock();
        if inner.facade_resources.len() >= max_facade_resources {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                format!("open facade resource limit {max_facade_resources} reached"),
                Retryability::AfterDelay,
                operation,
            ));
        }
        if let Some(owner) = owner_session
            // The owner is an ACP session id; session handle ids are minted
            // independently, so match against the session records.
            && !inner
                .sessions
                .values()
                .any(|record| record.acp_session_id == owner)
        {
            let session = handle(owner.to_string(), HandleKind::Session, inner.generation);
            drop(inner);
            return Err(self.resolve_error(&session, HandleKind::Session, operation));
        }
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::FacadeResource)?;
        let record = Arc::new(FacadeResourceRecord {
            family: family.to_string(),
            resource_type: resource_type.to_string(),
            owner_session: owner_session.map(str::to_string),
        });
        inner.facade_resources.insert(id.clone(), record.clone());
        Ok((
            handle(id, HandleKind::FacadeResource, inner.generation),
            record,
        ))
    }

    /// Resolve one facade resource through the fixed ladder
    /// (shape → kind → generation → issued/closed).
    pub fn facade_resource(
        &self,
        handle: &WireHandle,
        operation: &str,
    ) -> Result<Arc<FacadeResourceRecord>, EchoSdkError> {
        self.check_shape_and_generation(handle, HandleKind::FacadeResource, operation)?;
        let found = self.lock().facade_resources.get(&handle.id).cloned();
        found.ok_or_else(|| self.resolve_error(handle, HandleKind::FacadeResource, operation))
    }

    /// Close one Host-issued facade resource through the same generation and
    /// tombstone authority used by session/connection teardown.
    #[allow(dead_code)]
    pub fn close_facade_resource(
        &self,
        handle: &WireHandle,
        operation: &str,
    ) -> Result<bool, EchoSdkError> {
        self.check_shape_and_generation(handle, HandleKind::FacadeResource, operation)?;
        let mut inner = self.lock();
        if inner.facade_resources.remove(&handle.id).is_none() {
            // Do not resolve while holding `inner`: `resolve_error` acquires
            // the same non-reentrant mutex. Closed resource handles are
            // idempotent, matching the stream close contract.
            drop(inner);
            return if self.is_closed(handle) {
                Ok(false)
            } else {
                Err(self.resolve_error(handle, HandleKind::FacadeResource, operation))
            };
        }

        // Resource and all of its facade streams are released under one
        // registry lock. A concurrent resolve therefore observes either the
        // complete live set or the complete tombstoned set, never a dangling
        // stream whose resource has already been closed.
        let stream_ids = inner
            .streams
            .iter()
            .filter(|(_, record)| {
                record.facade && record.resource_id.as_deref() == Some(handle.id.as_str())
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for stream_id in stream_ids {
            inner.streams.remove(&stream_id);
            inner.tombstones.push_back((HandleKind::Stream, stream_id));
        }
        inner
            .tombstones
            .push_back((HandleKind::FacadeResource, handle.id.clone()));
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
        Ok(true)
    }

    /// Open a facade data/event stream over an issued facade resource. The
    /// stream uses the same generation-fenced `Stream` handle authority as
    /// run replay and extension callbacks; only its lifecycle metadata is
    /// facade-specific.
    #[allow(dead_code)]
    pub fn register_facade_stream(
        &self,
        max_facade_streams: usize,
        resource: &WireHandle,
        owner_session: Option<&str>,
        operation: &str,
    ) -> Result<WireHandle, EchoSdkError> {
        let resource_record = self.facade_resource(resource, operation)?;
        if resource_record.owner_session.as_deref() != owner_session {
            return Err(sdk_error(
                ExtensionErrorCode::InvalidValue,
                "facade stream owner does not match its resource",
                Retryability::Never,
                operation,
            ));
        }
        let mut inner = self.lock();
        let open_facade_streams = inner
            .streams
            .values()
            .filter(|record| record.facade)
            .count();
        if open_facade_streams >= max_facade_streams {
            return Err(sdk_error(
                ExtensionErrorCode::PayloadTooLarge,
                format!("open facade stream limit {max_facade_streams} reached"),
                Retryability::AfterDelay,
                operation,
            ));
        }
        self.enforce_budget(&mut inner, 1)?;
        let id = Self::mint_id(&mut inner, HandleKind::Stream)?;
        inner.streams.insert(
            id.clone(),
            Arc::new(StreamRecord {
                run_handle_id: resource.id.clone(),
                owner_session: owner_session.map(str::to_string),
                resource_id: Some(resource.id.clone()),
                facade: true,
                state: Mutex::new(StreamState::default()),
            }),
        );
        self.insert(&mut inner, HandleKind::Stream, id)
    }

    /// Resolve an issued facade stream and enforce its optional Session
    /// owner. Run and extension streams cannot be reinterpreted as facade
    /// streams even though all three share the wire `Stream` kind.
    #[allow(dead_code)]
    pub fn facade_stream(
        &self,
        handle: &WireHandle,
        owner_session: Option<&str>,
        operation: &str,
    ) -> Result<Arc<StreamRecord>, EchoSdkError> {
        self.check_shape_and_generation(handle, HandleKind::Stream, operation)?;
        let found = self.lock().streams.get(&handle.id).cloned();
        let record =
            found.ok_or_else(|| self.resolve_error(handle, HandleKind::Stream, operation))?;
        if !record.facade {
            return Err(handle_error(
                ExtensionErrorCode::InvalidValue,
                "stream handle does not address a facade stream",
                operation,
                handle,
            ));
        }
        if record.owner_session.as_deref() != owner_session {
            return Err(handle_error(
                ExtensionErrorCode::InvalidValue,
                "facade stream belongs to another Session",
                operation,
                handle,
            ));
        }
        Ok(record)
    }

    /// Return the facade stream's highest delivered sequence and
    /// cancellation flag after full handle/owner validation.
    #[allow(dead_code)]
    pub fn facade_stream_state(
        &self,
        handle: &WireHandle,
        owner_session: Option<&str>,
        operation: &str,
    ) -> Result<(u64, bool), EchoSdkError> {
        let record = self.facade_stream(handle, owner_session, operation)?;
        let state = record
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Ok((state.last_sequence, state.cancelled))
    }

    /// Advance a facade stream watermark. Sequences are strictly monotonic
    /// and a cancelled stream never accepts another item.
    #[allow(dead_code)]
    pub fn advance_facade_stream(
        &self,
        handle: &WireHandle,
        owner_session: Option<&str>,
        sequence: u64,
        operation: &str,
    ) -> Result<(), EchoSdkError> {
        let record = self.facade_stream(handle, owner_session, operation)?;
        let mut state = record
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.cancelled {
            return Err(handle_error(
                ExtensionErrorCode::Cancelled,
                "facade stream is cancelled",
                operation,
                handle,
            ));
        }
        if state.terminal {
            return Err(handle_error(
                ExtensionErrorCode::ClosedHandle,
                "facade stream already reached its terminal",
                operation,
                handle,
            ));
        }
        if sequence <= state.last_sequence {
            return Err(handle_error(
                ExtensionErrorCode::InvalidRequest,
                format!(
                    "facade stream sequence {sequence} does not advance past {}",
                    state.last_sequence
                ),
                operation,
                handle,
            ));
        }
        state.last_sequence = sequence;
        Ok(())
    }

    /// Atomically allocate the final sequence and mark the facade stream
    /// terminal. This is the only terminal transition; the caller can then
    /// return the terminal event and tombstone the handle without another
    /// consumer racing a duplicate terminal.
    pub fn settle_facade_stream(
        &self,
        handle: &WireHandle,
        owner_session: Option<&str>,
        cancelled: bool,
        operation: &str,
    ) -> Result<u64, EchoSdkError> {
        let record = self.facade_stream(handle, owner_session, operation)?;
        let mut state = record
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.terminal {
            return Err(handle_error(
                ExtensionErrorCode::ClosedHandle,
                "facade stream already reached its terminal",
                operation,
                handle,
            ));
        }
        let sequence = state.last_sequence.checked_add(1).ok_or_else(|| {
            handle_error(
                ExtensionErrorCode::FrameworkError,
                "facade stream sequence exhausted",
                operation,
                handle,
            )
        })?;
        state.last_sequence = sequence;
        state.cancelled |= cancelled;
        state.terminal = true;
        Ok(sequence)
    }

    /// Cooperatively cancel a facade stream. Repeated cancellation is an
    /// idempotent `false`; invalid, stale and foreign handles still fail the
    /// common ladder.
    #[allow(dead_code)]
    pub fn cancel_facade_stream(
        &self,
        handle: &WireHandle,
        owner_session: Option<&str>,
        operation: &str,
    ) -> Result<bool, EchoSdkError> {
        let record = self.facade_stream(handle, owner_session, operation)?;
        let mut state = record
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.cancelled {
            Ok(false)
        } else {
            state.cancelled = true;
            Ok(true)
        }
    }

    /// Close one facade stream. Repeated close is idempotent; a handle that
    /// was never issued still fails instead of being treated as closed.
    #[allow(dead_code)]
    pub fn close_facade_stream(
        &self,
        handle: &WireHandle,
        owner_session: Option<&str>,
        operation: &str,
    ) -> Result<bool, EchoSdkError> {
        self.check_shape_and_generation(handle, HandleKind::Stream, operation)?;
        let removed = {
            let mut inner = self.lock();
            match inner.streams.get(&handle.id) {
                Some(record)
                    if record.facade && record.owner_session.as_deref() == owner_session =>
                {
                    inner.streams.remove(&handle.id)
                }
                Some(record) if !record.facade => {
                    return Err(handle_error(
                        ExtensionErrorCode::InvalidValue,
                        "stream handle does not address a facade stream",
                        operation,
                        handle,
                    ));
                }
                Some(_) => {
                    return Err(handle_error(
                        ExtensionErrorCode::InvalidValue,
                        "facade stream belongs to another Session",
                        operation,
                        handle,
                    ));
                }
                None => None,
            }
        };
        if removed.is_none() {
            return if self.is_closed(handle) {
                Ok(false)
            } else {
                Err(self.resolve_error(handle, HandleKind::Stream, operation))
            };
        }
        let mut inner = self.lock();
        inner
            .tombstones
            .push_back((HandleKind::Stream, handle.id.clone()));
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
        Ok(true)
    }

    /// Close every facade stream owned by one facade resource. This is the
    /// resource-close cascade; run and extension streams are unaffected.
    #[allow(dead_code)]
    pub fn close_facade_streams_of_resource(&self, resource_id: &str) -> usize {
        let mut inner = self.lock();
        let ids = inner
            .streams
            .iter()
            .filter(|(_, record)| {
                record.facade && record.resource_id.as_deref() == Some(resource_id)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in &ids {
            inner.streams.remove(id);
            inner.tombstones.push_back((HandleKind::Stream, id.clone()));
        }
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
        ids.len()
    }

    #[allow(dead_code)]
    pub fn facade_stream_count_of_resource(&self, resource_id: &str) -> usize {
        self.lock()
            .streams
            .values()
            .filter(|record| record.facade && record.resource_id.as_deref() == Some(resource_id))
            .count()
    }

    /// Close every facade resource owned by one session (session close
    /// cascade). Returns the closed handle ids so the facade runtime can
    /// cascade its stream bookkeeping.
    #[allow(dead_code)]
    pub fn close_facade_resources_of(&self, owner: &str) -> Vec<String> {
        let mut inner = self.lock();
        let owned: Vec<String> = inner
            .facade_resources
            .iter()
            .filter(|(_, record)| record.owner_session.as_deref() == Some(owner))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &owned {
            inner.facade_resources.remove(id);
            inner
                .tombstones
                .push_back((HandleKind::FacadeResource, id.clone()));
        }
        let stream_ids = inner
            .streams
            .iter()
            .filter(|(_, record)| record.facade && record.owner_session.as_deref() == Some(owner))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in stream_ids {
            inner.streams.remove(&id);
            inner.tombstones.push_back((HandleKind::Stream, id));
        }
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
        owned
    }

    /// Close every open facade resource (connection teardown).
    #[allow(dead_code)]
    pub fn close_all_facade_resources(&self) -> Vec<String> {
        let mut inner = self.lock();
        let closed: Vec<String> = inner.facade_resources.keys().cloned().collect();
        for id in &closed {
            inner
                .tombstones
                .push_back((HandleKind::FacadeResource, id.clone()));
        }
        inner.facade_resources.clear();
        let stream_ids = inner
            .streams
            .iter()
            .filter(|(_, record)| record.facade)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in stream_ids {
            inner.streams.remove(&id);
            inner.tombstones.push_back((HandleKind::Stream, id));
        }
        while inner.tombstones.len() > inner.max_handles {
            inner.tombstones.pop_front();
        }
        closed
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
            let task_run_id = record.task_run_handle_id.clone();
            let plan_task_ids = inner
                .plan_tasks
                .iter()
                .filter(|(_, plan_task)| plan_task.task_run_handle_id == task_run_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for plan_task_id in plan_task_ids {
                if let Some(plan_task) = inner.plan_tasks.remove(&plan_task_id) {
                    inner.plan_task_index.remove(&(
                        plan_task.task_run_handle_id.clone(),
                        plan_task.task_id.clone(),
                    ));
                }
                inner
                    .tombstones
                    .push_back((HandleKind::PlanTask, plan_task_id));
            }
            if inner.task_runs.remove(&task_run_id).is_some() {
                inner
                    .tombstones
                    .push_back((HandleKind::TaskRun, task_run_id));
            }
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

    pub(crate) fn is_closed(&self, handle: &WireHandle) -> bool {
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

    fn tool_descriptor(name: &str) -> ExtensionDescriptor {
        ExtensionDescriptor::Tool {
            descriptor_version: 1,
            name: name.to_string(),
            description: String::new(),
            parameters: echo_sdk_protocol::scalar::WireValue::Null,
            schema_revision: WireU64::from_u64(1),
            required_input_modalities: Vec::new(),
            required_permissions: Vec::new(),
            risk_level: echo_sdk_protocol::methods::ToolRiskLevelWire::ReadOnly,
            supports_streaming: false,
            exempt_from_batch_timeout: false,
            allows_parallel_batch_execution: true,
            manages_own_timeout: false,
        }
    }

    #[test]
    fn semantic_extension_identities_are_unique() {
        let registry = registry();
        let register = |kind, implementation_id, descriptor| {
            registry.register_extension(
                ExtensionRegistrationLimits {
                    max_extensions: 16,
                    max_descriptor_bytes: MAX_EXTENSION_DESCRIPTOR_BYTES,
                },
                kind,
                implementation_id,
                descriptor,
                None,
                false,
            )
        };

        assert!(register(ExtensionKind::Tool, "tool-a", tool_descriptor("search")).is_ok());
        assert!(register(ExtensionKind::Tool, "tool-b", tool_descriptor("search")).is_err());

        let custom_agent = |name: &str| ExtensionDescriptor::CustomAgent {
            descriptor_version: 1,
            name: name.to_string(),
            model_name: "fixture".to_string(),
            system_prompt: String::new(),
            tool_names: Vec::new(),
        };
        assert!(
            register(
                ExtensionKind::CustomAgent,
                "agent-a",
                custom_agent("reviewer")
            )
            .is_ok()
        );
        assert!(
            register(
                ExtensionKind::CustomAgent,
                "agent-b",
                custom_agent("reviewer")
            )
            .is_err()
        );

        let first_factory_instance = registry.register_extension(
            ExtensionRegistrationLimits {
                max_extensions: 16,
                max_descriptor_bytes: MAX_EXTENSION_DESCRIPTOR_BYTES,
            },
            ExtensionKind::CustomAgent,
            "factory-instance-a",
            custom_agent("factory-result"),
            None,
            true,
        );
        let second_factory_instance = registry.register_extension(
            ExtensionRegistrationLimits {
                max_extensions: 16,
                max_descriptor_bytes: MAX_EXTENSION_DESCRIPTOR_BYTES,
            },
            ExtensionKind::CustomAgent,
            "factory-instance-b",
            custom_agent("factory-result"),
            None,
            true,
        );
        assert!(first_factory_instance.is_ok());
        assert!(second_factory_instance.is_ok());
        let session_custom_agents = registry.session_extensions_of_kind(ExtensionKind::CustomAgent);
        assert_eq!(session_custom_agents.len(), 1);
        assert_eq!(
            session_custom_agents
                .first()
                .map(|(_, record)| record.implementation_id.as_str()),
            Some("agent-a")
        );

        assert!(
            register(
                ExtensionKind::Store,
                "store-a",
                ExtensionDescriptor::Store {
                    descriptor_version: 1,
                    search_modes: Vec::new(),
                }
            )
            .is_ok()
        );
        assert!(
            register(
                ExtensionKind::Store,
                "store-b",
                ExtensionDescriptor::Store {
                    descriptor_version: 1,
                    search_modes: Vec::new(),
                }
            )
            .is_err()
        );

        assert!(
            register(
                ExtensionKind::HumanLoopProvider,
                "human-a",
                ExtensionDescriptor::HumanLoopProvider {
                    descriptor_version: 1,
                }
            )
            .is_ok()
        );
        assert!(
            register(
                ExtensionKind::HumanLoopProvider,
                "human-b",
                ExtensionDescriptor::HumanLoopProvider {
                    descriptor_version: 1,
                }
            )
            .is_err()
        );
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

    /// The TaskRun/PlanTask ladder runs shape → kind → generation before
    /// any lookup: a stale generation on an unknown id reports
    /// `stale_handle` (not `invalid_value`), proving the generation check
    /// precedes the map lookup (design §10.4).
    #[test]
    fn task_handle_ladder_checks_generation_before_lookup() {
        let registry = registry();
        let stale = run_handle("never-issued", 6, HandleKind::TaskRun);
        let error = registry
            .task_run(&stale)
            .err()
            .expect("stale must fail before lookup");
        assert_eq!(error.code, ExtensionErrorCode::StaleHandle);
        let future = run_handle("never-issued", 8, HandleKind::TaskRun);
        let error = registry
            .task_run(&future)
            .err()
            .expect("future generation must fail");
        assert_eq!(error.code, ExtensionErrorCode::InvalidValue);
        let wrong_kind = run_handle("never-issued", 7, HandleKind::Session);
        let error = registry
            .task_run(&wrong_kind)
            .err()
            .expect("wrong kind must fail");
        assert_eq!(error.code, ExtensionErrorCode::InvalidValue);
        // The PlanTask ladder checks both handles the same way.
        let plan_task = run_handle("never-issued", 6, HandleKind::PlanTask);
        let task_run = run_handle("never-issued", 7, HandleKind::TaskRun);
        let error = registry
            .plan_task_for_run(&plan_task, &task_run, "test")
            .err()
            .expect("stale plan-task generation must fail");
        assert_eq!(error.code, ExtensionErrorCode::StaleHandle);
    }

    #[test]
    fn facade_stream_uses_registry_generation_owner_and_sequence_ladder() {
        let registry = registry();
        let (resource, _) = registry
            .register_facade_resource(8, "test", "test.resource", None, "test")
            .unwrap_or_else(|error| panic!("resource registration failed: {error:?}"));
        let stream = registry
            .register_facade_stream(2, &resource, None, "test")
            .unwrap_or_else(|error| panic!("stream registration failed: {error:?}"));
        assert!(
            registry
                .advance_facade_stream(&stream, None, 1, "test")
                .is_ok()
        );
        assert!(
            registry
                .advance_facade_stream(&stream, None, 1, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::InvalidRequest)
        );
        assert!(
            registry
                .cancel_facade_stream(&stream, None, "test")
                .is_ok_and(|changed| changed)
        );
        assert!(
            registry
                .advance_facade_stream(&stream, None, 2, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::Cancelled)
        );
        let stale = run_handle(&stream.id, 6, HandleKind::Stream);
        assert!(
            registry
                .facade_stream(&stale, None, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::StaleHandle)
        );
        assert!(
            registry
                .close_facade_stream(&stream, None, "test")
                .is_ok_and(|closed| closed)
        );
        assert!(
            registry
                .close_facade_stream(&stream, None, "test")
                .is_ok_and(|closed| !closed)
        );
    }

    #[test]
    fn facade_stream_resource_cascade_does_not_touch_other_resources() {
        let registry = registry();
        let (first, _) = registry
            .register_facade_resource(8, "test", "first", None, "test")
            .unwrap_or_else(|error| panic!("resource registration failed: {error:?}"));
        let (second, _) = registry
            .register_facade_resource(8, "test", "second", None, "test")
            .unwrap_or_else(|error| panic!("resource registration failed: {error:?}"));
        let first_stream = registry
            .register_facade_stream(8, &first, None, "test")
            .unwrap_or_else(|error| panic!("stream registration failed: {error:?}"));
        let second_stream = registry
            .register_facade_stream(8, &second, None, "test")
            .unwrap_or_else(|error| panic!("stream registration failed: {error:?}"));
        assert_eq!(registry.close_facade_streams_of_resource(&first.id), 1);
        assert!(
            registry
                .facade_stream(&first_stream, None, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::ClosedHandle)
        );
        assert!(registry.facade_stream(&second_stream, None, "test").is_ok());
    }

    #[test]
    fn closing_facade_resource_atomically_closes_associated_streams() {
        let registry = registry();
        let resource_result =
            registry.register_facade_resource(8, "test", "resource", None, "test");
        assert!(resource_result.is_ok(), "resource registration failed");
        let Some((resource, _)) = resource_result.ok() else {
            return;
        };
        let stream_result = registry.register_facade_stream(8, &resource, None, "test");
        assert!(stream_result.is_ok(), "stream registration failed");
        let Some(stream) = stream_result.ok() else {
            return;
        };

        assert!(
            registry
                .close_facade_resource(&resource, "test")
                .is_ok_and(|closed| closed)
        );
        assert!(
            registry
                .facade_resource(&resource, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::ClosedHandle)
        );
        assert!(
            registry
                .facade_stream(&stream, None, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::ClosedHandle)
        );
        assert!(
            registry
                .advance_facade_stream(&stream, None, 1, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::ClosedHandle)
        );
        assert!(
            registry
                .close_facade_stream(&stream, None, "test")
                .is_ok_and(|closed| !closed)
        );
        assert!(
            registry
                .close_facade_resource(&resource, "test")
                .is_ok_and(|closed| !closed)
        );
    }

    #[test]
    fn resource_close_preserves_stale_generation_classification_for_old_streams() {
        let registry = registry();
        let resource_result =
            registry.register_facade_resource(8, "test", "resource", None, "test");
        assert!(resource_result.is_ok(), "resource registration failed");
        let Some((resource, _)) = resource_result.ok() else {
            return;
        };
        let stream_result = registry.register_facade_stream(8, &resource, None, "test");
        assert!(stream_result.is_ok(), "stream registration failed");
        let Some(stream) = stream_result.ok() else {
            return;
        };
        assert!(registry.close_facade_resource(&resource, "test").is_ok());

        let stale = run_handle(&stream.id, 6, HandleKind::Stream);
        assert!(
            registry
                .facade_stream(&stale, None, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::StaleHandle)
        );
        assert!(
            registry
                .advance_facade_stream(&stale, None, 1, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::StaleHandle)
        );
        assert!(
            registry
                .close_facade_stream(&stale, None, "test")
                .is_err_and(|error| error.code == ExtensionErrorCode::StaleHandle)
        );
    }
}
