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
use echo_core::agent::{
    AgentInvocationContext, AgentPhase, EventIdentity, ExecutionStatus, ExecutionStatusKind,
};
use echo_core::llm::types::Message;
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
    /// 原样保留 source 的 typed Err（包括尚未物理结算的观察）。
    Failure(ReactError),
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
    /// execute/chat 两个 typed 入口收到的完整 Message（包括附件与回放字段）。
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

    /// 原样提供下一次流的 source 脚本；不补 Started、成功或 EOF。
    /// 故意不完整、外来身份及 unknown 脚本由调用方显式编排。
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

    /// 两个 typed 入口收到的完整 Message 数量。
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

    /// Drain invocation contexts recorded by value-scoped streaming methods.
    pub fn take_invocation_contexts(&self) -> Vec<echo_core::agent::AgentInvocationContext> {
        let mut contexts = self
            .invocation_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut *contexts)
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

    fn record_invocation(&self, message: Message, invocation: &AgentInvocationContext) {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(message.content.as_text().unwrap_or_default());
        self.messages
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(message);
        self.invocation_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(invocation.clone());
    }

    fn next_event_stream<'a>(
        &'a self,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> Result<BoxStream<'a, Result<AgentEvent>>> {
        let scripted = self
            .event_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front();
        // builder 的响应就是本测试 producer 明确拥有的成功，不从下游文字反推状态。
        // identity 在 source 创建时取一次，整个响应和取消路径保持同一身份。
        let native = scripted.is_none();
        let identity = EventIdentity::from_invocation(&invocation)?;
        let execution_id = identity
            .execution_id
            .as_ref()
            .map(|value| value.as_str())
            .unwrap_or(identity.turn_id.as_str())
            .to_string();
        let request_id = identity
            .message_id
            .as_ref()
            .map(|value| value.as_str().to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let status = move |kind, sequence| {
            AgentEvent::ExecutionStatus(ExecutionStatus {
                execution_id: execution_id.clone(),
                request_id: Some(request_id.clone()),
                sequence,
                phase: AgentPhase::Thinking,
                status: kind,
                reason: None,
                adapter_generated: false,
            })
        };
        let steps = match scripted {
            Some(steps) => steps,
            None => vec![
                MockAgentStep::Event(Box::new(status(ExecutionStatusKind::Started, 0))),
                MockAgentStep::Event(Box::new(AgentEvent::FinalAnswer(self.next_response()?))),
                MockAgentStep::Event(Box::new(status(ExecutionStatusKind::Settled, 1))),
            ],
        };
        let initial_delay = self.delay_ms;
        let mut steps = steps;
        if native && initial_delay > 0 {
            steps.insert(
                1,
                MockAgentStep::Delay(std::time::Duration::from_millis(initial_delay)),
            );
        }
        let events = async_stream::stream! {
            let _invocation = invocation;
            if !native && initial_delay > 0 {
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
                    MockAgentStep::Failure(error) => {
                        yield Err(error);
                        return;
                    }
                    MockAgentStep::Delay(delay) => {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                if native { yield Ok(status(ExecutionStatusKind::Cancelled, 1)); }
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

    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.record_invocation(message, &invocation);
            self.next_event_stream(cancel, invocation)
        })
    }

    // Mock 本来对 execute/chat 使用同一响应队列；两个入口显式接纳完整值。
    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move {
            self.record_invocation(message, &invocation);
            self.next_event_stream(cancel, invocation)
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
    messages: Arc<Mutex<Vec<Message>>>,
    invocation_contexts: Arc<Mutex<Vec<AgentInvocationContext>>>,
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
            messages: Arc::new(Mutex::new(Vec::new())),
            invocation_contexts: Arc::new(Mutex::new(Vec::new())),
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

    pub fn received_messages(&self) -> Vec<Message> {
        self.messages
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn invocation_contexts(&self) -> Vec<AgentInvocationContext> {
        self.invocation_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn typed_source<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> Result<BoxStream<'a, Result<AgentEvent>>> {
        self.record_call(&message.content.as_text().unwrap_or_default());
        self.messages
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(message);
        self.invocation_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(invocation.clone());
        let scripted = self
            .event_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front();
        let Some(steps) = scripted else {
            return Err(self.failure.error(self.error_message.clone()));
        };
        Ok(self.script_stream(steps, cancel, invocation))
    }

    fn record_call(&self, task: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(task.to_string());
    }

    fn script_stream<'a>(
        &'a self,
        steps: Vec<MockAgentStep>,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxStream<'a, Result<AgentEvent>> {
        let events = async_stream::stream! {
            let _invocation = invocation;
            for step in steps {
                match step {
                    MockAgentStep::Event(event) => yield Ok(*event),
                    MockAgentStep::Error(message) => {
                        yield Err(self.failure.error(message));
                        return;
                    }
                    MockAgentStep::Failure(error) => {
                        yield Err(error);
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

    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move { self.typed_source(message, cancel, invocation) })
    }

    fn chat_stream_message_with_invocation_context<'a>(
        &'a self,
        message: Message,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move { self.typed_source(message, cancel, invocation) })
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

    fn partial_source_steps() -> Vec<MockAgentStep> {
        vec![
            MockAgentStep::Event(Box::new(AgentEvent::ExecutionStatus(ExecutionStatus {
                adapter_generated: false,
                execution_id: "partial-execution".into(),
                request_id: Some("partial-request".into()),
                sequence: 0,
                phase: AgentPhase::Thinking,
                status: ExecutionStatusKind::Started,
                reason: None,
            }))),
            MockAgentStep::Event(Box::new(AgentEvent::AssistantMessage {
                block: echo_core::agent::AssistantMessageBlock {
                    provider_source: None,
                    provider: None,
                    reasoning: None,
                    block_id: "partial-block".into(),
                    request_id: "partial-request".into(),
                    sequence: 0,
                    phase: echo_core::agent::AssistantPhase::Commentary,
                    delta: "part".into(),
                    complete: false,
                },
            })),
        ]
    }

    #[tokio::test]
    async fn mock_agent_scripts_partial_event_then_error() -> Result<()> {
        let mut steps = partial_source_steps();
        steps.push(MockAgentStep::Error("stream failed".to_string()));
        let agent = MockAgent::new("partial").with_event_script(steps);
        let mut events = agent.execute_stream("work").await?;

        assert!(matches!(events.next().await,
            Some(Ok(AgentEvent::ExecutionStatus(status))) if status.status == ExecutionStatusKind::Started));
        assert!(matches!(events.next().await,
            Some(Ok(AgentEvent::AssistantMessage { block })) if block.delta == "part" && !block.complete));
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
        let mut steps = partial_source_steps();
        steps.push(MockAgentStep::Error("deadline".to_string()));
        let agent = FailingMockAgent::new("partial-failure", "default failure")
            .with_failure(MockAgentFailure::Timeout)
            .with_event_script(steps);
        let mut events = agent
            .execute_stream_with_cancel("work", CancellationToken::new())
            .await?;

        assert!(matches!(events.next().await,
            Some(Ok(AgentEvent::ExecutionStatus(status))) if status.status == ExecutionStatusKind::Started));
        assert!(matches!(events.next().await,
            Some(Ok(AgentEvent::AssistantMessage { block })) if block.delta == "part" && !block.complete));
        assert!(matches!(
            events.next().await,
            Some(Err(ReactError::Agent(error)))
                if matches!(*error, AgentError::Timeout(ref message) if message == "deadline")
        ));
        assert!(events.next().await.is_none());
        assert_eq!(agent.calls(), vec!["work".to_string()]);
        Ok(())
    }
    struct SourceGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for SourceGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn typed_mock_execute_and_chat_keep_full_input_and_native_source_identity() -> Result<()>
    {
        use echo_core::tools::{ExternalRunContext, InvocationResourceGuard, ToolAccess};
        use std::sync::atomic::{AtomicUsize, Ordering};
        for chat in [false, true] {
            let agent = MockAgent::new("typed").with_response("最终中文🌊");
            let message: Message = serde_json::from_value(serde_json::json!({
                "role":"user", "name":"caller", "content":[
                    {"type":"text", "text":"完整输入"},
                    {"type":"image_url", "image_url":{"url":"data:image/png;base64,YQ=="}}
                ], "reasoning_content":"回放字段"
            }))?;
            let expected = serde_json::to_value(&message)?;
            let history = vec![
                Message::user("历史".into()),
                Message::assistant("历史答复".into()),
            ];
            let expected_history = serde_json::to_value(&history)?;
            let cancel = CancellationToken::new();
            let drops = Arc::new(AtomicUsize::new(0));
            let invocation = AgentInvocationContext {
                runtime: Some(ExternalRunContext {
                    run_id: Some("mock-run".into()),
                    execution_id: Some("mock-execution".into()),
                    message_id: Some("mock-request".into()),
                    turn_id: Some("mock-turn".into()),
                    conversation_id: Some("mock-conversation".into()),
                    ..Default::default()
                }),
                runtime_state_id: Some("mock-state".into()),
                transcript_generation_id: Some("mock-state".into()),
                working_dir: Some(std::path::PathBuf::from("/mock/workdir")),
                cancel: Some(cancel.clone()),
                history: Some(history),
                allowed_tools: Some(std::collections::HashSet::from(["read_file".into()])),
                disabled_tools: Some(std::collections::HashSet::from(["write_file".into()])),
                visible_tools: Some(std::collections::HashSet::from(["read_file".into()])),
                tool_access: Some(ToolAccess::ReadOnly),
                resource_guards: vec![InvocationResourceGuard::new(SourceGuard(drops.clone()))],
                ..Default::default()
            };
            let mut source = if chat {
                agent
                    .chat_stream_message_with_invocation_context(
                        message,
                        cancel.clone(),
                        invocation,
                    )
                    .await?
            } else {
                agent
                    .execute_stream_message_with_invocation_context(
                        message,
                        cancel.clone(),
                        invocation,
                    )
                    .await?
            };
            assert_eq!(serde_json::to_value(agent.last_message())?, expected);
            let contexts = agent.take_invocation_contexts();
            let context = contexts
                .first()
                .ok_or_else(|| ReactError::Other("typed invocation missing".into()))?;
            assert_eq!(serde_json::to_value(&context.history)?, expected_history);
            assert_eq!(context.runtime_state_id.as_deref(), Some("mock-state"));
            assert_eq!(
                context.transcript_generation_id.as_deref(),
                Some("mock-state")
            );
            assert_eq!(
                context.working_dir.as_deref(),
                Some(std::path::Path::new("/mock/workdir"))
            );
            assert_eq!(context.tool_access, Some(ToolAccess::ReadOnly));
            assert!(
                context
                    .allowed_tools
                    .as_ref()
                    .is_some_and(|tools| tools.contains("read_file"))
            );
            assert!(
                context
                    .disabled_tools
                    .as_ref()
                    .is_some_and(|tools| tools.contains("write_file"))
            );
            assert!(
                context
                    .visible_tools
                    .as_ref()
                    .is_some_and(|tools| tools.contains("read_file"))
            );
            assert!(
                context
                    .resource_guards
                    .iter()
                    .any(|guard| guard.retains::<SourceGuard>())
            );
            let recorded_cancel = context
                .cancel
                .clone()
                .ok_or_else(|| ReactError::Other("cancel token missing".into()))?;
            let runtime = context
                .runtime
                .as_ref()
                .ok_or_else(|| ReactError::Other("runtime missing".into()))?;
            assert_eq!(runtime.run_id.as_deref(), Some("mock-run"));
            assert_eq!(runtime.turn_id.as_deref(), Some("mock-turn"));
            assert_eq!(runtime.message_id.as_deref(), Some("mock-request"));
            assert_eq!(
                runtime.conversation_id.as_deref(),
                Some("mock-conversation")
            );
            drop(contexts);
            let started = source
                .next()
                .await
                .transpose()?
                .ok_or_else(|| ReactError::Other("source Started missing".into()))?;
            assert!(
                matches!(&started, AgentEvent::ExecutionStatus(status) if !status.adapter_generated
                && status.status == ExecutionStatusKind::Started && status.execution_id == "mock-execution"
                && status.request_id.as_deref() == Some("mock-request") && status.sequence == 0)
            );
            assert!(
                matches!(source.next().await, Some(Ok(AgentEvent::FinalAnswer(answer))) if answer == "最终中文🌊")
            );
            assert!(
                matches!(source.next().await, Some(Ok(AgentEvent::ExecutionStatus(status)))
                if !status.adapter_generated && status.status == ExecutionStatusKind::Settled
                && status.execution_id == "mock-execution" && status.request_id.as_deref() == Some("mock-request")
                && status.sequence == 1)
            );
            cancel.cancel();
            assert!(recorded_cancel.is_cancelled());
            assert_eq!(drops.load(Ordering::SeqCst), 0, "terminal 不是 source EOF");
            assert!(source.next().await.is_none());
            drop(source);
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            assert_eq!(agent.calls(), vec!["完整输入".to_string()]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn default_mock_cancellation_is_owned_by_the_same_started_source() -> Result<()> {
        let agent = MockAgent::new("cancel")
            .with_response("unused")
            .with_delay_ms(60_000);
        let cancel = CancellationToken::new();
        let mut source = agent
            .execute_stream_with_cancel("work", cancel.clone())
            .await?;
        let started = source
            .next()
            .await
            .transpose()?
            .ok_or_else(|| ReactError::Other("Started missing".into()))?;
        let AgentEvent::ExecutionStatus(started) = started else {
            return Err(ReactError::Other("non-native Started".into()));
        };
        cancel.cancel();
        assert!(
            matches!(source.next().await, Some(Ok(AgentEvent::ExecutionStatus(status)))
            if status.status == ExecutionStatusKind::Cancelled && status.execution_id == started.execution_id
            && status.request_id == started.request_id && !status.adapter_generated)
        );
        assert!(matches!(
            source.next().await,
            Some(Ok(AgentEvent::Cancelled))
        ));
        assert!(source.next().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn explicit_mock_scripts_keep_partial_unknown_and_foreign_facts_without_success()
    -> Result<()> {
        for chat in [false, true] {
            let status = AgentEvent::ExecutionStatus(ExecutionStatus {
                adapter_generated: false,
                execution_id: "foreign-source".into(),
                request_id: Some("foreign-request".into()),
                sequence: 7,
                phase: AgentPhase::Thinking,
                status: ExecutionStatusKind::Settled,
                reason: None,
            });
            let agent = MockAgent::new("explicit")
                .with_default_success("must not be appended")
                .with_event_script(vec![
                    MockAgentStep::Event(Box::new(AgentEvent::FinalAnswer("partial".into()))),
                    MockAgentStep::Event(Box::new(status)),
                ]);
            let mut source = if chat {
                agent.chat_stream("work").await?
            } else {
                agent.execute_stream("work").await?
            };
            assert!(
                matches!(source.next().await, Some(Ok(AgentEvent::FinalAnswer(answer))) if answer == "partial")
            );
            assert!(
                matches!(source.next().await, Some(Ok(AgentEvent::ExecutionStatus(status)))
                if status.execution_id == "foreign-source" && status.sequence == 7)
            );
            assert!(source.next().await.is_none());
            for failing in [false, true] {
                let steps = vec![MockAgentStep::Failure(ReactError::ExecutionNotSettled {
                    run_id: "original-run".into(),
                    execution_id: Some("original-execution".into()),
                })];
                let producer: Box<dyn Agent> = if failing {
                    Box::new(
                        FailingMockAgent::new("unknown", "must not replace typed Err")
                            .with_event_script(steps),
                    )
                } else {
                    Box::new(
                        MockAgent::new("unknown")
                            .with_default_success("must not append success")
                            .with_event_script(steps),
                    )
                };
                let mut original = if chat {
                    producer.chat_stream("work").await?
                } else {
                    producer.execute_stream("work").await?
                };
                assert!(
                    matches!(original.next().await, Some(Err(ReactError::ExecutionNotSettled { run_id, execution_id: Some(execution_id) }))
                    if run_id == "original-run" && execution_id == "original-execution")
                );
                assert!(original.next().await.is_none());
            }
            let partial = MockAgent::new("partial").with_event_script(vec![MockAgentStep::Event(
                Box::new(AgentEvent::FinalAnswer("unqualified".into())),
            )]);
            assert!(
                if chat {
                    partial.chat("work").await
                } else {
                    partial.execute("work").await
                }
                .is_err()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn failing_mock_typed_execute_and_chat_keep_original_setup_failure() -> Result<()> {
        for chat in [false, true] {
            for failure in [
                MockAgentFailure::Initialization,
                MockAgentFailure::Subagent,
                MockAgentFailure::Timeout,
                MockAgentFailure::Cancelled,
                MockAgentFailure::PermissionDenied,
            ] {
                let agent = FailingMockAgent::new("failure", "original setup failure")
                    .with_failure(failure);
                let result = if chat {
                    agent
                        .chat_stream_message_with_invocation_context(
                            Message::user("typed input".into()),
                            CancellationToken::new(),
                            AgentInvocationContext::default(),
                        )
                        .await
                } else {
                    agent
                        .execute_stream_message_with_invocation_context(
                            Message::user("typed input".into()),
                            CancellationToken::new(),
                            AgentInvocationContext::default(),
                        )
                        .await
                };
                let error = match result {
                    Err(ReactError::Agent(error)) => error,
                    _ => return Err(ReactError::Other("typed setup failure was replaced".into())),
                };
                assert!(match (failure, error.as_ref()) {
                    (
                        MockAgentFailure::Initialization,
                        AgentError::InitializationFailed(message),
                    )
                    | (MockAgentFailure::Subagent, AgentError::SubagentError(message))
                    | (MockAgentFailure::Timeout, AgentError::Timeout(message))
                    | (MockAgentFailure::Cancelled, AgentError::Cancelled(message))
                    | (MockAgentFailure::PermissionDenied, AgentError::PermissionDenied(message)) =>
                        message == "original setup failure",
                    _ => false,
                });
                assert_eq!(agent.calls(), vec!["typed input".to_string()]);
                assert_eq!(agent.received_messages().len(), 1);
                assert_eq!(agent.invocation_contexts().len(), 1);
            }
        }
        Ok(())
    }
}
