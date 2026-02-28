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
use crate::llm::LlmClient;
use crate::llm::types::Message;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 预设响应的枚举（文本或错误）
enum MockLlmResponse {
    Content(String),
    Err(ReactError),
}

/// 可脚本化的 Mock LLM 客户端。
///
/// 按顺序返回预设的响应；队列耗尽后返回 `EmptyResponse` 错误。
/// 所有调用都被记录，可通过 [`call_count`](MockLlmClient::call_count) /
/// [`last_messages`](MockLlmClient::last_messages) 等方法检查。
pub struct MockLlmClient {
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
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
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

    /// 追加一条网络错误（常用的便捷方法）
    pub fn with_network_error(self, msg: impl Into<String>) -> Self {
        self.with_error(ReactError::Llm(LlmError::NetworkError(msg.into())))
    }

    /// 追加一条限流错误（429），用于测试重试逻辑
    pub fn with_rate_limit_error(self) -> Self {
        self.with_error(ReactError::Llm(LlmError::ApiError {
            status: 429,
            message: "Too Many Requests".to_string(),
        }))
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
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat_simple(&self, messages: Vec<Message>) -> Result<String> {
        // 记录本次调用
        self.calls.lock().unwrap().push(messages);

        // 返回下一个预设响应
        match self.responses.lock().unwrap().pop_front() {
            Some(MockLlmResponse::Content(text)) => Ok(text),
            Some(MockLlmResponse::Err(e)) => Err(e),
            None => Err(ReactError::Llm(LlmError::EmptyResponse)),
        }
    }
}
