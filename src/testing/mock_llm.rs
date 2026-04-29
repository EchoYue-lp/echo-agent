//! Mock LLM 客户端，用于在不发起真实 HTTP 请求的情况下测试使用了 [`LlmClient`] 的组件。
//!
//! 典型用途：
//! - 测试 [`SummaryCompressor`] 和 [`HybridCompressor`]（它们通过 `LlmClient` 调用 LLM）
//! - 测试自定义 [`ContextCompressor`] 实现
//! - 任何注入了 `Arc<dyn LlmClient>` 依赖的组件
//!
//! # 示例
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
//!         .with_response("第一次响应")
//!         .with_response("第二次响应")
//! );
//!
//! let r1 = mock.chat_simple(vec![Message::user("hi".to_string())]).await.unwrap();
//! assert_eq!(r1, "第一次响应");
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

/// 预设响应的枚举（文本、工具调用或错误）
enum MockLlmResponse {
    Content(String),
    ToolCalls(Vec<ToolCall>),
    Err(ReactError),
}

/// 可脚本化的 Mock LLM 客户端。
///
/// 按顺序返回预设的响应；队列耗尽后返回 `EmptyResponse` 错误。
/// 所有调用都被记录，可通过 [`call_count`](MockLlmClient::call_count) /
/// [`last_messages`](MockLlmClient::last_messages) 等方法检查。
pub struct MockLlmClient {
    model_name: String,
    responses: Arc<Mutex<VecDeque<MockLlmResponse>>>,
    /// 每次调用时收到的 messages 列表，按顺序记录
    calls: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Default for MockLlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockLlmClient {
    /// 创建空 Mock，尚未设置任何响应
    pub fn new() -> Self {
        Self {
            model_name: "mock-model".to_string(),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 设置模型名称
    pub fn with_model_name(mut self, name: impl Into<String>) -> Self {
        self.model_name = name.into();
        self
    }

    /// 追加一条成功响应文本
    pub fn with_response(self, text: impl Into<String>) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(MockLlmResponse::Content(text.into()));
        self
    }

    /// 批量追加多条成功响应
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap();
            for t in texts {
                q.push_back(MockLlmResponse::Content(t.into()));
            }
        }
        self
    }

    /// 追加一条错误响应（用于测试错误处理路径）
    pub fn with_error(self, err: ReactError) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(MockLlmResponse::Err(err));
        self
    }

    /// 追加一次工具调用响应（模拟 LLM 发起 tool call）
    ///
    /// # 示例
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
            .unwrap()
            .push_back(MockLlmResponse::ToolCalls(vec![tc]));
        self
    }

    /// 追加一次多工具调用响应（并行 tool calls）
    pub fn then_tool_calls(self, calls: Vec<ToolCall>) -> Self {
        self.responses
            .lock()
            .unwrap()
            .push_back(MockLlmResponse::ToolCalls(calls));
        self
    }

    /// 追加一条网络错误（常用的便捷方法）
    pub fn with_network_error(self, msg: impl Into<String>) -> Self {
        self.with_error(ReactError::Llm(Box::new(LlmError::NetworkError(
            msg.into(),
        ))))
    }

    /// 追加一条限流错误（429），用于测试重试逻辑
    pub fn with_rate_limit_error(self) -> Self {
        self.with_error(ReactError::Llm(Box::new(LlmError::ApiError {
            status: 429,
            message: "Too Many Requests".to_string(),
        })))
    }

    /// 已发生的调用总次数
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// 最后一次调用时传入的 messages（若从未调用则返回 `None`）
    pub fn last_messages(&self) -> Option<Vec<Message>> {
        self.calls.lock().unwrap().last().cloned()
    }

    /// 所有历史调用的 messages（按时序排列）
    pub fn all_calls(&self) -> Vec<Vec<Message>> {
        self.calls.lock().unwrap().clone()
    }

    /// 剩余未消费的预设响应数量
    pub fn remaining(&self) -> usize {
        self.responses.lock().unwrap().len()
    }

    /// 清空所有已记录的调用历史（响应队列不受影响）
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    /// 取出下一个响应（文本或工具调用）
    fn pop_response(&self) -> Result<PopResult> {
        match self.responses.lock().unwrap().pop_front() {
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
            // 记录本次调用
            self.calls.lock().unwrap().push(request.messages);

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
            // 记录本次调用
            self.calls.lock().unwrap().push(request.messages);

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
