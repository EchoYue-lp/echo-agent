use crate::agent::{Agent, CancellationToken};
use crate::error::{ReactError, Result};
use agent_client_protocol::schema::v1::{
    ClientCapabilities, McpServer, Meta, NewSessionRequest, SessionId,
};
use futures::future::BoxFuture;
use futures::future::join_all;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::{Mutex, Notify, RwLock};

/// Complete ACP context used to create one independent framework Agent.
///
/// Implementations may delegate ordinary construction to
/// [`crate::agent::factory::AgentFactory`]. This protocol-specific hook also
/// carries asynchronous Session setup inputs that the generic factory config
/// does not own, including MCP declarations and negotiated Client capability.
#[derive(Debug, Clone)]
pub struct AcpSessionContext {
    /// Generated stable ACP Session identity.
    pub session_id: SessionId,
    /// Absolute primary working directory from `session/new`.
    pub cwd: PathBuf,
    /// Additional absolute workspace roots requested by the Client.
    pub additional_directories: Vec<PathBuf>,
    /// MCP declarations that must be prepared before the Agent is returned.
    pub mcp_servers: Vec<McpServer>,
    /// Client capability snapshot captured during `initialize`.
    pub client_capabilities: ClientCapabilities,
    /// Namespaced request metadata preserved for the Session factory.
    pub meta: Option<Meta>,
}

/// Creates the independent framework Agent that owns one ACP Session's history.
///
/// Stable ACP requires ResourceLink prompts. Returned Agents must therefore
/// implement the structured chat methods when the Client sends a ResourceLink;
/// a text-only Agent fails that Prompt explicitly instead of receiving a
/// flattened private text convention. The factory must also prepare every MCP
/// declaration in [`AcpSessionContext`] before it returns successfully.
pub trait AcpSessionFactory: Send + Sync + 'static {
    /// Prepare and return a new Agent that exclusively owns this Session.
    fn create_session(
        &self,
        context: AcpSessionContext,
    ) -> BoxFuture<'static, Result<Box<dyn Agent>>>;
}

impl<F, Fut> AcpSessionFactory for F
where
    F: Fn(AcpSessionContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Box<dyn Agent>>> + Send + 'static,
{
    fn create_session(
        &self,
        context: AcpSessionContext,
    ) -> BoxFuture<'static, Result<Box<dyn Agent>>> {
        Box::pin((self)(context))
    }
}

/// Cancellation authority of one in-flight execution on a Session.
#[derive(Clone)]
pub struct ActiveTurn {
    pub(crate) id: String,
    pub(crate) message_id: String,
    pub(crate) cancel: CancellationToken,
}

impl ActiveTurn {
    /// Stable run identity of the active execution (`<session>:turn:<n>`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Cancellation token shared with the framework driver.
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Exclusive lease on a Session's single active run slot. Dropping the lease
/// releases the slot; standard Prompts and extension Runs hold exactly one
/// lease for the lifetime of their driver.
pub struct ActiveTurnLease {
    pub(crate) session: Weak<AcpSession>,
    pub turn: ActiveTurn,
}

impl Drop for ActiveTurnLease {
    fn drop(&mut self) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let mut slot = session
            .turn
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot
            .active
            .as_ref()
            .is_some_and(|active| active.id == self.turn.id)
        {
            slot.active = None;
            session.turn_settled.notify_waiters();
        }
    }
}

struct TurnSlot {
    next_sequence: u64,
    active: Option<ActiveTurn>,
}

#[derive(Debug, Clone)]
enum SessionClosePhase {
    Prepared,
    Running,
    // None means the original owner ended without a canonical observation
    // identity. Retain this receipt/Agent; a waiter is not a replacement owner.
    NotSettled(Option<(String, Option<String>)>),
    Failed(String),
    Settled,
}

#[derive(Debug)]
struct SessionCloseAttempt {
    next_generation: u64,
    current: Option<Arc<SessionCloseReceipt>>,
}

#[derive(Debug)]
struct SessionCloseReceipt {
    generation: u64,
    phase: StdMutex<SessionClosePhase>,
    settled: Notify,
    #[cfg(test)]
    abort: StdMutex<Option<tokio::task::AbortHandle>>,
}

/// Typed capability for the second phase of one ACP Session close attempt.
///
/// The lease is cloneable so framework and facade waiters can join the same
/// attempt. An ordinary returned failure remains stable for this generation;
/// callers may begin again for its retry. A not-settled observation keeps this
/// same generation and cannot authorize a repeated close mutation.
#[derive(Clone)]
pub struct SessionCloseLease {
    registry_id: String,
    session_id: SessionId,
    session: Arc<AcpSession>,
    receipt: Arc<SessionCloseReceipt>,
}

impl SessionCloseLease {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    pub fn generation(&self) -> u64 {
        self.receipt.generation
    }
}

impl std::fmt::Debug for SessionCloseLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionCloseLease")
            .field("session_id", &self.session_id)
            .field("generation", &self.receipt.generation)
            .finish_non_exhaustive()
    }
}

/// One ACP Session: an independent framework Agent plus the Session-scoped
/// run slot. Shared by the standard profile and negotiated extension
/// profiles — there is deliberately no second Session map.
pub struct AcpSession {
    pub context: AcpSessionContext,
    pub agent: Arc<dyn Agent>,
    turn: StdMutex<TurnSlot>,
    turn_settled: Notify,
    closed: AtomicBool,
    close_attempt: StdMutex<SessionCloseAttempt>,
}

impl AcpSession {
    fn new(context: AcpSessionContext, agent: Box<dyn Agent>) -> Self {
        Self {
            context,
            agent: Arc::from(agent),
            turn: StdMutex::new(TurnSlot {
                next_sequence: 0,
                active: None,
            }),
            turn_settled: Notify::new(),
            closed: AtomicBool::new(false),
            close_attempt: StdMutex::new(SessionCloseAttempt {
                next_generation: 0,
                current: None,
            }),
        }
    }

    pub fn begin_turn(self: &Arc<Self>) -> Result<ActiveTurnLease> {
        let mut slot = self.turn.lock().unwrap_or_else(|error| error.into_inner());
        if self.closed.load(Ordering::Acquire) {
            return Err(ReactError::Other("ACP Session is closed".to_string()));
        }
        if slot.active.is_some() {
            return Err(ReactError::Other(
                "ACP Session already has an active Prompt Turn".to_string(),
            ));
        }
        let next = slot
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| ReactError::Other("ACP Session turn sequence exhausted".to_string()))?;
        slot.next_sequence = next;
        let session_id = self.context.session_id.to_string();
        let active = ActiveTurn {
            id: format!("{session_id}:turn:{next}"),
            message_id: format!("{session_id}:message:{next}"),
            cancel: CancellationToken::new(),
        };
        slot.active = Some(active.clone());
        Ok(ActiveTurnLease {
            session: Arc::downgrade(self),
            turn: active,
        })
    }

    /// Reserve this Session's native Turn identities before admitting a Run
    /// after recovery. The framework owns the `<session>:turn:<n>` namespace;
    /// adapters pass persisted identities without interpreting or replacing them.
    ///
    /// All identities must belong to this Session and contain a positive,
    /// canonical decimal `u64`. A restored `u64::MAX` frontier keeps history
    /// available while subsequent Turn admission reports exhaustion. Validation is
    /// atomic with allocation: an invalid batch, active Turn, or closed Session
    /// leaves the allocation frontier unchanged. Repeated reservations are
    /// idempotent and never lower an existing frontier. This operation neither
    /// resumes historical execution nor changes its receipt or journal.
    ///
    /// ```
    /// # #[cfg(feature = "acp")]
    /// # fn restore(session: &echo_agent::acp::AcpSession, persisted_ids: &[String])
    /// #     -> echo_agent::error::Result<()> {
    /// session.reserve_recovered_turn_ids(persisted_ids)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn reserve_recovered_turn_ids(&self, run_ids: &[String]) -> Result<()> {
        let mut slot = self.turn.lock().unwrap_or_else(|error| error.into_inner());
        if self.closed.load(Ordering::Acquire) {
            return Err(ReactError::Other("ACP Session is closed".to_string()));
        }
        if slot.active.is_some() {
            return Err(ReactError::Other(
                "ACP Session cannot reserve recovered Turn identities while a Turn is active"
                    .to_string(),
            ));
        }
        let prefix = format!("{}:turn:", self.context.session_id);
        let mut recovered_sequence = slot.next_sequence;
        for run_id in run_ids {
            let sequence_text = run_id.strip_prefix(&prefix).ok_or_else(|| {
                ReactError::Other(
                    "Recovered ACP Turn identity does not belong to this Session namespace"
                        .to_string(),
                )
            })?;
            let sequence = sequence_text.parse::<u64>().map_err(|_| {
                ReactError::Other(
                    "Recovered ACP Turn identity must contain a canonical positive u64 sequence"
                        .to_string(),
                )
            })?;
            if sequence == 0 || sequence_text != sequence.to_string() {
                return Err(ReactError::Other(
                    "Recovered ACP Turn identity must contain a canonical positive u64 sequence"
                        .to_string(),
                ));
            }
            recovered_sequence = recovered_sequence.max(sequence);
        }
        // Commit only after every identity validates. A partial recovery must
        // not change future allocation when its caller rolls back publication.
        slot.next_sequence = recovered_sequence;
        Ok(())
    }

    pub fn cancel_active(&self) -> bool {
        let cancel = self
            .turn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
            .as_ref()
            .map(|active| active.cancel.clone());
        if let Some(cancel) = cancel {
            cancel.cancel();
            true
        } else {
            false
        }
    }

    fn mark_closed(&self) {
        let _slot = self.turn.lock().unwrap_or_else(|error| error.into_inner());
        self.closed.store(true, Ordering::Release);
    }

    fn prepare_close(self: &Arc<Self>, registry_id: &str) -> Result<SessionCloseLease> {
        self.mark_closed();
        self.cancel_active();
        let mut attempt = self
            .close_attempt
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current_phase = attempt.current.as_ref().map(|receipt| {
            receipt
                .phase
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        });
        let receipt = match current_phase {
            Some(
                SessionClosePhase::Prepared
                | SessionClosePhase::Running
                | SessionClosePhase::NotSettled(_)
                | SessionClosePhase::Settled,
            ) => attempt.current.as_ref().cloned().ok_or_else(|| {
                ReactError::Other("ACP Session close receipt disappeared".to_string())
            })?,
            Some(SessionClosePhase::Failed(_)) | None => {
                let next = attempt.next_generation.checked_add(1).ok_or_else(|| {
                    ReactError::Other("ACP Session close generation exhausted".to_string())
                })?;
                attempt.next_generation = next;
                let receipt = Arc::new(SessionCloseReceipt {
                    generation: next,
                    phase: StdMutex::new(SessionClosePhase::Prepared),
                    settled: Notify::new(),
                    #[cfg(test)]
                    abort: StdMutex::new(None),
                });
                attempt.current = Some(Arc::clone(&receipt));
                receipt
            }
        };
        Ok(SessionCloseLease {
            registry_id: registry_id.to_string(),
            session_id: self.context.session_id.clone(),
            session: Arc::clone(self),
            receipt,
        })
    }

    pub async fn wait_until_idle(&self) {
        loop {
            let notified = self.turn_settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .turn
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .active
                .is_none()
            {
                return;
            }
            notified.await;
        }
    }
}

pub struct SessionRegistry {
    registry_id: String,
    factory: Arc<dyn AcpSessionFactory>,
    max_sessions: usize,
    admission_open: AtomicBool,
    creation_gate: Mutex<()>,
    client_capabilities: RwLock<Option<ClientCapabilities>>,
    sessions: RwLock<HashMap<SessionId, Arc<AcpSession>>>,
}

enum FinishCloseAction {
    Start,
    Wait,
    Settled,
    Observation {
        run_id: String,
        execution_id: Option<String>,
    },
    Failed(String),
}

#[cfg(test)]
struct FinishCloseBarrier {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

struct SessionCloseTaskOwner {
    receipt: Arc<SessionCloseReceipt>,
    armed: bool,
}

impl SessionCloseTaskOwner {
    fn new(receipt: Arc<SessionCloseReceipt>) -> Self {
        Self {
            receipt,
            armed: true,
        }
    }

    fn settle(&mut self, phase: SessionClosePhase) {
        *self
            .receipt
            .phase
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = phase;
        #[cfg(test)]
        {
            *self
                .receipt
                .abort
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = None;
        }
        self.armed = false;
        self.receipt.settled.notify_waiters();
    }
}

impl Drop for SessionCloseTaskOwner {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut phase = self
            .receipt
            .phase
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if matches!(&*phase, SessionClosePhase::Running) {
            // Losing the local close task is not a physical backend receipt.
            // There may be no real Run identity to return, so keep the existing
            // Notify observation pending instead of fabricating a terminal.
            *phase = SessionClosePhase::NotSettled(None);
        }
        #[cfg(test)]
        {
            *self
                .receipt
                .abort
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = None;
        }
        drop(phase);
        self.receipt.settled.notify_waiters();
    }
}

async fn run_session_close_attempt(session: Arc<AcpSession>, mut owner: SessionCloseTaskOwner) {
    session.wait_until_idle().await;
    let result = session.agent.close().await;
    owner.settle(match result {
        Ok(()) => SessionClosePhase::Settled,
        Err(ReactError::ExecutionNotSettled {
            run_id,
            execution_id,
        }) => SessionClosePhase::NotSettled(Some((run_id, execution_id))),
        Err(error) => SessionClosePhase::Failed(error.to_string()),
    });
}

impl SessionRegistry {
    pub fn new(factory: Arc<dyn AcpSessionFactory>, max_sessions: usize) -> Self {
        Self {
            registry_id: uuid::Uuid::new_v4().to_string(),
            factory,
            max_sessions,
            admission_open: AtomicBool::new(true),
            creation_gate: Mutex::new(()),
            client_capabilities: RwLock::new(None),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn initialize(&self, capabilities: ClientCapabilities) {
        *self.client_capabilities.write().await = Some(capabilities);
    }

    pub fn close_admission(&self) {
        self.admission_open.store(false, Ordering::Release);
    }

    fn ensure_admission(&self) -> Result<()> {
        if self.admission_open.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ReactError::Other(
                "ACP Session registry is closing".to_string(),
            ))
        }
    }

    pub async fn create(&self, request: NewSessionRequest) -> Result<SessionId> {
        validate_session_paths(&request)?;
        let context = AcpSessionContext {
            session_id: SessionId::new(format!("sess_{}", uuid::Uuid::new_v4())),
            cwd: request.cwd,
            additional_directories: request.additional_directories,
            mcp_servers: request.mcp_servers,
            client_capabilities: self.capabilities().await?,
            meta: request.meta,
        };
        self.insert_session(context).await
    }

    /// Client capability snapshot captured during `initialize`.
    pub async fn capabilities(&self) -> Result<ClientCapabilities> {
        self.client_capabilities
            .read()
            .await
            .clone()
            .ok_or_else(|| ReactError::Other("ACP connection is not initialized".to_string()))
    }

    /// Insert a fully described Session, creating its independent Agent
    /// through the connection's single factory. Standard `session/new` and
    /// extension profile create/load both funnel through here so the
    /// Session limit and per-Session Agent ownership stay authoritative.
    pub async fn insert_session(&self, context: AcpSessionContext) -> Result<SessionId> {
        let _creation = self.creation_gate.lock().await;
        self.ensure_admission()?;
        if self.sessions.read().await.len() >= self.max_sessions {
            return Err(ReactError::Other(format!(
                "ACP Session limit {} reached",
                self.max_sessions
            )));
        }
        if self.sessions.read().await.contains_key(&context.session_id) {
            return Err(ReactError::Other(format!(
                "ACP Session {} is already registered",
                context.session_id
            )));
        }
        let agent = self.factory.create_session(context.clone()).await?;
        // ACP Sessions own distinct Agents, so binding the Session cwd as the
        // Agent default cannot leak across conversations. This also keeps the
        // adapter compatible with text-only Agent implementations.
        agent.set_working_dir(Some(context.cwd.clone()));
        let session_id = context.session_id.clone();
        let session = Arc::new(AcpSession::new(context, agent));
        if !self.admission_open.load(Ordering::Acquire) {
            // Shutdown that raced the async factory still owns this Agent.
            // Retain it as a closed Session for close_all after the creation
            // gate is released; the caller must not receive a usable handle.
            session.mark_closed();
            self.sessions.write().await.insert(session_id, session);
            return Err(ReactError::Other(
                "ACP Session registry is closing".to_string(),
            ));
        }
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), session);
        Ok(session_id)
    }

    pub async fn get(&self, session_id: &SessionId) -> Option<Arc<AcpSession>> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// Begin closing one Session without waiting for its active run.
    ///
    /// The returned lease proves that new turns are fenced and the current
    /// turn cancellation has been signalled. Facades may drain their own
    /// operation leases before calling [`Self::finish_close_session`].
    pub async fn begin_close_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionCloseLease>> {
        let _creation = self.creation_gate.lock().await;
        let session = self.sessions.read().await.get(session_id).cloned();
        let Some(session) = session else {
            return Ok(None);
        };
        session.prepare_close(&self.registry_id).map(Some)
    }

    /// Finish a close attempt after facade-owned operations have drained.
    ///
    /// Concurrent callers with the same lease join one Agent close attempt.
    /// An ordinary failed attempt preserves the Session and returns the same
    /// failure to every waiter; begin again for its retry. A nonterminal
    /// observation preserves its identity and original generation. Unknown
    /// owner loss without an identity keeps the existing Notify wait pending.
    pub async fn finish_close_session(&self, lease: SessionCloseLease) -> Result<()> {
        self.finish_close_session_inner(lease, None).await
    }

    async fn finish_close_session_inner(
        &self,
        lease: SessionCloseLease,
        #[cfg(test)] mut barrier: Option<FinishCloseBarrier>,
        #[cfg(not(test))] _barrier: Option<()>,
    ) -> Result<()> {
        if lease.registry_id != self.registry_id {
            return Err(ReactError::Other(format!(
                "ACP Session {} close lease belongs to another registry",
                lease.session_id
            )));
        }

        loop {
            let notified = lease.receipt.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let action = {
                let mut phase = lease
                    .receipt
                    .phase
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                match phase.clone() {
                    SessionClosePhase::Prepared => {
                        *phase = SessionClosePhase::Running;
                        FinishCloseAction::Start
                    }
                    SessionClosePhase::Running | SessionClosePhase::NotSettled(None) => {
                        FinishCloseAction::Wait
                    }
                    SessionClosePhase::NotSettled(Some((run_id, execution_id))) => {
                        FinishCloseAction::Observation {
                            run_id,
                            execution_id,
                        }
                    }
                    SessionClosePhase::Settled => FinishCloseAction::Settled,
                    SessionClosePhase::Failed(error) => FinishCloseAction::Failed(error),
                }
            };
            match action {
                FinishCloseAction::Start => {
                    let session = Arc::clone(&lease.session);
                    match tokio::runtime::Handle::try_current() {
                        Ok(handle) => {
                            let owner = SessionCloseTaskOwner::new(Arc::clone(&lease.receipt));
                            let close_task =
                                handle.spawn(run_session_close_attempt(session, owner));
                            #[cfg(test)]
                            {
                                *lease
                                    .receipt
                                    .abort
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner()) =
                                    Some(close_task.abort_handle());
                            }
                            #[cfg(not(test))]
                            let _close_task = close_task;
                        }
                        Err(error) => {
                            let message = format!(
                                "ACP Session close requires an active Tokio runtime: {error}"
                            );
                            *lease
                                .receipt
                                .phase
                                .lock()
                                .unwrap_or_else(|lock_error| lock_error.into_inner()) =
                                SessionClosePhase::Failed(message);
                            lease.receipt.settled.notify_waiters();
                        }
                    }
                }
                FinishCloseAction::Wait => {
                    #[cfg(test)]
                    if let Some(barrier) = barrier.take() {
                        barrier.entered.notify_waiters();
                        barrier.release.notified().await;
                        continue;
                    }
                    notified.await;
                }
                FinishCloseAction::Settled => {
                    let mut sessions = self.sessions.write().await;
                    if sessions
                        .get(&lease.session_id)
                        .is_some_and(|current| Arc::ptr_eq(current, &lease.session))
                    {
                        sessions.remove(&lease.session_id);
                    }
                    return Ok(());
                }
                FinishCloseAction::Observation {
                    run_id,
                    execution_id,
                } => {
                    return Err(ReactError::ExecutionNotSettled {
                        run_id,
                        execution_id,
                    });
                }
                FinishCloseAction::Failed(error) => {
                    return Err(ReactError::Other(format!(
                        "ACP Session {} close did not settle: {error}",
                        lease.session_id
                    )));
                }
            }
        }
    }

    /// Close one Session through the canonical two-phase lifecycle.
    pub async fn close_session(&self, session_id: &SessionId) -> Result<bool> {
        let Some(lease) = self.begin_close_session(session_id).await? else {
            return Ok(false);
        };
        self.finish_close_session(lease).await?;
        Ok(true)
    }

    pub async fn cancel(&self, session_id: &SessionId) -> bool {
        let session = self.get(session_id).await;
        if let Some(session) = session {
            session.cancel_active()
        } else {
            false
        }
    }

    pub async fn close_all(&self) -> Result<()> {
        self.close_admission();
        let leases = {
            let _creation = self.creation_gate.lock().await;
            let guard = self.sessions.read().await;
            guard
                .values()
                .map(|session| session.prepare_close(&self.registry_id))
                .collect::<Result<Vec<_>>>()?
        };
        let results = join_all(leases.into_iter().map(|lease| async move {
            let id = lease.session_id.clone();
            (id, self.finish_close_session(lease).await)
        }))
        .await;
        let mut failures = Vec::new();
        let mut observation = None;
        for (id, result) in results {
            match result {
                Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                    // join_all has already driven every original close. Each
                    // receipt retains its own identity; preserve one original
                    // typed refusal rather than flattening any into Failed.
                    observation.get_or_insert(error);
                }
                Err(error) => failures.push(format!("{id}: {error}")),
                Ok(()) => {}
            }
        }
        if let Some(error) = observation {
            return Err(error);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ReactError::Other(format!(
                "ACP Session close_all failed to settle {} Session(s): {}",
                failures.len(),
                failures.join("; ")
            )))
        }
    }
}

pub(crate) fn validate_session_paths(request: &NewSessionRequest) -> Result<()> {
    if !request.cwd.is_absolute() {
        return Err(ReactError::Other(
            "ACP Session cwd must be absolute".to_string(),
        ));
    }
    if request
        .additional_directories
        .iter()
        .any(|path| !path.is_absolute())
    {
        return Err(ReactError::Other(
            "ACP Session additional directories must be absolute".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentEvent;
    use futures::stream::{self, BoxStream, StreamExt};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct CloseProbeAgent {
        hang: bool,
        close_started: Arc<AtomicBool>,
    }

    struct RetryCloseProbeAgent {
        failures_remaining: std::sync::atomic::AtomicUsize,
        attempts: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct ControlledCloseProbeAgent {
        attempts: Arc<std::sync::atomic::AtomicUsize>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl Agent for CloseProbeAgent {
        fn name(&self) -> &str {
            "close-probe"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.close_started.store(true, Ordering::Release);
                if self.hang {
                    futures::future::pending::<Result<()>>().await
                } else {
                    Ok(())
                }
            })
        }

        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(task.to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            let answer = task.to_string();
            Box::pin(
                async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) },
            )
        }
    }

    impl Agent for RetryCloseProbeAgent {
        fn name(&self) -> &str {
            "retry-close-probe"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
            let attempt = self
                .attempts
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1);
            let fail = self
                .failures_remaining
                .try_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
            Box::pin(async move {
                if fail {
                    Err(ReactError::Other(format!(
                        "injected Session close failure attempt {attempt}"
                    )))
                } else {
                    Ok(())
                }
            })
        }

        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(task.to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            let answer = task.to_string();
            Box::pin(
                async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) },
            )
        }
    }

    impl Agent for ControlledCloseProbeAgent {
        fn name(&self) -> &str {
            "controlled-close-probe"
        }

        fn model_name(&self) -> &str {
            "test"
        }

        fn system_prompt(&self) -> &str {
            "test"
        }

        fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
            self.attempts.fetch_add(1, Ordering::AcqRel);
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            Box::pin(async move {
                started.notify_waiters();
                release.notified().await;
                Ok(())
            })
        }

        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(task.to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            let answer = task.to_string();
            Box::pin(
                async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) },
            )
        }
    }

    type ClosePhysicalJoin =
        futures::future::Shared<BoxFuture<'static, std::result::Result<(), String>>>;

    struct PhysicalCloseObservationAgent {
        attempts: Arc<std::sync::atomic::AtomicUsize>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        report: Option<Arc<Notify>>,
        stopped: Arc<AtomicBool>,
        producer: Arc<StdMutex<Option<ClosePhysicalJoin>>>,
        effect: PathBuf,
    }

    impl Agent for PhysicalCloseObservationAgent {
        fn name(&self) -> &str {
            "physical-close-observation"
        }
        fn model_name(&self) -> &str {
            "test"
        }
        fn system_prompt(&self) -> &str {
            "test"
        }
        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(task.to_string()) })
        }
        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
            let answer = task.to_string();
            Box::pin(
                async move { Ok(stream::iter(vec![Ok(AgentEvent::FinalAnswer(answer))]).boxed()) },
            )
        }
        fn close<'a>(&'a self) -> BoxFuture<'a, Result<()>> {
            use futures::FutureExt;
            Box::pin(async move {
                if self.attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                    let started = self.started.clone();
                    let entered = started.notified();
                    tokio::pin!(entered);
                    entered.as_mut().enable();
                    let notify = started.clone();
                    let release = self.release.clone();
                    let effect = self.effect.clone();
                    let stopped = self.stopped.clone();
                    let producer = tokio::spawn(async move {
                        let write = tokio::fs::write(&effect, b"original-close-entered").await;
                        notify.notify_waiters();
                        write.map_err(|error| error.to_string())?;
                        release.notified().await;
                        tokio::fs::write(&effect, b"original-close-physically-joined")
                            .await
                            .map_err(|error| error.to_string())?;
                        stopped.store(true, Ordering::Release);
                        Ok::<(), String>(())
                    });
                    let joined = async move { producer.await.map_err(|error| error.to_string())? }
                        .boxed()
                        .shared();
                    *self
                        .producer
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(joined);
                    entered.await;
                }
                if let Some(report) = self.report.as_ref() {
                    report.notified().await;
                }
                Err(ReactError::ExecutionNotSettled {
                    run_id: "original-close-run".to_string(),
                    execution_id: Some("original-close-execution".to_string()),
                })
            })
        }
    }

    async fn assert_close_observation_keeps_original_receipt(all: bool) -> Result<()> {
        let directory = tempfile::tempdir()?;
        let effect = directory.path().join("close-effect.txt");
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let producer = Arc::new(StdMutex::new(None::<ClosePhysicalJoin>));
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory unused in controlled close".to_string(),
            ))
        });
        let registry = SessionRegistry::new(unused_factory, 2);
        let session = Arc::new(AcpSession::new(
            context("physical-close"),
            Box::new(PhysicalCloseObservationAgent {
                attempts: attempts.clone(),
                started: Arc::new(Notify::new()),
                release: release.clone(),
                report: None,
                stopped: stopped.clone(),
                producer: producer.clone(),
                effect: effect.clone(),
            }),
        ));
        registry
            .sessions
            .write()
            .await
            .insert(session.context.session_id.clone(), session.clone());
        let peer_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        if all {
            let peer = Arc::new(AcpSession::new(
                context("settled-peer"),
                Box::new(RetryCloseProbeAgent {
                    failures_remaining: std::sync::atomic::AtomicUsize::new(0),
                    attempts: peer_attempts.clone(),
                }),
            ));
            registry
                .sessions
                .write()
                .await
                .insert(peer.context.session_id.clone(), peer);
        }
        let observations = async {
            let original = registry
                .begin_close_session(&session.context.session_id)
                .await?
                .ok_or_else(|| ReactError::Other("original close receipt missing".to_string()))?;
            let first = tokio::time::timeout(Duration::from_secs(1), async {
                if all {
                    registry.close_all().await
                } else {
                    registry.finish_close_session(original.clone()).await
                }
            })
            .await
            .map_err(|_| ReactError::Other("close observation did not return".to_string()))?;
            let repeated = registry
                .begin_close_session(&session.context.session_id)
                .await?
                .ok_or_else(|| ReactError::Other("repeated close receipt missing".to_string()))?;
            let second = tokio::time::timeout(Duration::from_secs(1), async {
                if all {
                    registry.close_all().await
                } else {
                    registry.finish_close_session(repeated.clone()).await
                }
            })
            .await
            .map_err(|_| ReactError::Other("repeated observation did not return".to_string()))?;
            let held_effect = tokio::fs::read(&effect).await?;
            Ok::<_, ReactError>((
                first,
                second,
                original.generation(),
                repeated.generation(),
                attempts.load(Ordering::Acquire),
                stopped.load(Ordering::Acquire),
                held_effect,
            ))
        }
        .await;
        // Always release and join the one actual backend before failing a test.
        release.notify_one();
        let joined = producer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(joined) = joined {
            tokio::time::timeout(Duration::from_secs(1), joined)
                .await
                .map_err(|_| ReactError::Other("original close backend did not join".to_string()))?
                .map_err(ReactError::Other)?;
        }
        let (
            first,
            second,
            generation,
            repeated_generation,
            calls,
            stopped_while_held,
            held_effect,
        ) = observations?;
        let is_observation = |result: &Result<()>| {
            matches!(result,
            Err(ReactError::ExecutionNotSettled { run_id, execution_id })
            if run_id == "original-close-run" && execution_id.as_deref() == Some("original-close-execution"))
        };
        eprintln!(
            "CLOSE_OBSERVATION_BOUNDARY {}",
            serde_json::json!({
                "all": all, "first": format!("{first:?}"), "second": format!("{second:?}"),
                "original_generation":generation, "repeated_generation":repeated_generation,
                "close_calls_while_original_held":calls, "physical_stopped_while_held":stopped_while_held,
                "physical_joined_after_release":stopped.load(Ordering::Acquire)
            })
        );
        assert!(
            is_observation(&first),
            "close must preserve original nonterminal identity"
        );
        assert!(is_observation(&second));
        assert_eq!(
            generation, repeated_generation,
            "observation is not a retry receipt"
        );
        assert_eq!(calls, 1, "repeat observation must not resend close");
        assert!(!stopped_while_held);
        assert_eq!(held_effect, b"original-close-entered");
        assert!(stopped.load(Ordering::Acquire));
        assert_eq!(
            tokio::fs::read(&effect).await?,
            b"original-close-physically-joined"
        );
        assert!(
            registry
                .get(&session.context.session_id)
                .await
                .is_some_and(|current| Arc::ptr_eq(&current, &session))
        );
        assert!(session.begin_turn().is_err());
        if all {
            assert_eq!(
                peer_attempts.load(Ordering::Acquire),
                1,
                "unknown must not skip a known peer close"
            );
            assert!(
                registry
                    .get(&SessionId::new("settled-peer".to_string()))
                    .await
                    .is_none()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn session_close_observation_preserves_identity_and_same_original_receipt() -> Result<()>
    {
        assert_close_observation_keeps_original_receipt(false).await
    }

    #[tokio::test]
    async fn session_close_observation_close_all_joins_peers_without_reissuing_unknown()
    -> Result<()> {
        assert_close_observation_keeps_original_receipt(true).await
    }

    fn context(id: &str) -> AcpSessionContext {
        AcpSessionContext {
            session_id: SessionId::new(id.to_string()),
            cwd: std::env::temp_dir(),
            additional_directories: Vec::new(),
            mcp_servers: Vec::new(),
            client_capabilities: ClientCapabilities::default(),
            meta: None,
        }
    }

    fn recovery_session(id: &str) -> Arc<AcpSession> {
        Arc::new(AcpSession::new(
            context(id),
            Box::new(CloseProbeAgent {
                hang: false,
                close_started: Arc::new(AtomicBool::new(false)),
            }),
        ))
    }

    #[test]
    fn recovered_turn_reservation_continues_after_persisted_identity() -> Result<()> {
        let session = recovery_session("恢复:🧭");
        session.reserve_recovered_turn_ids(&["恢复:🧭:turn:1".to_string()])?;
        let turn = session.begin_turn()?;
        assert_eq!(turn.turn.id(), "恢复:🧭:turn:2");
        assert_eq!(turn.turn.message_id, "恢复:🧭:message:2");
        Ok(())
    }

    #[test]
    fn recovered_turn_reservation_is_idempotent_and_never_lowers_frontier() -> Result<()> {
        let session = recovery_session("resume");
        let persisted = vec!["resume:turn:9".to_string(), "resume:turn:3".to_string()];
        session.reserve_recovered_turn_ids(&persisted)?;
        session.reserve_recovered_turn_ids(&persisted)?;
        let turn = session.begin_turn()?;
        assert_eq!(turn.turn.id(), "resume:turn:10");
        drop(turn);
        session.reserve_recovered_turn_ids(&["resume:turn:1".to_string()])?;
        session.reserve_recovered_turn_ids(&[])?;
        assert_eq!(session.begin_turn()?.turn.id(), "resume:turn:11");
        Ok(())
    }

    #[test]
    fn recovered_turn_reservation_rejects_invalid_batch_without_partial_commit() -> Result<()> {
        for invalid in [
            "other:turn:2",
            "resume:task:2",
            "resume:turn:0",
            "resume:turn:01",
            "resume:turn:+1",
            "resume:turn:1:extra",
            "resume:turn:🧭",
            "resume:turn:18446744073709551616",
        ] {
            let session = recovery_session("resume");
            assert!(
                session
                    .reserve_recovered_turn_ids(&[
                        "resume:turn:17".to_string(),
                        invalid.to_string(),
                    ])
                    .is_err(),
                "invalid recovered identity was accepted: {invalid}"
            );
            assert_eq!(session.begin_turn()?.turn.id(), "resume:turn:1");
        }
        Ok(())
    }

    #[test]
    fn recovered_turn_reservation_preserves_checked_last_allocation() -> Result<()> {
        let session = recovery_session("boundary");
        let before_last = u64::MAX.saturating_sub(1);
        session.reserve_recovered_turn_ids(&[format!("boundary:turn:{before_last}")])?;
        let turn = session.begin_turn()?;
        assert_eq!(turn.turn.id(), format!("boundary:turn:{}", u64::MAX));
        drop(turn);
        assert!(session.begin_turn().is_err());
        session.reserve_recovered_turn_ids(&["boundary:turn:1".to_string()])?;
        assert!(session.begin_turn().is_err());
        Ok(())
    }

    #[test]
    fn recovered_turn_reservation_restores_last_native_identity_without_admission() -> Result<()> {
        let original = recovery_session("exhausted");
        original.reserve_recovered_turn_ids(&[format!(
            "exhausted:turn:{}",
            u64::MAX.saturating_sub(1)
        )])?;
        let last_turn = original.begin_turn()?;
        let persisted_identity = last_turn.turn.id().to_string();
        assert_eq!(persisted_identity, format!("exhausted:turn:{}", u64::MAX));
        drop(last_turn);

        let restored = recovery_session("exhausted");
        restored.reserve_recovered_turn_ids(std::slice::from_ref(&persisted_identity))?;
        restored.reserve_recovered_turn_ids(std::slice::from_ref(&persisted_identity))?;
        assert!(restored.begin_turn().is_err());
        let slot = restored
            .turn
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(slot.next_sequence, u64::MAX);
        assert!(slot.active.is_none());
        assert!(!restored.closed.load(Ordering::Acquire));
        assert_eq!(persisted_identity, format!("exhausted:turn:{}", u64::MAX));
        Ok(())
    }

    #[test]
    fn recovered_turn_reservation_rejects_active_and_closed_sessions() -> Result<()> {
        let session = recovery_session("fenced");
        let turn = session.begin_turn()?;
        assert!(
            session
                .reserve_recovered_turn_ids(&["fenced:turn:20".to_string()])
                .is_err()
        );
        drop(turn);
        assert_eq!(session.begin_turn()?.turn.id(), "fenced:turn:2");
        session.mark_closed();
        assert!(
            session
                .reserve_recovered_turn_ids(&["fenced:turn:20".to_string()])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn recovered_turn_reservation_and_admission_share_one_linearization_point() -> Result<()> {
        let session = recovery_session("race");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let admission = std::thread::spawn({
            let session = Arc::clone(&session);
            let barrier = Arc::clone(&barrier);
            move || {
                barrier.wait();
                session.begin_turn()
            }
        });
        let reservation = std::thread::spawn({
            let session = Arc::clone(&session);
            move || {
                barrier.wait();
                session.reserve_recovered_turn_ids(&["race:turn:9".to_string()])
            }
        });
        // Keep the returned lease alive until both racers have finished. If
        // admission wins, reservation must observe that occupied turn slot.
        let turn = admission
            .join()
            .map_err(|_| ReactError::Other("Turn admission thread failed".to_string()))??;
        let reserved = reservation
            .join()
            .map_err(|_| ReactError::Other("Turn reservation thread failed".to_string()))?;
        if reserved.is_ok() {
            assert_eq!(turn.turn.id(), "race:turn:10");
        } else {
            assert_eq!(turn.turn.id(), "race:turn:1");
        }
        Ok(())
    }

    async fn wait_for(predicate: impl Fn() -> bool) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("timed out waiting for close probe".to_string()))
    }

    #[tokio::test]
    async fn close_all_cancels_every_turn_before_concurrent_agent_close() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = Arc::new(SessionRegistry::new(unused_factory, 2));
        let first_close = Arc::new(AtomicBool::new(false));
        let second_close = Arc::new(AtomicBool::new(false));
        let first = Arc::new(AcpSession::new(
            context("first"),
            Box::new(CloseProbeAgent {
                hang: true,
                close_started: first_close.clone(),
            }),
        ));
        let second = Arc::new(AcpSession::new(
            context("second"),
            Box::new(CloseProbeAgent {
                hang: false,
                close_started: second_close.clone(),
            }),
        ));
        let first_turn = first.begin_turn()?;
        let second_turn = second.begin_turn()?;
        let first_cancel = first_turn.turn.cancel.clone();
        let second_cancel = second_turn.turn.cancel.clone();
        {
            let mut sessions = registry.sessions.write().await;
            sessions.insert(first.context.session_id.clone(), first);
            sessions.insert(second.context.session_id.clone(), second);
        }

        let close_task = tokio::spawn({
            let registry = registry.clone();
            async move { registry.close_all().await }
        });
        wait_for(|| first_cancel.is_cancelled() && second_cancel.is_cancelled()).await?;
        drop(first_turn);
        drop(second_turn);
        wait_for(|| first_close.load(Ordering::Acquire) && second_close.load(Ordering::Acquire))
            .await?;
        close_task.abort();
        let _ = close_task.await;
        Ok(())
    }

    #[tokio::test]
    async fn close_all_retains_failed_sessions_for_retry() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = SessionRegistry::new(unused_factory, 2);
        let failed_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let settled_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let failed = Arc::new(AcpSession::new(
            context("failed"),
            Box::new(RetryCloseProbeAgent {
                failures_remaining: std::sync::atomic::AtomicUsize::new(1),
                attempts: Arc::clone(&failed_attempts),
            }),
        ));
        let settled = Arc::new(AcpSession::new(
            context("settled"),
            Box::new(RetryCloseProbeAgent {
                failures_remaining: std::sync::atomic::AtomicUsize::new(0),
                attempts: Arc::clone(&settled_attempts),
            }),
        ));
        {
            let mut sessions = registry.sessions.write().await;
            sessions.insert(failed.context.session_id.clone(), Arc::clone(&failed));
            sessions.insert(settled.context.session_id.clone(), settled);
        }

        assert!(registry.close_all().await.is_err());
        let sessions = registry.sessions.read().await;
        assert_eq!(sessions.len(), 1);
        assert!(
            sessions
                .get(&failed.context.session_id)
                .is_some_and(|current| Arc::ptr_eq(current, &failed))
        );
        drop(sessions);
        assert_eq!(failed_attempts.load(Ordering::Acquire), 1);
        assert_eq!(settled_attempts.load(Ordering::Acquire), 1);

        registry.close_all().await?;
        assert!(registry.sessions.read().await.is_empty());
        assert_eq!(failed_attempts.load(Ordering::Acquire), 2);
        assert_eq!(settled_attempts.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn registry_close_permanently_fences_session_creation() -> Result<()> {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory: Arc<dyn AcpSessionFactory> = Arc::new({
            let attempts = Arc::clone(&attempts);
            move |_context| {
                let attempts = Arc::clone(&attempts);
                async move {
                    attempts.fetch_add(1, Ordering::AcqRel);
                    Ok(Box::new(RetryCloseProbeAgent {
                        failures_remaining: std::sync::atomic::AtomicUsize::new(0),
                        attempts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    }) as Box<dyn Agent>)
                }
            }
        });
        let registry = SessionRegistry::new(factory, 2);
        registry.close_all().await?;
        assert!(registry.insert_session(context("late")).await.is_err());
        assert_eq!(attempts.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[tokio::test]
    async fn begin_close_cancels_before_external_drain_and_finish() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = SessionRegistry::new(unused_factory, 1);
        let close_started = Arc::new(AtomicBool::new(false));
        let session = Arc::new(AcpSession::new(
            context("two-phase"),
            Box::new(CloseProbeAgent {
                hang: false,
                close_started: Arc::clone(&close_started),
            }),
        ));
        let turn = session.begin_turn()?;
        let cancellation = turn.turn.cancel.clone();
        registry
            .sessions
            .write()
            .await
            .insert(session.context.session_id.clone(), Arc::clone(&session));

        let lease = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("close lease missing".to_string()))?;
        assert!(cancellation.is_cancelled());
        assert!(session.begin_turn().is_err());
        assert!(registry.get(&session.context.session_id).await.is_some());
        assert!(!close_started.load(Ordering::Acquire));

        // A facade drains its own operation leases here. The framework turn
        // remains registered until that external drain releases its run owner.
        drop(turn);
        registry.finish_close_session(lease).await?;
        assert!(close_started.load(Ordering::Acquire));
        assert!(registry.get(&session.context.session_id).await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_finish_joins_one_agent_close_attempt() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = Arc::new(SessionRegistry::new(unused_factory, 1));
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let session = Arc::new(AcpSession::new(
            context("single-flight"),
            Box::new(ControlledCloseProbeAgent {
                attempts: Arc::clone(&attempts),
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            }),
        ));
        registry
            .sessions
            .write()
            .await
            .insert(session.context.session_id.clone(), Arc::clone(&session));
        let lease = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("close lease missing".to_string()))?;

        let close_started = started.notified();
        let first = tokio::spawn({
            let registry = Arc::clone(&registry);
            let lease = lease.clone();
            async move { registry.finish_close_session(lease).await }
        });
        let second = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move { registry.finish_close_session(lease).await }
        });
        tokio::time::timeout(Duration::from_secs(1), close_started)
            .await
            .map_err(|_| ReactError::Other("Agent close did not start".to_string()))?;
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        release.notify_one();
        first
            .await
            .map_err(|error| ReactError::Other(format!("first finish join failed: {error}")))??;
        second
            .await
            .map_err(|error| ReactError::Other(format!("second finish join failed: {error}")))??;
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        assert!(registry.get(&session.context.session_id).await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn close_observation_unidentified_owner_loss_keeps_original_generation() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let effect = directory.path().join("held-close-effect.txt");
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let report = Arc::new(Notify::new());
        let stopped = Arc::new(AtomicBool::new(false));
        let producer = Arc::new(StdMutex::new(None::<ClosePhysicalJoin>));
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory unused in owner-loss test".to_string(),
            ))
        });
        let registry = Arc::new(SessionRegistry::new(unused_factory, 1));
        let session = Arc::new(AcpSession::new(
            context("lost-close-owner"),
            Box::new(PhysicalCloseObservationAgent {
                attempts: attempts.clone(),
                started: started.clone(),
                release: release.clone(),
                report: Some(report.clone()),
                stopped: stopped.clone(),
                producer: producer.clone(),
                effect: effect.clone(),
            }),
        ));
        registry
            .sessions
            .write()
            .await
            .insert(session.context.session_id.clone(), session.clone());
        let lease = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("original close lease missing".to_string()))?;
        let entered = started.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();
        let mut first = tokio::spawn({
            let registry = registry.clone();
            let lease = lease.clone();
            async move { registry.finish_close_session(lease).await }
        });
        tokio::time::timeout(Duration::from_secs(1), &mut entered)
            .await
            .map_err(|_| ReactError::Other("original close producer did not enter".to_string()))?;
        let abort = lease
            .receipt
            .abort
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .ok_or_else(|| ReactError::Other("original close owner missing".to_string()))?;
        abort.abort();
        let first_observation = tokio::time::timeout(Duration::from_millis(100), &mut first).await;
        let first_returned_without_identity = first_observation.is_ok();
        let repeated_lease = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("repeated close lease missing".to_string()))?;
        let repeated_generation = repeated_lease.generation();
        let mut repeated = tokio::spawn({
            let registry = registry.clone();
            async move { registry.finish_close_session(repeated_lease).await }
        });
        let repeated_observation =
            tokio::time::timeout(Duration::from_millis(100), &mut repeated).await;
        let repeated_returned_without_identity = repeated_observation.is_ok();
        let calls_while_held = attempts.load(Ordering::Acquire);
        let stopped_while_held = stopped.load(Ordering::Acquire);
        let held_effect = tokio::fs::read(&effect).await;
        // These are test-owned observer tasks, not the original physical backend.
        // Close the actual backend and join it before asserting the counterexample.
        release.notify_one();
        report.notify_waiters();
        let original = producer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let joined = match original {
            Some(joined) => Some(tokio::time::timeout(Duration::from_secs(1), joined).await),
            None => None,
        };
        if !first_returned_without_identity {
            first.abort();
            let _ = first.await;
        }
        if !repeated_returned_without_identity
            && tokio::time::timeout(Duration::from_millis(100), &mut repeated)
                .await
                .is_err()
        {
            repeated.abort();
            let _ = repeated.await;
        }
        if let Some(joined) = joined {
            joined
                .map_err(|_| ReactError::Other("original close backend did not join".to_string()))?
                .map_err(ReactError::Other)?;
        } else {
            return Err(ReactError::Other(
                "original backend join missing".to_string(),
            ));
        }
        eprintln!(
            "CLOSE_OWNER_LOSS_BOUNDARY {}",
            serde_json::json!({
                "returned_without_canonical_identity":first_returned_without_identity,
                "repeat_returned_without_canonical_identity":repeated_returned_without_identity,
                "original_generation":lease.generation(), "repeated_generation":repeated_generation,
                "actual_close_calls_while_held":calls_while_held,
                "physical_stopped_while_held":stopped_while_held,
                "original_backend_joined_after_release":stopped.load(Ordering::Acquire)
            })
        );
        assert!(
            !first_returned_without_identity,
            "lost owner is not an ordinary failed close receipt"
        );
        assert!(!repeated_returned_without_identity);
        assert_eq!(lease.generation(), repeated_generation);
        assert_eq!(
            calls_while_held, 1,
            "unknown close owner cannot authorize a second close mutation"
        );
        assert!(!stopped_while_held);
        assert_eq!(held_effect?, b"original-close-entered");
        assert!(stopped.load(Ordering::Acquire));
        assert_eq!(
            tokio::fs::read(&effect).await?,
            b"original-close-physically-joined"
        );
        assert!(registry.get(&session.context.session_id).await.is_some());
        assert!(session.begin_turn().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn late_settled_lease_does_not_conflict_with_same_id_replacement() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = SessionRegistry::new(unused_factory, 1);
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let original = Arc::new(AcpSession::new(
            context("reused-id"),
            Box::new(RetryCloseProbeAgent {
                failures_remaining: std::sync::atomic::AtomicUsize::new(0),
                attempts,
            }),
        ));
        registry
            .sessions
            .write()
            .await
            .insert(original.context.session_id.clone(), Arc::clone(&original));
        let lease = registry
            .begin_close_session(&original.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("close lease missing".to_string()))?;
        let late = lease.clone();
        registry.finish_close_session(lease).await?;

        let replacement = Arc::new(AcpSession::new(
            context("reused-id"),
            Box::new(CloseProbeAgent {
                hang: false,
                close_started: Arc::new(AtomicBool::new(false)),
            }),
        ));
        registry.sessions.write().await.insert(
            replacement.context.session_id.clone(),
            Arc::clone(&replacement),
        );

        registry.finish_close_session(late).await?;
        let current = registry
            .get(&replacement.context.session_id)
            .await
            .ok_or_else(|| ReactError::Other("replacement Session was removed".to_string()))?;
        assert!(Arc::ptr_eq(&current, &replacement));
        Ok(())
    }

    #[tokio::test]
    async fn running_waiter_uses_receipt_after_same_id_replacement_race() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = Arc::new(SessionRegistry::new(unused_factory, 1));
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let close_started = Arc::new(Notify::new());
        let close_release = Arc::new(Notify::new());
        let original = Arc::new(AcpSession::new(
            context("running-reused-id"),
            Box::new(ControlledCloseProbeAgent {
                attempts,
                started: Arc::clone(&close_started),
                release: Arc::clone(&close_release),
            }),
        ));
        registry
            .sessions
            .write()
            .await
            .insert(original.context.session_id.clone(), Arc::clone(&original));
        let lease = registry
            .begin_close_session(&original.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("close lease missing".to_string()))?;

        let started = close_started.notified();
        let first = tokio::spawn({
            let registry = Arc::clone(&registry);
            let lease = lease.clone();
            async move { registry.finish_close_session(lease).await }
        });
        tokio::time::timeout(Duration::from_secs(1), started)
            .await
            .map_err(|_| ReactError::Other("Agent close did not start".to_string()))?;

        let waiter_entered = Arc::new(Notify::new());
        let waiter_release = Arc::new(Notify::new());
        let entered = waiter_entered.notified();
        let waiter_entered_for_late = Arc::clone(&waiter_entered);
        let waiter_release_for_late = Arc::clone(&waiter_release);
        let late = tokio::spawn({
            let registry = Arc::clone(&registry);
            async move {
                registry
                    .finish_close_session_inner(
                        lease,
                        Some(FinishCloseBarrier {
                            entered: waiter_entered_for_late,
                            release: waiter_release_for_late,
                        }),
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), entered)
            .await
            .map_err(|_| ReactError::Other("running waiter did not reach barrier".to_string()))?;

        close_release.notify_one();
        first
            .await
            .map_err(|error| ReactError::Other(format!("first finish join failed: {error}")))??;
        let replacement = Arc::new(AcpSession::new(
            context("running-reused-id"),
            Box::new(CloseProbeAgent {
                hang: false,
                close_started: Arc::new(AtomicBool::new(false)),
            }),
        ));
        registry.sessions.write().await.insert(
            replacement.context.session_id.clone(),
            Arc::clone(&replacement),
        );
        waiter_release.notify_one();
        late.await
            .map_err(|error| ReactError::Other(format!("late finish join failed: {error}")))??;

        let current = registry
            .get(&replacement.context.session_id)
            .await
            .ok_or_else(|| ReactError::Other("replacement Session was removed".to_string()))?;
        assert!(Arc::ptr_eq(&current, &replacement));
        Ok(())
    }

    #[tokio::test]
    async fn failed_finish_requires_new_begin_generation_for_retry() -> Result<()> {
        let unused_factory: Arc<dyn AcpSessionFactory> = Arc::new(|_context| async {
            Err(ReactError::Other(
                "factory is unused in this test".to_string(),
            ))
        });
        let registry = SessionRegistry::new(unused_factory, 1);
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let session = Arc::new(AcpSession::new(
            context("retry-generation"),
            Box::new(RetryCloseProbeAgent {
                failures_remaining: std::sync::atomic::AtomicUsize::new(2),
                attempts: Arc::clone(&attempts),
            }),
        ));
        registry
            .sessions
            .write()
            .await
            .insert(session.context.session_id.clone(), Arc::clone(&session));
        let first = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("first close lease missing".to_string()))?;
        let late_first = first.clone();
        let first_error = registry
            .finish_close_session(first)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("first close unexpectedly settled".to_string()))?
            .to_string();
        assert!(first_error.contains("attempt 1"));
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        assert!(registry.get(&session.context.session_id).await.is_some());

        let second = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("second close lease missing".to_string()))?;
        assert_eq!(second.generation(), 2);
        let second_error = registry
            .finish_close_session(second)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("second close unexpectedly settled".to_string()))?
            .to_string();
        assert!(second_error.contains("attempt 2"));
        assert_eq!(attempts.load(Ordering::Acquire), 2);

        let late_first_error = registry
            .finish_close_session(late_first)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("late first lease unexpectedly settled".to_string()))?
            .to_string();
        assert_eq!(late_first_error, first_error);
        assert_eq!(attempts.load(Ordering::Acquire), 2);

        let retry = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("retry close lease missing".to_string()))?;
        assert_eq!(retry.generation(), 3);
        registry.finish_close_session(retry).await?;
        assert_eq!(attempts.load(Ordering::Acquire), 3);
        assert!(registry.get(&session.context.session_id).await.is_none());
        Ok(())
    }
}
