//! Mock Agent，实现 [`Agent`] trait，用于测试多 Agent 编排时替换真实的 SubAgent。
//!
//! 在测试编排逻辑时，我们通常希望：
//! - 不发起真实 LLM 调用
//! - 控制每个 SubAgent 的返回内容
//! - 验证 SubAgent 被调用了几次，以及每次收到什么任务
//!
//! # 示例
//!
//! ```rust
//! use echo_agent::testing::MockAgent;
//! use echo_agent::agent::Agent;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let mut agent = MockAgent::new("math_agent")
//!     .with_response("结果是 42")
//!     .with_response("结果是 100");
//!
//! let r1 = agent.execute("计算 6 * 7").await.unwrap();
//! let r2 = agent.execute("计算 10 * 10").await.unwrap();
//! assert_eq!(r1, "结果是 42");
//! assert_eq!(r2, "结果是 100");
//! assert_eq!(agent.call_count(), 2);
//! assert_eq!(agent.calls()[0], "计算 6 * 7");
//! # }
//! ```

use crate::agent::{Agent, AgentEvent};
use crate::error::{AgentError, ReactError, Result};
use async_trait::async_trait;
use futures::stream;
use futures::stream::BoxStream;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// ── MockAgent ─────────────────────────────────────────────────────────────────

/// 可脚本化的 Mock Agent。
///
/// 按顺序返回预设的响应；队列耗尽后每次调用都返回 `"mock agent response"`。
/// `execute()` 和 `chat()` 的消息均被记录，可通过 [`calls()`](MockAgent::calls) 检查。
/// `reset()` 清空调用历史，模拟真实 Agent 的对话重置语义。
pub struct MockAgent {
    name: String,
    model_name: String,
    system_prompt: String,
    responses: Arc<Mutex<VecDeque<String>>>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl MockAgent {
    /// 创建具名 Mock Agent
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model_name: "mock-model".to_string(),
            system_prompt: "You are a mock agent".to_string(),
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 设置模型名称（用于需要检查 model_name 的测试）
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model_name = model.into();
        self
    }

    /// 设置系统提示词
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// 追加一条预设响应
    pub fn with_response(self, text: impl Into<String>) -> Self {
        self.responses.lock().unwrap().push_back(text.into());
        self
    }

    /// 批量追加多条预设响应
    pub fn with_responses(self, texts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        {
            let mut q = self.responses.lock().unwrap();
            for t in texts {
                q.push_back(t.into());
            }
        }
        self
    }

    /// 已被调用的总次数
    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// 所有历史调用的任务字符串（按时序排列）
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// 最后一次调用时的任务字符串（若从未调用则返回 `None`）
    pub fn last_task(&self) -> Option<String> {
        self.calls.lock().unwrap().last().cloned()
    }

    /// 清空调用历史（响应队列不受影响）
    ///
    /// 仅用于测试断言重置，不等同于 `Agent::reset()`。
    pub fn reset_calls(&self) {
        self.calls.lock().unwrap().clear();
    }

    fn next_response(&self) -> String {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "mock agent response".to_string())
    }
}

#[async_trait]
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

    async fn execute(&mut self, task: &str) -> Result<String> {
        self.calls.lock().unwrap().push(task.to_string());
        Ok(self.next_response())
    }

    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        let answer = self.execute(task).await?;
        let event_stream = stream::once(async move { Ok(AgentEvent::FinalAnswer(answer)) });
        Ok(Box::pin(event_stream))
    }

    /// `chat()` 同样记录调用，并消费预设响应队列。
    /// 注意：MockAgent 不维护真实的对话上下文，这里仅满足调用合约。
    async fn chat(&mut self, message: &str) -> Result<String> {
        self.calls.lock().unwrap().push(message.to_string());
        Ok(self.next_response())
    }

    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        let answer = self.chat(message).await?;
        let event_stream = stream::once(async move { Ok(AgentEvent::FinalAnswer(answer)) });
        Ok(Box::pin(event_stream))
    }

    /// 清空调用历史，模拟真实 Agent 的重置语义。
    fn reset(&mut self) {
        self.calls.lock().unwrap().clear();
    }
}

/// 产生总是返回错误的 Mock Agent（用于测试编排容错行为）
pub struct FailingMockAgent {
    name: String,
    error_message: String,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FailingMockAgent {
    /// 创建失败型 Mock Agent
    pub fn new(name: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            error_message: error_message.into(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait]
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

    async fn execute(&mut self, task: &str) -> Result<String> {
        self.calls.lock().unwrap().push(task.to_string());
        Err(ReactError::Agent(AgentError::InitializationFailed(
            self.error_message.clone(),
        )))
    }

    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        let err = self.execute(task).await.unwrap_err();
        let event_stream = stream::once(async move { Err(err) });
        Ok(Box::pin(event_stream))
    }

    async fn chat(&mut self, message: &str) -> Result<String> {
        self.execute(message).await
    }

    fn reset(&mut self) {
        self.calls.lock().unwrap().clear();
    }
}
