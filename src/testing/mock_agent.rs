//! Mock Agent, implementing the [`Agent`] trait, for replacing real SubAgents
//! when testing multi-Agent orchestration.
//!
//! When testing orchestration logic, we typically want to:
//! - Avoid making real LLM calls
//! - Control the return content of each SubAgent
//! - Verify how many times a SubAgent was called, and what task it received each time
//!
//! # Example
//!
//! ```rust
//! use echo_agent::testing::MockAgent;
//! use echo_agent::agent::Agent;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let mut agent = MockAgent::new("math_agent")
//!     .with_response("The result is 42")
//!     .with_response("The result is 100");
//!
//! let r1 = agent.execute("compute 6 * 7").await.unwrap();
//! let r2 = agent.execute("compute 10 * 10").await.unwrap();
//! assert_eq!(r1, "The result is 42");
//! assert_eq!(r2, "The result is 100");
//! assert_eq!(agent.call_count(), 2);
//! assert_eq!(agent.calls()[0], "compute 6 * 7");
//! # }
//! ```

use crate::agent::{Agent, AgentEvent, CancellationToken};
use crate::error::{AgentError, ReactError, Result};
use futures::future::BoxFuture;
use futures::stream;
use futures::stream::BoxStream;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

// ── MockAgent ─────────────────────────────────────────────────────────────────

/// A scriptable Mock Agent.
///
/// Returns preset responses in order; once the queue is exhausted, each call
/// returns `"mock agent response"`.
/// Messages from both `execute()` and `chat()` are recorded, and can be inspected
/// via [`calls()`](MockAgent::calls).
/// `reset()` clears the call history, simulating the conversation-reset semantics
/// of a real Agent.
pub struct MockAgent {
    name: String,
    model_name: String,
    system_prompt: String,
    responses: Arc<Mutex<VecDeque<String>>>,
    calls: Arc<Mutex<Vec<String>>>,
    /// Multimodal messages received via `execute_stream_message_with_cancel`
    /// (records whether dispatch forwarded attachments to the worker).
    messages: Arc<Mutex<Vec<echo_core::llm::types::Message>>>,
    /// `set_working_dir` calls recorded in order (Sprint 8 isolation tests).
    /// Each entry is the path the agent was asked to bind (`None` = clear).
    working_dirs: Arc<Mutex<Vec<Option<std::path::PathBuf>>>>,
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
            calls: self.calls.clone(),
            messages: self.messages.clone(),
            working_dirs: self.working_dirs.clone(),
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
            calls: Arc::new(Mutex::new(Vec::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            working_dirs: Arc::new(Mutex::new(Vec::new())),
        }
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
    /// forwarded a `Message`). Used to verify workers see user attachments.
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

    fn next_response(&self) -> String {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .unwrap_or_else(|| "mock agent response".to_string())
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
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(task.to_string());
            Ok(self.next_response())
        })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let answer = self.execute(task).await?;
            let event_stream = stream::once(async move { Ok(AgentEvent::FinalAnswer(answer)) });
            Ok(Box::pin(event_stream) as BoxStream<'a, Result<AgentEvent>>)
        })
    }

    /// Multimodal dispatch override: record the received message (so tests can
    /// assert workers saw user attachments) and consume a preset response like
    /// the text path. Without this override, the trait default would reject
    /// multimodal dispatch — making it impossible to test message forwarding.
    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        message: echo_core::llm::types::Message,
        _cancel: CancellationToken,
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
            let answer = self.next_response();
            let event_stream = stream::once(async move { Ok(AgentEvent::FinalAnswer(answer)) });
            Ok(Box::pin(event_stream) as BoxStream<'a, Result<AgentEvent>>)
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
            Ok(self.next_response())
        })
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let answer = self.chat(message).await?;
            let event_stream = stream::once(async move { Ok(AgentEvent::FinalAnswer(answer)) });
            Ok(Box::pin(event_stream) as BoxStream<'a, Result<AgentEvent>>)
        })
    }

    /// Clear call history, simulating the reset semantics of a real Agent.
    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
        })
    }

    /// Record `set_working_dir` calls so Sprint 8 isolation tests can verify
    /// the worker was chrooted into the worktree (and cleared afterwards).
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
    calls: Arc<Mutex<Vec<String>>>,
}

impl FailingMockAgent {
    /// Create a failing Mock Agent
    pub fn new(name: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            error_message: error_message.into(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get the number of times this Mock Agent has been called.
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
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
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(task.to_string());
            Err(ReactError::Agent(Box::new(
                AgentError::InitializationFailed(self.error_message.clone()),
            )))
        })
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            let err = self.execute(task).await.unwrap_err();
            let event_stream = stream::once(async move { Err(err) });
            Ok(Box::pin(event_stream) as BoxStream<'a, Result<AgentEvent>>)
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
