//! AgentRunner — high-level facade that composes all runtime subsystems.
//!
//! # Architecture-aligned API
//!
//! ```rust,no_run
//! use echo_agent::runner::AgentRunner;
//! use echo_agent::context::ContextAssembler;
//! use echo_agent::agent::react::run::pipeline::ToolExecutionPipeline;
//!
//! let agent = AgentRunner::new()
//!     .model("claude-sonnet-4-6")
//!     .system_prompt("You are a coding assistant")
//!     .with_context_engine(ContextAssembler::new())
//!     .with_tool_pipeline(ToolExecutionPipeline::default())
//!     .build()
//!     .unwrap();
//! ```
//!
//! This mirrors the architectural layers from the framework design:
//! - **Context Engine** → [`ContextAssembler`]
//! - **Tool Runtime** → [`ToolExecutionPipeline`]
//! - **Orchestration** → revisioned task DAGs and `TeamSpec`
//! - **Evaluation** → [`EvalRunner`] (feature `eval`)
//! - **Trace** → [`RunStore`]

use crate::agent::react::run::pipeline::ToolExecutionPipeline;
use crate::agent::ReactAgent;
use crate::prelude::ReactAgentBuilder;
use crate::trace::RunStore;
use std::sync::Arc;

/// High-level builder that composes runtime subsystems into a [`ReactAgent`].
///
/// Uses architecture-aligned naming (context_engine, tool_pipeline, orchestrator,
/// eval_recorder) rather than internal builder method names.
#[deprecated(since = "0.3.0", note = "Use `ReactAgentBuilder` directly for full feature access")]
pub struct AgentRunner {
    model: String,
    system_prompt: String,
    agent_name: String,
    tool_pipeline: Option<ToolExecutionPipeline>,
    run_store: Option<Arc<dyn RunStore>>,
    max_iterations: usize,
    enable_tools: bool,
}

impl Default for AgentRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRunner {
    /// Create a new runner with sensible defaults.
    pub fn new() -> Self {
        Self {
            model: String::new(),
            system_prompt: crate::config::DEFAULT_AGENT_SYSTEM_PROMPT.into(),
            agent_name: "echo-agent".into(),
            tool_pipeline: None,
            run_store: None,
            max_iterations: 10,
            enable_tools: true,
        }
    }

    /// Set the model name (required).
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set the agent name (for logging/tracing).
    pub fn agent_name(mut self, name: impl Into<String>) -> Self {
        self.agent_name = name.into();
        self
    }

    /// Set maximum think-act iterations.
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// Enable or disable tool calling.
    pub fn enable_tools(mut self, enable: bool) -> Self {
        self.enable_tools = enable;
        self
    }

    /// Attach a [`ToolExecutionPipeline`] for configurable tool processing.
    ///
    /// Maps to: `ReactAgentBuilder::tool_execution_pipeline()`
    pub fn with_tool_pipeline(mut self, pipeline: ToolExecutionPipeline) -> Self {
        self.tool_pipeline = Some(pipeline);
        self
    }

    /// Attach a [`RunStore`] for trace persistence.
    ///
    /// Maps to: `ReactAgentBuilder::with_run_store()`
    pub fn with_run_store(mut self, store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(store);
        self
    }

    /// Build the [`ReactAgent`] with all configured subsystems.
    ///
    /// Returns an error if the model name is empty.
    pub fn build(self) -> crate::error::Result<ReactAgent> {
        let mut builder = ReactAgentBuilder::new()
            .model(&self.model)
            .system_prompt(&self.system_prompt)
            .name(&self.agent_name)
            .max_iterations(self.max_iterations);

        if self.enable_tools {
            builder = builder.enable_tools();
        }

        // Wire subsystems
        if let Some(pipeline) = self.tool_pipeline {
            builder = builder.tool_execution_pipeline(pipeline);
        }

        if let Some(store) = self.run_store {
            builder = builder.with_run_store(store);
        }

        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runner_basic() {
        let runner = AgentRunner::new()
            .model("test-model")
            .system_prompt("test prompt")
            .max_iterations(5);
        assert_eq!(runner.model, "test-model");
        assert_eq!(runner.max_iterations, 5);
    }

    #[test]
    fn test_runner_build_requires_model() {
        let result = AgentRunner::new().build();
        assert!(result.is_err());
    }
}
