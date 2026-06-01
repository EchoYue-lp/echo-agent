//! Agent builder

use crate::agent::react::run::pipeline::ToolExecutionPipeline;
use crate::agent::{
    Agent, AgentCallback, AgentConfig, AgentMode, AgentRole, DefaultModeEngine,
    InterventionCallback, ModeEngine,
};
use crate::audit::AuditLogger;
use crate::context::ContextAssembler;
use crate::error::Result;
use crate::guard::{Guard, GuardManager};
#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopProvider, PermissionService};
use crate::llm::{LlmClient, LlmConfig, OpenAiClient, ResponseFormat};
use crate::memory::checkpointer::Checkpointer;
use crate::memory::snapshot::{SnapshotManager, SnapshotPolicy};
use crate::memory::store::Store;
use crate::prelude::ReactAgent;
use crate::sandbox::SandboxManager;
use crate::tools::permission::PermissionPolicy;
use crate::tools::{Tool, ToolExecutionConfig};
use crate::trace::RunStore;
use echo_core::agent::PromptTemplateManager;
use echo_core::circuit_breaker::CircuitBreakerConfig;
use std::sync::Arc;

/// Agent builder
///
/// Provides a fluent API to configure and build an Agent.
/// Specify the concrete type via a generic parameter, returning a `Box<dyn Agent>` abstraction.
pub struct ReactAgentBuilder {
    name: String,
    model: String,
    system_prompt: String,
    role: AgentRole,
    llm_client: Option<Arc<dyn LlmClient>>,
    llm_config: Option<LlmConfig>,
    tools: Vec<Box<dyn Tool>>,
    enable_builtin_tools: bool,
    enable_memory: bool,
    enable_task: bool,
    enable_human_in_loop: bool,
    enable_subagent: bool,
    enable_cot: bool,
    tool_error_feedback: bool,
    tool_execution: ToolExecutionConfig,
    max_iterations: usize,
    token_limit: usize,
    callbacks: Vec<Arc<dyn AgentCallback>>,
    store: Option<Arc<dyn Store>>,
    checkpointer: Option<Arc<dyn Checkpointer>>,
    session_id: Option<String>,
    conversation_id: Option<String>,
    #[cfg(feature = "human-loop")]
    approval_provider: Option<Arc<dyn HumanLoopProvider>>,
    #[cfg(feature = "human-loop")]
    permission_service: Option<Arc<PermissionService>>,
    guards: Vec<Arc<dyn Guard>>,
    permission_policy: Option<Arc<dyn PermissionPolicy>>,
    audit_logger: Option<Arc<dyn AuditLogger>>,
    snapshot_policy: Option<SnapshotPolicy>,
    max_snapshots: usize,
    response_format: Option<ResponseFormat>,
    max_tool_output_tokens: Option<usize>,
    circuit_breaker_config: Option<CircuitBreakerConfig>,
    sandbox_manager: Option<Arc<SandboxManager>>,
    run_store: Option<Arc<dyn RunStore>>,
    tool_execution_pipeline: Option<Arc<ToolExecutionPipeline>>,
    context_assembler: Option<ContextAssembler>,
    /// Agent operating mode, used to auto-configure system prompt and tools
    mode: Option<AgentMode>,
    /// Mode engine for resolving mode configuration (defaults to DefaultModeEngine)
    mode_engine: Option<Arc<dyn ModeEngine>>,
    /// Prompt template engine for variable substitution in system prompts
    prompt_template_engine: Option<Arc<PromptTemplateManager>>,
    /// Intervention callbacks that can influence agent behavior
    /// (block tool calls, inject context, redirect execution, cancel).
    intervention_callbacks: Vec<Arc<dyn InterventionCallback>>,
}

impl Default for ReactAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactAgentBuilder {
    /// Create a new builder (default ReAct mode)
    pub fn new() -> Self {
        Self {
            name: "assistant".to_string(),
            model: String::new(),
            system_prompt: "You are a helpful assistant".to_string(),
            role: AgentRole::default(),
            llm_client: None,
            llm_config: None,
            tools: Vec::new(),
            enable_builtin_tools: false,
            enable_memory: false,
            enable_task: false,
            enable_human_in_loop: false,
            enable_subagent: false,
            enable_cot: true,
            tool_error_feedback: true,
            tool_execution: ToolExecutionConfig::default(),
            max_iterations: 10,
            token_limit: usize::MAX,
            callbacks: Vec::new(),
            store: None,
            checkpointer: None,
            session_id: None,
            conversation_id: None,
            #[cfg(feature = "human-loop")]
            approval_provider: None,
            #[cfg(feature = "human-loop")]
            permission_service: None,
            guards: Vec::new(),
            permission_policy: None,
            audit_logger: None,
            snapshot_policy: None,
            max_snapshots: 10,
            response_format: None,
            max_tool_output_tokens: None,
            circuit_breaker_config: None,
            sandbox_manager: None,
            run_store: None,
            tool_execution_pipeline: None,
            context_assembler: None,
            mode: None,
            mode_engine: None,
            prompt_template_engine: None,
            intervention_callbacks: Vec::new(),
        }
    }

    // ── Preset Configurations ───────────────────────────────────────────────────

    /// Create a simple conversation Agent (no tools, no memory)
    ///
    /// Suitable for simple Q&A scenarios.
    pub fn simple(model: &str, system_prompt: &str) -> Result<ReactAgent> {
        Self::new()
            .model(model)
            .system_prompt(system_prompt)
            .build()
    }

    /// Create a standard Agent (tools + chain-of-thought enabled)
    ///
    /// Suitable for most Agent scenarios.
    pub fn standard(model: &str, name: &str, system_prompt: &str) -> Result<ReactAgent> {
        Self::new()
            .model(model)
            .name(name)
            .system_prompt(system_prompt)
            .enable_tools()
            .build()
    }

    /// Create a full-featured Agent (tools, memory, planning)
    ///
    /// Suitable for complex autonomous Agent scenarios.
    pub fn full_featured(model: &str, name: &str, system_prompt: &str) -> Result<ReactAgent> {
        Self::new()
            .model(model)
            .name(name)
            .system_prompt(system_prompt)
            .enable_tools()
            .enable_memory()
            .enable_planning()
            .build()
    }
    // ── Basic Configuration ─────────────────────────────────────────────────────

    /// Set Agent name
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set model name
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set Agent role
    pub fn role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    // ── LLM Configuration ───────────────────────────────────────────────────────

    /// Set custom LLM client
    ///
    /// Use this method to:
    /// - Inject a Mock client for testing
    /// - Use a custom LLM implementation
    /// - Share an LLM client instance
    pub fn llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.model = client.model_name().to_string();
        self.llm_client = Some(client);
        self
    }

    /// Set LLM configuration (dependency injection)
    ///
    /// For dynamically configuring API endpoint, keys, etc., without using environment variables.
    pub fn llm_config(mut self, config: LlmConfig) -> Self {
        self.model = config.model.clone();
        self.llm_config = Some(config);
        self
    }

    /// Use OpenAI client (convenience method)
    ///
    /// Reads configuration from environment variables.
    pub fn with_openai(mut self, model: &str) -> Result<Self> {
        let client = Arc::new(OpenAiClient::from_env(model)?);
        self.llm_client = Some(client);
        self.model = model.to_string();
        Ok(self)
    }

    // ── Tool Configuration ──────────────────────────────────────────────────────

    /// Enable built-in tools (via the `enable_tool` flag)
    pub fn enable_tools(mut self) -> Self {
        self.enable_builtin_tools = true;
        self
    }

    /// Disable built-in tools
    pub fn disable_tools(mut self) -> Self {
        self.enable_builtin_tools = false;
        self
    }

    /// Register a single tool
    pub fn tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Batch register tools
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    // ── Feature Flags ───────────────────────────────────────────────────────────

    /// Enable long-term memory
    pub fn enable_memory(mut self) -> Self {
        self.enable_memory = true;
        self
    }

    /// Enable task planning
    pub fn enable_planning(mut self) -> Self {
        self.enable_task = true;
        self
    }

    /// Enable human-in-the-loop
    pub fn enable_human_in_loop(mut self) -> Self {
        self.enable_human_in_loop = true;
        self
    }

    /// Enable sub-Agent dispatch
    pub fn enable_subagent(mut self) -> Self {
        self.enable_subagent = true;
        self
    }

    /// Enable chain-of-thought guidance
    pub fn enable_cot(mut self) -> Self {
        self.enable_cot = true;
        self
    }

    /// Disable chain-of-thought guidance
    pub fn disable_cot(mut self) -> Self {
        self.enable_cot = false;
        self
    }

    // ── Structured Output ────────────────────────────────────────────────────────

    /// Declare the Agent's structured output type
    ///
    /// Automatically generates `response_format` from `T`'s [`JsonSchema`](schemars::JsonSchema),
    /// works with [`ReactAgent::execute_typed`] to directly obtain deserialized results.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use schemars::JsonSchema;
    /// use serde::Deserialize;
    ///
    /// #[derive(Debug, Deserialize, JsonSchema)]
    /// struct Person { name: String, age: u32 }
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// let agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .output_type::<Person>()
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn output_type<T: schemars::JsonSchema>(mut self) -> Self {
        let schema_gen = schemars::r#gen::SchemaGenerator::default();
        let root_schema = schema_gen.into_root_schema_for::<T>();
        let schema_value = serde_json::to_value(root_schema).unwrap_or_default();
        let type_name = std::any::type_name::<T>()
            .rsplit("::")
            .next()
            .unwrap_or("output")
            .to_lowercase();
        self.response_format = Some(ResponseFormat::json_schema(type_name, schema_value));
        self
    }

    /// Manually set response format
    pub fn response_format(mut self, fmt: ResponseFormat) -> Self {
        self.response_format = Some(fmt);
        self
    }

    // ── Execution Parameters ────────────────────────────────────────────────────

    /// Set maximum iteration count
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// Set tool error feedback toggle
    pub fn tool_error_feedback(mut self, enabled: bool) -> Self {
        self.tool_error_feedback = enabled;
        self
    }

    /// Set tool execution configuration
    pub fn tool_execution(mut self, config: ToolExecutionConfig) -> Self {
        self.tool_execution = config;
        self
    }

    /// Set token limit
    pub fn token_limit(mut self, limit: usize) -> Self {
        self.token_limit = limit;
        self
    }

    /// Set maximum token count for a single tool output
    ///
    /// Tool output exceeding this limit is automatically truncated, with `[Output truncated, N tokens total]` appended.
    /// Prevents a single tool call from overflowing the context window.
    pub fn max_tool_output_tokens(mut self, max: usize) -> Self {
        self.max_tool_output_tokens = Some(max);
        self
    }

    // ── Callbacks and Extensions ─────────────────────────────────────────────────

    /// Add callback
    pub fn callback(mut self, callback: Arc<dyn AgentCallback>) -> Self {
        self.callbacks.push(callback);
        self
    }

    /// Add an intervention callback that can influence agent behavior.
    ///
    /// Intervention callbacks are checked before tool execution, LLM reasoning,
    /// and final answers. They can block actions, inject context, redirect
    /// execution, modify tool arguments, or cancel the entire run.
    ///
    /// Unlike `AgentCallback` (which is observational), `InterventionCallback`
    /// can *influence* the agent's decisions.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use echo_agent::agent::InterventionCallback;
    /// use echo_agent::agent::InterventionResult;
    ///
    /// struct BlockShell;
    ///
    /// impl InterventionCallback for BlockShell {
    ///     fn on_tool_call<'a>(
    ///         &'a self,
    ///         _agent: &'a str,
    ///         tool: &'a str,
    ///         _args: &'a serde_json::Value,
    ///     ) -> futures::future::BoxFuture<'a, InterventionResult> {
    ///         Box::pin(async move {
    ///             if tool == "shell" {
    ///                 InterventionResult::block("shell is blocked")
    ///             } else {
    ///                 InterventionResult::allow()
    ///             }
    ///         })
    ///     }
    /// }
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// let agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .system_prompt("You are a helpful assistant")
    ///     .enable_tools()
    ///     .intervention_callback(Arc::new(BlockShell))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn intervention_callback(mut self, callback: Arc<dyn InterventionCallback>) -> Self {
        self.intervention_callbacks.push(callback);
        self
    }

    /// Set long-term memory Store
    pub fn store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Inject an external Store and automatically register the four built-in tools: remember / recall / search_memory / forget
    ///
    /// This is a shortcut from "having a memory store" to "the Agent can use memory autonomously",
    /// equivalent to `.store(store).enable_memory()`, but supports any `Store` implementation
    /// (such as `EmbeddingStore`) without depending on the default `FileStore`.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// let store = Arc::new(InMemoryStore::new());
    /// let agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .with_memory_tools(store)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_memory_tools(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self.enable_memory = true;
        self
    }

    /// Set Checkpointer (also sets session_id)
    pub fn checkpointer(
        mut self,
        checkpointer: Arc<dyn Checkpointer>,
        session_id: impl Into<String>,
    ) -> Self {
        self.checkpointer = Some(checkpointer);
        self.session_id = Some(session_id.into());
        self
    }

    /// Set Checkpointer (using the already-set session_id)
    /// Must call session_id() first to set the thread identifier
    pub fn checkpointer_only(mut self, checkpointer: Arc<dyn Checkpointer>) -> Self {
        self.checkpointer = Some(checkpointer);
        self
    }

    /// Set session_id (thread identifier)
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set conversation_id (history projection identifier)
    ///
    /// Unlike `session_id`, `conversation_id` is only used for `ConversationStore`
    /// transcript/history projections; if conversation history persistence is enabled, this should be set explicitly.
    pub fn conversation_id(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    #[cfg(feature = "human-loop")]
    /// Set approval Provider
    pub fn approval_provider(mut self, provider: Arc<dyn HumanLoopProvider>) -> Self {
        self.approval_provider = Some(provider);
        self
    }

    #[cfg(feature = "human-loop")]
    /// Set unified permission service
    ///
    /// Once set, this service takes priority for permission checks,
    /// falling back to the legacy PermissionPolicy logic.
    pub fn permission_service(mut self, service: Arc<PermissionService>) -> Self {
        self.permission_service = Some(service);
        self
    }

    // ── Guardrails & Permissions & Audit ─────────────────────────────────────────

    /// Add guard
    pub fn guard(mut self, guard: Arc<dyn Guard>) -> Self {
        self.guards.push(guard);
        self
    }

    /// Batch add guards
    pub fn guards(mut self, guards: Vec<Arc<dyn Guard>>) -> Self {
        self.guards.extend(guards);
        self
    }

    /// Add content safety guard (PII detection/redaction/rejection)
    #[cfg(feature = "content-guard")]
    pub fn with_content_guard(mut self, mode: echo_core::guard::content::ContentGuardMode) -> Self {
        let guard = echo_core::guard::content::ContentGuard::new(mode);
        self.guards.push(Arc::new(guard));
        self
    }

    /// Set tool permission policy
    pub fn permission_policy(mut self, policy: Arc<dyn PermissionPolicy>) -> Self {
        self.permission_policy = Some(policy);
        self
    }

    /// Set audit logger
    pub fn audit_logger(mut self, logger: Arc<dyn AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    // ── Snapshot Configuration ──────────────────────────────────────────────────

    /// Set snapshot policy, enabling state snapshot functionality
    ///
    /// When enabled, each iteration of the ReAct loop can automatically capture conversation history snapshots.
    /// On exception, `agent.rollback(n)` can roll back to a previous known-good state.
    pub fn snapshot_policy(mut self, policy: SnapshotPolicy) -> Self {
        self.snapshot_policy = Some(policy);
        self
    }

    /// Set maximum snapshot retention count (default 10)
    pub fn max_snapshots(mut self, max: usize) -> Self {
        self.max_snapshots = max;
        self
    }

    /// Enable circuit breaker
    ///
    /// Opens the circuit after `failure_threshold` consecutive LLM failures, waits for `timeout` before entering half-open probing.
    pub fn with_circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker_config = Some(config);
        self
    }

    /// Attach a [`RunStore`] for persisting execution traces.
    ///
    /// When set, every agent invocation records a full [`Run`](crate::trace::Run)
    /// with events, token usage, and timings.
    pub fn with_run_store(mut self, store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(store);
        self
    }

    /// Set a custom [`ToolExecutionPipeline`] for fine-grained control over
    /// the tool execution lifecycle (hooks, permissions, guards, etc.).
    ///
    /// When set, `execute_tool_feedback_raw` delegates to this pipeline.
    /// When `None` (default), the built-in inline implementation is used.
    pub fn tool_execution_pipeline(mut self, pipeline: ToolExecutionPipeline) -> Self {
        self.tool_execution_pipeline = Some(Arc::new(pipeline));
        self
    }

    /// Attach a [`ContextAssembler`] for centralized message list construction.
    ///
    /// When set, `run_react_loop` delegates context assembly to the assembler
    /// instead of pushing messages individually. When `None` (default), the
    /// built-in scattered push logic is used.
    pub fn with_context_assembler(mut self, assembler: ContextAssembler) -> Self {
        self.context_assembler = Some(assembler);
        self
    }

    /// Set sandbox manager, providing secure isolation for skill script execution
    pub fn sandbox_manager(mut self, manager: Arc<SandboxManager>) -> Self {
        self.sandbox_manager = Some(manager);
        self
    }

    /// Set the agent operating mode.
    ///
    /// When set, this auto-configures:
    /// - System prompt from the mode engine (can be overridden by `system_prompt()`)
    /// - Recommended tools from the mode engine (added to the tool list)
    /// - The mode field on `AgentConfig`
    pub fn mode(mut self, mode: AgentMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Set a custom mode engine for resolving mode configuration.
    ///
    /// Defaults to `DefaultModeEngine` if not set. Use `LocalizedModeEngine`
    /// for Chinese or other locale-specific prompts.
    pub fn mode_engine(mut self, engine: Arc<dyn ModeEngine>) -> Self {
        self.mode_engine = Some(engine);
        self
    }

    /// Set a prompt template engine for variable substitution in system prompts.
    ///
    /// When set, the agent can use the template engine to render prompt
    /// templates with dynamic variable substitution (e.g., `{{name}}`,
    /// `{{mode}}`). This enables centralized template management and
    /// dynamic prompt assembly.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use echo_core::agent::PromptTemplateManager;
    /// use std::sync::Arc;
    ///
    /// let engine = Arc::new(PromptTemplateManager::with_default_mode_templates());
    /// let agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .with_prompt_template_engine(engine)
    ///     .build()?;
    /// # Ok(())
    /// ```
    pub fn with_prompt_template_engine(mut self, engine: Arc<PromptTemplateManager>) -> Self {
        self.prompt_template_engine = Some(engine);
        self
    }

    // ── Build ──────────────────────────────────────────────────────────────────

    /// Build ReAct Agent (internal method)
    pub fn build(self) -> Result<ReactAgent> {
        // ── Construction-time validation ────────────────────────────────────────────
        if self.model.trim().is_empty() {
            return Err(crate::error::ConfigError::MissingConfig(
                "model".to_string(),
                "Model name cannot be empty".to_string(),
            )
            .into());
        }
        // max_iterations == 0 means unlimited (no iteration limit)
        if self.enable_subagent && !self.enable_builtin_tools {
            return Err(crate::error::ConfigError::ConfigFileError(
                "Enabling sub-agent dispatch (enable_subagent) requires enabling tool calls (enable_builtin_tools)"
                    .to_string(),
            )
            .into());
        }

        let mut config = AgentConfig::new(&self.model, &self.name, &self.system_prompt)
            .role(self.role)
            .enable_tool(self.enable_builtin_tools)
            .enable_memory(self.enable_memory)
            .enable_task(self.enable_task)
            .enable_human_in_loop(self.enable_human_in_loop)
            .enable_subagent(self.enable_subagent)
            .enable_cot(self.enable_cot)
            .tool_error_feedback(self.tool_error_feedback)
            .tool_execution(self.tool_execution)
            .max_iterations(self.max_iterations)
            .token_limit(self.token_limit);

        if let Some(fmt) = self.response_format {
            config = config.response_format(fmt);
        }
        if let Some(max) = self.max_tool_output_tokens {
            config = config.max_tool_output_tokens(max);
        }

        for callback in self.callbacks {
            config = config.with_callback(callback);
        }

        if let Some(session_id) = &self.session_id {
            config = config.session_id(session_id);
        }
        if let Some(conversation_id) = &self.conversation_id {
            config = config.conversation_id(conversation_id);
        }

        // ── Mode auto-configuration ─────────────────────────────────────────────
        // When a mode is set, auto-apply the mode's system prompt and recommended tools.
        let tools_to_register_after_build: Vec<Box<dyn Tool>>;
        if let Some(mode) = self.mode {
            config = config.mode(mode);
            let engine: Arc<dyn ModeEngine> = self
                .mode_engine
                .unwrap_or_else(|| Arc::new(DefaultModeEngine));
            let mode_config = engine.mode_config(&mode);
            // Only override system prompt if the user hasn't explicitly set one
            // (i.e., the system_prompt is still the default "You are a helpful assistant")
            if self.system_prompt == "You are a helpful assistant" {
                config = config.system_prompt(&mode_config.system_prompt_template);
            }
            // Note: recommended_tools filtering is handled by AgentConfig.allowed_tools.
            // The actual tool registration happens after agent construction below.
            // For now, we set allowed_tools to restrict the agent to mode-recommended tools
            // only when the mode has a non-empty recommended list.
            if !mode_config.recommended_tools.is_empty() {
                config = config.allowed_tools(mode_config.recommended_tools);
            }
            // No additional tools to register; mode filtering uses allowed_tools
            tools_to_register_after_build = self.tools;
        } else {
            tools_to_register_after_build = self.tools;
        }

        // When the user passes a custom Store via with_memory_tools(store),
        // skip the automatic FileStore initialization inside ReactAgent::new(),
        // instead manually inject the user-provided Store during the build() phase.
        let has_external_store = self.store.is_some();
        if has_external_store {
            config = config.enable_memory(false);
        }

        let mut agent = crate::agent::react::ReactAgent::new(config);

        if let Some(llm_client) = self.llm_client {
            agent.set_llm_client(llm_client);
        }

        // Inject LLM config
        if let Some(llm_config) = self.llm_config {
            agent.set_llm_config(llm_config);
        }

        // Register custom tools
        for tool in tools_to_register_after_build {
            agent.add_tool(tool);
        }

        // Set Store (also registers remember/recall/search_memory/forget tools)
        if let Some(store) = self.store {
            agent.set_memory_store(store);
        }

        // Set Checkpointer
        if let (Some(checkpointer), Some(session_id)) = (self.checkpointer, self.session_id) {
            agent.set_checkpointer(checkpointer, session_id);
        }

        #[cfg(feature = "human-loop")]
        if let Some(provider) = self.approval_provider {
            agent.set_approval_provider(provider);
        }

        #[cfg(feature = "human-loop")]
        if let Some(service) = self.permission_service {
            agent.set_permission_service(service);
        }

        // Set guardrails
        if !self.guards.is_empty() {
            agent.set_guard_manager(GuardManager::from_guards(self.guards));
        }

        // Set permission policy
        if let Some(policy) = self.permission_policy {
            agent.set_permission_policy(policy);
        }

        // Set audit logger
        if let Some(logger) = self.audit_logger {
            agent.set_audit_logger(logger);
        }

        // Set snapshot manager
        if let Some(policy) = self.snapshot_policy {
            agent.set_snapshot_manager(SnapshotManager::new(policy, self.max_snapshots));
        }

        // Set circuit breaker
        if let Some(cb_config) = self.circuit_breaker_config {
            agent.set_circuit_breaker(cb_config);
        }

        // Set sandbox manager
        if let Some(manager) = self.sandbox_manager {
            agent.set_sandbox_manager(manager);
        }

        // Set run store
        if let Some(store) = self.run_store {
            agent.run_store = Some(store);
        }

        // Set tool execution pipeline
        if let Some(pipeline) = self.tool_execution_pipeline {
            agent.tool_execution_pipeline = Some(pipeline);
        }

        // Set context assembler
        if let Some(assembler) = self.context_assembler {
            agent.context_assembler = Some(assembler);
        }

        // Set prompt template engine
        if let Some(engine) = self.prompt_template_engine {
            agent.set_prompt_template_engine(engine);
        }

        // Set intervention callbacks
        if !self.intervention_callbacks.is_empty() {
            agent.tools.intervention_callbacks = self.intervention_callbacks;
        }

        Ok(agent)
    }

    /// Build the agent and return it as a trait object.
    ///
    /// This is useful when you need polymorphic agent handling (e.g., passing
    /// different agent types through a unified interface, or storing agents
    /// in a collection).
    ///
    /// ```rust,no_run
    /// use echo_agent::agent::Agent;
    /// use echo_agent::prelude::ReactAgentBuilder;
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// let agent: Box<dyn Agent> = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .system_prompt("You are an assistant")
    ///     .build_boxed()?;
    /// let answer = agent.execute("Hello").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_boxed(self) -> Result<Box<dyn Agent>> {
        self.build().map(|a| Box::new(a) as Box<dyn Agent>)
    }
}

// ── AgentBuilder Trait Implementation ─────────────────────────────────────────

impl echo_core::agent::builder::AgentBuilder for ReactAgentBuilder {
    type Agent = ReactAgent;

    fn model(self, model: impl Into<String>) -> Self {
        self.model(model)
    }

    fn system_prompt(self, prompt: impl Into<String>) -> Self {
        self.system_prompt(prompt)
    }

    fn name(self, name: impl Into<String>) -> Self {
        self.name(name)
    }

    fn max_iterations(self, max: usize) -> Self {
        self.max_iterations(max)
    }

    fn token_limit(self, limit: usize) -> Self {
        self.token_limit(limit)
    }

    fn tool(self, tool: Box<dyn crate::tools::Tool>) -> Self {
        self.tool(tool)
    }

    fn tools(self, tools: Vec<Box<dyn crate::tools::Tool>>) -> Self {
        self.tools(tools)
    }

    fn build(self) -> Result<ReactAgent> {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockLlmClient;
    use std::sync::Arc;

    #[test]
    fn test_builder_basic() {
        let builder = ReactAgentBuilder::new()
            .name("test-agent")
            .model("qwen3-max")
            .system_prompt("Test");

        assert_eq!(builder.name, "test-agent");
        assert_eq!(builder.model, "qwen3-max");
        assert_eq!(builder.system_prompt, "Test");
    }

    #[test]
    fn test_builder_chaining() {
        let builder = ReactAgentBuilder::new()
            .model("qwen3-max")
            .enable_tools()
            .enable_memory()
            .max_iterations(20);

        assert!(builder.enable_builtin_tools);
        assert!(builder.enable_memory);
        assert_eq!(builder.max_iterations, 20);
    }

    #[test]
    fn test_react_agent_builder() {
        let builder = ReactAgentBuilder::new()
            .model("qwen3-max")
            .system_prompt("Test")
            .enable_tools();

        assert!(builder.enable_builtin_tools);
    }

    #[test]
    fn test_builder_llm_config_syncs_runtime_model_name() {
        let agent = ReactAgentBuilder::new()
            .llm_config(LlmConfig::openai("sk-demo", "gpt-4o"))
            .system_prompt("Test")
            .build()
            .unwrap();

        assert_eq!(agent.config().get_model_name(), "gpt-4o");
        assert_eq!(
            agent.llm_config().map(|cfg| cfg.model.as_str()),
            Some("gpt-4o")
        );
    }

    #[test]
    fn test_builder_llm_client_syncs_runtime_model_name() {
        let agent = ReactAgentBuilder::new()
            .llm_client(Arc::new(
                MockLlmClient::new().with_model_name("mock-topology"),
            ))
            .system_prompt("Test")
            .build()
            .unwrap();

        assert_eq!(agent.config().get_model_name(), "mock-topology");
    }

    #[test]
    fn test_builder_tool_execution_config_is_applied() {
        let agent = ReactAgentBuilder::new()
            .model("qwen3-max")
            .tool_execution(ToolExecutionConfig {
                timeout_ms: 120_000,
                ..ToolExecutionConfig::default()
            })
            .build()
            .unwrap();

        assert_eq!(agent.config().get_tool_execution().timeout_ms, 120_000);
    }
}
