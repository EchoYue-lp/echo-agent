//! Mock tool for testing Agent tool-calling behavior without depending on
//! external services.
//!
//! # Typical uses
//! - Testing tool parameter parsing logic
//! - Replacing real tools (databases, HTTP, etc.) in integration tests
//! - Testing Agent fault-tolerance behavior when tool execution fails
//!
//! # Example
//!
//! ```rust
//! use echo_agent::testing::MockTool;
//! use echo_agent::tools::Tool;
//! use std::collections::HashMap;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let tool = MockTool::new("calculator")
//!     .with_description("Compute the sum of two numbers")
//!     .with_response("The result is 42");
//!
//! let params = HashMap::new();
//! let result = tool.execute(params).await.unwrap();
//! assert!(result.success);
//! assert_eq!(result.output, "The result is 42");
//! assert_eq!(tool.call_count(), 1);
//! # }
//! ```

use crate::error::{ReactError, Result, ToolError};
use crate::tools::{Tool, ToolContext, ToolParameters, ToolResult, ToolStreamEvent};
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Enum of preset execution results
enum MockToolResponse {
    Success(String),
    Failure(String),
    Result(Box<ToolResult>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockToolContext {
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub turn_id: Option<String>,
    pub execution_id: Option<String>,
    pub call_id: Option<String>,
    pub cancellation_provided: bool,
}

/// A scriptable Mock Tool.
///
/// Returns preset execution results in order; once the queue is exhausted,
/// returns the last response (if any), otherwise returns a default success
/// response of `"mock response"`.
pub struct MockTool {
    name: String,
    description: String,
    parameters: Value,
    responses: Arc<Mutex<VecDeque<MockToolResponse>>>,
    /// The parameters received on each call, recorded in order
    calls: Arc<Mutex<Vec<HashMap<String, Value>>>>,
    contexts: Arc<Mutex<Vec<MockToolContext>>>,
    stream_scripts: Arc<Mutex<VecDeque<Vec<ToolStreamEvent>>>>,
    /// Optional delay before each execution (lets tests control completion
    /// order in concurrent batches).
    delay: Option<std::time::Duration>,
    default_success: Option<String>,
}

impl MockTool {
    /// Create a named Mock Tool (description and parameter schema use defaults)
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: "A mock tool for testing".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::<HashMap<String, Value>>::new())),
            contexts: Arc::new(Mutex::new(Vec::new())),
            stream_scripts: Arc::new(Mutex::new(VecDeque::new())),
            delay: None,
            default_success: None,
        }
    }

    /// Set the tool description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set parameter JSON Schema
    pub fn with_parameters(mut self, schema: Value) -> Self {
        self.parameters = schema;
        self
    }

    /// Append a successful response text
    pub fn with_response(self, text: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockToolResponse::Success(text.into()));
        self
    }

    /// Append multiple successful responses in bulk
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            for t in texts {
                q.push_back(MockToolResponse::Success(t.into()));
            }
        }
        self
    }

    /// Append a failure response (for testing Agent behavior on tool failure)
    pub fn with_failure(self, msg: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockToolResponse::Failure(msg.into()));
        self
    }

    pub fn with_result(self, result: ToolResult) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(MockToolResponse::Result(Box::new(result)));
        self
    }

    pub fn with_error(self, message: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(MockToolResponse::Error(message.into()));
        self
    }

    pub fn with_stream_script(self, events: Vec<ToolStreamEvent>) -> Self {
        self.stream_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(events);
        self
    }

    /// Sleep for this duration before each execution. Use to control
    /// completion order in concurrent batch tests (e.g. a slow peer that
    /// finishes after a fast one).
    pub fn with_delay(mut self, delay: std::time::Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Explicitly allow calls after the response script is exhausted.
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

    /// Total number of calls executed
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// The parameters passed in the last call (returns `None` if never called)
    pub fn last_args(&self) -> Option<HashMap<String, Value>> {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .cloned()
    }

    /// All historical call parameters (in chronological order)
    pub fn all_calls(&self) -> Vec<HashMap<String, Value>> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn contexts(&self) -> Vec<MockToolContext> {
        self.contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Clear recorded call history
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
    }

    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        let context = ToolContext::default();
        Box::pin(async move { self.execute_with_context(params, &context).await })
    }

    fn execute_with_context<'a>(
        &'a self,
        params: ToolParameters,
        context: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            // Record this call's parameters
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(params.clone());
            self.contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(MockToolContext {
                    conversation_id: context.conversation_id.clone(),
                    run_id: context.run_id.clone(),
                    turn_id: context.turn_id.clone(),
                    execution_id: context.execution_id.clone(),
                    call_id: context.call_id.clone(),
                    cancellation_provided: context.cancel.is_some(),
                });

            if let Some(d) = self.delay {
                if let Some(cancel) = context.cancel.as_ref() {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            return Err(ReactError::Agent(Box::new(
                                crate::error::AgentError::Cancelled(
                                    "mock tool cancelled".to_string(),
                                ),
                            )));
                        }
                        _ = tokio::time::sleep(d) => {}
                    }
                } else {
                    tokio::time::sleep(d).await;
                }
            }

            let response = self
                .responses
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front();
            match response {
                Some(MockToolResponse::Success(text)) => Ok(ToolResult::success(text)),
                Some(MockToolResponse::Failure(msg)) => Ok(ToolResult::error(msg)),
                Some(MockToolResponse::Result(result)) => Ok(*result),
                Some(MockToolResponse::Error(message)) => {
                    Err(ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                        tool: self.name.clone(),
                        message,
                    })))
                }
                None => self
                    .default_success
                    .clone()
                    .map(ToolResult::success)
                    .ok_or_else(|| {
                        ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                            tool: self.name.clone(),
                            message: "MockTool response script exhausted".to_string(),
                        }))
                    }),
            }
        })
    }

    fn execute_stream_with_context<'a>(
        &'a self,
        params: ToolParameters,
        context: &ToolContext,
    ) -> BoxFuture<'a, Result<std::pin::Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>>
    {
        let context = context.clone();
        Box::pin(async move {
            let scripted = self
                .stream_scripts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front();
            let events = match scripted {
                Some(events) => events,
                None => vec![ToolStreamEvent::Complete(
                    self.execute_with_context(params, &context).await?,
                )],
            };
            Ok(Box::pin(stream::iter(events))
                as std::pin::Pin<
                    Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>,
                >)
        })
    }

    fn supports_streaming(&self) -> bool {
        !self
            .stream_scripts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    }
}
