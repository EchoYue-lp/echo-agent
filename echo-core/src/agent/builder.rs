//! Unified `AgentBuilder` trait — common interface for constructing any agent type.
//!
//! All concrete builders (`ReactAgentBuilder`, `SubagentBuilder`, etc.) implement
//! this trait so code can be written generically over agent type.

use crate::agent::Agent;
use crate::error::Result;
use crate::tools::Tool;

/// Minimal interface shared by ALL agent builders.
///
/// # Type Parameters
///
/// * `A` — The concrete agent type this builder constructs.
///
/// # Example
///
/// ```rust,ignore
/// fn create<B: AgentBuilder>(builder: B, task: &str) -> Result<B::Agent> {
///     builder.model("qwen3-max").system_prompt("...").build()
/// }
/// ```
pub trait AgentBuilder: Sized {
    /// The concrete agent type this builder constructs.
    type Agent: Agent;

    /// Set the model name (e.g., "qwen3-max", "gpt-4o").
    fn model(self, model: impl Into<String>) -> Self;

    /// Set the system prompt that seeds the agent's behavior.
    fn system_prompt(self, prompt: impl Into<String>) -> Self;

    /// Set a human-readable name for the agent.
    fn name(self, name: impl Into<String>) -> Self;

    /// Set the maximum number of reasoning iterations.
    fn max_iterations(self, max: usize) -> Self;

    /// Set the maximum token limit for the context window.
    fn token_limit(self, limit: usize) -> Self;

    /// Register a single tool.
    fn tool(self, tool: Box<dyn Tool>) -> Self;

    /// Batch-register tools.
    fn tools(self, tools: Vec<Box<dyn Tool>>) -> Self {
        tools.into_iter().fold(self, |b, t| b.tool(t))
    }

    /// Finalize and construct the agent.
    fn build(self) -> Result<Self::Agent>;

    /// Build the agent and return it as a trait object.
    fn build_boxed(self) -> Result<Box<dyn Agent>>
    where
        Self::Agent: 'static,
    {
        self.build().map(|a| Box::new(a) as Box<dyn Agent>)
    }
}
