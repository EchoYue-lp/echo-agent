//! Protocol-neutral per-connection runtime shared by the standard ACP
//! profile and any negotiated extension profile (supreme plan 05, todo
//! `extract-shared-acp-runtime`).
//!
//! Design invariants (see `docs/supreme/specs/2026-09-04-source-first-
//! multilanguage-sdk-runtime/design.md` §10/§11):
//!
//! - There is exactly one Session map, one Run authority, one event ledger
//!   and one cancellation authority per connection. `session/prompt` and
//!   extension `run/start` both enter through
//!   [`AcpConnectionServices::begin_run`], so a Session can never carry two
//!   concurrent runs and both entry points observe the same run ids.
//! - Every accepted [`EventEnvelope`] is committed to the run ledger first
//!   (optionally through the durable journal hook) and only then rendered as
//!   the standard `session/update` projection and forwarded to extension
//!   observers. Both views are projections of one committed fact; neither
//!   may invent a sequence or a terminal.
//! - Status, terminal and receipt facts advance only from the framework
//!   [`AgentTurnDriver`] and its [`TurnReceipt`] — this module stores them,
//!   it never derives them.
//!
//! The root crate stays protocol-neutral: extension negotiation payloads are
//! opaque `serde_json::Value`s and typed `_echo_agent/*` handlers are
//! attached by the profile implementor through
//! [`AcpConnectionProfile::attach`] using the official builder composition.

use crate::agent::EventEnvelope;
use crate::error::{ReactError, Result};
use crate::runtime::{AgentTurnDriver, EventSink, SinkControl, TurnReceipt, TurnRequest};
use agent_client_protocol::schema::v1::SessionId;
use agent_client_protocol::{Client, ConnectionTo};
use async_trait::async_trait;
use echo_state::journal::EventJournal;
use futures::future::BoxFuture;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Notify, RwLock};

use super::extension::ExtensionInvocationAuthority;
use super::projection::AcpEventProjector;
use super::session::{AcpSession, ActiveTurnLease, SessionRegistry};

/// Default bound on envelope bytes retained per in-memory run ledger.
const DEFAULT_MAX_LEDGER_BYTES: usize = 8 * 1024 * 1024;
/// Default bound on envelopes retained per in-memory run ledger.
const DEFAULT_MAX_LEDGER_EVENTS: usize = 10_000;

/// Negotiated connection mode. `Standard` is unconditional; `Extended` is
/// entered only when a profile's hello negotiation accepts the Client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionMode {
    #[default]
    Standard,
    Extended,
}

/// Bounds for the per-run in-memory event ledger. The ledger is the live
/// replay authority for runs without a journal hook; slow consumers can
/// never grow it beyond these bounds — they observe a typed gap instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpLedgerLimits {
    pub max_events: usize,
    pub max_bytes: usize,
}

impl Default for AcpLedgerLimits {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_MAX_LEDGER_EVENTS,
            max_bytes: DEFAULT_MAX_LEDGER_BYTES,
        }
    }
}

/// Per-run event ledger: the durable-first, bounded authority of committed
/// envelopes for one run. The optional journal hook receives every envelope
/// before the in-memory ring; a journal failure fails the run (the sink
/// error becomes the driver's `Failed` receipt), so a run can never report
/// success over an unverified journal.
pub struct EventLedger {
    limits: AcpLedgerLimits,
    journal: Option<Arc<dyn EventJournal<EventEnvelope>>>,
    inner: StdMutex<LedgerInner>,
}

struct LedgerInner {
    events: VecDeque<EventEnvelope>,
    total_bytes: usize,
    last_sequence: u64,
}

impl EventLedger {
    pub fn new(
        limits: AcpLedgerLimits,
        journal: Option<Arc<dyn EventJournal<EventEnvelope>>>,
    ) -> Self {
        Self {
            limits,
            journal,
            inner: StdMutex::new(LedgerInner {
                events: VecDeque::new(),
                total_bytes: 0,
                last_sequence: 0,
            }),
        }
    }

    pub fn commit(&self, envelope: EventEnvelope) -> Result<()> {
        if let Some(journal) = &self.journal {
            journal.append(envelope.clone()).map_err(|error| {
                ReactError::Other(format!("run event journal rejected the envelope: {error}"))
            })?;
        }
        let serialized = serde_json::to_vec(&envelope).map_err(|error| {
            ReactError::Other(format!("event envelope is not encodable: {error}"))
        })?;
        let bytes = serialized.len();
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ReactError::Other("run ledger lock poisoned".to_string()))?;
        inner.last_sequence = inner.last_sequence.max(envelope.sequence);
        inner.events.push_back(envelope);
        inner.total_bytes = inner
            .total_bytes
            .checked_add(bytes)
            .ok_or_else(|| ReactError::Other("run ledger byte accounting overflow".to_string()))?;
        while inner.events.len() > self.limits.max_events
            || inner.total_bytes > self.limits.max_bytes
        {
            let Some(evicted) = inner.events.pop_front() else {
                break;
            };
            let evicted_bytes = serde_json::to_vec(&evicted)
                .map(|bytes| bytes.len())
                .unwrap_or_default();
            inner.total_bytes = inner.total_bytes.saturating_sub(evicted_bytes);
        }
        Ok(())
    }

    /// Committed envelopes strictly after `after_sequence`, in order.
    pub fn events_after(&self, after_sequence: u64) -> Vec<EventEnvelope> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        inner
            .events
            .iter()
            .filter(|envelope| envelope.sequence > after_sequence)
            .cloned()
            .collect()
    }

    /// Oldest envelope sequence still retained in memory (0 when empty).
    pub fn retained_floor(&self) -> u64 {
        let Ok(inner) = self.inner.lock() else {
            return 0;
        };
        inner
            .events
            .front()
            .map(|envelope| envelope.sequence)
            .unwrap_or(0)
    }

    /// Highest committed sequence (0 when empty).
    pub fn last_sequence(&self) -> u64 {
        let Ok(inner) = self.inner.lock() else {
            return 0;
        };
        inner.last_sequence
    }

    /// Durable retention floor reported by the journal hook, when present.
    /// Replays that would need sequences below this floor return a typed gap
    /// instead of silently skipping facts.
    pub fn journal_retained_floor(&self) -> Option<u64> {
        self.journal
            .as_ref()
            .map(|journal| journal.retained_floor())
    }

    /// Highest durable sequence of the journal hook, when present.
    pub fn journal_last_sequence(&self) -> Option<u64> {
        self.journal.as_ref().map(|journal| journal.last_sequence())
    }

    /// Flush the journal hook if it exposes a sync operation.
    pub fn flush(&self) -> Result<()> {
        // `EventJournal` has no flush method; segmented implementations keep
        // their own durability contract. Hooking is the journal's job — the
        // ledger only guarantees it was consulted for every envelope.
        Ok(())
    }
}

/// Extension-side consumer of committed run events. Observers run after the
/// ledger commit, in envelope order; an error fails the run through the
/// driver's exactly-one-terminal contract. Live-delivery bounds (ACK
/// windows, gaps) are the observer implementation's contract, never the
/// ledger's.
#[async_trait]
pub trait RunEventObserver: Send + Sync {
    async fn on_committed_event(&self, envelope: &EventEnvelope) -> Result<()>;
}

/// Everything one run needs at start. Built by the entry point (standard
/// Prompt handler or extension Run handler) after it took the Session's
/// single run slot.
pub struct RunStartSpec {
    pub session: Arc<AcpSession>,
    pub active: ActiveTurnLease,
    pub run_id: String,
    pub stream_id: String,
    pub turn: TurnRequest,
    /// Standard `session/update` projection for this run.
    pub projector: Option<AcpEventProjector>,
    /// Durable journal hook; `None` keeps the ledger memory-only.
    pub journal: Option<Arc<dyn EventJournal<EventEnvelope>>>,
    /// Extension observers invoked after the ledger commit.
    pub observers: Vec<Arc<dyn RunEventObserver>>,
}

/// One registered execution. `run_id` equals the Session turn id for both
/// standard and extension starts, so the two entry points share identity.
pub struct RunEntry {
    pub run_id: String,
    pub session_id: SessionId,
    pub stream_id: String,
    pub ledger: Arc<EventLedger>,
    cancel: crate::agent::CancellationToken,
    lifecycle: StdMutex<RunLifecycle>,
    settled: Notify,
    revoked: AtomicBool,
}

#[derive(Clone)]
enum RunLifecycle {
    Running,
    Settled(Arc<TurnReceipt>),
}

impl RunEntry {
    pub(crate) fn new(
        run_id: String,
        session_id: SessionId,
        stream_id: String,
        ledger: Arc<EventLedger>,
        cancel: crate::agent::CancellationToken,
    ) -> Self {
        Self {
            run_id,
            session_id,
            stream_id,
            ledger,
            cancel,
            lifecycle: StdMutex::new(RunLifecycle::Running),
            settled: Notify::new(),
            revoked: AtomicBool::new(false),
        }
    }

    /// Framework cancellation token of this run.
    pub fn cancellation(&self) -> crate::agent::CancellationToken {
        self.cancel.clone()
    }

    /// Request cancellation. Racing with natural completion resolves through
    /// the framework's own terminal semantics — this only signals the token.
    pub fn cancel(&self) -> bool {
        if self.is_running() {
            self.cancel.cancel();
            true
        } else {
            false
        }
    }

    pub(crate) fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
        self.cancel();
    }

    pub(crate) fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.lifecycle
                .lock()
                .map(|lifecycle| lifecycle.clone())
                .unwrap_or(RunLifecycle::Running),
            RunLifecycle::Running
        )
    }

    /// The settled receipt, when the framework already produced one.
    pub fn receipt(&self) -> Option<Arc<TurnReceipt>> {
        self.lifecycle
            .lock()
            .ok()
            .and_then(|lifecycle| match lifecycle.clone() {
                RunLifecycle::Running => None,
                RunLifecycle::Settled(receipt) => Some(receipt),
            })
    }

    pub(crate) fn settle(&self, receipt: TurnReceipt) {
        if let Ok(mut lifecycle) = self.lifecycle.lock()
            && matches!(*lifecycle, RunLifecycle::Running)
        {
            *lifecycle = RunLifecycle::Settled(Arc::new(receipt));
            self.settled.notify_waiters();
        }
    }

    /// Resolve with the single framework receipt. Never fabricates a
    /// terminal: if the driver task was aborted before settling, this waits
    /// forever — close paths cancel runs and await settlement instead.
    pub async fn wait_receipt(&self) -> Arc<TurnReceipt> {
        loop {
            let notified = self.settled.notified();
            if let Some(receipt) = self.receipt() {
                return receipt;
            }
            notified.await;
        }
    }
}

/// Registry of runs for one connection: the single Run authority.
#[derive(Default)]
pub(crate) struct RunRegistry {
    runs: RwLock<HashMap<String, Arc<RunEntry>>>,
}

impl RunRegistry {
    async fn register(&self, entry: Arc<RunEntry>) -> Result<()> {
        let mut runs = self.runs.write().await;
        if runs.contains_key(&entry.run_id) {
            return Err(ReactError::Other(format!(
                "run id {} is already registered",
                entry.run_id
            )));
        }
        runs.insert(entry.run_id.clone(), entry);
        Ok(())
    }

    pub async fn get(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        self.runs.read().await.get(run_id).cloned()
    }

    pub async fn remove(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        self.runs.write().await.remove(run_id)
    }

    /// Cancel every live run and wait (bounded) for their settlements.
    /// Entries whose driver task was aborted settle through the abort path;
    /// waiting runs resolve as soon as their receipt exists or the entry is
    /// dropped with the connection.
    async fn cancel_and_wait(&self, wait: Duration) {
        let entries: Vec<Arc<RunEntry>> = self.runs.read().await.values().cloned().collect();
        for entry in &entries {
            entry.cancel();
        }
        let _ = tokio::time::timeout(
            wait,
            futures::future::join_all(entries.iter().map(|entry| {
                let entry = entry.clone();
                async move {
                    if entry.is_running() {
                        // Races with the driver task: either the receipt lands
                        // here or the connection teardown aborts the task.
                        let _ = entry.wait_receipt().await;
                    }
                }
            })),
        )
        .await;
        self.runs.write().await.clear();
    }

    async fn run_count(&self) -> usize {
        self.runs.read().await.len()
    }
}

/// Ledger-first composite sink: commit to the ledger (journal included),
/// then render the standard projection, then forward to extension
/// observers. Any failure fails the run — no view silently diverges from a
/// committed fact.
struct SharedRunSink {
    ledger: Arc<EventLedger>,
    projector: Option<AcpEventProjector>,
    observers: Vec<Arc<dyn RunEventObserver>>,
}

#[async_trait]
impl EventSink for SharedRunSink {
    async fn on_event(&self, envelope: EventEnvelope) -> Result<SinkControl> {
        self.ledger.commit(envelope.clone())?;
        if let Some(projector) = &self.projector {
            projector.emit(&envelope).await?;
        }
        for observer in &self.observers {
            observer.on_committed_event(&envelope).await?;
        }
        Ok(SinkControl::Continue)
    }
}

/// Per-connection shared runtime: the single Session/Run/event authority
/// used by both ACP profiles.
pub struct AcpConnectionServices {
    mode: RwLock<ConnectionMode>,
    sessions: Arc<SessionRegistry>,
    runs: RunRegistry,
    ledger_limits: AcpLedgerLimits,
    config: Arc<super::adapter::AcpAdapterConfig>,
    admission_open: AtomicBool,
    extensions: Arc<ExtensionInvocationAuthority>,
}

impl AcpConnectionServices {
    pub fn new(
        sessions: Arc<SessionRegistry>,
        config: Arc<super::adapter::AcpAdapterConfig>,
    ) -> Self {
        // The adapter config is validated before the connection spawns, so a
        // non-positive concurrency cannot reach here; degrade to a single
        // permit instead of panicking if it ever does.
        let extensions = ExtensionInvocationAuthority::new(config.max_extension_concurrency)
            .unwrap_or_else(|error| {
                tracing::warn!(
                    "invalid extension concurrency {} ({}), falling back to 1",
                    config.max_extension_concurrency,
                    error
                );
                ExtensionInvocationAuthority::new_saturating(1)
            });
        Self {
            mode: RwLock::new(ConnectionMode::Standard),
            sessions,
            runs: RunRegistry::default(),
            ledger_limits: AcpLedgerLimits::default(),
            config,
            admission_open: AtomicBool::new(true),
            extensions,
        }
    }

    /// The connection-scoped extension invocation authority (design §12.3).
    /// Shared by every extension proxy on this connection; never carries a
    /// second run/session/terminal authority.
    pub fn extensions(&self) -> &Arc<ExtensionInvocationAuthority> {
        &self.extensions
    }

    /// Override the in-memory ledger bounds (extension profiles derive them
    /// from their negotiated limits).
    pub fn with_ledger_limits(mut self, limits: AcpLedgerLimits) -> Self {
        self.ledger_limits = limits;
        self
    }

    pub fn with_ledger_limits_mut(&mut self, limits: AcpLedgerLimits) -> &mut Self {
        self.ledger_limits = limits;
        self
    }

    pub fn ledger_limits(&self) -> AcpLedgerLimits {
        self.ledger_limits
    }

    /// The one Session authority of this connection.
    pub fn sessions(&self) -> &Arc<SessionRegistry> {
        &self.sessions
    }

    pub async fn mode(&self) -> ConnectionMode {
        *self.mode.read().await
    }

    pub async fn is_extended(&self) -> bool {
        self.mode().await == ConnectionMode::Extended
    }

    pub fn admission_open(&self) -> bool {
        self.admission_open.load(Ordering::Acquire)
    }

    pub fn close_admission(&self) {
        self.admission_open.store(false, Ordering::Release);
    }

    pub fn ensure_admission(&self) -> Result<()> {
        if self.admission_open() {
            Ok(())
        } else {
            Err(ReactError::Other("ACP Host is shutting down".to_string()))
        }
    }

    pub async fn create_session(
        &self,
        request: agent_client_protocol::schema::v1::NewSessionRequest,
    ) -> Result<SessionId> {
        self.ensure_admission()?;
        let session_id = self.sessions.create(request).await?;
        if !self.admission_open() {
            let _ = self.sessions.close_session(&session_id).await;
            return Err(ReactError::Other("ACP Host is shutting down".to_string()));
        }
        Ok(session_id)
    }

    pub async fn insert_session(
        &self,
        context: super::session::AcpSessionContext,
    ) -> Result<SessionId> {
        self.ensure_admission()?;
        let session_id = self.sessions.insert_session(context).await?;
        if !self.admission_open() {
            let _ = self.sessions.close_session(&session_id).await;
            return Err(ReactError::Other("ACP Host is shutting down".to_string()));
        }
        Ok(session_id)
    }

    pub(crate) async fn set_mode(&self, mode: ConnectionMode) {
        *self.mode.write().await = mode;
    }

    /// Resolve one run by id.
    pub async fn run(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        self.runs.get(run_id).await
    }

    /// Remove a prepared run when a later setup or connection spawn fails.
    /// Dropping the returned task future releases the Session lease; this
    /// method removes only the shared Run registration.
    pub async fn remove_run(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        let entry = self.runs.remove(run_id).await;
        if let Some(entry) = &entry {
            entry.revoke();
        }
        entry
    }

    /// Build the standard projection for one run with the adapter's bounds.
    pub fn projector(
        &self,
        session_id: SessionId,
        connection: ConnectionTo<Client>,
    ) -> AcpEventProjector {
        AcpEventProjector::new(
            session_id,
            connection,
            self.config.max_update_chars,
            self.config.max_updates_per_turn,
            self.config.max_total_update_chars,
        )
    }

    /// Register one run and return the driver task for the caller to spawn
    /// on the connection. Registration reserves the run id; the driver only
    /// starts once the spawned task runs. A duplicate run id is a caller
    /// bug and fails without touching the Session's run slot.
    pub async fn prepare_run(
        self: &Arc<Self>,
        spec: RunStartSpec,
    ) -> Result<(
        Arc<RunEntry>,
        BoxFuture<'static, agent_client_protocol::Result<()>>,
    )> {
        self.ensure_admission()?;
        let ledger = Arc::new(EventLedger::new(self.ledger_limits, spec.journal));
        let entry = Arc::new(RunEntry::new(
            spec.run_id.clone(),
            spec.session.context.session_id.clone(),
            spec.stream_id.clone(),
            ledger.clone(),
            spec.active.turn.cancellation(),
        ));
        self.runs.register(entry.clone()).await?;
        if !self.admission_open() {
            let _ = self.runs.remove(&entry.run_id).await;
            entry.cancel();
            return Err(ReactError::Other("ACP Host is shutting down".to_string()));
        }

        let sink = Arc::new(SharedRunSink {
            ledger,
            projector: spec.projector,
            observers: spec.observers,
        });
        let agent = spec.session.agent.clone();
        let turn = spec.turn;
        let drive_entry = entry.clone();
        let lease = spec.active;
        let task: BoxFuture<'static, agent_client_protocol::Result<()>> = Box::pin(async move {
            let receipt = if drive_entry.is_revoked() {
                match TurnReceipt::cancelled(drive_entry.run_id.clone()) {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        return Err(
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        );
                    }
                }
            } else {
                AgentTurnDriver
                    .drive(agent.as_ref(), turn, sink.as_ref())
                    .await
            };
            drive_entry.settle(receipt);
            drop(lease);
            Ok(())
        });
        Ok((entry, task))
    }

    /// Cancel one run by id. Returns false for unknown or already settled
    /// runs; the unique terminal still comes from the framework.
    pub async fn cancel_run(&self, run_id: &str) -> bool {
        let Some(entry) = self.runs.get(run_id).await else {
            return false;
        };
        entry.cancel()
    }

    /// Number of registered runs (live and settled) on this connection.
    pub async fn run_count(&self) -> usize {
        self.runs.run_count().await
    }

    /// Unified close chain: cancel live runs and await their receipts with a
    /// bounded wait, then close all Session Agents. Used by connection close
    /// and stdin EOF alike.
    pub async fn close(&self, timeout: Duration) -> Result<()> {
        self.close_admission();
        self.cancel_and_wait_runs(timeout).await;
        self.close_sessions().await
    }

    pub async fn cancel_and_wait_runs(&self, timeout: Duration) {
        self.runs.cancel_and_wait(timeout).await;
    }

    pub async fn close_sessions(&self) -> Result<()> {
        self.sessions.close_all().await
    }
}

/// Standard responses a profile may annotate with extension handles
/// (`_meta` bridging, design §10.4).
#[derive(Debug, Clone, Copy)]
pub enum StandardBridgeOutcome<'a> {
    /// `session/new` succeeded for this ACP Session id.
    SessionCreated { session_id: &'a str },
    /// A Prompt run started on this Session with these run/stream ids.
    PromptStarted {
        session_id: &'a str,
        run_id: &'a str,
        stream_id: &'a str,
    },
}

/// Context handed to a profile when it builds the per-run extension
/// observers for one run.
pub struct RunObserverContext<'a> {
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub stream_id: &'a str,
    pub connection: ConnectionTo<Client>,
}

/// A composable extension profile over the shared connection runtime.
///
/// The adapter keeps the single official connection: `initialize`, stdio,
/// the reader/writer loop, standard handlers and the close chain are built
/// exactly once. A profile contributes (a) the capability advertisement
/// under `agentCapabilities._meta.<key>`, (b) the hello decision that
/// promotes the connection to [`ConnectionMode::Extended`], (c) per-run
/// event observers, (d) `_meta` bridging on standard responses, and (e) its
/// typed handlers through [`AcpConnectionProfile::attach`], which the
/// adapter merges with the official `Builder::with_connection_builder`.
pub trait AcpConnectionProfile: Send + Sync + 'static {
    /// `(_meta key, value)` published in every `initialize` response. Return
    /// `None` to never advertise (the profile then behaves as standard-only).
    fn advertisement_meta(&self) -> Option<(String, serde_json::Value)> {
        None
    }

    /// Decide the connection mode from the raw Client hello. `Ok(())` enters
    /// Extended mode; `Err(reason)` keeps Standard mode without failing the
    /// standard `initialize`. A plain Client (no `_meta` entry at all) never
    /// reaches this method.
    fn negotiate_hello(&self, hello: &serde_json::Value) -> std::result::Result<(), String>;

    /// Annotate a successful standard response for handle bridging in
    /// Extended mode. Returning `Some(meta)` sets the response's `_meta`.
    fn annotate_standard(
        &self,
        outcome: StandardBridgeOutcome<'_>,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let _ = outcome;
        None
    }

    /// Register a Session created through the standard ACP `session/new`
    /// path. Extension profiles use this to make the response's bridge handle
    /// address the same Session registry entry.
    fn register_standard_session(
        &self,
        _session_id: &str,
        _cwd: &std::path::Path,
    ) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Register a Run created through standard ACP `session/prompt` before
    /// its driver task is spawned. The profile may persist its start record
    /// and issue the same Run/Stream handles used by extension requests.
    fn register_standard_run(
        &self,
        _session_id: &str,
        _entry: Arc<RunEntry>,
    ) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Return the durable journal hook for a Run. Standard-only profiles
    /// return `None`; the core profile uses this for both prompt entry paths.
    fn run_journal(
        &self,
        _run_id: &str,
    ) -> std::result::Result<Option<Arc<dyn EventJournal<EventEnvelope>>>, String> {
        Ok(None)
    }

    /// Persist the exact framework receipt after a Run settles. The default
    /// profile does nothing; persistence-aware profiles store the receipt
    /// without deriving a second terminal.
    fn persist_run_settled(
        &self,
        _entry: &RunEntry,
        _receipt: &TurnReceipt,
    ) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Undo profile-side registrations made before the shared runtime could
    /// spawn a driver, keeping a failed standard Prompt transactional.
    fn rollback_run(&self, _run_id: &str, _stream_id: &str) {}

    /// Notify the profile that the driver task was accepted by the official
    /// connection, allowing it to attach durable settlement observation.
    fn run_spawned(&self, _entry: Arc<RunEntry>) {}

    /// Flush profile-owned durable state after Run cancellation/wait and
    /// before Session Agents/MCP are closed.
    fn flush_before_agents(&self) -> std::result::Result<(), String> {
        Ok(())
    }

    fn wait_for_settlements(
        &self,
        _timeout: Duration,
    ) -> BoxFuture<'static, std::result::Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    /// Extension observers for one run in Extended mode. Called once per
    /// run start, before the driver spawns.
    fn run_observers(&self, context: RunObserverContext<'_>) -> Vec<Arc<dyn RunEventObserver>> {
        let _ = context;
        Vec::new()
    }

    /// Build this profile's typed handler builder. The adapter merges it
    /// after the standard handlers, so exactly one official dispatch chain
    /// exists. Extension handlers must check
    /// [`AcpConnectionServices::is_extended`] themselves and answer
    /// method-not-found when the connection stayed Standard.
    fn attach(
        &self,
        services: Arc<AcpConnectionServices>,
    ) -> agent_client_protocol::Builder<
        agent_client_protocol::Agent,
        impl agent_client_protocol::HandleDispatchFrom<agent_client_protocol::Client>,
        impl agent_client_protocol::RunWithConnectionTo<agent_client_protocol::Client>,
        impl agent_client_protocol::HandleConnectionClose<agent_client_protocol::Client>,
        agent_client_protocol::RawConnectionContext,
    >;
}
