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
//! - **Orchestration** → [`TeamAgent`]
//! - **Evaluation** → [`EvalRunner`]
//! - **Trace** → [`RunStore`]

use crate::agent::react::run::pipeline::ToolExecutionPipeline;
use crate::agent::subagent::team::TeamAgent;
use crate::agent::ReactAgent;
use crate::context::ContextAssembler;
use crate::eval::EvalRunner;
use crate::prelude::ReactAgentBuilder;
use crate::trace::RunStore;
use std::sync::Arc;

/// High-level builder that composes runtime subsystems into a [`ReactAgent`].
///
/// Uses architecture-aligned naming (context_engine, tool_pipeline, orchestrator,
/// eval_recorder) rather than internal builder method names.
pub struct AgentRunner {
    model: String,
    system_prompt: String,
    agent_name: String,
    context_engine: Option<ContextAssembler>,
    tool_pipeline: Option<ToolExecutionPipeline>,
    orchestrator: Option<TeamAgent>,
    eval_recorder: Option<EvalRunner>,
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
            system_prompt: "You are a helpful assistant".into(),
            agent_name: "echo-agent".into(),
            context_engine: None,
            tool_pipeline: None,
            orchestrator: None,
            eval_recorder: None,
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

    /// Attach a [`ContextAssembler`] for centralized message list construction.
    ///
    /// Maps to: `ReactAgentBuilder::with_context_assembler()`
    pub fn with_context_engine(mut self, assembler: ContextAssembler) -> Self {
        self.context_engine = Some(assembler);
        self
    }

    /// Attach a [`ToolExecutionPipeline`] for configurable tool processing.
    ///
    /// Maps to: `ReactAgentBuilder::tool_execution_pipeline()`
    pub fn with_tool_pipeline(mut self, pipeline: ToolExecutionPipeline) -> Self {
        self.tool_pipeline = Some(pipeline);
        self
    }

    /// Attach a [`TeamAgent`] for multi-agent orchestration.
    ///
    /// **Status: stored but not yet executed automatically.**
    /// The team must be invoked manually via `TeamAgent::execute()` after `build()`.
    /// Future integration will auto-register team agents with `AgentDispatchTool`.
    pub fn with_orchestrator(mut self, team: TeamAgent) -> Self {
        self.orchestrator = Some(team);
        self
    }

    /// Attach an [`EvalRunner`] for automatic evaluation.
    ///
    /// **Status: stored but not yet executed automatically.**
    /// Use `runner.run(&case, agent)` manually after `build()`.
    /// Future integration will auto-evaluate runs against configured cases.
    pub fn with_eval_recorder(mut self, runner: EvalRunner) -> Self {
        self.eval_recorder = Some(runner);
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
        if let Some(assembler) = self.context_engine {
            builder = builder.with_context_assembler(assembler);
        }

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
