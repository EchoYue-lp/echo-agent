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
use std::sync::{Arc, Mutex as StdMutex, Weak};
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
/// before the in-memory ring; a journal failure is reported as delivery
/// failure by the driven receipt. It does not create a second execution
/// terminal when the producer already emitted one.
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
/// ledger commit, in envelope order; an error becomes delivery failure on the
/// driver's receipt. Live-delivery bounds (ACK windows, gaps) are the observer
/// implementation's contract, never the ledger's execution authority.
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
    // Retain this Run's original Session admission when a foreign Agent can
    // report uncertainty but cannot provide a physical-settlement receipt.
    unsettled_lease: StdMutex<Option<ActiveTurnLease>>,
    settled: Notify,
    revoked: AtomicBool,
}

#[derive(Clone)]
enum RunLifecycle {
    Running {
        // Observer knowledge only: neither a terminal nor permission to retry.
        driver_error: Option<agent_client_protocol::Error>,
    },
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
            lifecycle: StdMutex::new(RunLifecycle::Running { driver_error: None }),
            unsettled_lease: StdMutex::new(None),
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
                .unwrap_or(RunLifecycle::Running { driver_error: None }),
            RunLifecycle::Running { .. }
        )
    }

    /// The settled receipt, when the framework already produced one.
    pub fn receipt(&self) -> Option<Arc<TurnReceipt>> {
        self.lifecycle
            .lock()
            .ok()
            .and_then(|lifecycle| match lifecycle.clone() {
                RunLifecycle::Running { .. } => None,
                RunLifecycle::Settled(receipt) => Some(receipt),
            })
    }

    fn observe_driver_error(&self, lease: ActiveTurnLease, error: agent_client_protocol::Error) {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let RunLifecycle::Running { driver_error } = &mut *lifecycle
            && driver_error.is_none()
        {
            *self
                .unsettled_lease
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(lease);
            *driver_error = Some(error);
            self.settled.notify_waiters();
        }
    }

    pub(crate) fn settle(&self, receipt: TurnReceipt) {
        if let Ok(mut lifecycle) = self.lifecycle.lock()
            && matches!(*lifecycle, RunLifecycle::Running { .. })
        {
            *lifecycle = RunLifecycle::Settled(Arc::new(receipt));
            drop(
                self.unsettled_lease
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take(),
            );
            self.settled.notify_waiters();
        }
    }

    /// Resolve with the single framework receipt. Never fabricates a
    /// terminal: if the driver task was aborted before settling, this waits
    /// forever — close paths cancel runs and await settlement instead.
    pub async fn wait_receipt(&self) -> Arc<TurnReceipt> {
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(receipt) = self.receipt() {
                return receipt;
            }
            notified.await;
        }
    }

    /// Observe the original receipt or a driver error without declaring settlement.
    ///
    /// An error preserves its structured observation identity and the original
    /// Session lease. It is not a terminal receipt or permission to retry. A
    /// committed receipt takes precedence over earlier observer knowledge.
    /// [`Self::wait_receipt`] remains the receipt-only boundary for persistence
    /// and physical close. Multiple and later observers see the same result.
    pub async fn wait_result(&self) -> agent_client_protocol::Result<Arc<TurnReceipt>> {
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let result = {
                let lifecycle = self
                    .lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                match &*lifecycle {
                    RunLifecycle::Settled(receipt) => Some(Ok(receipt.clone())),
                    RunLifecycle::Running { driver_error } => driver_error.clone().map(Err),
                }
            };
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }
}

type RunEntries = StdMutex<HashMap<String, Arc<RunEntry>>>;

/// Registry of runs for one connection: the single Run authority.
#[derive(Default)]
pub(crate) struct RunRegistry {
    // All map operations are short and contain no await/user code. A prepared
    // future must also be able to roll back its exact registration from Drop.
    runs: Arc<RunEntries>,
}

impl RunRegistry {
    async fn register(&self, entry: Arc<RunEntry>) -> Result<()> {
        let mut runs = self.runs.lock().unwrap_or_else(|error| error.into_inner());
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
        self.runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned()
    }

    pub async fn remove(&self, run_id: &str) -> Option<Arc<RunEntry>> {
        let mut runs = self.runs.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = runs.get(run_id)
            && entry.is_running()
        {
            // A cold owner's Drop rolls back its own registration. Once the
            // driver started, revocation cannot erase its close obligation.
            return Some(entry.clone());
        }
        runs.remove(run_id)
    }

    /// Cancel every live run and wait (bounded) for their settlements.
    /// Only an original driver receipt removes a started Run. An abnormal
    /// task loss retains nonterminal observer knowledge and the original lease.
    async fn cancel_and_wait(&self, wait: Duration) -> usize {
        let entries: Vec<Arc<RunEntry>> = self
            .runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect();
        for entry in &entries {
            entry.cancel();
        }
        let _ = tokio::time::timeout(
            wait,
            futures::future::join_all(entries.iter().map(|entry| {
                let entry = entry.clone();
                async move {
                    if entry.is_running() {
                        let _ = entry.wait_receipt().await;
                    }
                }
            })),
        )
        .await;
        let mut runs = self.runs.lock().unwrap_or_else(|error| error.into_inner());
        runs.retain(|_, entry| entry.is_running());
        runs.len()
    }

    async fn run_count(&self) -> usize {
        self.runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

// Resource custody only. RunLifecycle remains the sole result/receipt state.
// The registry holds the entry, not this task or its JoinHandle, and this owner
// has only a weak link back to the map, so no strong ownership cycle is formed.
struct RunDriverOwner {
    entry: Arc<RunEntry>,
    registry: Weak<RunEntries>,
    lease: Option<ActiveTurnLease>,
    started: bool,
}

impl RunDriverOwner {
    fn observe_error(&mut self, error: agent_client_protocol::Error) {
        if let Some(lease) = self.lease.take() {
            self.entry.observe_driver_error(lease, error);
        }
    }

    fn settle(&mut self, receipt: TurnReceipt) {
        self.entry.settle(receipt);
        drop(self.lease.take());
    }
}

impl Drop for RunDriverOwner {
    fn drop(&mut self) {
        if self.lease.is_none() {
            return;
        }
        self.entry.cancel();
        if !self.started {
            // No Agent code ran. Roll back only this exact prepared entry;
            // neither a replacement with the same id nor another Run is ours.
            self.entry.revoke();
            if let Some(registry) = self.registry.upgrade() {
                let mut runs = registry.lock().unwrap_or_else(|error| error.into_inner());
                if runs
                    .get(&self.entry.run_id)
                    .is_some_and(|entry| Arc::ptr_eq(entry, &self.entry))
                {
                    runs.remove(&self.entry.run_id);
                }
            }
            return;
        }
        // Panic/runtime abort cannot stand in for original physical completion.
        // Retain the same lease in RunEntry and expose an observation only.
        self.observe_error(agent_client_protocol::Error::internal_error().data(
            serde_json::json!({
                "observation": "not_settled",
                "run_id": self.entry.run_id,
                "reason": "original ACP driver task ended before its receipt",
            }),
        ));
    }
}

struct RunDriverWaitCancellation {
    cancel: crate::agent::CancellationToken,
    armed: bool,
}

impl Drop for RunDriverWaitCancellation {
    fn drop(&mut self) {
        if self.armed {
            // Dropping a connection observer requests cancellation; the same
            // independently owned driver must keep polling through cleanup.
            self.cancel.cancel();
        }
    }
}

/// Ledger-first composite sink: commit to the ledger (journal included),
/// then render the standard projection, then forward to extension
/// observers. Any error is surfaced as delivery failure; the driver retains a
/// producer terminal that was already observed before this sink call.
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
        self.sessions.close_admission();
        self.extensions.close_admission();
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

    /// Return the cancellation token of the active run owned by one Session.
    /// A Session permits at most one active run, so this is an unambiguous
    /// bridge for trait callbacks whose public method does not carry a token.
    pub async fn active_run_cancellation(
        &self,
        session_id: &str,
    ) -> Option<crate::agent::CancellationToken> {
        self.runs
            .runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .find(|entry| entry.session_id.to_string() == session_id && entry.is_running())
            .map(|entry| entry.cancellation())
    }

    /// Remove a prepared run when a later setup or connection spawn fails.
    /// Revocation cancels the same Run. Dropping an unstarted task rolls back
    /// its registration and lease; a started Run remains until a real receipt.
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

    /// Register one run and return its observer task for the caller to spawn
    /// on the connection. First poll starts the original driver on an owned
    /// Tokio task; losing the connection observer cannot destroy that driver.
    /// Dropping an unpolled task rolls back its exact registration and lease.
    /// A duplicate run id fails without touching the Session's run slot.
    /// Run-level driver errors are recorded on [`RunEntry::wait_result`];
    /// the task completes successfully after recording them because an official
    /// ACP spawned-task error shuts down the entire connection. Receipt-only
    /// observers continue waiting for authoritative physical settlement.
    pub async fn prepare_run(
        self: &Arc<Self>,
        spec: RunStartSpec,
    ) -> Result<(
        Arc<RunEntry>,
        BoxFuture<'static, agent_client_protocol::Result<()>>,
    )> {
        self.ensure_admission()?;
        // The Session lease already allocated this ACP Turn. Pass that same
        // identity into the Agent, rather than letting its stream entry mint
        // an unrelated turn that tracked steering cannot address. Application
        // TaskRun identity, lineage and invocation policy remain caller-owned.
        let canonical_turn = spec.active.turn.id();
        if spec.run_id != canonical_turn || spec.turn.identity.turn_id.as_str() != canonical_turn {
            return Err(ReactError::Other(
                "ACP Run identity must match the reserved Session Turn".to_string(),
            ));
        }
        let mut turn = spec.turn;
        let invocation = turn.invocation.get_or_insert_with(Default::default);
        let runtime = invocation.runtime.get_or_insert_with(Default::default);
        if runtime
            .turn_id
            .as_deref()
            .is_some_and(|turn_id| turn_id != canonical_turn)
        {
            return Err(ReactError::Other(
                "ACP invocation Turn identity conflicts with the reserved Session Turn".to_string(),
            ));
        }
        runtime.turn_id = Some(canonical_turn.to_string());
        let ledger = Arc::new(EventLedger::new(self.ledger_limits, spec.journal));
        let entry = Arc::new(RunEntry::new(
            spec.run_id.clone(),
            spec.session.context.session_id.clone(),
            spec.stream_id.clone(),
            ledger.clone(),
            spec.active.turn.cancellation(),
        ));
        self.runs.register(entry.clone()).await?;
        let mut owner = RunDriverOwner {
            entry: entry.clone(),
            registry: Arc::downgrade(&self.runs.runs),
            lease: Some(spec.active),
            started: false,
        };
        if !self.admission_open() {
            // The cold owner synchronously rolls back this exact registration.
            return Err(ReactError::Other("ACP Host is shutting down".to_string()));
        }

        let sink = Arc::new(SharedRunSink {
            ledger,
            projector: spec.projector,
            observers: spec.observers,
        });
        let agent = spec.session.agent.clone();
        let task: BoxFuture<'static, agent_client_protocol::Result<()>> = Box::pin(async move {
            let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
                agent_client_protocol::Error::internal_error().data(format!(
                    "ACP Run driver requires an active Tokio runtime: {error}"
                ))
            })?;
            let mut wait_cancel = RunDriverWaitCancellation {
                cancel: owner.entry.cancellation(),
                armed: true,
            };
            // Spawn once, only when the accepted observer is first polled.
            // The original owner is built before spawn, so a never-polled
            // execution task still performs the exact cold rollback in Drop.
            let driver = runtime.spawn(async move {
                owner.started = true;
                let receipt = if owner.entry.is_revoked() {
                    match TurnReceipt::cancelled(owner.entry.run_id.clone()) {
                        Ok(receipt) => receipt,
                        Err(error) => {
                            return Err(agent_client_protocol::Error::internal_error()
                                .data(error.to_string()));
                        }
                    }
                } else {
                    match AgentTurnDriver
                        .drive(agent.as_ref(), turn, sink.as_ref())
                        .await
                    {
                        Ok(receipt) => receipt,
                        Err(error) => {
                            let data = match error {
                                ReactError::ExecutionNotSettled {
                                    run_id,
                                    execution_id,
                                } => {
                                    let mut details = serde_json::Map::from_iter([
                                        (
                                            "observation".to_string(),
                                            serde_json::Value::String("not_settled".to_string()),
                                        ),
                                        ("run_id".to_string(), serde_json::Value::String(run_id)),
                                    ]);
                                    if let Some(execution_id) = execution_id {
                                        details.insert(
                                            "execution_id".to_string(),
                                            serde_json::Value::String(execution_id),
                                        );
                                    }
                                    serde_json::Value::Object(details)
                                }
                                error => serde_json::Value::String(error.to_string()),
                            };
                            owner.observe_error(
                                agent_client_protocol::Error::internal_error().data(data),
                            );
                            return Ok(());
                        }
                    }
                };
                owner.settle(receipt);
                Ok(())
            });
            let joined = driver.await;
            wait_cancel.armed = false;
            joined.map_err(|error| {
                // RunDriverOwner already retained nonterminal loss on the
                // original entry. This genuine task failure remains an error.
                agent_client_protocol::Error::internal_error().data(format!(
                    "ACP original driver task failed (panic={}, cancelled={}): {error}",
                    error.is_panic(),
                    error.is_cancelled(),
                ))
            })?
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

    /// Close extension and Run admission, cancel both invocation classes,
    /// wait for their owners, then close Session Agents.
    pub async fn close(&self, timeout: Duration) -> Result<()> {
        self.close_admission();
        self.extensions.cancel_all();
        let stage = (timeout / 2).max(Duration::from_millis(1));
        tokio::time::timeout(timeout, async {
            let (extensions, runs) = tokio::join!(
                self.extensions.drain(stage),
                self.cancel_and_wait_runs_report(stage),
            );
            if extensions > 0 || runs > 0 {
                return Err(ReactError::Other(format!(
                    "ACP close retained {extensions} extension invocation(s) and {runs} Run(s) without framework receipts"
                )));
            }
            tokio::time::timeout(stage, self.close_sessions())
                .await
                .map_err(|_| ReactError::Other("ACP Session Agent close timed out".to_string()))?
        })
        .await
        .map_err(|_| ReactError::Other("ACP connection close timed out".to_string()))?
    }

    pub async fn cancel_and_wait_runs(&self, timeout: Duration) {
        let _ = self.runs.cancel_and_wait(timeout).await;
    }

    pub(crate) async fn cancel_and_wait_runs_report(&self, timeout: Duration) -> usize {
        self.runs.cancel_and_wait(timeout).await
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
    /// connection. This hook must not persist the terminal receipt;
    /// [`Self::persist_run_settled`] is the standard Prompt's sole durable
    /// settlement authority.
    fn run_spawned(&self, _entry: Arc<RunEntry>) {}

    /// Flush profile-owned durable state after Run cancellation/wait and
    /// before Session Agents/MCP are closed.
    fn flush_before_agents(&self) -> std::result::Result<(), String> {
        Ok(())
    }

    /// Release connection-owned profile resources (e.g. extension
    /// registrations) after Session Agents/MCP closed. Last step of the
    /// teardown order in design §12.3/§16.
    fn release_after_agents(&self) {}

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

#[cfg(test)]
mod close_tests {
    use super::*;

    #[tokio::test]
    async fn run_close_timeout_keeps_unsettled_framework_receipt_for_retry() -> Result<()> {
        let runs = RunRegistry::default();
        let cancellation = crate::agent::CancellationToken::new();
        let entry = Arc::new(RunEntry::new(
            "run-close-timeout".to_string(),
            SessionId::new("session-close-timeout"),
            "stream-close-timeout".to_string(),
            Arc::new(EventLedger::new(AcpLedgerLimits::default(), None)),
            cancellation.clone(),
        ));
        runs.register(Arc::clone(&entry)).await?;

        runs.cancel_and_wait(Duration::from_millis(20)).await;
        assert!(cancellation.is_cancelled());
        assert_eq!(runs.run_count().await, 1);

        entry.settle(TurnReceipt::cancelled(entry.run_id.clone())?);
        runs.cancel_and_wait(Duration::from_secs(1)).await;
        assert_eq!(runs.run_count().await, 0);
        Ok(())
    }
}

#[cfg(test)]
mod turn_identity_tests {
    use super::*;
    use crate::agent::{
        Agent, AgentEvent, AgentInvocationContext, CancellationToken, EventIdentity,
    };
    use crate::llm::types::Message;
    use crate::runtime::{TurnMode, TurnOutcome};
    use crate::tools::{ExternalRunContext, NestedDelegationPolicy, SubagentLineage};
    use futures::{FutureExt as _, StreamExt, stream::BoxStream};

    #[derive(Clone, Default)]
    struct CapturingAgent {
        calls: Arc<StdMutex<Vec<(String, AgentInvocationContext)>>>,
    }

    impl CapturingAgent {
        fn record<'a>(
            &'a self,
            text: String,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((text, invocation));
                Ok(
                    futures::stream::iter([Ok(AgentEvent::FinalAnswer("captured".to_string()))])
                        .boxed(),
                )
            })
        }
    }

    impl Agent for CapturingAgent {
        fn name(&self) -> &str {
            "original-context-consumer"
        }
        fn model_name(&self) -> &str {
            "none"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn tool_names(&self) -> Vec<String> {
            Vec::new()
        }
        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(task.to_string()) })
        }
        fn execute_stream<'a>(
            &'a self,
            text: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.record(text.to_string(), AgentInvocationContext::default())
        }
        fn chat_stream_with_cancel<'a>(
            &'a self,
            text: &'a str,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.record(text.to_string(), AgentInvocationContext::default())
        }
        fn execute_stream_with_invocation_context<'a>(
            &'a self,
            text: &'a str,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.record(text.to_string(), invocation)
        }
        fn chat_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.record(message.content.as_text().unwrap_or_default(), invocation)
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            _cancel: CancellationToken,
            invocation: AgentInvocationContext,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            self.record(message.content.as_text().unwrap_or_default(), invocation)
        }
    }

    async fn prepared_session<A: Agent + Clone + 'static>(
        agent: A,
    ) -> Result<(Arc<AcpConnectionServices>, Arc<AcpSession>)> {
        let factory: Arc<dyn super::super::session::AcpSessionFactory> =
            Arc::new(move |_context| {
                let agent = agent.clone();
                async move { Ok(Box::new(agent) as Box<dyn Agent>) }
            });
        let registry = Arc::new(SessionRegistry::new(factory, 4));
        let services = Arc::new(AcpConnectionServices::new(
            registry.clone(),
            Arc::new(super::super::adapter::AcpAdapterConfig::default()),
        ));
        let context = super::super::session::AcpSessionContext {
            session_id: SessionId::new("original-Session-中文"),
            cwd: std::env::current_dir()?,
            additional_directories: Vec::new(),
            mcp_servers: Vec::new(),
            client_capabilities: Default::default(),
            meta: None,
        };
        let session_id = services.insert_session(context).await?;
        let session = registry
            .get(&session_id)
            .await
            .ok_or_else(|| ReactError::Other("original Session missing".to_string()))?;
        Ok((services, session))
    }

    fn spec(
        session: Arc<AcpSession>,
        mode: TurnMode,
        structured: bool,
        invocation: Option<AgentInvocationContext>,
    ) -> Result<RunStartSpec> {
        let active = session.begin_turn()?;
        let run_id = active.turn.id().to_string();
        let identity = EventIdentity::for_chat(
            Some(session.context.session_id.to_string()),
            run_id.clone(),
            active.turn.message_id.clone(),
            Some(run_id.clone()),
        )?;
        let stream_id = identity.stream_id.to_string();
        let mut turn = if structured {
            TurnRequest::from_message(identity, Message::user("original input 中文🌊".to_string()))
        } else {
            TurnRequest::new(identity, "original input 中文🌊")
        }
        .mode(mode)
        .cancel(active.turn.cancellation());
        if let Some(invocation) = invocation {
            turn = turn.invocation(invocation);
        }
        Ok(RunStartSpec {
            session,
            active,
            run_id,
            stream_id,
            turn,
            projector: None,
            journal: None,
            observers: Vec::new(),
        })
    }

    #[tokio::test]
    async fn unpolled_run_driver_drop_rolls_back_registration_and_original_lease() -> Result<()> {
        for revoke_first in [false, true] {
            let agent = CapturingAgent::default();
            let (services, session) = prepared_session(agent.clone()).await?;
            let original = spec(session.clone(), TurnMode::Chat, false, None)?;
            let (entry, task) = services.prepare_run(original).await?;
            let weak_entry = Arc::downgrade(&entry);
            assert!(session.wait_until_idle().now_or_never().is_none());
            if revoke_first {
                let removed = services.remove_run(&entry.run_id).await;
                assert!(removed.is_some_and(|value| Arc::ptr_eq(&value, &entry)));
            }
            drop(task);
            assert!(services.run(&entry.run_id).await.is_none());
            assert!(entry.cancellation().is_cancelled());
            assert!(entry.receipt().is_none());
            assert!(session.wait_until_idle().now_or_never().is_some());
            assert!(
                agent
                    .calls
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .is_empty()
            );
            drop(session.begin_turn()?);
            drop(entry);
            assert!(weak_entry.upgrade().is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn aborted_started_driver_owner_retains_observation_and_original_lease() -> Result<()> {
        let (services, session) = prepared_session(CapturingAgent::default()).await?;
        let original = spec(session.clone(), TurnMode::Chat, false, None)?;
        let entry = Arc::new(RunEntry::new(
            original.run_id,
            session.context.session_id.clone(),
            original.stream_id,
            Arc::new(EventLedger::new(AcpLedgerLimits::default(), None)),
            original.active.turn.cancellation(),
        ));
        services.runs.register(entry.clone()).await?;
        let mut owner = RunDriverOwner {
            entry: entry.clone(),
            registry: Arc::downgrade(&services.runs.runs),
            lease: Some(original.active),
            started: false,
        };
        let (started, observed) = tokio::sync::oneshot::channel();
        // Inject loss at the private resource-owner boundary, not a fake
        // Agent terminal. The real two-entry integration tests drive Agents.
        let task = tokio::spawn(async move {
            owner.started = true;
            let _ = started.send(());
            std::future::pending::<()>().await;
            drop(owner);
        });
        observed
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        task.abort();
        assert!(task.await.is_err());
        let error = entry.wait_result().await.err().ok_or_else(|| {
            ReactError::Other("aborted driver owner manufactured a receipt".to_string())
        })?;
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "observation": "not_settled",
                "run_id": entry.run_id,
                "reason": "original ACP driver task ended before its receipt",
            }))
        );
        assert!(entry.receipt().is_none());
        assert!(entry.cancellation().is_cancelled());
        assert!(session.wait_until_idle().now_or_never().is_none());
        let _ = services.remove_run(&entry.run_id).await;
        assert!(services.run(&entry.run_id).await.is_some());
        assert_eq!(
            services
                .cancel_and_wait_runs_report(Duration::from_millis(1))
                .await,
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn completed_run_driver_releases_entry_without_a_registry_cycle() -> Result<()> {
        let agent = CapturingAgent::default();
        let (services, session) = prepared_session(agent.clone()).await?;
        let weak_services = Arc::downgrade(&services);
        let original = spec(session.clone(), TurnMode::Chat, false, None)?;
        let (entry, task) = services.prepare_run(original).await?;
        let weak_entry = Arc::downgrade(&entry);
        task.await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let receipt = entry.receipt().ok_or_else(|| {
            ReactError::Other("original completed driver did not publish receipt".to_string())
        })?;
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        assert_eq!(
            agent
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            1
        );
        assert!(session.wait_until_idle().now_or_never().is_some());
        let removed = services.remove_run(&entry.run_id).await;
        assert!(removed.is_some_and(|value| Arc::ptr_eq(&value, &entry)));
        drop(entry);
        assert!(weak_entry.upgrade().is_none());
        drop(services);
        assert!(weak_services.upgrade().is_none());
        Ok(())
    }

    async fn assert_identity(mode: TurnMode, structured: bool) -> Result<()> {
        let agent = CapturingAgent::default();
        let (services, session) = prepared_session(agent.clone()).await?;
        let original = spec(session, mode, structured, None)?;
        let expected = original.active.turn.id().to_string();
        let (entry, task) = services.prepare_run(original).await?;
        task.await.map_err(|e| ReactError::Other(e.to_string()))?;
        let calls = agent.calls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(calls.len(), 1);
        let (text, context) = calls
            .first()
            .ok_or_else(|| ReactError::Other("Agent was not invoked".to_string()))?;
        assert_eq!(text, "original input 中文🌊");
        assert_eq!(
            context.runtime.as_ref().and_then(|r| r.turn_id.as_deref()),
            Some(expected.as_str())
        );
        assert!(
            context
                .runtime
                .as_ref()
                .and_then(|r| r.run_id.as_ref())
                .is_none(),
            "a chat Turn must not invent a TaskRun identity"
        );
        let receipt = entry
            .receipt()
            .ok_or_else(|| ReactError::Other("original receipt missing".to_string()))?;
        assert_eq!(receipt.turn_id.as_str(), expected);
        assert_eq!(receipt.outcome, TurnOutcome::Completed);
        Ok(())
    }

    #[derive(Clone)]
    struct ForeignObservationAgent;

    impl Agent for ForeignObservationAgent {
        fn name(&self) -> &str {
            "foreign-observer"
        }
        fn model_name(&self) -> &str {
            "none"
        }
        fn system_prompt(&self) -> &str {
            ""
        }
        fn execute<'a>(&'a self, _task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async {
                Err(ReactError::ExecutionNotSettled {
                    run_id: "foreign-physical-run".to_string(),
                    execution_id: Some("foreign-physical-attempt".to_string()),
                })
            })
        }
        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            Box::pin(async {
                Err(ReactError::ExecutionNotSettled {
                    run_id: "foreign-physical-run".to_string(),
                    execution_id: Some("foreign-physical-attempt".to_string()),
                })
            })
        }
    }

    #[tokio::test]
    async fn run_observation_wakes_all_waiters_without_settling_receipt() -> Result<()> {
        let (services, session) = prepared_session(ForeignObservationAgent).await?;
        let (entry, task) = services
            .prepare_run(spec(session.clone(), TurnMode::Execute, false, None)?)
            .await?;
        let mut waiters = (0..16)
            .map(|_| Box::pin(entry.wait_result()))
            .collect::<Vec<_>>();
        for waiter in &mut waiters {
            assert!(futures::poll!(waiter.as_mut()).is_pending());
        }
        let mut receipt_only = Box::pin(entry.wait_receipt());
        assert!(futures::poll!(receipt_only.as_mut()).is_pending());
        task.await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let expected = serde_json::json!({
            "observation": "not_settled", "run_id": "foreign-physical-run",
            "execution_id": "foreign-physical-attempt",
        });
        let results =
            tokio::time::timeout(Duration::from_secs(1), futures::future::join_all(waiters))
                .await
                .map_err(|_| {
                    ReactError::Other("Run observation lost a waiter wakeup".to_string())
                })?;
        for result in results {
            assert_eq!(
                result.err().and_then(|error| error.data),
                Some(expected.clone())
            );
        }
        assert_eq!(
            entry.wait_result().await.err().and_then(|error| error.data),
            Some(expected)
        );
        assert!(futures::poll!(receipt_only.as_mut()).is_pending());
        assert!(entry.receipt().is_none());
        assert!(session.begin_turn().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn real_run_receipt_precedes_observation_and_rejects_late_error() -> Result<()> {
        let (services, session) = prepared_session(ForeignObservationAgent).await?;
        let (entry, task) = services
            .prepare_run(spec(session.clone(), TurnMode::Execute, false, None)?)
            .await?;
        task.await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(entry.wait_result().await.is_err());
        // Exercise the authority's real settle operation, rather than treating
        // driver task completion or observation as a receipt.
        entry.settle(TurnReceipt::cancelled(entry.run_id.clone())?);
        let receipt = entry.wait_receipt().await;
        let result = entry
            .wait_result()
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(Arc::ptr_eq(&receipt, &result));
        let next_lease = session.begin_turn()?;
        entry.observe_driver_error(
            next_lease,
            agent_client_protocol::Error::internal_error().data("stale observation"),
        );
        let result = entry
            .wait_result()
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(Arc::ptr_eq(&receipt, &result));
        assert!(!entry.is_running());
        assert!(
            session.begin_turn().is_ok(),
            "a late observation must not retain a new lease"
        );
        Ok(())
    }

    #[tokio::test]
    async fn foreign_setup_observation_retains_same_session_lease_and_registry_identity()
    -> Result<()> {
        let (services, session) = prepared_session(ForeignObservationAgent).await?;
        let original = spec(session.clone(), TurnMode::Execute, false, None)?;
        let run_id = original.run_id.clone();
        let (entry, task) = services.prepare_run(original).await?;
        task.await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let error = entry.wait_result().await.err().ok_or_else(|| {
            ReactError::Other("foreign observation unexpectedly returned success".to_string())
        })?;
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "observation": "not_settled", "run_id": "foreign-physical-run",
                "execution_id": "foreign-physical-attempt",
            }))
        );
        assert!(entry.is_running());
        assert!(entry.receipt().is_none());
        assert_eq!(
            entry
                .unsettled_lease
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|lease| lease.turn.id()),
            Some(run_id.as_str())
        );
        assert!(session.begin_turn().is_err());
        let recovered = services.run(&run_id).await.ok_or_else(|| {
            ReactError::Other("uncertain original Run entry disappeared".to_string())
        })?;
        assert!(Arc::ptr_eq(&entry, &recovered));
        let removed = services.remove_run(&run_id).await;
        assert!(
            removed
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, &entry))
        );
        assert!(services.run(&run_id).await.is_some());
        assert!(entry.cancellation().is_cancelled());
        assert!(session.begin_turn().is_err());
        assert_eq!(
            services
                .runs
                .cancel_and_wait(Duration::from_millis(1))
                .await,
            1
        );
        assert!(entry.receipt().is_none());
        assert!(session.begin_turn().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn text_chat_uses_reserved_turn_identity_in_actual_agent_context() -> Result<()> {
        assert_identity(TurnMode::Chat, false).await
    }
    #[tokio::test]
    async fn structured_chat_uses_reserved_turn_identity_in_actual_agent_context() -> Result<()> {
        assert_identity(TurnMode::Chat, true).await
    }
    #[tokio::test]
    async fn text_execute_uses_reserved_turn_identity_in_actual_agent_context() -> Result<()> {
        assert_identity(TurnMode::Execute, false).await
    }
    #[tokio::test]
    async fn structured_execute_uses_reserved_turn_identity_in_actual_agent_context() -> Result<()>
    {
        assert_identity(TurnMode::Execute, true).await
    }

    #[tokio::test]
    async fn original_task_lineage_and_invocation_permissions_survive_turn_binding() -> Result<()> {
        let agent = CapturingAgent::default();
        let (services, session) = prepared_session(agent.clone()).await?;
        let policy = NestedDelegationPolicy {
            can_spawn_subagents: true,
            delegate_depth: 1,
            max_delegate_depth: 3,
        };
        let lineage = SubagentLineage {
            run_id: Some("TaskRun-original".to_string()),
            task_id: Some("PlanTask-original".to_string()),
            execution_id: Some("attempt-original".to_string()),
            ..Default::default()
        };
        let invocation = AgentInvocationContext {
            runtime: Some(ExternalRunContext {
                run_id: Some("TaskRun-original".to_string()),
                conversation_id: Some("conversation-original".to_string()),
                delegation_policy: Some(policy),
                subagent_lineage: Some(lineage.clone()),
                ..Default::default()
            }),
            working_dir: Some(std::env::current_dir()?),
            disabled_tools: Some(["blocked".to_string()].into()),
            visible_tools: Some(["allowed".to_string()].into()),
            history: Some(vec![Message::user("prior history".to_string())]),
            ..Default::default()
        };
        let original = spec(session, TurnMode::Execute, false, Some(invocation))?;
        let expected = original.run_id.clone();
        let (entry, task) = services.prepare_run(original).await?;
        task.await.map_err(|e| ReactError::Other(e.to_string()))?;
        let calls = agent.calls.lock().unwrap_or_else(|e| e.into_inner());
        let (_, context) = calls
            .first()
            .ok_or_else(|| ReactError::Other("Agent not invoked".to_string()))?;
        let runtime = context
            .runtime
            .as_ref()
            .ok_or_else(|| ReactError::Other("context runtime missing".to_string()))?;
        assert_eq!(runtime.turn_id.as_deref(), Some(expected.as_str()));
        assert_eq!(runtime.run_id.as_deref(), Some("TaskRun-original"));
        assert_eq!(
            runtime.conversation_id.as_deref(),
            Some("conversation-original")
        );
        assert_eq!(runtime.delegation_policy, Some(policy));
        assert_eq!(runtime.subagent_lineage, Some(lineage));
        assert_eq!(context.working_dir, Some(std::env::current_dir()?));
        assert_eq!(context.disabled_tools, Some(["blocked".to_string()].into()));
        assert_eq!(context.visible_tools, Some(["allowed".to_string()].into()));
        assert_eq!(
            context
                .history
                .as_ref()
                .and_then(|h| h.first())
                .and_then(|m| m.content.as_text())
                .as_deref(),
            Some("prior history")
        );
        assert_eq!(
            entry.receipt().map(|r| r.outcome.clone()),
            Some(TurnOutcome::Completed)
        );
        Ok(())
    }
    #[tokio::test]
    async fn explicit_conflicting_turn_rejects_before_publication_or_agent_invocation() -> Result<()>
    {
        let agent = CapturingAgent::default();
        let (services, session) = prepared_session(agent.clone()).await?;
        let original = spec(
            session.clone(),
            TurnMode::Chat,
            false,
            Some(AgentInvocationContext {
                runtime: Some(ExternalRunContext {
                    turn_id: Some("other-turn".to_string()),
                    run_id: Some("TaskRun-original".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )?;
        let run_id = original.run_id.clone();
        let result = services.prepare_run(original).await;
        assert!(
            matches!(result, Err(ReactError::Other(message)) if message.contains("invocation Turn identity conflicts"))
        );
        assert!(services.run(&run_id).await.is_none());
        assert!(
            agent
                .calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        );
        let replacement = session.begin_turn()?;
        assert_ne!(replacement.turn.id(), run_id);
        Ok(())
    }

    #[tokio::test]
    async fn conflicting_run_or_request_identity_cannot_replace_reserved_turn() -> Result<()> {
        for replace_request in [false, true] {
            let agent = CapturingAgent::default();
            let (services, session) = prepared_session(agent.clone()).await?;
            let mut original = spec(session.clone(), TurnMode::Chat, false, None)?;
            let run_id = original.run_id.clone();
            if replace_request {
                original.turn = TurnRequest::new(
                    EventIdentity::for_chat(
                        Some(session.context.session_id.to_string()),
                        "forged-turn".to_string(),
                        "forged-message".to_string(),
                        None,
                    )?,
                    "same input",
                );
            } else {
                original.run_id = "forged-run".to_string();
            }
            let result = services.prepare_run(original).await;
            assert!(
                matches!(result, Err(ReactError::Other(message)) if message.contains("Run identity must match"))
            );
            assert!(services.run(&run_id).await.is_none());
            assert!(services.run("forged-run").await.is_none());
            assert!(
                agent
                    .calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty()
            );
            let replacement = session.begin_turn()?;
            assert_ne!(replacement.turn.id(), run_id);
        }
        Ok(())
    }

    #[tokio::test]
    async fn explicit_matching_turn_is_preserved_without_reallocating_identity() -> Result<()> {
        let agent = CapturingAgent::default();
        let (services, session) = prepared_session(agent.clone()).await?;
        let mut original = spec(session, TurnMode::Chat, false, None)?;
        let run_id = original.run_id.clone();
        original.turn.invocation = Some(AgentInvocationContext {
            runtime: Some(ExternalRunContext {
                turn_id: Some(run_id.clone()),
                run_id: Some("TaskRun-retained".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let (entry, task) = services.prepare_run(original).await?;
        task.await.map_err(|e| ReactError::Other(e.to_string()))?;
        let calls = agent.calls.lock().unwrap_or_else(|e| e.into_inner());
        let runtime = calls
            .first()
            .and_then(|(_, context)| context.runtime.as_ref())
            .ok_or_else(|| ReactError::Other("Agent runtime missing".to_string()))?;
        assert_eq!(runtime.turn_id.as_deref(), Some(run_id.as_str()));
        assert_eq!(runtime.run_id.as_deref(), Some("TaskRun-retained"));
        assert_eq!(entry.receipt().map(|r| r.turn_id.to_string()), Some(run_id));
        Ok(())
    }
}
