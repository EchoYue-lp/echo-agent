//! Stateless Runner API
//!
//! Provides a set of static methods for quickly executing Agent tasks without managing the Agent lifecycle.
//!
//! # Typical usage
//!
//! ```rust,no_run
//! use echo_agent::agent::runner::Runner;
//!
//! # #[tokio::main]
//! # async fn main() -> echo_agent::error::Result<()> {
//! // One-liner execution
//! let answer = Runner::run("qwen3-max", "You are an assistant", "Hello").await?;
//! println!("{}", answer);
//!
//! // With tools
//! let answer = Runner::builder("qwen3-max")
//!     .system_prompt("You are a math assistant")
//!     .enable_tools()
//!     .run("1+1=?")
//!     .await?;
//! # Ok(())
//! # }
//! ```

use crate::agent::Agent;
use crate::agent::react::builder::ReactAgentBuilder;
use crate::error::Result;
use crate::tools::Tool;
use std::sync::Arc;

/// Stateless Agent executor
///
/// Each `run()` creates a brand-new Agent instance,
/// retaining no state. Suitable for serverless functions, API handlers, etc.
pub struct Runner;

impl Runner {
    /// Minimal execution: one line to complete an Agent call
    pub async fn run(model: &str, system_prompt: &str, task: &str) -> Result<String> {
        let agent = ReactAgentBuilder::simple(model, system_prompt)?;
        agent.execute(task).await
    }

    /// Create a RunnerBuilder for further configuration
    pub fn builder(model: &str) -> RunnerBuilder {
        RunnerBuilder {
            inner: ReactAgentBuilder::new().model(model),
        }
    }
}

/// Builder for Runner, supports chained configuration before execution
pub struct RunnerBuilder {
    inner: ReactAgentBuilder,
}

impl RunnerBuilder {
    /// Set system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.inner = self.inner.system_prompt(prompt);
        self
    }

    /// Enable built-in tools
    pub fn enable_tools(mut self) -> Self {
        self.inner = self.inner.enable_tools();
        self
    }

    /// Register a custom tool
    pub fn tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.inner = self.inner.tool(tool);
        self
    }

    /// Batch register tools
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.inner = self.inner.tools(tools);
        self
    }

    /// Set custom LLM client
    pub fn llm_client(mut self, client: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.inner = self.inner.llm_client(client);
        self
    }

    /// Set maximum iteration count
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.inner = self.inner.max_iterations(max);
        self
    }

    /// Execute task and return result
    pub async fn run(self, task: &str) -> Result<String> {
        let agent = self.inner.build()?;
        agent.execute(task).await
    }

    /// Build an Agent instance (when streaming execution is needed, build first then manually call execute_stream)
    ///
    /// ```rust,no_run
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// use echo_agent::agent::{Agent, runner::Runner};
    ///
    /// let mut agent = Runner::builder("qwen3-max")
    ///     .system_prompt("You are an assistant")
    ///     .build()?;
    /// let stream = agent.execute_stream("Hello").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build(self) -> Result<crate::agent::react::ReactAgent> {
        self.inner.build()
    }

    /// Build the agent as a trait object for polymorphic usage.
    ///
    /// ```rust,no_run
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// use echo_agent::agent::{Agent, runner::Runner};
    ///
    /// let agent: Box<dyn Agent> = Runner::builder("qwen3-max")
    ///     .system_prompt("You are an assistant")
    ///     .build_boxed()?;
    /// let answer = agent.execute("Hello").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_boxed(self) -> Result<Box<dyn Agent>> {
        self.inner.build_boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_builder_compiles() {
        // Verify API compiles
        let _builder = Runner::builder("test-model")
            .system_prompt("You are an assistant")
            .enable_tools()
            .max_iterations(5);
    }
}
