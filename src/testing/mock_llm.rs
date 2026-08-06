//! Mock LLM client for testing components that use [`LlmClient`] without making
//! real HTTP requests.
//!
//! Typical uses:
//! - Testing [`SummaryCompressor`] (which calls LLMs via `LlmClient`)
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
use std::time::Duration;

/// Enum of preset responses (text, tool calls, or errors)
enum MockLlmResponse {
    Content(String, Option<crate::llm::types::Usage>),
    ToolCalls(Message, Option<crate::llm::types::Usage>),
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
    tool_choices: Arc<Mutex<Vec<Option<String>>>>,
    tool_counts: Arc<Mutex<Vec<usize>>>,
    /// Optional delay before returning each response. When set, `chat` and
    /// `chat_stream` sleep for this duration, but will return early with a
    /// Cancelled error if the request's `cancel_token` is triggered. This lets
    /// tests simulate "running" LLM calls that can be interrupted (Phase 3).
    delay: Option<Duration>,
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
            tool_choices: Arc::new(Mutex::new(Vec::new())),
            tool_counts: Arc::new(Mutex::new(Vec::new())),
            delay: None,
        }
    }

    /// Set a delay before each response. When combined with a request
    /// `cancel_token`, this lets tests verify mid-flight cancellation (Phase 3).
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
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
            .push_back(MockLlmResponse::Content(text.into(), None));
        self
    }

    /// Append a text response with provider-reported usage.
    pub fn with_response_usage(
        self,
        text: impl Into<String>,
        usage: crate::llm::types::Usage,
    ) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::Content(text.into(), Some(usage)));
        self
    }

    /// Append multiple successful responses in bulk
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            for t in texts {
                q.push_back(MockLlmResponse::Content(t.into(), None));
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
            .push_back(MockLlmResponse::ToolCalls(
                Message::assistant_with_tools(vec![tc]),
                None,
            ));
        self
    }

    /// Append a DeepSeek-style tool-call response with reasoning content.
    #[cfg(test)]
    pub(crate) fn then_reasoning_tool_call(
        self,
        id: impl Into<String>,
        function_name: impl Into<String>,
        arguments: impl Into<String>,
        content: impl Into<String>,
        reasoning_content: impl Into<String>,
    ) -> Self {
        let tc = ToolCall {
            id: id.into(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: function_name.into(),
                arguments: arguments.into(),
            },
        };
        let mut message = Message::assistant_with_tools(vec![tc]);
        message.content = crate::llm::types::MessageContent::Text(content.into());
        message.reasoning_content = Some(reasoning_content.into());
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::ToolCalls(message, None));
        self
    }

    /// Append a tool-call response with provider-reported usage.
    pub fn then_tool_call_with_usage(
        self,
        id: impl Into<String>,
        function_name: impl Into<String>,
        arguments: impl Into<String>,
        usage: crate::llm::types::Usage,
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
            .push_back(MockLlmResponse::ToolCalls(
                Message::assistant_with_tools(vec![tc]),
                Some(usage),
            ));
        self
    }

    /// Append a multi-tool-call response (parallel tool calls)
    pub fn then_tool_calls(self, calls: Vec<ToolCall>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::ToolCalls(
                Message::assistant_with_tools(calls),
                None,
            ));
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
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last()
            .cloned()
    }

    /// All historical call messages (in chronological order)
    pub fn all_calls(&self) -> Vec<Vec<Message>> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Tool-choice values received by each request.
    pub fn all_tool_choices(&self) -> Vec<Option<String>> {
        self.tool_choices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Number of exposed tool definitions received by each request.
    pub fn all_tool_counts(&self) -> Vec<usize> {
        self.tool_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Number of remaining unconsumed preset responses
    pub fn remaining(&self) -> usize {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Clear all recorded call history (response queue is unaffected)
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// Pop the next response (text or tool calls)
    fn pop_response(&self) -> Result<PopResult> {
        match self
            .responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
        {
            Some(MockLlmResponse::Content(text, usage)) => Ok(PopResult::Content(text, usage)),
            Some(MockLlmResponse::ToolCalls(message, usage)) => {
                Ok(PopResult::ToolCalls(message, usage))
            }
            Some(MockLlmResponse::Err(e)) => Err(e),
            None => Err(ReactError::Llm(Box::new(LlmError::EmptyResponse))),
        }
    }
}

enum PopResult {
    Content(String, Option<crate::llm::types::Usage>),
    ToolCalls(Message, Option<crate::llm::types::Usage>),
}

impl LlmClient for MockLlmClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            // Record this call
            self.tool_counts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.tools.as_ref().map_or(0, Vec::len));
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.messages);
            self.tool_choices
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.tool_choice);

            // Optional delay with cancel-awareness (Phase 3: lets tests verify
            // mid-flight cancellation).
            if let Some(d) = self.delay {
                let token = request.cancel_token.unwrap_or_default();
                tokio::select! {
                    _ = token.cancelled() => {
                        return Err(ReactError::Other("mock LLM call cancelled".into()));
                    }
                    _ = tokio::time::sleep(d) => {}
                }
            }

            match self.pop_response()? {
                PopResult::Content(text, usage) => Ok(ChatResponse {
                    message: Message::assistant(text),
                    finish_reason: Some("stop".to_string()),
                    raw: crate::llm::types::ChatCompletionResponse {
                        usage,
                        ..crate::llm::types::ChatCompletionResponse::default()
                    },
                }),
                PopResult::ToolCalls(message, usage) => Ok(ChatResponse {
                    message,
                    finish_reason: Some("tool_calls".to_string()),
                    raw: crate::llm::types::ChatCompletionResponse {
                        usage,
                        ..crate::llm::types::ChatCompletionResponse::default()
                    },
                }),
            }
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Result<ChatChunk>>>> {
        Box::pin(async move {
            // Record this call
            self.tool_counts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.tools.as_ref().map_or(0, Vec::len));
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.messages);
            self.tool_choices
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.tool_choice);

            // Optional delay with cancel-awareness (Phase 3).
            if let Some(d) = self.delay {
                let token = request.cancel_token.unwrap_or_default();
                tokio::select! {
                    _ = token.cancelled() => {
                        return Err(ReactError::Other("mock LLM stream cancelled".into()));
                    }
                    _ = tokio::time::sleep(d) => {}
                }
            }

            match self.pop_response()? {
                PopResult::Content(text, usage) => {
                    let stream = futures::stream::once(async move {
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: Some("assistant".to_string()),
                                content: Some(text),
                                reasoning_content: None,
                                tool_calls: None,
                            },
                            finish_reason: Some("stop".to_string()),
                            usage,
                        })
                    });
                    Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
                }
                PopResult::ToolCalls(message, usage) => {
                    // Convert ToolCall → DeltaToolCall for streaming
                    let delta_calls: Vec<DeltaToolCall> = message
                        .tool_calls
                        .unwrap_or_default()
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
                    let content = message.content.as_text();
                    let reasoning_content = message.reasoning_content;
                    let stream = futures::stream::once(async move {
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: Some("assistant".to_string()),
                                content,
                                reasoning_content,
                                tool_calls: Some(delta_calls),
                            },
                            finish_reason: Some("tool_calls".to_string()),
                            usage,
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
