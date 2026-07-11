//! AgentHandle — scoped access wrapper around a shared ReactAgent.
//!
//! Replaces raw `Arc<RwLock<ReactAgent>>` so that callers never lock
//! or import `ReactAgent` directly. The handle provides:
//!
//! | Method | Lock | Closure |
//! |--------|------|---------|
//! | [`read`](AgentHandle::read) | read (async) | sync |
//! | [`read_async`](AgentHandle::read_async) | read (async) | async (boxed) |
//! | [`write`](AgentHandle::write) | write (async) | sync |
//! | [`write_async`](AgentHandle::write_async) | write (async) | async (boxed) |
//! | [`try_write`](AgentHandle::try_write) | write (try) | sync |
//! | [`inner`](AgentHandle::inner) | none | escape hatch |
//!
//! `read_async` and `write_async` hold the lock across `.await`; do not use them
//! for long-running streams or request/response loops.
//!
//! Execution serialization is handled internally by `ReactAgent.execution_mutex`
//! — callers do NOT need to acquire any additional lock before calling
//! `execute()`, `chat()`, or streaming methods.

use crate::agent::{Agent, AgentEvent};
use crate::error::Result;
use crate::workflow::SharedAgent;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::stream::StreamExt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::ReactAgent;

// ── RwLockAgentWrapper ─────────────────────────────────────────────────────────

/// Wrapper that implements [`Agent`] by delegating through a `RwLock<ReactAgent>`.
///
/// This enables `AgentHandle -> SharedAgent` conversion for use with
/// Graph workflow methods. Each `Agent` trait method acquires a read lock,
/// calls the corresponding `ReactAgent` method, and awaits the result
/// within the lock scope.
///
/// Immutable fields (`name`, `model_name`, `system_prompt`) are cached at
/// construction time to avoid holding the lock for sync trait methods that
/// return `&str`.
struct RwLockAgentWrapper {
    inner: Arc<RwLock<ReactAgent>>,
    name: String,
    model_name: String,
    system_prompt: String,
}

impl RwLockAgentWrapper {
    async fn from_handle(handle: &AgentHandle) -> Self {
        let guard = handle.agent.read().await;
        Self {
            inner: handle.agent.clone(),
            name: guard.name().to_string(),
            model_name: guard.model_name().to_string(),
            system_prompt: guard.system_prompt().to_string(),
        }
    }
}

impl Agent for RwLockAgentWrapper {
    fn name(&self) -> &str {
        &self.name
    }
    fn model_name(&self) -> &str {
        &self.model_name
    }
    fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        let inner = self.inner.clone();
        let task = task.to_string();
        Box::pin(async move {
            let guard = inner.read().await;
            guard.execute(&task).await
        })
    }

    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        let inner = self.inner.clone();
        let message = message.to_string();
        Box::pin(async move {
            let guard = inner.read().await;
            guard.chat(&message).await
        })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let inner = self.inner.clone();
        let task = task.to_string();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<AgentEvent>>();

            tokio::spawn(async move {
                let guard = inner.read().await;
                match guard.execute_stream(&task).await {
                    Ok(mut stream) => {
                        while let Some(event) = stream.next().await {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });

            let stream = futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (event, rx))
            });

            Ok(stream.boxed())
        })
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let inner = self.inner.clone();
        let message = message.to_string();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<AgentEvent>>();

            tokio::spawn(async move {
                let guard = inner.read().await;
                match guard.chat_stream(&message).await {
                    Ok(mut stream) => {
                        while let Some(event) = stream.next().await {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            });

            let stream = futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (event, rx))
            });

            Ok(stream.boxed())
        })
    }

    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let guard = inner.read().await;
            guard.reset().await;
        })
    }
}

// ── AgentHandle ────────────────────────────────────────────────────────────────

/// Scoped-access handle for a shared [`ReactAgent`].
///
/// Provides safe, ergonomic access to the agent without requiring callers
/// to manage locks directly. Execution serialization (preventing concurrent
/// `execute`/`chat`/stream calls) is handled internally by `ReactAgent`'s
/// `execution_mutex`.
///
/// Clone is cheap (Arc clone).
#[derive(Clone)]
pub struct AgentHandle {
    agent: Arc<RwLock<ReactAgent>>,
}

impl AgentHandle {
    /// Inject user input into the active turn without waiting for its stream.
    pub async fn steer_input(
        &self,
        expected_turn_id: Option<&str>,
        message: crate::llm::types::Message,
    ) -> std::result::Result<String, crate::agent::TurnSteerError> {
        let guard = self.agent.read().await;
        guard.steer_input(expected_turn_id, message)
    }

    /// Wrap an existing `Arc<RwLock<ReactAgent>>`.
    pub fn from_arc(agent: Arc<RwLock<ReactAgent>>) -> Self {
        Self { agent }
    }

    /// Build from an owned `ReactAgent`.
    pub fn new(agent: ReactAgent) -> Self {
        Self {
            agent: Arc::new(RwLock::new(agent)),
        }
    }

    /// Escape hatch — return the inner `Arc<RwLock<ReactAgent>>`.
    pub fn inner(&self) -> &Arc<RwLock<ReactAgent>> {
        &self.agent
    }

    /// Produce a [`SharedAgent`] view of the same underlying `ReactAgent`.
    ///
    /// The returned `SharedAgent` delegates all [`Agent`] trait calls through
    /// the `RwLock`. Immutable fields are cached at conversion time.
    pub async fn as_shared_agent(&self) -> SharedAgent {
        Arc::new(RwLockAgentWrapper::from_handle(self).await) as SharedAgent
    }

    /// Acquire read lock, run a **sync** closure, return its result.
    pub async fn read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&ReactAgent) -> R,
    {
        let guard = self.agent.read().await;
        f(&guard)
    }

    /// Acquire read lock, run a closure that returns a boxed future,
    /// and `.await` that future while still holding the lock.
    ///
    /// Do not use this for streams or other long-running operations.
    pub async fn read_async<F, R>(&self, f: F) -> R
    where
        F: for<'a> FnOnce(&'a ReactAgent) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>,
    {
        let guard = self.agent.read().await;
        f(&guard).await
    }

    /// Acquire write lock, run a **sync** closure, return its result.
    pub async fn write<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut ReactAgent) -> R,
    {
        let mut guard = self.agent.write().await;
        f(&mut guard)
    }

    /// Acquire write lock, run a closure that returns a boxed future,
    /// and `.await` that future while still holding the lock.
    ///
    /// Do not use this for streams or other long-running operations; it blocks
    /// all readers and writers until the awaited future completes.
    pub async fn write_async<F, R>(&self, f: F) -> R
    where
        F: for<'a> FnOnce(&'a mut ReactAgent) -> Pin<Box<dyn Future<Output = R> + Send + 'a>>,
    {
        let mut guard = self.agent.write().await;
        f(&mut guard).await
    }

    /// Non-blocking write attempt — returns `None` if the lock is
    /// already held by another caller.
    pub fn try_write<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut ReactAgent) -> R,
    {
        let mut guard = self.agent.try_write().ok()?;
        Some(f(&mut guard))
    }

    /// Convert this handle into a `Box<dyn Agent>` that delegates through the
    /// `RwLock` at runtime. Immutable fields (name, model_name, system_prompt)
    /// are cached at conversion time; all other [`Agent`] trait methods acquire
    /// a read lock for each call.
    ///
    /// This is useful when an agent must be both:
    /// - accessible through an `AgentHandle` (for tool registration / lifecycle),
    /// - passable as `Box<dyn Agent>` (e.g. to a subagent registry).
    pub async fn to_boxed_agent(&self) -> Box<dyn Agent> {
        Box::new(RwLockAgentWrapper::from_handle(self).await)
    }
}
