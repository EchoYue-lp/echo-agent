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

use crate::error::Result;
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Enum of preset execution results
enum MockToolResponse {
    Success(String),
    Failure(String),
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
            .unwrap()
            .push_back(MockToolResponse::Success(text.into()));
        self
    }

    /// Append multiple successful responses in bulk
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap();
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
            .unwrap()
            .push_back(MockToolResponse::Failure(msg.into()));
        self
    }

    /// Total number of calls executed
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// The parameters passed in the last call (returns `None` if never called)
    pub fn last_args(&self) -> Option<HashMap<String, Value>> {
        self.calls.lock().unwrap().last().cloned()
    }

    /// All historical call parameters (in chronological order)
    pub fn all_calls(&self) -> Vec<HashMap<String, Value>> {
        self.calls.lock().unwrap().clone()
    }

    /// Clear recorded call history
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap().clear();
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
        Box::pin(async move {
            // Record this call's parameters
            self.calls.lock().unwrap().push(params.clone());

            let response = self.responses.lock().unwrap().pop_front();
            match response {
                Some(MockToolResponse::Success(text)) => Ok(ToolResult::success(text)),
                Some(MockToolResponse::Failure(msg)) => Ok(ToolResult::error(msg)),
                // Return default success when queue is exhausted
                None => Ok(ToolResult::success("mock response".to_string())),
            }
        })
    }
}
