//! 无状态 Runner API
//!
//! 提供一组静态方法，用于快速执行 Agent 任务，无需管理 Agent 生命周期。
//!
//! # 典型用法
//!
//! ```rust,no_run
//! use echo_agent::agent::runner::Runner;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! // 一行执行
//! let answer = Runner::run("qwen3-max", "你是助手", "你好").await?;
//! println!("{}", answer);
//!
//! // 带工具
//! let answer = Runner::builder("qwen3-max")
//!     .system_prompt("你是计算助手")
//!     .enable_tools()
//!     .run("1+1=?")
//!     .await?;
//! # Ok(())
//! # }
//! ```

use crate::agent::react_agent::builder::ReactAgentBuilder;
use crate::agent::Agent;
use crate::error::Result;
use crate::tools::Tool;
use std::sync::Arc;

/// 无状态 Agent 执行器
///
/// 每次 `run()` / `run_stream()` 都创建一个全新的 Agent 实例，
/// 不保留任何状态。适合无服务器函数、API handler 等场景。
pub struct Runner;

impl Runner {
    /// 最简执行：一行代码完成 Agent 调用
    pub async fn run(model: &str, system_prompt: &str, task: &str) -> Result<String> {
        let mut agent = ReactAgentBuilder::simple(model, system_prompt)?;
        agent.execute(task).await
    }

    /// 创建 RunnerBuilder 进行更多配置
    pub fn builder(model: &str) -> RunnerBuilder {
        RunnerBuilder {
            inner: ReactAgentBuilder::new().model(model),
        }
    }
}

/// Runner 的构建器，支持链式配置后执行
pub struct RunnerBuilder {
    inner: ReactAgentBuilder,
}

impl RunnerBuilder {
    /// 设置系统提示词
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.inner = self.inner.system_prompt(prompt);
        self
    }

    /// 启用内置工具
    pub fn enable_tools(mut self) -> Self {
        self.inner = self.inner.enable_tools();
        self
    }

    /// 注册自定义工具
    pub fn tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.inner = self.inner.tool(tool);
        self
    }

    /// 批量注册工具
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.inner = self.inner.tools(tools);
        self
    }

    /// 设置自定义 LLM 客户端
    pub fn llm_client(mut self, client: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.inner = self.inner.llm_client(client);
        self
    }

    /// 设置最大迭代次数
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.inner = self.inner.max_iterations(max);
        self
    }

    /// 执行任务并返回结果
    pub async fn run(self, task: &str) -> Result<String> {
        let mut agent = self.inner.build()?;
        agent.execute(task).await
    }

    /// 构建 Agent 实例（当需要流式执行时，先 build 再手动调用 execute_stream）
    ///
    /// ```rust,no_run
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// use echo_agent::agent::{Agent, runner::Runner};
    ///
    /// let mut agent = Runner::builder("qwen3-max")
    ///     .system_prompt("你是助手")
    ///     .build()?;
    /// let stream = agent.execute_stream("你好").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build(self) -> Result<crate::agent::react_agent::ReactAgent> {
        self.inner.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_builder_compiles() {
        // 验证 API 编译通过
        let _builder = Runner::builder("test-model")
            .system_prompt("你是助手")
            .enable_tools()
            .max_iterations(5);
    }
}
