//! Agent 抽象层
//!
//! 定义 [`Agent`] 核心 trait、事件枚举 [`AgentEvent`] 和回调接口 [`AgentCallback`]。
//! 主要实现为 [`react_agent::ReactAgent`]。

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

/// SubAgent 注册表类型别名
pub(crate) type SubAgentMap = Arc<RwLock<HashMap<String, Arc<AsyncMutex<Box<dyn Agent>>>>>>;

mod config;
mod planning;
pub mod react_agent;

/// Agent 执行过程中产生的事件流元素
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
}

/// Agent 的统一执行接口
#[async_trait]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn system_prompt(&self) -> &str;

    /// 阻塞执行，返回最终答案文本
    async fn execute(&mut self, task: &str) -> Result<String>;

    /// 流式执行，返回 [`AgentEvent`] 事件流
    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>;
}

/// Agent 生命周期回调接口
///
/// 实现该 trait 可观测 Agent 的每个执行阶段。所有方法均有默认空实现，
/// 按需覆盖即可（如埋点、日志、UI 更新等）。
#[async_trait]
pub trait AgentCallback: Send + Sync {
    /// LLM 推理开始前触发，可获取当前完整消息历史
    async fn on_think_start(&self, _agent: &str, _messages: &[Message]) {}
    /// LLM 推理结束后触发，可获取本轮步骤列表
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
