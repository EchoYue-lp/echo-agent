//! Agent 抽象层
//!
//! 定义 [`Agent`] 核心 trait、事件枚举 [`AgentEvent`] 和回调接口 [`AgentCallback`]。
//! 主要实现为 [`react_agent::ReactAgent`]。
//!
//! # 快速开始
//!
//! ```rust,no_run
//! use echo_agent::prelude::*;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! // 使用 Builder 创建 Agent
//! let agent = ReactAgentBuilder::new()
//!     .model("qwen3-max")
//!     .system_prompt("你是一个有帮助的助手")
//!     .enable_tools()
//!     .build()?;
//!
//! println!("Agent name: {}", agent.name());
//! println!("Model: {}", agent.model_name());
//! # Ok(())
//! # }
//! ```

use crate::agent::react_agent::StepType;
use crate::error::{ReactError, Result};
use crate::llm::types::Message;
use async_trait::async_trait;
pub use config::{AgentConfig, AgentRole};
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex as AsyncMutex;
pub use tokio_util::sync::CancellationToken;

/// SubAgent 注册表类型别名
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<AsyncMutex<Box<dyn Agent>>>>>>;

mod config;
mod planning;
pub mod react_agent;

pub use react_agent::builder::ReactAgentBuilder;

/// AgentBuilder 是 ReactAgentBuilder 的别名，用于宏和极简 API
pub type AgentBuilder = ReactAgentBuilder;

/// Agent 执行过程中产生的事件流元素
///
/// 在流式执行（`execute_stream` / `chat_stream`）中，Agent 会逐步产出事件。
/// 消费者可以根据事件类型更新 UI 或记录日志。
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
/// use futures::StreamExt;
///
/// # #[tokio::main]
/// # async fn main() -> echo_agent::error::Result<()> {
/// let mut agent = ReactAgentBuilder::simple("qwen3-max", "助手")?;
///
/// let mut stream = agent.chat_stream("你好").await?;
/// while let Some(event) = stream.next().await {
///     match event? {
///         AgentEvent::Token(t) => print!("{}", t),
///         AgentEvent::FinalAnswer(a) => println!("\n答案: {}", a),
///         _ => {}
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub enum AgentEvent {
    /// 流式 Token 片段（来自 LLM 增量输出）
    Token(String),
    /// LLM 决定调用工具
    ToolCall { name: String, args: Value },
    /// 工具执行完毕，返回观测结果
    ToolResult { name: String, output: String },
    /// 最终答案已生成
    FinalAnswer(String),
    /// 执行被取消
    Cancelled,
}

/// Agent 的统一执行接口
///
/// 所有 Agent 都必须实现此 trait。提供了阻塞执行、流式执行、多轮对话等核心方法。
///
/// # 核心方法
///
/// - [`execute`](Agent::execute) / [`execute_stream`](Agent::execute_stream): 单轮执行，每次重置上下文
/// - [`chat`](Agent::chat) / [`chat_stream`](Agent::chat_stream): 多轮对话，上下文跨轮保留
/// - [`reset`](Agent::reset): 清空对话历史
///
/// # 示例：简单对话
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() -> echo_agent::error::Result<()> {
/// let mut agent = ReactAgentBuilder::simple("qwen3-max", "助手")?;
///
/// // 单轮执行
/// let answer = agent.execute("1+1等于几？").await?;
/// println!("Answer: {}", answer);
/// # Ok(())
/// # }
/// ```
///
/// # 示例：多轮对话
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
///
/// # #[tokio::main]
/// # async fn main() -> echo_agent::error::Result<()> {
/// let mut agent = ReactAgentBuilder::simple("qwen3-max", "助手")?;
///
/// // 多轮对话，上下文保留
/// agent.chat("我叫小明").await?;
/// let answer = agent.chat("我叫什么名字？").await?;
/// println!("Answer: {}", answer); // 应该回答"小明"
///
/// // 重置对话
/// agent.reset();
/// let answer = agent.chat("我叫什么名字？").await?;
/// println!("Answer: {}", answer); // 不记得了
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait Agent: Send + Sync {
    /// 返回 Agent 名称
    fn name(&self) -> &str;

    /// 返回使用的模型名称
    fn model_name(&self) -> &str;

    /// 返回系统提示词
    fn system_prompt(&self) -> &str;

    /// 返回已注册的工具名称列表
    fn tool_names(&self) -> Vec<String> {
        vec![]
    }

    /// 获取工具定义列表（包含名称、描述、参数 Schema）
    fn tool_definitions(&self) -> Vec<crate::llm::types::ToolDefinition> {
        vec![]
    }

    /// 返回已安装的 Skill 名称列表
    fn skill_names(&self) -> Vec<String> {
        vec![]
    }

    /// 返回已连接的 MCP 服务端名称列表
    fn mcp_server_names(&self) -> Vec<String> {
        vec![]
    }

    /// 关闭 Agent，释放资源
    async fn close(&mut self) {}

    /// 单轮执行（阻塞）。每次调用重置上下文。
    ///
    /// 适用于独立任务，不保留历史。连续对话请用 [`chat`](Agent::chat)。
    async fn execute(&mut self, task: &str) -> Result<String>;

    /// 单轮执行（流式）。每次调用重置上下文。
    ///
    /// 适用于独立任务，不保留历史。连续对话请用 [`chat_stream`](Agent::chat_stream)。
    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>;

    /// 单轮执行（流式，支持取消）。
    ///
    /// 当 `cancel` 被触发时，流将提前终止并返回 `AgentEvent::Cancelled`。
    async fn execute_stream_with_cancel(
        &mut self,
        task: &str,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        let _ = cancel;
        self.execute_stream(task).await
    }

    /// 多轮对话（阻塞）。上下文跨轮保留。
    ///
    /// 用 [`reset`](Agent::reset) 开启新会话。
    async fn chat(&mut self, message: &str) -> Result<String> {
        self.execute(message).await
    }

    /// 多轮对话（流式）。上下文跨轮保留。
    ///
    /// 用 [`reset`](Agent::reset) 开启新会话。
    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        self.execute_stream(message).await
    }

    /// 多轮对话（流式，支持取消）。
    async fn chat_stream_with_cancel(
        &mut self,
        message: &str,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        let _ = cancel;
        self.chat_stream(message).await
    }

    /// 清除对话历史，开启新会话。
    fn reset(&mut self) {}
}

/// Agent 生命周期回调接口
///
/// 实现此 trait 可观测 Agent 执行的每个阶段，用于日志、监控、UI 更新等。
///
/// # 示例：记录工具调用
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
/// use async_trait::async_trait;
/// use std::sync::atomic::{AtomicUsize, Ordering};
///
/// struct ToolCounter {
///     count: AtomicUsize,
/// }
///
/// #[async_trait]
/// impl AgentCallback for ToolCounter {
///     async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &serde_json::Value) {
///         println!("调用工具: {}", tool);
///     }
///
///     async fn on_tool_end(&self, _agent: &str, tool: &str, result: &str) {
///         println!("工具 {} 返回: {}", tool, result);
///     }
/// }
///
/// # fn main() {
/// let callback = std::sync::Arc::new(ToolCounter { count: AtomicUsize::new(0) });
/// // agent.add_callback(callback);
/// # }
/// ```
#[async_trait]
pub trait AgentCallback: Send + Sync {
    /// LLM 推理开始前触发
    async fn on_think_start(&self, _agent: &str, _messages: &[Message]) {}
    /// LLM 推理结束后触发
    async fn on_think_end(&self, _agent: &str, _steps: &[StepType]) {}
    /// 工具执行开始前触发
    async fn on_tool_start(&self, _agent: &str, _tool: &str, _args: &Value) {}
    /// 工具执行成功后触发
    async fn on_tool_end(&self, _agent: &str, _tool: &str, _result: &str) {}
    /// 工具执行失败后触发
    async fn on_tool_error(&self, _agent: &str, _tool: &str, _err: &ReactError) {}
    /// 最终答案生成后触发
    async fn on_final_answer(&self, _agent: &str, _answer: &str) {}
    /// 每轮迭代开始前触发，`iteration` 从 0 计数
    async fn on_iteration(&self, _agent: &str, _iteration: usize) {}
}
