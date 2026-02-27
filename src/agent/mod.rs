use crate::error::Result;
use async_trait::async_trait;
pub use config::{AgentConfig, AgentRole};
use futures::stream::BoxStream;
use serde_json::Value;

mod config;
mod planning;
pub mod react_agent;

pub enum AgentEvent {
    Token(String),
    ToolCall { name: String, args: Value },
    ToolResult { name: String, output: String },
    FinalAnswer(String),
}

/// 一个 agent 应该有：系统提示词、可调用的工具
#[async_trait]
pub trait Agent: Send + Sync {
    /// agent 的名称
    fn name(&self) -> &str;

    /// 模型名称
    fn model_name(&self) -> &str;

    /// 系统提示词
    fn system_prompt(&self) -> &str;

    /// 核心执行方法
    async fn execute(&mut self, task: &str) -> Result<String>;

    /// 流式执行方法
    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>;
}
