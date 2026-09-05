use crate::agent::{Agent, CancellationToken};
use crate::error::{ReactError, Result};
use agent_client_protocol::schema::v1::{
    ClientCapabilities, McpServer, Meta, NewSessionRequest, SessionId,
};
use futures::future::BoxFuture;
use futures::future::join_all;
use std::collections::HashMap;
use std::path::PathBuf;
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

#[derive(Clone)]
pub(crate) struct ActiveTurn {
    pub id: String,
    pub message_id: String,
    pub cancel: CancellationToken,
}

pub(crate) struct ActiveTurnLease {
    session: Weak<AcpSession>,
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

pub(crate) struct AcpSession {
    pub context: AcpSessionContext,
    pub agent: Arc<dyn Agent>,
    turn: StdMutex<TurnSlot>,
    turn_settled: Notify,
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
        }
    }

    pub fn begin_turn(self: &Arc<Self>) -> Result<ActiveTurnLease> {
        let mut slot = self.turn.lock().unwrap_or_else(|error| error.into_inner());
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

    async fn wait_until_idle(&self) {
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

pub(crate) struct SessionRegistry {
    factory: Arc<dyn AcpSessionFactory>,
    max_sessions: usize,
    creation_gate: Mutex<()>,
    client_capabilities: RwLock<Option<ClientCapabilities>>,
    sessions: RwLock<HashMap<SessionId, Arc<AcpSession>>>,
}

impl SessionRegistry {
    pub fn new(factory: Arc<dyn AcpSessionFactory>, max_sessions: usize) -> Self {
        Self {
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
        let _creation = self.creation_gate.lock().await;
        if self.sessions.read().await.len() >= self.max_sessions {
            return Err(ReactError::Other(format!(
                "ACP Session limit {} reached",
                self.max_sessions
            )));
        }
        let capabilities = self
            .client_capabilities
            .read()
            .await
            .clone()
            .ok_or_else(|| ReactError::Other("ACP connection is not initialized".to_string()))?;
        let session_id = SessionId::new(format!("sess_{}", uuid::Uuid::new_v4()));
        let context = AcpSessionContext {
            session_id: session_id.clone(),
            cwd: request.cwd,
            additional_directories: request.additional_directories,
            mcp_servers: request.mcp_servers,
            client_capabilities: capabilities,
            meta: request.meta,
        };
        let agent = self.factory.create_session(context.clone()).await?;
        // ACP Sessions own distinct Agents, so binding the Session cwd as the
        // Agent default cannot leak across conversations. This also keeps the
        // adapter compatible with text-only Agent implementations.
        agent.set_working_dir(Some(context.cwd.clone()));
        self.sessions.write().await.insert(
            session_id.clone(),
            Arc::new(AcpSession::new(context, agent)),
        );
        Ok(session_id)
    }

    pub async fn get(&self, session_id: &SessionId) -> Option<Arc<AcpSession>> {
        self.sessions.read().await.get(session_id).cloned()
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
        let sessions = {
            let mut guard = self.sessions.write().await;
            guard
                .drain()
                .map(|(_, session)| session)
                .collect::<Vec<_>>()
        };
        for session in &sessions {
            session.cancel_active();
        }
        let results = join_all(sessions.into_iter().map(|session| async move {
            session.wait_until_idle().await;
            session.agent.close().await
        }))
        .await;
        results
            .into_iter()
            .find_map(Result::err)
            .map_or(Ok(()), Err)
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
}
