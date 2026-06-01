//! Agent trait definitions — public API contract for agent types.
//!
//! These traits define what an agent *is* and what it can *do*,
//! independent of the concrete implementation.

use echo_core::agent::Agent;

/// Configuration for building an agent.
///
/// This is a simplified builder config that captures the essential
/// parameters needed to construct any agent type.
#[derive(Debug, Clone)]
pub struct AgentBuildConfig {
    /// Agent name (used for identification and logging).
    pub name: String,
    /// Model name to use for LLM calls.
    pub model: String,
    /// System prompt.
    pub system_prompt: String,
    /// Maximum number of iterations (ReAct loops, plan steps, etc.).
    pub max_iterations: u32,
    /// Whether to enable tool use.
    pub tools_enabled: bool,
    /// Whether to enable memory.
    pub memory_enabled: bool,
}

impl Default for AgentBuildConfig {
    fn default() -> Self {
        Self {
            name: "echo-assistant".to_string(),
            model: "qwen-plus".to_string(),
            system_prompt: "You are a helpful AI assistant.".to_string(),
            max_iterations: 10,
            tools_enabled: true,
            memory_enabled: true,
        }
    }
}

/// Builder trait for constructing agents.
///
/// Each agent type provides its own builder that implements this trait.
pub trait AgentBuilder: Sized {
    /// The agent type this builder produces.
    type Agent: Agent;

    /// Create a new builder with default settings.
    fn new() -> Self;

    /// Set the agent name.
    fn name(mut self, name: impl Into<String>) -> Self
    where
        Self: HasConfig,
    {
        self.config_mut().name = name.into();
        self
    }

    /// Set the model name.
    fn model(mut self, model: impl Into<String>) -> Self
    where
        Self: HasConfig,
    {
        self.config_mut().model = model.into();
        self
    }

    /// Set the system prompt.
    fn system_prompt(mut self, prompt: impl Into<String>) -> Self
    where
        Self: HasConfig,
    {
        self.config_mut().system_prompt = prompt.into();
        self
    }

    /// Set the maximum iterations.
    fn max_iterations(mut self, max: u32) -> Self
    where
        Self: HasConfig,
    {
        self.config_mut().max_iterations = max;
        self
    }

    /// Build the agent.
    fn build(self) -> Result<Self::Agent, echo_core::error::ReactError>;
}

/// Helper trait for builders that have an AgentBuildConfig.
pub trait HasConfig {
    fn config_mut(&mut self) -> &mut AgentBuildConfig;
}

/// Agent execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Run the agent synchronously, blocking until completion.
    Sync,
    /// Run the agent with streaming output.
    Streaming,
    /// Run the agent in the background.
    Background,
}

/// Result of an agent execution.
#[derive(Debug, Clone)]
pub struct AgentRunResult {
    /// The agent's final response text.
    pub output: String,
    /// Number of iterations/steps taken.
    pub iterations: u32,
    /// Total tokens used.
    pub tokens_used: u64,
    /// Tool calls made during execution.
    pub tool_calls: u32,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}
