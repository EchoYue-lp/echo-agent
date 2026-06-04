//! Agent Factory pattern — configuration-based agent creation
//!
//! Provides an [`AgentFactoryConfig`] that captures configuration for creating an agent,
//! and an [`AgentFactory`] trait that produces agents from a config.
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig};
//!
//! let config = AgentFactoryConfig::new()
//!     .model("qwen3-max")
//!     .name("my-agent")
//!     .with_system_prompt("You are a helpful assistant");
//!
//! let factory = DefaultAgentFactory;
//! let agent = factory.create_agent(config)?;
//! ```

use crate::error::Result;
use crate::tools::Tool;

// ── Agent Factory Config ────────────────────────────────────────────────────

/// Configuration for creating an agent via an [`AgentFactory`].
///
/// Captures model, name, system prompt, and tools needed to construct an agent.
/// The factory reads this config and delegates to the appropriate builder.
///
/// Note: This struct does **not** implement `Clone` because it owns
/// `Box<dyn Tool>` instances which are not clonable. The factory consumes
/// the config entirely via [`AgentFactoryConfig::into_tools`].
pub struct AgentFactoryConfig {
    /// LLM model identifier (e.g., "qwen3-max", "gpt-4o").
    model: String,
    /// Human-readable agent name used in logs and orchestration.
    name: String,
    /// System prompt that seeds the agent's behavior.
    system_prompt: String,
    /// Custom tools to register on the agent.
    tools: Vec<Box<dyn Tool>>,
}

impl AgentFactoryConfig {
    /// Create a new factory config.
    ///
    /// Defaults: model = "", name = "assistant", system_prompt = "You are a helpful assistant".
    pub fn new() -> Self {
        Self {
            model: String::new(),
            name: "assistant".to_string(),
            system_prompt: "You are a helpful assistant".to_string(),
            tools: Vec::new(),
        }
    }

    // ── Builder-style setters ──────────────────────────────────────────────────

    /// Set the LLM model name.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the agent name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Register a single tool.
    pub fn tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Batch-register tools.
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    // ── Accessors ──────────────────────────────────────────────────────────────

    /// The LLM model identifier.
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// The agent name.
    pub fn agent_name(&self) -> &str {
        &self.name
    }

    /// The system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Number of custom tools registered.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Consume the config and return the owned tools vector.
    ///
    /// This is the primary way a factory extracts tools from the config,
    /// since `Box<dyn Tool>` cannot be cloned.
    pub fn into_tools(self) -> Vec<Box<dyn Tool>> {
        self.tools
    }
}

impl Default for AgentFactoryConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ── Agent Factory Trait ─────────────────────────────────────────────────────

/// Trait for creating agents from an [`AgentFactoryConfig`].
pub trait AgentFactory: Send + Sync {
    /// Create an agent from the given configuration.
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn crate::agent::Agent>>;
}

// ── Default Agent Factory ───────────────────────────────────────────────────

/// Default implementation of [`AgentFactory`].
///
/// The facade crate (echo-agent) provides a concrete implementation
/// that uses `ReactAgentBuilder`.
pub struct DefaultAgentFactory;

impl AgentFactory for DefaultAgentFactory {
    fn create_agent(&self, _config: AgentFactoryConfig) -> Result<Box<dyn crate::agent::Agent>> {
        Err(crate::error::ReactError::Other(
            "DefaultAgentFactory::create_agent must be called from the facade crate \
             (echo_agent), which provides the concrete ReactAgentBuilder-based implementation. \
             Use echo_agent::agent::default_factory::DefaultAgentFactory instead.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_factory_config_new() {
        let config = AgentFactoryConfig::new();
        assert_eq!(config.model_name(), "");
        assert_eq!(config.agent_name(), "assistant");
        assert_eq!(config.system_prompt(), "You are a helpful assistant");
    }

    #[test]
    fn test_factory_config_builder() {
        let config = AgentFactoryConfig::new()
            .model("qwen3-max")
            .name("my-agent")
            .with_system_prompt("You are a coder");

        assert_eq!(config.model_name(), "qwen3-max");
        assert_eq!(config.agent_name(), "my-agent");
        assert_eq!(config.system_prompt(), "You are a coder");
    }

    #[test]
    fn test_factory_config_default() {
        let config = AgentFactoryConfig::default();
        assert_eq!(config.agent_name(), "assistant");
    }
}
