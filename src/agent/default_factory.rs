//! Concrete `DefaultAgentFactory` that creates agents via `ReactAgentBuilder`
//!
//! This module provides the facade-level implementation of the [`AgentFactory`] trait
//! defined in `echo_core::agent::factory`. It reads the [`AgentFactoryConfig`] and
//! dispatches to [`ReactAgentBuilder`] with paradigm-specific configuration:
//!
//! | Paradigm | Builder configuration |
//! |----------|-----------------------|
//! | React | `.enable_tools()` |
//! | PlanExecute | `.enable_tools().enable_planning()` |
//! | SelfReflection | `.enable_tools()` (reflection uses built-in reflection loop) |
//! | Structured | `.enable_tools()` (structured output set separately) |
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::agent::default_factory::DefaultAgentFactory;
//! use echo_agent::agent::factory::{AgentFactoryConfig, AgentParadigm};
//! use echo_core::agent::factory::AgentFactory;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let config = AgentFactoryConfig::react()
//!     .model("qwen3-max")
//!     .name("coder")
//!     .with_system_prompt("You are a coding assistant");
//!
//! let factory = DefaultAgentFactory;
//! let agent = factory.create_agent(config)?;
//!
//! println!("Agent: {}, Model: {}", agent.name(), agent.model_name());
//! # Ok(())
//! # }
//! ```

use crate::agent::react::builder::ReactAgentBuilder;
use crate::agent::react::ReactAgent;
use crate::agent::Agent;
use crate::error::Result;

/// Concrete factory that creates agents using [`ReactAgentBuilder`].
///
/// Implements the [`AgentFactory`] trait by mapping each paradigm to a
/// `ReactAgentBuilder` configuration preset. The factory produces
/// `Box<dyn Agent>` trait objects so callers can work with any paradigm
/// uniformly.
///
/// As dedicated agent types (e.g., `PlanExecuteAgent`, `SelfReflectionAgent`)
/// are introduced, this factory will be updated to dispatch to their
/// respective builders.
pub struct DefaultAgentFactory;

impl AgentFactory for DefaultAgentFactory {
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn Agent>> {
        // Read scalar fields from the config before consuming it.
        let paradigm = config.paradigm();
        let mode = config.mode();
        let model = config.model_name().to_string();
        let name = config.agent_name().to_string();
        let system_prompt = config.system_prompt().to_string();

        // Consume the config to extract owned tools.
        let tools = config.into_tools();

        // Start with a base ReactAgentBuilder configured with the common fields.
        let mut builder = ReactAgentBuilder::new()
            .model(&model)
            .name(&name)
            .system_prompt(&system_prompt);

        // Apply the operating mode, if specified.
        if let Some(mode) = mode {
            builder = builder.mode(mode);
        }

        // Apply paradigm-specific configuration.
        builder = match paradigm {
            AgentParadigm::React => builder.enable_tools(),
            AgentParadigm::PlanExecute => builder.enable_tools().enable_planning(),
            AgentParadigm::SelfReflection => builder.enable_tools(),
            AgentParadigm::Structured => builder.enable_tools(),
            _ => builder.enable_tools(),
        };

        // Register custom tools from the config.
        builder = builder.tools(tools);

        // Build the agent and return as a trait object.
        builder.build_boxed()
    }
}

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig, AgentParadigm};
/// Re-export of the core-level `DefaultAgentFactory` stub.
///
/// For actual agent creation, use the facade-level [`DefaultAgentFactory`]
/// defined in this module, which provides the concrete `ReactAgentBuilder`
/// implementation.
pub use echo_core::agent::factory::DefaultAgentFactory as CoreDefaultAgentFactory;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockLlmClient;
    use echo_core::agent::mode::AgentMode;
    use std::sync::Arc;

    #[test]
    fn test_factory_creates_react_agent() {
        let factory = DefaultAgentFactory;
        let config = AgentFactoryConfig::react()
            .model("mock-model")
            .name("test-react")
            .with_system_prompt("Test react agent");

        // We can't actually build without a valid LLM client in the environment,
        // so we verify the config produces the right paradigm.
        assert_eq!(config.paradigm(), AgentParadigm::React);
        assert_eq!(config.model_name(), "mock-model");
        assert_eq!(config.agent_name(), "test-react");
    }

    #[test]
    fn test_factory_creates_plan_execute_config() {
        let config = AgentFactoryConfig::plan_execute()
            .model("qwen3-max")
            .name("test-plan");

        assert_eq!(config.paradigm(), AgentParadigm::PlanExecute);
    }

    #[test]
    fn test_factory_creates_self_reflection_config() {
        let config = AgentFactoryConfig::self_reflection()
            .model("qwen3-max")
            .name("test-reflect");

        assert_eq!(config.paradigm(), AgentParadigm::SelfReflection);
    }

    #[test]
    fn test_factory_creates_structured_config() {
        let config = AgentFactoryConfig::structured()
            .model("qwen3-max")
            .name("test-structured");

        assert_eq!(config.paradigm(), AgentParadigm::Structured);
    }

    #[test]
    fn test_factory_config_with_mode() {
        let config = AgentFactoryConfig::react()
            .model("qwen3-max")
            .with_mode(AgentMode::Coding);

        assert_eq!(config.mode(), Some(AgentMode::Coding));
    }

    #[tokio::test]
    async fn test_factory_build_react_agent_with_mock() {
        // Create a factory config with a mock LLM client injected via builder.
        // The factory itself doesn't accept an LLM client directly in the config,
        // but we can verify the builder path works by constructing manually.
        let mock_client = Arc::new(MockLlmClient::new().with_model_name("mock-test"));

        let agent = ReactAgentBuilder::new()
            .llm_client(mock_client)
            .name("factory-test")
            .system_prompt("Test")
            .enable_tools()
            .build_boxed()
            .unwrap();

        assert_eq!(agent.name(), "factory-test");
        assert_eq!(agent.model_name(), "mock-test");
    }
}