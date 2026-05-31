//! Mock LLM client for testing components that use [`LlmClient`] without making
//! real HTTP requests.
//!
//! Typical uses:
//! - Testing [`SummaryCompressor`] and [`HybridCompressor`] (which call LLMs via `LlmClient`)
//! - Testing custom [`ContextCompressor`] implementations
//! - Any component that has `Arc<dyn LlmClient>` injected as a dependency
//!
//! # Example
//!
//! ```rust
//! use echo_agent::testing::MockLlmClient;
//! use echo_agent::llm::LlmClient;
//! use echo_agent::llm::types::Message;
//! use std::sync::Arc;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let mock = Arc::new(
//!     MockLlmClient::new()
//!         .with_response("First response")
//!         .with_response("Second response")
//! );
//!
//! let r1 = mock.chat_simple(vec![Message::user("hi".to_string())]).await.unwrap();
//! assert_eq!(r1, "First response");
//! assert_eq!(mock.call_count(), 1);
//! # }
//! ```

use crate::error::{LlmError, ReactError, Result};
use crate::llm::types::{
    DeltaFunctionCall, DeltaMessage, DeltaToolCall, FunctionCall, Message, ToolCall,
};
use crate::llm::{ChatChunk, ChatRequest, ChatResponse, LlmClient};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Enum of preset responses (text, tool calls, or errors)
enum MockLlmResponse {
    Content(String),
    ToolCalls(Vec<ToolCall>),
    Err(ReactError),
}

/// A scriptable Mock LLM client.
///
/// Returns preset responses in order; once the queue is exhausted, returns an
/// `EmptyResponse` error.
/// All calls are recorded and can be inspected via [`call_count`](MockLlmClient::call_count) /
/// [`last_messages`](MockLlmClient::last_messages) and other methods.
pub struct MockLlmClient {
    model_name: String,
    responses: Arc<Mutex<VecDeque<MockLlmResponse>>>,
    /// The list of messages received on each call, recorded in order
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Default for MockLlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockLlmClient {
    /// Create an empty Mock with no preset responses yet
    pub fn new() -> Self {
        Self {
            model_name: "mock-model".to_string(),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Set the model name
    pub fn with_model_name(mut self, name: impl Into<String>) -> Self {
        self.model_name = name.into();
        self
    }

    /// Append a successful response text
    pub fn with_response(self, text: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::Content(text.into()));
        self
    }

    /// Append multiple successful responses in bulk
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            for t in texts {
                q.push_back(MockLlmResponse::Content(t.into()));
            }
        }
        self
    }

    /// Append an error response (for testing error handling paths)
    pub fn with_error(self, err: ReactError) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::Err(err));
        self
    }

    /// Append a tool call response (simulate an LLM initiating a tool call)
    ///
    /// # Example
    ///
    /// ```rust
    /// use echo_agent::testing::MockLlmClient;
    ///
    /// let mock = MockLlmClient::new()
    ///     .then_tool_call("call_1", "calculator", r#"{"a":1,"b":2}"#)
    ///     .with_response("The answer is 3");
    /// ```
    pub fn then_tool_call(
        self,
        id: impl Into<String>,
        function_name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        let tc = ToolCall {
            id: id.into(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: function_name.into(),
                arguments: arguments.into(),
            },
        };
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::ToolCalls(vec![tc]));
        self
    }

    /// Append a multi-tool-call response (parallel tool calls)
    pub fn then_tool_calls(self, calls: Vec<ToolCall>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::ToolCalls(calls));
        self
    }

    /// Append a network error (common convenience method)
    pub fn with_network_error(self, msg: impl Into<String>) -> Self {
        self.with_error(ReactError::Llm(Box::new(LlmError::NetworkError(
            msg.into(),
        ))))
    }

    /// Append a rate limit error (429), for testing retry logic
    pub fn with_rate_limit_error(self) -> Self {
        self.with_error(ReactError::Llm(Box::new(LlmError::ApiError {
            status: 429,
            message: "Too Many Requests".to_string(),
        })))
    }

    /// Total number of calls that have occurred
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// The messages passed in the last call (returns `None` if never called)
    pub fn last_messages(&self) -> Option<Vec<Message>> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).last().cloned()
    }

    /// All historical call messages (in chronological order)
    pub fn all_calls(&self) -> Vec<Vec<Message>> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Number of remaining unconsumed preset responses
    pub fn remaining(&self) -> usize {
        self.responses.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Clear all recorded call history (response queue is unaffected)
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Pop the next response (text or tool calls)
    fn pop_response(&self) -> Result<PopResult> {
        match self.responses.lock().unwrap_or_else(|e| e.into_inner()).pop_front() {
            Some(MockLlmResponse::Content(text)) => Ok(PopResult::Content(text)),
            Some(MockLlmResponse::ToolCalls(calls)) => Ok(PopResult::ToolCalls(calls)),
            Some(MockLlmResponse::Err(e)) => Err(e),
            None => Err(ReactError::Llm(Box::new(LlmError::EmptyResponse))),
        }
    }
}

enum PopResult {
    Content(String),
    ToolCalls(Vec<ToolCall>),
}

impl LlmClient for MockLlmClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            // Record this call
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(request.messages);

            match self.pop_response()? {
                PopResult::Content(text) => Ok(ChatResponse {
                    message: Message::assistant(text),
                    finish_reason: Some("stop".to_string()),
                    raw: crate::llm::types::ChatCompletionResponse::default(),
                }),
                PopResult::ToolCalls(calls) => Ok(ChatResponse {
                    message: Message::assistant_with_tools(calls),
                    finish_reason: Some("tool_calls".to_string()),
                    raw: crate::llm::types::ChatCompletionResponse::default(),
                }),
            }
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'_, Result<ChatChunk>>>> {
        Box::pin(async move {
            // Record this call
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(request.messages);

            match self.pop_response()? {
                PopResult::Content(text) => {
                    let stream = futures::stream::once(async move {
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: Some("assistant".to_string()),
                                content: Some(text),
                                reasoning_content: None,
                                tool_calls: None,
                            },
                            finish_reason: Some("stop".to_string()),
                            usage: None,
                        })
                    });
                    Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
                }
                PopResult::ToolCalls(calls) => {
                    // Convert ToolCall → DeltaToolCall for streaming
                    let delta_calls: Vec<DeltaToolCall> = calls
                        .into_iter()
                        .enumerate()
                        .map(|(i, tc)| DeltaToolCall {
                            index: i as u32,
                            id: Some(tc.id),
                            call_type: Some(tc.call_type),
                            function: Some(DeltaFunctionCall {
                                name: Some(tc.function.name),
                                arguments: Some(tc.function.arguments),
                            }),
                        })
                        .collect();
                    let stream = futures::stream::once(async move {
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: Some("assistant".to_string()),
                                content: None,
                                reasoning_content: None,
                                tool_calls: Some(delta_calls),
                            },
                            finish_reason: Some("tool_calls".to_string()),
                            usage: None,
                        })
                    });
                    Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
                }
            }
        })
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
