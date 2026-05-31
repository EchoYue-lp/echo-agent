//! Agent Factory pattern — paradigm-based agent creation
//!
//! Provides a [`AgentParadigm`] enum that classifies agent execution strategies,
//! an [`AgentFactoryConfig`] that captures configuration for creating an agent,
//! and an [`AgentFactory`] trait that produces agents from a config.
//!
//! # Paradigms
//!
//! | Paradigm | Description |
//! |----------|-------------|
//! | [`React`](AgentParadigm::React) | Think-Act-Observe loop (default) |
//! | [`PlanExecute`](AgentParadigm::PlanExecute) | Plan steps, then execute each step |
//! | [`SelfReflection`](AgentParadigm::SelfReflection) | Generate, critique, refine cycle |
//! | [`Structured`](AgentParadigm::Structured) | Schema-constrained output agent |
//!
//! # Example
//!
//! ```rust,ignore
//! use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig, AgentParadigm};
//!
//! let config = AgentFactoryConfig::new(AgentParadigm::React)
//!     .model("qwen3-max")
//!     .name("my-agent")
//!     .with_system_prompt("You are a helpful assistant");
//!
//! let factory = DefaultAgentFactory;
//! let agent = factory.create_agent(config)?;
//! ```

use crate::agent::mode::AgentMode;
use crate::error::Result;
use crate::tools::Tool;
use std::fmt;

// ── Agent Paradigm ──────────────────────────────────────────────────────────

/// Classification of agent execution paradigms.
///
/// Each paradigm determines the reasoning loop strategy the agent uses:
///
/// - **React** — classic Think-Act-Observe loop; the agent reasons step-by-step,
///   invoking tools as needed, and terminates when it produces a final answer.
/// - **PlanExecute** — the agent first generates a multi-step plan, then executes
///   each step sequentially, adapting if steps fail.
/// - **SelfReflection** — the agent generates an initial answer, critiques it,
///   and refines through multiple reflection iterations until a quality threshold
///   is met.
/// - **Structured** — the agent operates within a schema-constrained output format,
///   suitable for tasks requiring typed, deterministic responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AgentParadigm {
    /// Think-Act-Observe loop (default paradigm)
    React,
    /// Plan steps first, then execute each step
    PlanExecute,
    /// Generate, critique, and refine iteratively
    SelfReflection,
    /// Schema-constrained output agent
    Structured,
}

impl AgentParadigm {
    /// Parse a paradigm name into an `AgentParadigm`.
    ///
    /// Supports: "react", "plan-execute"/"plan_execute", "self-reflection"/"self_reflection", "structured".
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "react" => Some(AgentParadigm::React),
            "plan-execute" | "plan_execute" => Some(AgentParadigm::PlanExecute),
            "self-reflection" | "self_reflection" => Some(AgentParadigm::SelfReflection),
            "structured" => Some(AgentParadigm::Structured),
            _ => None,
        }
    }

    /// All currently defined paradigms.
    pub fn all() -> &'static [AgentParadigm] {
        &[
            AgentParadigm::React,
            AgentParadigm::PlanExecute,
            AgentParadigm::SelfReflection,
            AgentParadigm::Structured,
        ]
    }

    /// English display name for the paradigm.
    pub fn name(&self) -> &str {
        match self {
            AgentParadigm::React => "React",
            AgentParadigm::PlanExecute => "Plan-Execute",
            AgentParadigm::SelfReflection => "Self-Reflection",
            AgentParadigm::Structured => "Structured",
        }
    }
}

impl fmt::Display for AgentParadigm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Default for AgentParadigm {
    fn default() -> Self {
        AgentParadigm::React
    }
}

// ── Agent Factory Config ────────────────────────────────────────────────────

/// Configuration for creating an agent via an [`AgentFactory`].
///
/// Captures the paradigm, operating mode, model, name, system prompt, and
/// tools needed to construct an agent. The factory reads this config to
/// decide which builder to use and how to configure it.
///
/// Note: This struct does **not** implement `Clone` because it owns
/// `Box<dyn Tool>` instances which are not clonable. The factory consumes
/// the config entirely via [`AgentFactoryConfig::into_tools`].
pub struct AgentFactoryConfig {
    /// The execution paradigm that determines the agent's reasoning strategy.
    paradigm: AgentParadigm,
    /// Optional operating mode (e.g., Coding, Research) for auto-configuration.
    mode: Option<AgentMode>,
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
    /// Create a new factory config with the given paradigm.
    ///
    /// Defaults: model = "", name = "assistant", system_prompt = "You are a helpful assistant".
    pub fn new(paradigm: AgentParadigm) -> Self {
        Self {
            paradigm,
            mode: None,
            model: String::new(),
            name: "assistant".to_string(),
            system_prompt: "You are a helpful assistant".to_string(),
            tools: Vec::new(),
        }
    }

    /// Create a React paradigm config (convenience shorthand).
    pub fn react() -> Self {
        Self::new(AgentParadigm::React)
    }

    /// Create a Plan-Execute paradigm config.
    pub fn plan_execute() -> Self {
        Self::new(AgentParadigm::PlanExecute)
    }

    /// Create a Self-Reflection paradigm config.
    pub fn self_reflection() -> Self {
        Self::new(AgentParadigm::SelfReflection)
    }

    /// Create a Structured paradigm config.
    pub fn structured() -> Self {
        Self::new(AgentParadigm::Structured)
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

    /// Set the operating mode.
    pub fn with_mode(mut self, mode: AgentMode) -> Self {
        self.mode = Some(mode);
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

    /// The execution paradigm.
    pub fn paradigm(&self) -> AgentParadigm {
        self.paradigm
    }

    /// The operating mode, if set.
    pub fn mode(&self) -> Option<AgentMode> {
        self.mode
    }

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
        Self::new(AgentParadigm::default())
    }
}

// ── Agent Factory Trait ─────────────────────────────────────────────────────

/// Trait for creating agents from an [`AgentFactoryConfig`].
///
/// The factory pattern decouples agent construction from the caller's knowledge
/// of concrete agent types. A factory reads the paradigm from the config and
/// delegates to the appropriate builder.
///
/// # Example
///
/// ```rust,ignore
/// use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig, DefaultAgentFactory, AgentParadigm};
///
/// let factory = DefaultAgentFactory;
/// let config = AgentFactoryConfig::react()
///     .model("qwen3-max")
///     .name("coder")
///     .with_system_prompt("You are a coding assistant");
///
/// let agent = factory.create_agent(config)?;
/// let answer = agent.execute("Write a hello world in Rust").await?;
/// ```
pub trait AgentFactory: Send + Sync {
    /// Create an agent from the given configuration.
    ///
    /// Returns a `Box<dyn Agent>` so the caller can work with any paradigm
    /// uniformly, without knowing the concrete type.
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn crate::agent::Agent>>;
}

// ── Default Agent Factory ───────────────────────────────────────────────────

/// Default implementation of [`AgentFactory`] that creates agents based on
/// the paradigm specified in [`AgentFactoryConfig`].
///
/// Currently all paradigms are realized as `ReactAgent` variants:
/// - **React** — standard `ReactAgentBuilder` with tool support
/// - **PlanExecute** — `ReactAgentBuilder` with planning enabled
/// - **SelfReflection** — `ReactAgentBuilder` with self-reflection enabled
/// - **Structured** — `ReactAgentBuilder` with structured output configured
///
/// As dedicated agent types for each paradigm are introduced, this factory
/// will be updated to dispatch to the appropriate builder.
pub struct DefaultAgentFactory;

impl AgentFactory for DefaultAgentFactory {
    fn create_agent(&self, _config: AgentFactoryConfig) -> Result<Box<dyn crate::agent::Agent>> {
        // The facade crate provides `DefaultAgentFactory` with a full impl
        // that uses `ReactAgentBuilder`. Here in echo-core we provide a
        // minimal stub that returns an error indicating the caller should
        // use the facade-level factory.
        //
        // This design keeps echo-core free of concrete builder dependencies
        // while still defining the trait and types that the facade implements.
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
    fn test_paradigm_from_name() {
        assert_eq!(AgentParadigm::from_name("react"), Some(AgentParadigm::React));
        assert_eq!(AgentParadigm::from_name("plan-execute"), Some(AgentParadigm::PlanExecute));
        assert_eq!(AgentParadigm::from_name("plan_execute"), Some(AgentParadigm::PlanExecute));
        assert_eq!(AgentParadigm::from_name("self-reflection"), Some(AgentParadigm::SelfReflection));
        assert_eq!(AgentParadigm::from_name("self_reflection"), Some(AgentParadigm::SelfReflection));
        assert_eq!(AgentParadigm::from_name("structured"), Some(AgentParadigm::Structured));
        assert_eq!(AgentParadigm::from_name("unknown"), None);
    }

    #[test]
    fn test_paradigm_all() {
        assert_eq!(AgentParadigm::all().len(), 4);
    }

    #[test]
    fn test_paradigm_display() {
        assert_eq!(AgentParadigm::React.to_string(), "React");
        assert_eq!(AgentParadigm::PlanExecute.to_string(), "Plan-Execute");
        assert_eq!(AgentParadigm::SelfReflection.to_string(), "Self-Reflection");
        assert_eq!(AgentParadigm::Structured.to_string(), "Structured");
    }

    #[test]
    fn test_paradigm_default() {
        assert_eq!(AgentParadigm::default(), AgentParadigm::React);
    }

    #[test]
    fn test_factory_config_new() {
        let config = AgentFactoryConfig::new(AgentParadigm::React);
        assert_eq!(config.paradigm(), AgentParadigm::React);
        assert_eq!(config.model_name(), "");
        assert_eq!(config.agent_name(), "assistant");
        assert_eq!(config.system_prompt(), "You are a helpful assistant");
    }

    #[test]
    fn test_factory_config_shorthand() {
        let config = AgentFactoryConfig::react();
        assert_eq!(config.paradigm(), AgentParadigm::React);

        let config = AgentFactoryConfig::plan_execute();
        assert_eq!(config.paradigm(), AgentParadigm::PlanExecute);

        let config = AgentFactoryConfig::self_reflection();
        assert_eq!(config.paradigm(), AgentParadigm::SelfReflection);

        let config = AgentFactoryConfig::structured();
        assert_eq!(config.paradigm(), AgentParadigm::Structured);
    }

    #[test]
    fn test_factory_config_builder() {
        let config = AgentFactoryConfig::react()
            .model("qwen3-max")
            .name("my-agent")
            .with_system_prompt("You are a coder");

        assert_eq!(config.model_name(), "qwen3-max");
        assert_eq!(config.agent_name(), "my-agent");
        assert_eq!(config.system_prompt(), "You are a coder");
    }

    #[test]
    fn test_factory_config_mode() {
        let config = AgentFactoryConfig::react()
            .model("qwen3-max")
            .with_mode(AgentMode::Coding);
        assert_eq!(config.mode(), Some(AgentMode::Coding));
    }

    #[test]
    fn test_factory_config_default() {
        let config = AgentFactoryConfig::default();
        assert_eq!(config.paradigm(), AgentParadigm::React);
    }
}