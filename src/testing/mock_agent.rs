//! Mock Agent, implementing the [`Agent`] trait, for replacing real Subagents
//! when testing multi-Agent orchestration.
//!
//! When testing orchestration logic, we typically want to:
//! - Avoid making real LLM calls
//! - Control the return content of each Subagent
//! - Verify how many times a Subagent was called, and what task it received each time
//!
//! # Example
//!
//! ```rust
//! use echo_agent::testing::MockAgent;
//! use echo_agent::agent::Agent;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! let agent = MockAgent::new("math_agent")
//!     .with_response("The result is 42")
//!     .with_response("The result is 100");
//!
//! let r1 = agent.execute("compute 6 * 7").await?;
//! let r2 = agent.execute("compute 10 * 10").await?;
//! assert_eq!(r1, "The result is 42");
//! assert_eq!(r2, "The result is 100");
//! assert_eq!(agent.call_count(), 2);
//! assert_eq!(agent.calls().first().map(String::as_str), Some("compute 6 * 7"));
//! # Ok(())
//! # }
//! ```

use crate::agent::{Agent, AgentEvent, CancellationToken};
use crate::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── MockAgent ─────────────────────────────────────────────────────────────────

/// One step in a cancellation-aware Agent event script.
pub enum MockAgentStep {
    Event(Box<AgentEvent>),
    Error(String),
    Delay(std::time::Duration),
}

/// Failure category emitted by [`FailingMockAgent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockAgentFailure {
    Initialization,
    Subagent,
    Timeout,
    Cancelled,
    PermissionDenied,
}

impl MockAgentFailure {
    fn error(self, message: String) -> ReactError {
        let error = match self {
            Self::Initialization => AgentError::InitializationFailed(message),
            Self::Subagent => AgentError::SubagentError(message),
            Self::Timeout => AgentError::Timeout(message),
            Self::Cancelled => AgentError::Cancelled(message),
            Self::PermissionDenied => AgentError::PermissionDenied(message),
        };
        ReactError::Agent(Box::new(error))
    }
}

/// A scriptable Mock Agent.
///
/// Returns preset responses in order and fails closed when exhausted.
/// Messages from both `execute()` and `chat()` are recorded, and can be inspected
/// via [`calls()`](MockAgent::calls).
/// `reset()` clears the call history, simulating the conversation-reset semantics
/// of a real Agent.
pub struct MockAgent {
    name: String,
    model_name: String,
    system_prompt: String,
    responses: Arc<Mutex<VecDeque<String>>>,
    event_scripts: Arc<Mutex<VecDeque<Vec<MockAgentStep>>>>,
    calls: Arc<Mutex<Vec<String>>>,
    /// Multimodal messages received via `execute_stream_message_with_cancel`
    /// (records whether dispatch forwarded attachments to the subagent).
    messages: Arc<Mutex<Vec<echo_core::llm::types::Message>>>,
    /// Value-scoped invocation metadata received by streaming calls.
    invocation_contexts: Arc<Mutex<Vec<echo_core::agent::AgentInvocationContext>>>,
    /// `set_working_dir` calls recorded in order (Sprint 8 isolation tests).
    /// Each entry is the path the agent was asked to bind (`None` = clear).
    working_dirs: Arc<Mutex<Vec<Option<std::path::PathBuf>>>>,
    /// Artificial delay before returning (for background-dispatch tests).
    delay_ms: u64,
    default_success: Option<String>,
}

// All observable state is behind Arc<Mutex>, so cloning shares call/message
// history — tests can clone a MockAgent, register the clone, and assert on the
// original.
impl Clone for MockAgent {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            model_name: self.model_name.clone(),
            system_prompt: self.system_prompt.clone(),
            responses: self.responses.clone(),
            event_scripts: self.event_scripts.clone(),
            calls: self.calls.clone(),
            messages: self.messages.clone(),
            invocation_contexts: self.invocation_contexts.clone(),
            working_dirs: self.working_dirs.clone(),
            delay_ms: self.delay_ms,
            default_success: self.default_success.clone(),
        }
    }
}

impl MockAgent {
    /// Create a named Mock Agent
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model_name: "mock-model".to_string(),
            system_prompt: "You are a mock agent".to_string(),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            event_scripts: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            invocation_contexts: Arc::new(Mutex::new(Vec::new())),
            working_dirs: Arc::new(Mutex::new(Vec::new())),
            delay_ms: 0,
            default_success: None,
        }
    }

    /// Artificial delay before each response (background-dispatch tests).
    pub fn with_delay_ms(mut self, delay_ms: u64) -> Self {
        self.delay_ms = delay_ms;
        self
    }

    /// Set the model name (for tests that need to check model_name)
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_name = model.into();
        self
    }

    /// Set the system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Append a preset response
    pub fn with_response(self, text: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(text.into());
        self
    }

    /// Append multiple preset responses in bulk
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            for t in texts {
                q.push_back(t.into());
            }
        }
        self
    }

    /// Append one complete neutral event script for the next streaming call.
    pub fn with_event_script(self, steps: Vec<MockAgentStep>) -> Self {
        self.event_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(steps);
        self
    }

    /// Explicitly allow calls after the script is exhausted.
    pub fn with_default_success(mut self, text: impl Into<String>) -> Self {
        self.default_success = Some(text.into());
        self
    }

    pub fn remaining(&self) -> usize {
        self.responses
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    /// Total number of times called
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// All historical call task strings (in chronological order)
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The task string from the last call (returns `None` if never called)
    pub fn last_task(&self) -> Option<String> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .cloned()
    }

    /// Number of multimodal messages received via the message dispatch path.
    pub fn message_count(&self) -> usize {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// The last multimodal message received (returns `None` if dispatch never
    /// forwarded a `Message`). Used to verify subagents see user attachments.
    pub fn last_message(&self) -> Option<echo_core::llm::types::Message> {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .cloned()
    }

    /// Clear call history (response queue is unaffected)
    ///
    /// Used only for test assertion reset, not equivalent to `Agent::reset()`.
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// All `set_working_dir` calls recorded in order (Sprint 8 isolation tests).
    /// Each entry is the path the agent was asked to bind; `None` = clear.
    pub fn working_dir_calls(&self) -> Vec<Option<std::path::PathBuf>> {
        self.working_dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Invocation contexts received by value-scoped streaming methods.
    pub fn invocation_contexts(&self) -> Vec<echo_core::agent::AgentInvocationContext> {
        self.invocation_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn next_response(&self) -> Result<String> {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .or_else(|| self.default_success.clone())
            .ok_or_else(|| {
                ReactError::Agent(Box::new(AgentError::SubagentError(
                    "MockAgent response script exhausted".to_string(),
                )))
            })
    }

    fn next_event_stream<'a>(
        &'a self,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'a, Result<AgentEvent>>> {
        let scripted = self
            .event_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front();
        let steps = match scripted {
            Some(steps) => steps,
            None => vec![MockAgentStep::Event(Box::new(AgentEvent::FinalAnswer(
                self.next_response()?,
            )))],
        };
        let initial_delay = self.delay_ms;
        let events = async_stream::stream! {
            if initial_delay > 0 {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        yield Ok(AgentEvent::Cancelled);
                        return;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(initial_delay)) => {}
                }
            }
            for step in steps {
                match step {
                    MockAgentStep::Event(event) => yield Ok(*event),
                    MockAgentStep::Error(message) => {
                        yield Err(ReactError::Agent(Box::new(AgentError::SubagentError(message))));
                        return;
                    }
                    MockAgentStep::Delay(delay) => {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                yield Ok(AgentEvent::Cancelled);
                                return;
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                }
            }
        };
        Ok(Box::pin(events))
    }
}

impl Agent for MockAgent {
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
        Box::pin(async move {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(task.to_string());
            self.next_response()
        })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(task.to_string());
            self.next_event_stream(CancellationToken::new())
        })
    }

    fn execute_stream_with_cancel<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(task.to_string());
            self.next_event_stream(cancel)
        })
    }

    /// Multimodal dispatch override: record the received message (so tests can
    /// assert subagents saw user attachments) and consume a preset response like
    /// the text path. Without this override, the trait default would reject
    /// multimodal dispatch — making it impossible to test message forwarding.
    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        message: echo_core::llm::types::Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(message.clone());
            // Also record as a regular call (text part) for parity with last_task.
            let text = message.content.as_text().unwrap_or_default();
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(text);
            self.next_event_stream(cancel)
        })
    }

    fn execute_stream_with_invocation_context<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
        invocation: echo_core::agent::AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.invocation_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(invocation);
            self.execute_stream_with_cancel(task, cancel).await
        })
    }

    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: echo_core::llm::types::Message,
        cancel: CancellationToken,
        invocation: echo_core::agent::AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.invocation_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(invocation);
            self.execute_stream_message_with_cancel(message, cancel)
                .await
        })
    }

    /// `chat()` also records the call and consumes the preset response queue.
    /// Note: MockAgent does not maintain a real conversation context; this only satisfies the call contract.
    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(message.to_string());
            self.next_response()
        })
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(message.to_string());
            self.next_event_stream(CancellationToken::new())
        })
    }

    fn chat_stream_with_cancel<'a>(
        &'a self,
        message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(message.to_string());
            self.next_event_stream(cancel)
        })
    }

    /// Clear call history, simulating the reset semantics of a real Agent.
    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
        })
    }

    /// Record `set_working_dir` calls so Sprint 8 isolation tests can verify
    /// the subagent was chrooted into the worktree (and cleared afterwards).
    fn set_working_dir(&self, path: Option<std::path::PathBuf>) {
        self.working_dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(path);
    }

    fn clear_working_dir(&self) {
        self.working_dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(None);
    }
}

/// A Mock Agent that always returns an error (for testing orchestration fault-tolerance behavior)
pub struct FailingMockAgent {
    name: String,
    error_message: String,
    failure: MockAgentFailure,
    event_scripts: Arc<Mutex<VecDeque<Vec<MockAgentStep>>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FailingMockAgent {
    /// Create a failing Mock Agent
    pub fn new(name: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            error_message: error_message.into(),
            failure: MockAgentFailure::Initialization,
            event_scripts: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Choose the typed agent failure returned by every invocation.
    pub fn with_failure(mut self, failure: MockAgentFailure) -> Self {
        self.failure = failure;
        self
    }

    /// Append partial events, delays, and the eventual error for one stream.
    ///
    /// `MockAgentStep::Error` is mapped to the selected typed failure category.
    /// When no script remains, the configured default error is emitted.
    pub fn with_event_script(self, steps: Vec<MockAgentStep>) -> Self {
        self.event_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(steps);
        self
    }

    /// Get the number of times this Mock Agent has been called.
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// All historical call task strings in chronological order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record_call(&self, task: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(task.to_string());
    }

    fn next_event_stream<'a>(
        &'a self,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<AgentEvent>> {
        let steps = self
            .event_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front()
            .unwrap_or_else(|| vec![MockAgentStep::Error(self.error_message.clone())]);
        let events = async_stream::stream! {
            for step in steps {
                match step {
                    MockAgentStep::Event(event) => yield Ok(*event),
                    MockAgentStep::Error(message) => {
                        yield Err(self.failure.error(message));
                        return;
                    }
                    MockAgentStep::Delay(delay) => {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                yield Ok(AgentEvent::Cancelled);
                                return;
                            }
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                }
            }
        };
        Box::pin(events)
    }
}

impl Agent for FailingMockAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_name(&self) -> &str {
        "mock-model"
    }

    fn system_prompt(&self) -> &str {
        "failing mock agent"
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.record_call(task);
            Err(self.failure.error(self.error_message.clone()))
        })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.record_call(task);
            Ok(self.next_event_stream(CancellationToken::new()))
        })
    }

    fn execute_stream_with_cancel<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.record_call(task);
            Ok(self.next_event_stream(cancel))
        })
    }

    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.execute(message).await })
    }

    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn mock_agent_scripts_partial_event_then_error() -> Result<()> {
        let agent = MockAgent::new("partial").with_event_script(vec![
            MockAgentStep::Event(Box::new(AgentEvent::Token("part".to_string()))),
            MockAgentStep::Error("stream failed".to_string()),
        ]);
        let mut events = agent.execute_stream("work").await?;

        assert!(matches!(
            events.next().await,
            Some(Ok(AgentEvent::Token(token))) if token == "part"
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(ReactError::Agent(error)))
                if matches!(*error, AgentError::SubagentError(ref message) if message == "stream failed")
        ));
        assert!(events.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn failing_mock_agent_scripts_partial_event_then_typed_error() -> Result<()> {
        let agent = FailingMockAgent::new("partial-failure", "default failure")
            .with_failure(MockAgentFailure::Timeout)
            .with_event_script(vec![
                MockAgentStep::Event(Box::new(AgentEvent::Token("part".to_string()))),
                MockAgentStep::Error("deadline".to_string()),
            ]);
        let mut events = agent
            .execute_stream_with_cancel("work", CancellationToken::new())
            .await?;

        assert!(matches!(
            events.next().await,
            Some(Ok(AgentEvent::Token(token))) if token == "part"
        ));
        assert!(matches!(
            events.next().await,
            Some(Err(ReactError::Agent(error)))
                if matches!(*error, AgentError::Timeout(ref message) if message == "deadline")
        ));
        assert!(events.next().await.is_none());
        assert_eq!(agent.calls(), vec!["work".to_string()]);
        Ok(())
    }
}
