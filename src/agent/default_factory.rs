//! Concrete `DefaultAgentFactory` that creates agents via `ReactAgentBuilder`
//!
//! This module provides the facade-level implementation of the [`AgentFactory`] trait
//! defined in `echo_core::agent::factory`. It reads the [`AgentFactoryConfig`] and
//! creates a configured [`crate::agent::ReactAgent`].
//!
//! # Example
//!
//! ```rust,no_run
//! use echo_agent::agent::default_factory::DefaultAgentFactory;
//! use echo_agent::agent::factory::AgentFactoryConfig;
//! use echo_core::agent::factory::AgentFactory;
//!
//! # fn main() -> echo_agent::error::Result<()> {
//! let config = AgentFactoryConfig::new()
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

use crate::agent::Agent;
use crate::agent::react::builder::ReactAgentBuilder;
use crate::error::Result;

/// Concrete factory that creates agents using [`ReactAgentBuilder`].
pub struct DefaultAgentFactory;

impl AgentFactory for DefaultAgentFactory {
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn Agent>> {
        let model = config.model_name().to_string();
        let name = config.agent_name().to_string();
        let system_prompt = config.system_prompt().to_string();
        let tools = config.into_tools();

        ReactAgentBuilder::new()
            .model(&model)
            .name(&name)
            .system_prompt(&system_prompt)
            .enable_tools()
            .tools(tools)
            .build_boxed()
    }
}

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockLlmClient;
    use std::sync::Arc;

    #[test]
    fn test_factory_creates_config() {
        let config = AgentFactoryConfig::new()
            .model("mock-model")
            .name("test-agent")
            .with_system_prompt("Test agent");

        assert_eq!(config.model_name(), "mock-model");
        assert_eq!(config.agent_name(), "test-agent");
    }

    #[tokio::test]
    async fn test_factory_build_agent_with_mock() {
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
