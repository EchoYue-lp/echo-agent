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

/// Enum of preset responses (text, tool calls, scripted streams, or errors)
enum MockLlmResponse {
    Content(String, Option<crate::llm::types::Usage>),
    ToolCalls(Message, Option<crate::llm::types::Usage>),
    Stream(Vec<StreamChunk>),
    Err(ReactError),
}

/// One chunk of a scripted streaming turn.
///
/// Real providers never merge content and usage into a single chunk:
/// content arrives as deltas and the terminal chunk carries `finish_reason`
/// plus usage (Anthropic `message_delta.usage` carries only `output_tokens`;
/// OpenAI streams usage on a separate final chunk). `StreamChunk` lets tests
/// reproduce that wire shape instead of certifying the impossible
/// single-chunk shape the old mock emitted (F-TST-01-P1-01).
pub enum StreamChunk {
    /// A content or tool-call delta without terminal information.
    Delta(DeltaMessage),
    /// Terminal chunk: finish reason + provider-reported usage.
    Terminal {
        finish_reason: Option<String>,
        usage: Option<crate::llm::types::Usage>,
    },
    /// Mid-stream error (provider disconnect, malformed event, timeout).
    Err(ReactError),
    /// A cancellation-aware wait before the next scripted chunk.
    Delay(Duration),
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
    user_ids: Arc<Mutex<Vec<Option<String>>>>,
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
            user_ids: Arc::new(Mutex::new(Vec::new())),
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

    /// Append a scripted streaming turn. `chat_stream` emits the chunks in
    /// order; a `StreamChunk::Err` ends the stream with that error.
    ///
    /// Use this to reproduce the real provider wire shape: content deltas
    /// first, then a separate terminal chunk carrying `finish_reason` and
    /// usage (the single-chunk shape the plain builders emit is
    /// intentionally reserved for non-streaming tests).
    ///
    /// # Example
    ///
    /// ```rust
    /// use echo_agent::testing::{MockLlmClient, StreamChunk};
    /// use echo_agent::llm::types::{DeltaMessage, Usage};
    ///
    /// let usage = Usage {
    ///     prompt_tokens: Some(10),
    ///     completion_tokens: Some(5),
    ///     ..Default::default()
    /// };
    /// let mock = MockLlmClient::new().with_stream_script(vec![
    ///     StreamChunk::Delta(DeltaMessage {
    ///         role: Some("assistant".to_string()),
    ///         content: Some("Hello".to_string()),
    ///         reasoning_content: None,
    ///         reasoning_blocks: None,
    ///         tool_calls: None,
    ///     }),
    ///     StreamChunk::Terminal {
    ///         finish_reason: Some("stop".to_string()),
    ///         usage: Some(usage),
    ///     },
    /// ]);
    /// ```
    pub fn with_stream_script(self, chunks: Vec<StreamChunk>) -> Self {
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(MockLlmResponse::Stream(chunks));
        self
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

    /// Cache-partition identities received by each request.
    pub fn all_user_ids(&self) -> Vec<Option<String>> {
        self.user_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
        self.user_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.tool_choices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.tool_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
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
            Some(MockLlmResponse::Stream(chunks)) => Ok(PopResult::Stream(chunks)),
            Some(MockLlmResponse::Err(e)) => Err(e),
            None => Err(ReactError::Llm(Box::new(LlmError::EmptyResponse))),
        }
    }
}

enum PopResult {
    Content(String, Option<crate::llm::types::Usage>),
    ToolCalls(Message, Option<crate::llm::types::Usage>),
    Stream(Vec<StreamChunk>),
}

impl LlmClient for MockLlmClient {
    fn chat(&self, request: ChatRequest) -> BoxFuture<'_, Result<ChatResponse>> {
        Box::pin(async move {
            // Record this call
            self.tool_counts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.tools.as_ref().map_or(0, Vec::len));
            self.user_ids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.user_id.clone());
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
                let token = request.cancel_token.clone().unwrap_or_default();
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
                    usage: usage.clone(),
                    raw: crate::llm::types::ChatCompletionResponse {
                        usage,
                        ..crate::llm::types::ChatCompletionResponse::default()
                    },
                }),
                PopResult::ToolCalls(message, usage) => Ok(ChatResponse {
                    message,
                    finish_reason: Some("tool_calls".to_string()),
                    usage: usage.clone(),
                    raw: crate::llm::types::ChatCompletionResponse {
                        usage,
                        ..crate::llm::types::ChatCompletionResponse::default()
                    },
                }),
                // A scripted stream used through the non-streaming entry:
                // fold the deltas into a single assistant message and take
                // the last terminal's finish reason / usage.
                PopResult::Stream(chunks) => {
                    let mut text = String::new();
                    let mut reasoning = String::new();
                    let mut reasoning_blocks = Vec::new();
                    let mut finish_reason = None;
                    let mut usage = None;
                    for chunk in chunks {
                        match chunk {
                            StreamChunk::Delta(delta) => {
                                if let Some(t) = delta.content {
                                    text.push_str(&t);
                                }
                                if let Some(value) = delta.reasoning_content {
                                    reasoning.push_str(&value);
                                }
                                reasoning_blocks.extend(delta.reasoning_blocks.unwrap_or_default());
                            }
                            StreamChunk::Terminal {
                                finish_reason: fr,
                                usage: u,
                            } => {
                                finish_reason = fr;
                                usage = u;
                            }
                            StreamChunk::Err(e) => return Err(e),
                            StreamChunk::Delay(delay) => {
                                let token = request.cancel_token.clone().unwrap_or_default();
                                tokio::select! {
                                    _ = token.cancelled() => {
                                        return Err(ReactError::Agent(Box::new(
                                            crate::error::AgentError::Cancelled(
                                                "mock LLM call cancelled".to_string(),
                                            ),
                                        )));
                                    }
                                    _ = tokio::time::sleep(delay) => {}
                                }
                            }
                        }
                    }
                    let mut message = Message::assistant(text);
                    message.reasoning_content = (!reasoning.is_empty()).then_some(reasoning);
                    message.reasoning_blocks =
                        (!reasoning_blocks.is_empty()).then_some(reasoning_blocks);
                    Ok(ChatResponse {
                        message,
                        finish_reason,
                        usage: usage.clone(),
                        raw: crate::llm::types::ChatCompletionResponse {
                            usage,
                            ..crate::llm::types::ChatCompletionResponse::default()
                        },
                    })
                }
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
            self.user_ids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(request.user_id.clone());
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
                let token = request.cancel_token.clone().unwrap_or_default();
                tokio::select! {
                    _ = token.cancelled() => {
                        return Err(ReactError::Other("mock LLM stream cancelled".into()));
                    }
                    _ = tokio::time::sleep(d) => {}
                }
            }

            // Real providers stream content as deltas and report usage on a
            // separate terminal chunk; emit that shape instead of the
            // impossible single-chunk merge (F-TST-01-P1-01).
            match self.pop_response()? {
                PopResult::Content(text, usage) => {
                    let stream = futures::stream::iter([
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: Some("assistant".to_string()),
                                content: Some(text),
                                reasoning_content: None,
                                reasoning_blocks: None,
                                tool_calls: None,
                            },
                            finish_reason: None,
                            usage: None,
                        }),
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: None,
                                content: None,
                                reasoning_content: None,
                                reasoning_blocks: None,
                                tool_calls: None,
                            },
                            finish_reason: Some("stop".to_string()),
                            usage,
                        }),
                    ]);
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
                    let stream = futures::stream::iter([
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: Some("assistant".to_string()),
                                content,
                                reasoning_content,
                                reasoning_blocks: message.reasoning_blocks,
                                tool_calls: Some(delta_calls),
                            },
                            finish_reason: None,
                            usage: None,
                        }),
                        Ok(ChatChunk {
                            delta: DeltaMessage {
                                role: None,
                                content: None,
                                reasoning_content: None,
                                reasoning_blocks: None,
                                tool_calls: None,
                            },
                            finish_reason: Some("tool_calls".to_string()),
                            usage,
                        }),
                    ]);
                    Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
                }
                PopResult::Stream(chunks) => {
                    let cancel = request.cancel_token.unwrap_or_default();
                    let stream = async_stream::stream! {
                        for chunk in chunks {
                            match chunk {
                            StreamChunk::Delta(delta) => yield Ok(ChatChunk {
                                delta,
                                finish_reason: None,
                                usage: None,
                            }),
                            StreamChunk::Terminal {
                                finish_reason,
                                usage,
                            } => yield Ok(ChatChunk {
                                delta: DeltaMessage {
                                    role: None,
                                    content: None,
                                    reasoning_content: None,
                                    reasoning_blocks: None,
                                    tool_calls: None,
                                },
                                finish_reason,
                                usage,
                            }),
                            StreamChunk::Err(error) => {
                                yield Err(error);
                                return;
                            }
                            StreamChunk::Delay(delay) => {
                                tokio::select! {
                                    _ = cancel.cancelled() => {
                                        yield Err(ReactError::Agent(Box::new(
                                            crate::error::AgentError::Cancelled(
                                                "mock LLM stream cancelled".to_string(),
                                            ),
                                        )));
                                        return;
                                    }
                                    _ = tokio::time::sleep(delay) => {}
                                }
                            }
                            }
                        }
                    };
                    Ok(Box::pin(stream) as BoxStream<'_, Result<ChatChunk>>)
                }
            }
        })
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
