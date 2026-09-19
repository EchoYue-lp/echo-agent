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
/// attempt. A failed attempt remains stable for this generation; callers must
/// invoke [`SessionRegistry::begin_close_session`] again to obtain a retry.
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
    creation_gate: Mutex<()>,
    client_capabilities: RwLock<Option<ClientCapabilities>>,
    sessions: RwLock<HashMap<SessionId, Arc<AcpSession>>>,
}

enum FinishCloseAction {
    Start,
    Wait,
    Settled,
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
            *phase = SessionClosePhase::Failed(
                "owned Agent close task ended before publishing settlement".to_string(),
            );
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
        Err(error) => SessionClosePhase::Failed(error.to_string()),
    });
}

impl SessionRegistry {
    pub fn new(factory: Arc<dyn AcpSessionFactory>, max_sessions: usize) -> Self {
        Self {
            registry_id: uuid::Uuid::new_v4().to_string(),
            factory,
            max_sessions,
            creation_gate: Mutex::new(()),
            client_capabilities: RwLock::new(None),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn initialize(&self, capabilities: ClientCapabilities) {
        *self.client_capabilities.write().await = Some(capabilities);
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
        self.sessions.write().await.insert(
            session_id.clone(),
            Arc::new(AcpSession::new(context, agent)),
        );
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
    /// A failed attempt preserves the Session and returns the same failure to
    /// every waiter. Call `begin_close_session` again to obtain a retry lease.
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
                    SessionClosePhase::Running => FinishCloseAction::Wait,
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
        let _creation = self.creation_gate.lock().await;
        let leases = {
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
        for (id, result) in results {
            if let Err(error) = result {
                failures.push(format!("{id}: {error}"));
            }
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
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
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
    async fn aborted_owned_close_task_publishes_failure_and_allows_retry() -> Result<()> {
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
            context("aborted-owner"),
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
        let finish = tokio::spawn({
            let registry = Arc::clone(&registry);
            let lease = lease.clone();
            async move { registry.finish_close_session(lease).await }
        });
        tokio::time::timeout(Duration::from_secs(1), close_started)
            .await
            .map_err(|_| ReactError::Other("Agent close did not start".to_string()))?;
        let abort = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let abort = lease
                    .receipt
                    .abort
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                if let Some(abort) = abort {
                    return abort;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("owned close abort handle missing".to_string()))?;
        abort.abort();
        let error = tokio::time::timeout(Duration::from_secs(1), finish)
            .await
            .map_err(|_| ReactError::Other("finish waiter remained stuck".to_string()))?
            .map_err(|join_error| {
                ReactError::Other(format!("finish waiter join failed: {join_error}"))
            })?
            .err()
            .ok_or_else(|| ReactError::Other("aborted close unexpectedly settled".to_string()))?;
        assert!(error.to_string().contains("ended before publishing"));
        assert_eq!(attempts.load(Ordering::Acquire), 1);
        assert!(registry.get(&session.context.session_id).await.is_some());

        let retry = registry
            .begin_close_session(&session.context.session_id)
            .await?
            .ok_or_else(|| ReactError::Other("retry close lease missing".to_string()))?;
        release.notify_one();
        registry.finish_close_session(retry).await?;
        assert_eq!(attempts.load(Ordering::Acquire), 2);
        assert!(registry.get(&session.context.session_id).await.is_none());
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
