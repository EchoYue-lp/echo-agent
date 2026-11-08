//! Agent builder

use crate::agent::config::DEFAULT_TOKEN_LIMIT;
use crate::agent::react::run::pipeline::ToolExecutionPipeline;
use crate::agent::{Agent, AgentCallback, AgentConfig, InterventionCallback};
use crate::audit::{AuditLogger, DiagnosticDeliveryObserver};
use crate::error::Result;
use crate::guard::{Guard, GuardManager};
#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopProvider, PermissionService};
use crate::llm::{LlmClient, LlmConfig, ResponseFormat, SourcedLlmConfig};
use crate::memory::snapshot::{SnapshotManager, SnapshotPolicy};
use crate::memory::store::Store;
use crate::prelude::ReactAgent;
use crate::sandbox::SandboxManager;
use crate::tools::{Tool, ToolExecutionConfig};
use crate::trace::RunStore;
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
    llm_client: Option<Arc<dyn LlmClient>>,
    llm_config: Option<SourcedLlmConfig>,
    // Explicit construction input, consumed before the Agent is exposed. The
    // installed Agent remains the only runtime client/config authority.
    configured_llm_client: Option<(Arc<dyn LlmClient>, LlmConfig)>,
    tools: Vec<Box<dyn Tool>>,
    task_revision_service: Option<Arc<crate::tasks::TaskRevisionService>>,
    enable_builtin_tools: bool,
    #[cfg(feature = "files")]
    file_system: Option<Arc<dyn echo_core::tools::filesystem::FileSystem>>,
    /// 可选远端内容搜索接缝，只交给标准工具包的 `grep`；默认 `None`。
    #[cfg(feature = "files")]
    content_search_provider: Option<Arc<dyn echo_core::tools::search::ContentSearchProvider>>,
    auto_project_rules: bool,
    readonly_tools: bool,
    enable_memory: bool,
    command_cells: Option<Arc<dyn echo_core::tools::cell::CommandCellRegistry>>,
    enable_human_in_loop: bool,
    enable_subagent: bool,
    register_agent_dispatch_tool: bool,
    #[cfg(feature = "subagent")]
    register_subagent_message_tools: bool,
    /// Optional application-owned isolation provider for Fork dispatches.
    #[cfg(feature = "subagent")]
    subagent_isolation_provider:
        Option<std::sync::Arc<dyn crate::agent::subagent::IsolationProvider>>,
    #[cfg(feature = "subagent")]
    subagent_prompt_compiler:
        Option<std::sync::Arc<dyn crate::agent::subagent::SubagentPromptCompiler>>,
    /// Optional registry shared by a parent Agent and delegation-capable
    /// Subagents. Each Agent keeps its own executor/tool surface while resolving
    /// dispatch targets through this single registry.
    #[cfg(feature = "subagent")]
    subagent_registry: Option<std::sync::Arc<crate::agent::subagent::SubagentRegistry>>,
    tool_error_feedback: bool,
    tool_execution: ToolExecutionConfig,
    max_iterations: usize,
    run_budget: echo_core::agent::RunBudgetPolicy,
    persistence_settlement_timeout: std::time::Duration,
    model_profile: Option<echo_core::llm::capabilities::ModelProfileResolution>,
    model_streaming: bool,
    token_limit: usize,
    token_limit_explicit: bool,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    callbacks: Vec<Arc<dyn AgentCallback>>,
    store: Option<Arc<dyn Store>>,
    session_id: Option<String>,
    conversation_id: Option<String>,
    /// Session-bound working directory (worktree path). Propagated to
    /// `AgentConfig.working_dir` on build, then into every tool's ToolContext.
    working_dir: Option<std::path::PathBuf>,
    project_root: Option<std::path::PathBuf>,
    #[cfg(feature = "human-loop")]
    approval_provider: Option<Arc<dyn HumanLoopProvider>>,
    #[cfg(feature = "human-loop")]
    permission_service: Option<Arc<PermissionService>>,
    guards: Vec<Arc<dyn Guard>>,
    audit_logger: Option<Arc<dyn AuditLogger>>,
    diagnostic_delivery_observer: Option<Arc<dyn DiagnosticDeliveryObserver>>,
    snapshot_policy: Option<SnapshotPolicy>,
    max_snapshots: usize,
    response_format: Option<ResponseFormat>,
    max_tool_output_tokens: Option<usize>,
    tool_output_artifacts: Option<echo_core::tools::artifact::ToolOutputArtifactConfig>,
    circuit_breaker_config: Option<CircuitBreakerConfig>,
    sandbox_manager: Option<Arc<SandboxManager>>,
    run_store: Option<Arc<dyn RunStore>>,
    tool_execution_pipeline: Option<Arc<ToolExecutionPipeline>>,
    /// Intervention callbacks that can influence agent behavior
    /// (block tool calls, inject context, redirect execution, cancel).
    intervention_callbacks: Vec<Arc<dyn InterventionCallback>>,
    /// React loop checkpoint interval (0 = only at end, N = every N iterations).
    react_checkpoint_interval: usize,
    /// Optional intent router for pre-ReAct classification and routing.
    intent_router: Option<crate::intent::IntentRouter>,
    /// Optional runtime state store for checkpointing.
    state_store: Option<Arc<dyn crate::state::RuntimeStateStore>>,
    conversation_store: Option<Arc<dyn crate::memory::ConversationStore>>,
    /// Optional visibility horizon config for proactive tool trace compaction.
    visibility_horizon: Option<echo_state::compression::horizon::VisibilityHorizonConfig>,
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
            system_prompt: crate::config::DEFAULT_AGENT_SYSTEM_PROMPT.to_string(),
            llm_client: None,
            llm_config: None,
            configured_llm_client: None,
            tools: Vec::new(),
            task_revision_service: None,
            enable_builtin_tools: false,
            #[cfg(feature = "files")]
            file_system: None,
            #[cfg(feature = "files")]
            content_search_provider: None,
            auto_project_rules: true,
            readonly_tools: false,
            enable_memory: false,
            command_cells: None,
            enable_human_in_loop: false,
            enable_subagent: false,
            register_agent_dispatch_tool: false,
            #[cfg(feature = "subagent")]
            register_subagent_message_tools: false,
            #[cfg(feature = "subagent")]
            subagent_isolation_provider: None,
            #[cfg(feature = "subagent")]
            subagent_prompt_compiler: None,
            #[cfg(feature = "subagent")]
            subagent_registry: None,
            tool_error_feedback: true,
            tool_execution: ToolExecutionConfig::default(),
            max_iterations: 10,
            run_budget: echo_core::agent::RunBudgetPolicy::default(),
            persistence_settlement_timeout: std::time::Duration::from_secs(10),
            model_profile: None,
            model_streaming: true,
            token_limit: DEFAULT_TOKEN_LIMIT,
            token_limit_explicit: false,
            max_tokens: None,
            temperature: None,
            callbacks: Vec::new(),
            store: None,
            session_id: None,
            conversation_id: None,
            working_dir: None,
            project_root: None,
            #[cfg(feature = "human-loop")]
            approval_provider: None,
            #[cfg(feature = "human-loop")]
            permission_service: None,
            guards: Vec::new(),
            audit_logger: None,
            diagnostic_delivery_observer: None,
            snapshot_policy: None,
            max_snapshots: 10,
            response_format: None,
            max_tool_output_tokens: None,
            tool_output_artifacts: None,
            circuit_breaker_config: None,
            sandbox_manager: None,
            run_store: None,
            tool_execution_pipeline: None,
            intervention_callbacks: Vec::new(),
            react_checkpoint_interval: 0,
            intent_router: None,
            state_store: None,
            conversation_store: None,
            visibility_horizon: None,
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
            .build_strict()
    }

    /// 创建启用工具的标准Agent；原生reasoning取决于模型能力与调用配置。
    ///
    /// Suitable for most Agent scenarios.
    pub fn standard(model: &str, name: &str, system_prompt: &str) -> Result<ReactAgent> {
        Self::new()
            .model(model)
            .name(name)
            .system_prompt(system_prompt)
            .enable_tools()
            .build_strict()
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
            .build_strict()
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

    // ── LLM Configuration ───────────────────────────────────────────────────────

    /// Set custom LLM client
    ///
    /// Use this method to:
    /// - Inject a Mock client for testing
    /// - Use a custom LLM implementation
    /// - Share an LLM client instance
    pub fn llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.configured_llm_client = None;
        self.model = client.model_name().to_string();
        self.llm_client = Some(client);
        self
    }

    /// 显式选择模型HTTP传输；默认true，false仍复用同一ReAct和Managed结算路径。
    pub fn model_streaming(mut self, enabled: bool) -> Self {
        self.model_streaming = enabled;
        self
    }

    /// Set LLM configuration (dependency injection)
    ///
    /// For dynamically configuring API endpoint, keys, etc., without using environment variables.
    pub fn llm_config(mut self, config: LlmConfig) -> Self {
        self.configured_llm_client = None;
        self.model = config.model.clone();
        self.llm_config = Some(SourcedLlmConfig::new(config));
        self
    }

    /// Set LLM configuration together with sourced model facts.
    pub fn sourced_llm_config(mut self, config: SourcedLlmConfig) -> Self {
        self.configured_llm_client = None;
        self.model = config.config.model.clone();
        self.llm_config = Some(config);
        self
    }

    /// Supply an already built client and its original rebuild configuration.
    ///
    /// Like [`ReactAgent::new_with_configured_llm_client`], this retains the
    /// exact Arc without rebuilding it and resolves its profile once before
    /// Context initialization. The caller supplies the original validated
    /// configuration; [`Self::build`] rejects empty or mismatched model names
    /// without including configuration or credentials in the error.
    ///
    /// This replaces previous client/config inputs. A later [`Self::llm_client`],
    /// [`Self::llm_config`] or [`Self::sourced_llm_config`] replaces this pair.
    /// Ordinary client/config combinations keep their existing config-wins
    /// behavior. The pair's model remains authoritative over [`Self::model`].
    pub fn configured_llm_client(
        mut self,
        client: Arc<dyn LlmClient>,
        original_config: LlmConfig,
    ) -> Self {
        self.model = original_config.model.clone();
        self.llm_client = None;
        self.llm_config = None;
        self.configured_llm_client = Some((client, original_config));
        self
    }

    // ── Tool Configuration ──────────────────────────────────────────────────────

    /// Enable built-in tools (via the `enable_tool` flag)
    pub fn enable_tools(mut self) -> Self {
        self.enable_builtin_tools = true;
        self
    }

    /// Bind the original standard file tools to one immutable target.
    #[cfg(feature = "files")]
    pub fn file_system(
        mut self,
        file_system: Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = Some(file_system);
        self
    }

    /// 注入可选的远端内容搜索提供者，标准工具包会把它交给 `grep`。
    ///
    /// 这是纯可选项：`None`（默认）时 grep 保持本机遍历，未注入者与接入前
    /// 的构建结果、工具 schema 和执行行为完全一致。
    #[cfg(feature = "files")]
    pub fn content_search_provider(
        mut self,
        provider: Option<Arc<dyn echo_core::tools::search::ContentSearchProvider>>,
    ) -> Self {
        self.content_search_provider = provider;
        self
    }

    /// Select the existing automatic project-rule loader. Applications with
    /// their own provider-aware InstructionProvider disable this local loader
    /// and inject the original resolved rules in their pre-model phase.
    pub fn auto_project_rules(mut self, enabled: bool) -> Self {
        self.auto_project_rules = enabled;
        self
    }

    /// Disable built-in tools
    pub fn disable_tools(mut self) -> Self {
        self.enable_builtin_tools = false;
        self
    }

    /// Restrict the Agent tool surface to read-only capabilities.
    /// `enable_tools()` controls whether the standard tool pack is installed.
    pub fn readonly_tools(mut self) -> Self {
        self.readonly_tools = true;
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

    /// Replace the default in-memory task tools with a revision service backed
    /// by an application persistence and policy adapter.
    pub fn task_revision_service(
        mut self,
        service: Arc<crate::tasks::TaskRevisionService>,
    ) -> Self {
        self.task_revision_service = Some(service);
        self
    }

    // ── Feature Flags ───────────────────────────────────────────────────────────

    /// Enable long-term memory
    pub fn enable_memory(mut self) -> Self {
        self.enable_memory = true;
        self
    }

    /// Share a background-command cell registry (enables `shell
    /// background=true` plus the wait/stop_cell/list_cells tools). Inject ONE
    /// registry into the main agent and its subagents so they all observe the
    /// same cells.
    pub fn command_cells(
        mut self,
        cells: Arc<dyn echo_core::tools::cell::CommandCellRegistry>,
    ) -> Self {
        self.command_cells = Some(cells);
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

    /// Supply the application-owned isolation provider for Fork dispatches.
    #[cfg(feature = "subagent")]
    pub fn subagent_isolation_provider(
        mut self,
        provider: std::sync::Arc<dyn crate::agent::subagent::IsolationProvider>,
    ) -> Self {
        self.subagent_isolation_provider = Some(provider);
        self
    }

    /// Supply the compiler used for every subagent invocation created by this agent.
    #[cfg(feature = "subagent")]
    pub fn subagent_prompt_compiler(
        mut self,
        compiler: std::sync::Arc<dyn crate::agent::subagent::SubagentPromptCompiler>,
    ) -> Self {
        self.subagent_prompt_compiler = Some(compiler);
        self
    }

    /// Share one dispatch catalog across a parent Agent and its Subagents.
    ///
    /// The registry remains a framework runtime primitive. Product layers own
    /// which definitions are registered and which roles expose `agent_tool`.
    #[cfg(feature = "subagent")]
    pub fn subagent_registry(
        mut self,
        registry: std::sync::Arc<crate::agent::subagent::SubagentRegistry>,
    ) -> Self {
        self.subagent_registry = Some(registry);
        self
    }

    /// Register the `agent_tool` LLM-callable dispatch tool.
    ///
    /// Independent of `enable_subagent`. When called, the `AgentDispatchTool`
    /// is registered so the LLM can invoke `agent_tool`. When not called
    /// (default), the LLM cannot call `agent_tool`, but framework-level
    /// dispatch via `delegate_to_agent*` still works.
    pub fn register_agent_dispatch_tool(mut self) -> Self {
        self.register_agent_dispatch_tool = true;
        self
    }

    /// Register the in-tree Subagent communication tools
    /// (`subagent_message` + `subagent_list`) on the built agent.
    #[cfg(feature = "subagent")]
    pub fn register_subagent_message_tools(mut self) -> Self {
        self.register_subagent_message_tools = true;
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

    /// Set a non-zero maximum iteration count.
    ///
    /// [`Self::build`] rejects zero because an unbounded autonomous loop is not
    /// a valid framework execution policy.
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// Configure one-shot iteration wind-down and provider-reported token budget.
    pub fn run_budget(mut self, policy: echo_core::agent::RunBudgetPolicy) -> Self {
        self.run_budget = policy;
        self
    }

    /// Set the non-zero total budget for a transcript persistence safe point.
    pub fn persistence_settlement_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.persistence_settlement_timeout = timeout;
        self
    }

    /// Install a resolved provider/model profile that controls harness behavior.
    pub fn model_profile(mut self, profile: echo_core::llm::capabilities::ModelProfile) -> Self {
        self.model_profile = Some(
            echo_core::llm::capabilities::ModelProfileResolution::from_explicit_profile(
                profile,
                std::time::SystemTime::now(),
            ),
        );
        self
    }

    /// Install a resolved profile together with its provenance/freshness receipt.
    pub fn model_profile_resolution(
        mut self,
        resolution: echo_core::llm::capabilities::ModelProfileResolution,
    ) -> Self {
        self.model_profile = Some(resolution);
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
        self.token_limit_explicit = true;
        self
    }

    /// Set max output tokens for LLM generation (None = API default).
    pub fn max_tokens(mut self, max: Option<u32>) -> Self {
        self.max_tokens = max;
        self
    }

    /// Set sampling temperature for LLM generation (None = API default).
    pub fn temperature(mut self, temp: Option<f32>) -> Self {
        self.temperature = temp;
        self
    }

    /// Set maximum token count for a single tool output
    ///
    /// Oversized output is persisted as an artifact when artifact storage is
    /// configured, and falls back to UTF-8-safe truncation when it is disabled.
    pub fn max_tool_output_tokens(mut self, max: usize) -> Self {
        self.max_tool_output_tokens = Some(max);
        self
    }

    /// Store complete oversized tool output under the supplied application
    /// directory while keeping only a bounded projection in model context.
    pub fn tool_output_artifacts(
        mut self,
        config: echo_core::tools::artifact::ToolOutputArtifactConfig,
    ) -> Self {
        self.tool_output_artifacts = Some(config);
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
    /// use std::sync::Arc;
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

    /// Set an intent router for pre-ReAct classification and routing.
    ///
    /// When set, the agent will classify each user input before entering the
    /// ReAct loop, potentially shortcutting to a direct answer or activating
    /// a specific skill.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use echo_agent::intent::{IntentRouter, IntentRouterConfig};
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// let router = IntentRouter::new(
    ///     Box::new(echo_agent::intent::KeywordClassifier::default()),
    ///     IntentRouterConfig::default(),
    /// );
    /// let agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .intent_router(router)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn intent_router(mut self, router: crate::intent::IntentRouter) -> Self {
        self.intent_router = Some(router);
        self
    }

    /// Set a runtime state store for checkpointing agent execution state.
    pub fn state_store(mut self, store: Arc<dyn crate::state::RuntimeStateStore>) -> Self {
        self.state_store = Some(store);
        self
    }

    /// Set the committed transcript projection store.
    ///
    /// Durable projection requires pairing this with [`Self::state_store`].
    /// Invocation admission rejects an incomplete or non-atomic pair before
    /// context, model, trace, guard, or persistence side effects.
    pub fn conversation_store(mut self, store: Arc<dyn crate::memory::ConversationStore>) -> Self {
        self.conversation_store = Some(store);
        self
    }

    /// Set a visibility horizon for proactive tool trace compaction.
    ///
    /// When configured, tool call/result pairs beyond the active window
    /// are automatically compacted into symbolic summaries during each
    /// `prepare()` call, before the main compressor runs.
    pub fn visibility_horizon(
        mut self,
        config: echo_state::compression::horizon::VisibilityHorizonConfig,
    ) -> Self {
        self.visibility_horizon = Some(config);
        self
    }

    /// Set long-term memory Store
    pub fn store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Inject an external Store and register approved-memory `recall` and `search_memory`.
    ///
    /// Install a `MemoryLayerManager` to add journaled `remember` and `forget`.
    /// This is a shortcut from "having a memory store" to "the Agent can recall approved memory",
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
    ///     .with_memory_tools(store) // approved recall/search; install a layer manager to write
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_memory_tools(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self.enable_memory = true;
        self
    }

    /// Set session_id (thread identifier)
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set React loop checkpoint interval (in iterations).
    ///
    /// When > 0, the Agent saves a checkpoint every N iterations during the
    /// React loop. This enables crash recovery: on restart, the Agent restores
    /// the conversation history and continues from where it left off.
    ///
    /// Default: 0 (only checkpoint at end of execution).
    pub fn react_checkpoint_interval(mut self, interval: usize) -> Self {
        self.react_checkpoint_interval = interval;
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

    /// Set the session-bound working directory (worktree path). Propagated to
    /// `AgentConfig.working_dir`, which `ExecuteStage` injects into every tool
    /// call's `ToolContext` so shell/file/git tools run inside the worktree.
    pub fn working_dir(mut self, working_dir: impl Into<std::path::PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    /// Set the boundary for project instruction discovery.
    pub fn project_root(mut self, project_root: impl Into<std::path::PathBuf>) -> Self {
        self.project_root = Some(project_root.into());
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
    /// Once set, this service is the single permission-checking entry point.
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

    /// Set audit logger
    pub fn audit_logger(mut self, logger: Arc<dyn AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    /// Observe Trace and Audit persistence failures without changing the Agent
    /// execution result.
    pub fn diagnostic_delivery_observer(
        mut self,
        observer: Arc<dyn DiagnosticDeliveryObserver>,
    ) -> Self {
        self.diagnostic_delivery_observer = Some(observer);
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
    /// When absent, the standard pipeline is used.
    pub fn tool_execution_pipeline(mut self, pipeline: ToolExecutionPipeline) -> Self {
        self.tool_execution_pipeline = Some(Arc::new(pipeline));
        self
    }

    /// Set sandbox manager, providing secure isolation for skill script execution
    pub fn sandbox_manager(mut self, manager: Arc<SandboxManager>) -> Self {
        self.sandbox_manager = Some(manager);
        self
    }

    // ── Build ──────────────────────────────────────────────────────────────────

    /// Build ReAct Agent (internal method)
    pub fn build(self) -> Result<ReactAgent> {
        // ── Construction-time validation ────────────────────────────────────────────
        // A client may be attached later by an application-level setup flow.
        // Until then the agent can expose configuration and non-LLM tools, and
        // normal run paths return the existing missing-client error.
        if self.max_iterations == 0 {
            return Err(crate::error::ConfigError::ConfigFileError(
                "max_iterations must be greater than zero".to_string(),
            )
            .into());
        }
        if self.persistence_settlement_timeout.is_zero() {
            return Err(crate::error::ConfigError::ConfigFileError(
                "persistence_settlement_timeout must be greater than zero".to_string(),
            )
            .into());
        }
        if self.enable_subagent && !self.enable_builtin_tools {
            return Err(crate::error::ConfigError::ConfigFileError(
                "Enabling subagent dispatch (enable_subagent) requires enabling tool calls (enable_builtin_tools)"
                    .to_string(),
            )
            .into());
        }
        // Validate an explicitly supplied provider contract before creating
        // the agent, before installing stores, tools or other owned resources.
        // The actual fallible installation below propagates any build failure.
        if let Some(llm_config) = self.llm_config.as_ref() {
            llm_config.build_client()?;
        }

        let model_profile_resolution = self
            .model_profile
            .clone()
            .or_else(|| {
                self.llm_config
                    .as_ref()
                    .map(|config| config.resolve_model_profile_at(std::time::SystemTime::now()))
            })
            .or_else(|| {
                self.llm_client
                    .as_ref()
                    .map(|client| client.model_profile_resolution())
            });
        let mut config = AgentConfig::new(&self.model, &self.name, &self.system_prompt)
            .model_streaming(self.model_streaming)
            .enable_tool(self.enable_builtin_tools)
            .auto_project_rules(self.auto_project_rules)
            .readonly_tools(self.readonly_tools)
            .enable_memory(self.enable_memory)
            .enable_human_in_loop(self.enable_human_in_loop)
            .enable_subagent(self.enable_subagent)
            .register_agent_dispatch_tool(self.register_agent_dispatch_tool)
            .tool_error_feedback(self.tool_error_feedback)
            .tool_execution(self.tool_execution)
            .max_iterations(self.max_iterations)
            .run_budget(self.run_budget)
            .persistence_settlement_timeout(self.persistence_settlement_timeout)
            .max_tokens(self.max_tokens)
            .temperature(self.temperature);
        #[cfg(feature = "subagent")]
        {
            config.register_subagent_message_tools = self.register_subagent_message_tools;
        }
        #[cfg(feature = "files")]
        if let Some(file_system) = self.file_system.clone() {
            config = config.file_system(file_system);
        }
        #[cfg(feature = "files")]
        {
            config = config.content_search_provider(self.content_search_provider.clone());
        }
        if let Some(cells) = self.command_cells.clone() {
            config = config.command_cells(cells);
        }
        if self.token_limit_explicit {
            config = config.token_limit(self.token_limit);
        }
        if let Some(resolution) = model_profile_resolution {
            config = config.model_profile_resolution(resolution);
        }
        if let Some(project_root) = self.project_root {
            config = config.project_root(project_root);
        }

        #[cfg(feature = "subagent")]
        if let Some(provider) = self.subagent_isolation_provider.clone() {
            config = config.subagent_isolation_provider(provider);
        }
        #[cfg(feature = "subagent")]
        if let Some(compiler) = self.subagent_prompt_compiler.clone() {
            config = config.subagent_prompt_compiler(compiler);
        }

        if let Some(fmt) = self.response_format {
            config = config.response_format(fmt);
        }
        if let Some(max) = self.max_tool_output_tokens {
            config = config.max_tool_output_tokens(max);
        }
        if let Some(artifact_config) = self.tool_output_artifacts {
            config = config.tool_output_artifacts(Some(artifact_config));
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
        if let Some(working_dir) = &self.working_dir {
            // Mutex 中毒时恢复毒锁内部数据继续写入, 不 panic 整个 agent。
            *config.working_dir.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(working_dir.clone());
        }
        // Freeze the same validated pair used by direct construction before
        // creating Context or other owned subsystems. No client is rebuilt.
        let configured_llm_client = self
            .configured_llm_client
            .map(|(client, original_config)| -> Result<_> {
                let (model_name, profile) = ReactAgent::prepare_configured_llm_client(
                    &config,
                    client.as_ref(),
                    &original_config,
                )?;
                config.model_name = model_name.clone();
                config.model_profile = Some(profile.clone());
                Ok((client, original_config, model_name, profile))
            })
            .transpose()?;
        if config.token_budget_config.enabled {
            config.token_budget_config.build(config.token_limit)?;
        }
        if self.react_checkpoint_interval > 0 {
            config = config.react_checkpoint_interval(self.react_checkpoint_interval);
        }

        // When the user passes a custom Store via with_memory_tools(store),
        // skip the automatic FileStore initialization inside ReactAgent::new(),
        // instead manually inject the user-provided Store during the build() phase.
        let has_external_store = self.store.is_some();
        if has_external_store {
            config = config.enable_memory(false);
        }

        #[cfg(feature = "subagent")]
        let mut agent = if let Some(registry) = self.subagent_registry {
            crate::agent::react::ReactAgent::new_with_subagent_registry(config, registry)
        } else {
            crate::agent::react::ReactAgent::new(config)
        };
        #[cfg(not(feature = "subagent"))]
        let mut agent = crate::agent::react::ReactAgent::new(config);

        if let Some(service) = self.task_revision_service {
            crate::tasks::register_task_tools(&mut agent, service);
        }

        if let Some((client, original_config, model_name, profile)) = configured_llm_client {
            agent.install_llm_config(model_name, profile, client, Some(original_config));
        } else {
            if let Some(llm_client) = self.llm_client {
                agent.set_llm_client(llm_client);
            }
            // Preserve the existing config-wins behavior for ordinary inputs.
            if let Some(llm_config) = self.llm_config {
                agent.set_sourced_llm_config(llm_config)?;
            }
        }

        // Register all custom tools
        for tool in self.tools {
            agent.add_tool(tool);
        }

        // Set Store (registers approved recall/search tools without a layer manager).
        if let Some(store) = self.store {
            agent.set_memory_store(store)?;
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

        // Set audit logger
        if let Some(logger) = self.audit_logger {
            agent.set_audit_logger(logger);
        }
        if let Some(observer) = self.diagnostic_delivery_observer {
            agent.set_diagnostic_delivery_observer(observer);
        }

        // Set snapshot manager
        if let Some(policy) = self.snapshot_policy {
            agent.set_snapshot_manager(SnapshotManager::new(policy, self.max_snapshots));
        }

        // Set circuit breaker
        if let Some(cb_config) = self.circuit_breaker_config {
            agent.set_circuit_breaker(cb_config);
        }

        // Set sandbox manager (framework default: local process wrapper).
        // Applications that want OS sandboxing should inject their own manager.
        let manager = self
            .sandbox_manager
            .unwrap_or_else(|| std::sync::Arc::new(SandboxManager::local_only()));
        agent.set_sandbox_manager(manager);

        // Set run store
        if let Some(store) = self.run_store {
            agent.run_store = Some(store);
        }

        // Set tool execution pipeline
        if let Some(pipeline) = self.tool_execution_pipeline {
            agent.tool_execution_pipeline = Some(pipeline);
        }

        // Set intent router
        if let Some(router) = self.intent_router {
            agent.intent_router = Some(router);
        }

        // Set runtime state store
        if let Some(store) = self.state_store {
            agent.memory.state_store = Some(store);
        }
        if let Some(store) = self.conversation_store {
            agent.memory.conversation_store = Some(store);
        }

        // Set visibility horizon on the ContextManager
        if let Some(horizon_config) = self.visibility_horizon {
            let horizon_compressor =
                echo_state::compression::horizon::VisibilityHorizonCompressor::new(horizon_config);
            // Agent was just created — no other task holds the lock
            if let Ok(mut ctx) = agent.memory.context.try_lock() {
                ctx.set_visibility_horizon(horizon_compressor);
            } else {
                tracing::warn!("Could not acquire ContextManager lock to set visibility horizon");
            }
        }

        // Set intervention callbacks
        if !self.intervention_callbacks.is_empty() {
            agent.tools.intervention_callbacks = self.intervention_callbacks;
        }

        Ok(agent)
    }

    /// Build an executable agent and reject deferred LLM configuration.
    ///
    /// [`Self::build`] remains available for application bootstraps that
    /// resolve a model after assembling the framework capabilities. Public
    /// quick-start/preset entry points use this strict form so an agent cannot
    /// be created without an invocation-capable LLM dependency.
    pub fn build_strict(self) -> Result<ReactAgent> {
        if self.llm_client.is_none()
            && self.llm_config.is_none()
            && self.configured_llm_client.is_none()
        {
            return Err(crate::error::ConfigError::MissingConfig(
                "llm_client".to_string(),
                "inject an LlmClient or LlmConfig before using build_strict".to_string(),
            )
            .into());
        }
        self.build()
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
    fn rejects_unbounded_zero_iteration_configuration() {
        let result = ReactAgentBuilder::new()
            .model("test-model")
            .max_iterations(0)
            .build();
        assert!(matches!(result, Err(crate::error::ReactError::Config(_))));
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
    fn default_agent_uses_one_task_relation_api() -> std::result::Result<(), String> {
        let agent = ReactAgentBuilder::new()
            .model("test-model")
            .system_prompt("Test")
            .build()
            .map_err(|error| error.to_string())?;
        let names = agent.tool_names();
        for expected in ["task_create", "task_update", "task_list"] {
            assert!(names.iter().any(|name| name == expected));
        }
        assert!(!names.iter().any(|name| name == "todo_write"));
        Ok(())
    }

    #[test]
    fn builder_allows_deferred_llm_configuration() -> std::result::Result<(), String> {
        let agent = ReactAgentBuilder::new()
            .system_prompt("Test")
            .build()
            .map_err(|error| error.to_string())?;

        assert!(agent.model_name().is_empty());
        assert!(agent.llm_client().is_none());
        assert!(agent.llm_config().is_none());
        Ok(())
    }

    #[test]
    fn builder_applies_nonzero_persistence_settlement_timeout() -> std::result::Result<(), String> {
        let timeout = std::time::Duration::from_millis(750);
        let agent = ReactAgentBuilder::new()
            .persistence_settlement_timeout(timeout)
            .build()
            .map_err(|error| error.to_string())?;
        assert_eq!(agent.config().persistence_settlement_timeout, timeout);
        Ok(())
    }

    #[test]
    fn builder_rejects_zero_persistence_settlement_timeout() {
        let result = ReactAgentBuilder::new()
            .persistence_settlement_timeout(std::time::Duration::ZERO)
            .build();
        assert!(matches!(result, Err(crate::error::ReactError::Config(_))));
    }

    #[test]
    fn test_builder_llm_config_syncs_runtime_model_name() -> std::result::Result<(), String> {
        let config = LlmConfig::for_provider(
            "openai",
            "https://api.example.test/v1",
            "sk-demo",
            "gpt-5.5",
            echo_core::llm::LlmApiProtocol::Responses,
        )
        .map_err(|error| error.to_string())?;
        let agent = ReactAgentBuilder::new()
            .llm_config(config)
            .system_prompt("Test")
            .build()
            .map_err(|error| error.to_string())?;

        assert_eq!(agent.config().get_model_name(), "gpt-5.5");
        assert_eq!(
            agent.llm_config().map(|cfg| cfg.model.as_str()),
            Some("gpt-5.5")
        );
        Ok(())
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

    // ── 远端内容搜索接缝的构建期透传 ──

    /// 只回传一条远端记录，用来证明 grep 确实拿到了 builder 注入的提供者。
    #[cfg(feature = "files")]
    struct RemoteBuilderProvider;

    #[cfg(feature = "files")]
    impl echo_core::tools::search::ContentSearchProvider for RemoteBuilderProvider {
        fn search(
            &self,
            request: &echo_core::tools::search::ContentSearchRequest<'_>,
        ) -> crate::error::Result<Option<echo_core::tools::search::ContentSearchOutcome>> {
            Ok(Some(echo_core::tools::search::ContentSearchOutcome {
                records: vec![format!("{}:1:  remote-builder-hit", request.root.display())],
                truncated: false,
            }))
        }
    }

    #[cfg(feature = "files")]
    fn grep_parameters(pattern: &str, path: &str) -> echo_core::tools::ToolParameters {
        echo_core::tools::ToolParameters::from([
            ("pattern".to_string(), serde_json::json!(pattern)),
            ("path".to_string(), serde_json::json!(path)),
        ])
    }

    #[cfg(feature = "files")]
    fn installed_grep(agent: &ReactAgent) -> std::result::Result<Arc<dyn Tool>, String> {
        agent
            .tool_manager()
            .get_tool("grep")
            .ok_or_else(|| "standard pack did not install grep".to_string())
    }

    /// 不设置提供者时（默认路径），grep 仍是本机遍历，输出与接入前一致。
    #[cfg(feature = "files")]
    #[tokio::test]
    async fn builder_default_keeps_local_grep_behavior() -> std::result::Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let working = std::fs::canonicalize(dir.path()).map_err(|error| error.to_string())?;
        std::fs::write(working.join("notes.txt"), "TODO fix this\n")
            .map_err(|error| error.to_string())?;

        let agent = ReactAgentBuilder::new()
            .model("test-model")
            .system_prompt("Test")
            .enable_tools()
            .build()
            .map_err(|error| error.to_string())?;
        let tool = installed_grep(&agent)?;
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working),
            ..Default::default()
        };
        let result = tool
            .execute_with_context(grep_parameters("TODO", "notes.txt"), &ctx)
            .await
            .map_err(|error| error.to_string())?;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("fix this"));
        Ok(())
    }

    /// 设置提供者后，grep 不再读本机正文：目标目录在本机不存在也能返回记录。
    #[cfg(feature = "files")]
    #[tokio::test]
    async fn builder_forwards_content_search_provider_to_grep() -> std::result::Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let working = std::fs::canonicalize(dir.path()).map_err(|error| error.to_string())?;

        let agent = ReactAgentBuilder::new()
            .model("test-model")
            .system_prompt("Test")
            .enable_tools()
            .content_search_provider(Some(Arc::new(RemoteBuilderProvider)))
            .build()
            .map_err(|error| error.to_string())?;
        let tool = installed_grep(&agent)?;
        let ctx = echo_core::tools::ToolContext {
            working_dir: Some(working),
            ..Default::default()
        };
        let result = tool
            .execute_with_context(grep_parameters("needle", "remote-project"), &ctx)
            .await
            .map_err(|error| error.to_string())?;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("remote-builder-hit"));
        Ok(())
    }
}
