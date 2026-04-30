//! ReAct Agent core module
//!
//! ## Module Structure
//!
//! | File | Responsibility |
//! |------|----------------|
//! | `mod.rs` | Struct definition, `new()`, `impl Agent` trait |
//! | `run.rs` | Execution engine (`think` / `process_steps` / `run_react_loop`) |
//! | `capabilities.rs` | Capability configuration (tool / skill / MCP / subagent registration) |
//! | `extract.rs` | Structured JSON extraction (`extract_json` / `extract`) |

pub use crate::agent::config::{AgentConfig, AgentRole};
use crate::agent::subagent::SubagentRegistry;
use crate::agent::subagent::executor::{SubagentExecutor, SubagentExecutorConfig};
use crate::agent::{Agent, AgentEvent, CancellationToken};
use crate::compression::ContextManager;
use crate::error::{LlmError, ReactError, Result};
use crate::guard::GuardManager;
#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopProvider, PermissionService};
use crate::llm::config::LlmConfig;
#[cfg(feature = "mcp")]
use crate::mcp::McpManager;
use crate::memory::checkpointer::{Checkpointer, FileCheckpointer};
use crate::memory::snapshot::{SnapshotManager, StateSnapshot};
use crate::memory::store::{FileStore, Store};
use crate::sandbox::SandboxManager;
use crate::skills::SkillRegistry;
use crate::skills::hooks::HookRegistry;
#[cfg(feature = "tasks")]
use crate::tasks::TaskManager;
use crate::tools::ToolManager;
use crate::tools::builtin::agent_dispatch::AgentDispatchTool;
use crate::tools::builtin::answer::FinalAnswerTool;
#[cfg(feature = "human-loop")]
use crate::tools::builtin::human_in_loop::HumanInLoop;
use crate::tools::builtin::memory::{ForgetTool, RecallTool, RememberTool, SearchMemoryTool};
#[cfg(feature = "tasks")]
use crate::tools::builtin::plan::PlanTool;
#[cfg(feature = "tasks")]
use crate::tools::builtin::task::{
    CreateTaskTool, GetExecutionOrderTool, ListTasksTool, UpdateTaskTool, VisualizeDependenciesTool,
};
use echo_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use std::sync::Arc;
use tracing::{Instrument, info, info_span, warn};

use crate::agent::react::subsystems::approval::ApprovalSubsystem;
use crate::agent::react::subsystems::guard::GuardSubsystem;
use crate::agent::react::subsystems::memory::MemorySubsystem;
use crate::agent::react::subsystems::tool_exec::ToolExecutionSubsystem;

pub mod builder;
mod capabilities;
mod extract;
#[cfg(feature = "tasks")]
mod planning;
mod run;
pub mod structured;
pub(crate) mod subsystems;
#[cfg(test)]
mod tests;
// ── Built-in tool name constants ────────────────────────────────────────────────

pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";
#[cfg(feature = "tasks")]
pub(crate) const TOOL_CREATE_TASK: &str = "create_task";
#[cfg(feature = "tasks")]
pub(crate) const TOOL_PLAN: &str = "plan";
#[cfg(feature = "tasks")]
pub(crate) const TOOL_UPDATE_TASK: &str = "update_task";

/// Returns `true` if the LLM error is worth retrying (network, timeout, rate-limit, server 5xx).
pub(crate) fn is_retryable_llm_error(err: &ReactError) -> bool {
    match err {
        ReactError::Llm(e) => match e.as_ref() {
            LlmError::NetworkError(_) => true,
            LlmError::ApiError { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        },
        _ => false,
    }
}

// ── ReactAgent struct ───────────────────────────────────────────────────────────

/// ReAct (Reasoning + Acting) Agent implementation.
///
/// An autonomous agent based on the ReAct paradigm, supporting tool calling,
/// task planning, subagent dispatch, long-term memory, chain-of-thought,
/// context compression, and other core capabilities.
///
/// # Core Components
///
/// - **Configuration**: Behavior and capabilities controlled via `AgentConfig`
/// - **Context management**: Maintains conversation history with auto-compression and token counting
/// - **Tool management**: Register, discover, and execute tools with permission control and sandbox execution
/// - **Subagent system**: Supports Sync/Fork/Teammate dispatch modes
/// - **Memory system**: Long-term memory storage and retrieval
/// - **Skill system**: Code-based and file-based skill management
/// - **Hook system**: Tool-call interception and modification
pub struct ReactAgent {
    pub(crate) config: AgentConfig,
    /// Tool execution subsystem: tool registry/execution, Skill, Hook, MCP, SubAgent, Sandbox
    pub(crate) tools: ToolExecutionSubsystem,
    /// Guard & safety subsystem: guards, permission policy, audit logging, circuit breaker
    pub(crate) guard: GuardSubsystem,
    /// Memory & persistence subsystem: context management, long-term memory, snapshots, checkpointer
    pub(crate) memory: MemorySubsystem,
    /// Human-in-the-loop approval subsystem
    pub(crate) approval: ApprovalSubsystem,
    client: Arc<Client>,
    llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// LLM configuration (optional; falls back to environment variables when not set)
    llm_config: Option<LlmConfig>,
    /// Cancellation token for the current streaming request, set in
    /// `chat_stream_with_cancel` / `execute_stream_with_cancel`.
    /// `create_llm_stream` reads this field and passes it to the HTTP layer
    /// to support request-level stream cancellation.
    /// Uses `tokio::sync::Mutex` to support `&self` streaming methods.
    cancel_token: tokio::sync::Mutex<Option<CancellationToken>>,
}

// ── Construction & initialization ──────────────────────────────────────────────

impl ReactAgent {
    #[cfg(feature = "tasks")]
    pub(crate) fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && [TOOL_PLAN, TOOL_CREATE_TASK, TOOL_UPDATE_TASK]
                .iter()
                .all(|name| self.tools.tool_manager.get_tool(name).is_some())
    }

    #[cfg(not(feature = "tasks"))]
    #[allow(dead_code)]
    pub(crate) fn has_planning_tools(&self) -> bool {
        false
    }

    /// Chain-of-thought preamble auto-injected before tool calls.
    const COT_INSTRUCTION: &'static str =
        "Before calling any tool, briefly describe your analysis and execution plan.";

    /// Create a new ReAct Agent instance.
    ///
    /// # Parameters
    /// * `config` - Agent runtime configuration
    ///
    /// # Returns
    /// A fully initialized `ReactAgent` instance.
    ///
    /// # Details
    /// This method initializes all core components based on the config, including:
    /// - Context manager
    /// - Tool manager (tools enabled per config)
    /// - Subagent system (subagent dispatch enabled per config)
    /// - Memory system (long-term memory enabled per config)
    /// - Skill registry
    /// - Hook system
    pub fn new(config: AgentConfig) -> Self {
        let system_prompt = Self::build_system_prompt(&config);

        let context = Arc::new(tokio::sync::Mutex::new(
            ContextManager::builder(config.token_limit)
                .with_system(system_prompt)
                .build(),
        ));

        let mut tool_manager = ToolManager::new_with_config(config.tool_execution.clone());
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        // ── Core tools ─────────────────────────────────────────────
        tool_manager.register(Box::new(FinalAnswerTool));

        // ── Subsystem initialization ──────────────────────────────
        #[cfg(feature = "tasks")]
        let task_manager = Arc::new(TaskManager::default());
        let subagent_registry = Arc::new(SubagentRegistry::new());
        let subagent_executor = Arc::new(SubagentExecutor::new(
            subagent_registry.clone(),
            SubagentExecutorConfig::default(),
        ));
        #[cfg(feature = "human-loop")]
        let approval_provider = crate::human_loop::default_provider();

        // ── Feature-gated tool registration ───────────────────────
        // AgentDispatch is controlled by runtime config enable_subagent
        if config.enable_subagent {
            tool_manager.register(Box::new(AgentDispatchTool::new(
                subagent_executor.clone(),
                config.agent_name.clone(),
                CancellationToken::new(),
            )));
        }

        #[cfg(feature = "human-loop")]
        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop::new(approval_provider.clone())));
        }

        #[cfg(feature = "tasks")]
        if config.enable_task {
            tool_manager.register(Box::new(PlanTool));
            tool_manager.register(Box::new(CreateTaskTool::new(task_manager.clone())));
            tool_manager.register(Box::new(UpdateTaskTool::new(task_manager.clone())));
            tool_manager.register(Box::new(ListTasksTool::new(task_manager.clone())));
            tool_manager.register(Box::new(VisualizeDependenciesTool::new(
                task_manager.clone(),
            )));
            tool_manager.register(Box::new(GetExecutionOrderTool::new(task_manager.clone())));
        }
        Self::register_feature_gated_tools(&config, &mut tool_manager);

        // ── Memory store ──────────────────────────────────────────
        let store = Self::setup_memory_store(&config, &mut tool_manager);

        // ── Checkpointer ─────────────────────────────────────────
        let checkpointer = Self::setup_checkpointer(&config);

        Self {
            config,
            tools: ToolExecutionSubsystem {
                tool_manager,
                subagent_registry,
                #[cfg(feature = "tasks")]
                task_manager,
                skill_registry: SkillRegistry::new(),
                progressive_skill_registry: None,
                hook_registry: Arc::new(tokio::sync::RwLock::new(HookRegistry::new())),
                #[cfg(feature = "mcp")]
                mcp_manager: McpManager::new(),
                sandbox_manager: None,
            },
            guard: GuardSubsystem {
                guard_manager: None,
                permission_policy: None,
                audit_logger: None,
                circuit_breaker: None,
            },
            memory: MemorySubsystem {
                context,
                store,
                checkpointer,
                snapshot_manager: Arc::new(std::sync::RwLock::new(None)),
                conversation_store: None,
            },
            approval: ApprovalSubsystem {
                #[cfg(feature = "human-loop")]
                approval_provider,
                #[cfg(feature = "human-loop")]
                permission_service: None,
                #[cfg(feature = "human-loop")]
                pending_permission_rules: std::sync::Mutex::new(Vec::new()),
            },
            client: Arc::new(client),
            llm_client: None,
            llm_config: None,
            cancel_token: tokio::sync::Mutex::new(None),
        }
    }

    /// Create an Agent from a configuration file.
    ///
    /// Searches for `echo-agent.yaml` and loads the config.
    ///
    /// ```no_run
    /// use echo_agent::agent::react::ReactAgent;
    /// let agent = ReactAgent::from_config_file(None);
    /// ```
    pub fn from_config_file(path: Option<&str>) -> Self {
        let app_config = crate::config::load_config(path);
        Self::new(app_config.to_agent_config())
    }

    // ── Constructor helpers ───────────────────────────────────────────────────────

    fn build_system_prompt(config: &AgentConfig) -> String {
        let mut prompt = if config.enable_tool && config.enable_cot {
            format!(
                "{}\n\n{}",
                config.system_prompt.trim_end(),
                Self::COT_INSTRUCTION,
            )
        } else {
            config.system_prompt.clone()
        };

        #[cfg(feature = "project-rules")]
        if config.auto_project_rules {
            let wd = config
                .working_dir
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            prompt = echo_core::project_rules::inject_rules(&prompt, &wd);
        }

        prompt
    }

    fn register_feature_gated_tools(config: &AgentConfig, tool_manager: &mut ToolManager) {
        #[cfg(feature = "git")]
        if config.enable_tool {
            use crate::tools::builtin::git::{
                GitBlameTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool,
            };
            tool_manager.register(Box::new(GitStatusTool));
            tool_manager.register(Box::new(GitDiffTool));
            tool_manager.register(Box::new(GitLogTool));
            tool_manager.register(Box::new(GitBlameTool));
            tool_manager.register(Box::new(GitBranchTool));
            tool_manager.register(Box::new(GitCommitTool));
        }

        #[cfg(feature = "rag")]
        if config.enable_tool {
            use crate::tools::builtin::rag::{RagChunkDocumentTool, RagIndexTool, RagSearchTool};
            tool_manager.register(Box::new(RagIndexTool));
            tool_manager.register(Box::new(RagSearchTool));
            tool_manager.register(Box::new(RagChunkDocumentTool));
        }

        #[cfg(feature = "chart")]
        if config.enable_tool {
            tool_manager.register(Box::new(crate::tools::builtin::chart::GenerateChartTool));
        }

        #[cfg(feature = "database")]
        if config.enable_tool {
            use crate::tools::builtin::database::{
                DescribeTableTool, ListTablesTool, SqlQueryTool,
            };
            tool_manager.register(Box::new(SqlQueryTool));
            tool_manager.register(Box::new(ListTablesTool));
            tool_manager.register(Box::new(DescribeTableTool));
        }

        #[cfg(feature = "web")]
        if config.enable_tool {
            use crate::tools::builtin::browser::{WebExtractTool, WebFetchTool};
            tool_manager.register(Box::new(WebFetchTool));
            tool_manager.register(Box::new(WebExtractTool));
        }

        if config.enable_tool {
            #[cfg(feature = "media")]
            {
                use crate::tools::builtin::excel::{ExcelInfoTool, ExcelReadTool, ExcelToCsvTool};
                use crate::tools::builtin::image::ImageAnalysisTool;
                use crate::tools::builtin::pdf::{PdfExtractTool, PdfInfoTool};
                use crate::tools::builtin::text::{
                    TextExportTool, TextProcessTool, TextReadTool, TextSearchTool, TextStatsTool,
                };
                use crate::tools::builtin::word::{WordInfoTool, WordReadTool, WordStructureTool};

                tool_manager.register(Box::new(ImageAnalysisTool));
                tool_manager.register(Box::new(PdfExtractTool));
                tool_manager.register(Box::new(PdfInfoTool));
                tool_manager.register(Box::new(ExcelReadTool));
                tool_manager.register(Box::new(ExcelInfoTool));
                tool_manager.register(Box::new(ExcelToCsvTool));
                tool_manager.register(Box::new(WordReadTool));
                tool_manager.register(Box::new(WordInfoTool));
                tool_manager.register(Box::new(WordStructureTool));
                tool_manager.register(Box::new(TextReadTool));
                tool_manager.register(Box::new(TextSearchTool));
                tool_manager.register(Box::new(TextStatsTool));
                tool_manager.register(Box::new(TextProcessTool));
                tool_manager.register(Box::new(TextExportTool));
            }

            #[cfg(feature = "data")]
            {
                use crate::tools::builtin::data::{
                    DataAggregateTool, DataBinTool, DataContributionTool, DataExportTool,
                    DataFilterTool, DataProfileTool, DataRatioTool, DataReadTool, DataStatsTool,
                    DataTopNTool, DataTransformTool,
                };

                tool_manager.register(Box::new(DataReadTool));
                tool_manager.register(Box::new(DataFilterTool));
                tool_manager.register(Box::new(DataAggregateTool));
                tool_manager.register(Box::new(DataStatsTool));
                tool_manager.register(Box::new(DataTransformTool));
                tool_manager.register(Box::new(DataExportTool));
                tool_manager.register(Box::new(DataProfileTool));
                tool_manager.register(Box::new(DataTopNTool));
                tool_manager.register(Box::new(DataContributionTool));
                tool_manager.register(Box::new(DataBinTool));
                tool_manager.register(Box::new(DataRatioTool));
            }
        }
    }

    fn setup_memory_store(
        config: &AgentConfig,
        tool_manager: &mut ToolManager,
    ) -> Option<Arc<dyn Store>> {
        if !config.enable_memory {
            return None;
        }
        match FileStore::new(&config.memory_path) {
            Ok(file_store) => {
                let store: Arc<dyn Store> = Self::wrap_with_embedding_store_if_available(
                    Arc::new(file_store),
                    &config.memory_path,
                );
                let agent_name = config.agent_name.clone();
                let namespace = vec![agent_name, "memories".to_string()];
                tool_manager.register(Box::new(RememberTool::new(
                    store.clone(),
                    namespace.clone(),
                )));
                tool_manager.register(Box::new(RecallTool::new(store.clone(), namespace.clone())));
                tool_manager.register(Box::new(SearchMemoryTool::new(
                    store.clone(),
                    namespace.clone(),
                )));
                tool_manager.register(Box::new(ForgetTool::new(store.clone(), namespace)));
                Some(store)
            }
            Err(e) => {
                tracing::warn!("Long-term memory Store init failed, memory disabled: {e}");
                None
            }
        }
    }

    /// When embedding environment variables are configured, wraps the underlying
    /// Store with [`EmbeddingStore`] so that `remember` writes are auto-vectorized
    /// and `search_memory` hybrid search works.
    ///
    /// If no embedding is configured, returns the original Store unchanged.
    fn wrap_with_embedding_store_if_available(
        inner: Arc<dyn Store>,
        memory_path: &str,
    ) -> Arc<dyn Store> {
        use crate::memory::{EmbeddingStore, HttpEmbedder};

        if std::env::var("EMBEDDING_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
            && std::env::var("EMBEDDING_APIKEY").is_err()
        {
            tracing::info!(
                "Memory Store: keyword-only retrieval (no embedding env vars configured)"
            );
            return inner;
        }

        let embedder = Arc::new(HttpEmbedder::from_env());
        let vec_path = format!("{}.vecs.json", memory_path.trim_end_matches(".json"));

        match EmbeddingStore::with_persistence(Arc::clone(&inner), embedder, &vec_path) {
            Ok(embedding_store) => {
                tracing::info!(
                    vec_path = %vec_path,
                    "Memory Store: vector index enabled (semantic/hybrid search available)"
                );
                Arc::new(embedding_store)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "EmbeddingStore init failed, falling back to keyword-only retrieval"
                );
                inner
            }
        }
    }

    fn setup_checkpointer(config: &AgentConfig) -> Option<Arc<dyn Checkpointer>> {
        config.session_id.as_ref()?;
        match FileCheckpointer::new(&config.checkpointer_path) {
            Ok(cp) => Some(Arc::new(cp)),
            Err(e) => {
                tracing::warn!("Checkpointer init failed, session resume disabled: {e}");
                None
            }
        }
    }

    // ── LLM config injection ──────────────────────────────────────────────────────

    /// Inject a custom LLM configuration (dependency injection pattern).
    ///
    /// Use this method to:
    /// - Dynamically switch API configurations
    /// - Support multi-tenant scenarios
    /// - Facilitate testing
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use echo_agent::llm::LlmConfig;
    /// use echo_agent::prelude::*;
    ///
    /// let llm_config = LlmConfig::new(
    ///     "https://api.openai.com/v1/chat/completions",
    ///     "sk-...",
    ///     "qwen3-max",
    /// );
    ///
    /// let agent = ReactAgent::new(
    ///     AgentConfig::standard("qwen3-max", "assistant", "You are a helpful assistant")
    /// ).with_llm_config(llm_config);
    /// ```
    pub fn with_llm_config(mut self, config: LlmConfig) -> Self {
        self.config.model_name = config.model.clone();
        self.llm_config = Some(config);
        self
    }

    /// Inject a custom LLM client.
    pub fn with_llm_client(mut self, client: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.config.model_name = client.model_name().to_string();
        self.llm_client = Some(client);
        self
    }

    /// Set the LLM configuration.
    pub fn set_llm_config(&mut self, config: LlmConfig) {
        self.config.model_name = config.model.clone();
        self.llm_config = Some(config);
    }

    /// Set a custom LLM client.
    pub fn set_llm_client(&mut self, client: Arc<dyn crate::llm::LlmClient>) {
        self.config.model_name = client.model_name().to_string();
        self.llm_client = Some(client);
    }

    /// Get the current LLM configuration.
    pub fn llm_config(&self) -> Option<&LlmConfig> {
        self.llm_config.as_ref()
    }

    // ── Accessors & setters ──────────────────────────────────────────────────────

    /// Get a read-only reference to the AgentConfig.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Inject a custom long-term memory Store (replaces the injection channel only; does not re-register tools).
    pub fn set_store(&mut self, store: Arc<dyn Store>) {
        self.memory.store = Some(store);
    }

    /// Replace the long-term memory Store and re-register `remember` / `recall` / `forget` tools.
    ///
    /// ```rust,no_run
    /// use echo_agent::memory::{EmbeddingStore, FileStore, HttpEmbedder};
    /// use echo_agent::prelude::ReactAgent;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// # let config = unimplemented!();
    /// let inner = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
    /// let embedder = Arc::new(HttpEmbedder::from_env());
    /// let store = Arc::new(
    ///     EmbeddingStore::with_persistence(inner, embedder, "~/.echo-agent/store.vecs.json")?
    /// );
    ///
    /// let mut agent = ReactAgent::new(config);
    /// agent.set_memory_store(store);
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_memory_store(&mut self, store: Arc<dyn Store>) {
        let ns = vec![self.config.agent_name.clone(), "memories".to_string()];
        self.tools
            .tool_manager
            .register(Box::new(RememberTool::new(store.clone(), ns.clone())));
        self.tools
            .tool_manager
            .register(Box::new(RecallTool::new(store.clone(), ns.clone())));
        self.tools
            .tool_manager
            .register(Box::new(SearchMemoryTool::new(store.clone(), ns.clone())));
        self.tools
            .tool_manager
            .register(Box::new(ForgetTool::new(store.clone(), ns)));
        self.memory.store = Some(store);
    }

    /// Get a read-only reference to the current long-term memory Store.
    pub fn store(&self) -> Option<&Arc<dyn Store>> {
        self.memory.store.as_ref()
    }

    /// Inject a thread-state store and bind a session_id to enable cross-process thread recovery.
    pub fn set_checkpointer(&mut self, checkpointer: Arc<dyn Checkpointer>, session_id: String) {
        self.memory.checkpointer = Some(checkpointer);
        self.config.session_id = Some(session_id);
    }

    /// Semantic alias for `set_checkpointer()`.
    pub fn set_thread_store(&mut self, store: Arc<dyn Checkpointer>, session_id: String) {
        self.set_checkpointer(store, session_id);
    }

    /// Get a read-only reference to the current thread-state store.
    pub fn checkpointer(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.memory.checkpointer.as_ref()
    }

    /// Semantic alias for `checkpointer()`.
    pub fn thread_store(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.memory.checkpointer.as_ref()
    }

    /// Set the conversation_id used for conversation history projection.
    pub fn set_conversation_id(&mut self, conversation_id: impl Into<String>) {
        self.config.conversation_id = Some(conversation_id.into());
    }

    /// Get the current conversation_id for conversation history projection.
    pub fn conversation_id(&self) -> Option<&str> {
        self.config.get_conversation_id()
    }

    /// Get the current conversation history messages (read-only).
    pub async fn get_messages(&self) -> Vec<crate::llm::types::Message> {
        self.memory.context.lock().await.messages().to_vec()
    }

    /// Get the list of registered tool names.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.tool_manager.list_tools()
    }

    /// Get the list of registered Skill names.
    pub fn skill_names(&self) -> Vec<&str> {
        self.tools
            .skill_registry
            .list()
            .iter()
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Get the list of connected MCP server names.
    #[cfg(feature = "mcp")]
    pub fn mcp_server_names(&self) -> Vec<&str> {
        self.tools.mcp_manager.server_names()
    }

    #[cfg(not(feature = "mcp"))]
    pub fn mcp_server_names(&self) -> Vec<&str> {
        vec![]
    }

    /// Enable the circuit breaker.
    ///
    /// Automatically trips after consecutive LLM failures reach the threshold,
    /// then probes for recovery after the configured timeout.
    pub fn set_circuit_breaker(&mut self, config: CircuitBreakerConfig) {
        self.guard.circuit_breaker = Some(Arc::new(CircuitBreaker::new(config)));
    }

    /// Set the guard manager.
    pub fn set_guard_manager(&mut self, manager: GuardManager) {
        self.guard.guard_manager = Some(manager);
    }

    /// Set the permission policy.
    pub fn set_permission_policy(
        &mut self,
        policy: Arc<dyn crate::tools::permission::PermissionPolicy>,
    ) {
        self.guard.permission_policy = Some(policy);
    }

    #[cfg(feature = "human-loop")]
    /// Set the unified permission service.
    ///
    /// Once set, `check_tool_approval()` will prefer this service,
    /// falling back to the legacy PermissionPolicy logic.
    pub fn set_permission_service(&mut self, service: Arc<PermissionService>) {
        self.approval.permission_service = Some(service);
    }

    #[cfg(feature = "human-loop")]
    /// Build and set a unified PermissionService from legacy components.
    ///
    /// Merges the current `permission_policy` + `approval_provider` into a single
    /// `PermissionService`, ensuring correct pipeline order (mode → hooks → rules → handler).
    pub fn build_permission_service(&mut self) {
        use crate::human_loop::service::PermissionService;

        let policy = self.guard.permission_policy.take();
        let provider = self.approval.approval_provider.clone();

        let service = PermissionService::from_provider(provider);
        let service = if let Some(p) = policy {
            service.with_legacy_policy(p)
        } else {
            service
        };

        self.approval.permission_service = Some(Arc::new(service));
    }

    /// Set the audit logger.
    pub fn set_audit_logger(&mut self, logger: Arc<dyn crate::audit::AuditLogger>) {
        self.guard.audit_logger = Some(logger);
    }

    // ── Snapshots & rollback ────────────────────────────────────────────────────

    /// Set the sandbox manager to provide secure isolation for skill script execution.
    pub fn set_sandbox_manager(&mut self, manager: Arc<SandboxManager>) {
        self.tools
            .skill_registry
            .set_sandbox_manager(manager.clone());
        if let Some(shared) = &self.tools.progressive_skill_registry
            && let Ok(mut registry) = shared.try_write()
        {
            registry.set_sandbox_manager(manager.clone());
        }
        if let Ok(mut hooks) = self.tools.hook_registry.try_write() {
            hooks.set_sandbox_manager(manager.clone());
        }
        self.tools.sandbox_manager = Some(manager);
    }

    /// Enable state snapshot functionality.
    pub fn set_snapshot_manager(&self, manager: SnapshotManager) {
        let mut guard = self
            .memory
            .snapshot_manager
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(manager);
    }

    /// Manually capture a snapshot of the current conversation state, returning the snapshot ID.
    pub async fn snapshot(&self) -> Option<String> {
        let ctx = self.memory.context.lock().await;
        let messages = ctx.messages().to_vec();
        let mut guard = self
            .memory
            .snapshot_manager
            .write()
            .unwrap_or_else(|e| e.into_inner());
        guard.as_mut().map(|mgr| mgr.capture(0, &messages))
    }

    /// Roll back to a snapshot N steps ago.
    ///
    /// `steps_back = 1` means go back to the most recent snapshot.
    /// On success, restores the conversation history and returns snapshot info.
    pub async fn rollback(&self, steps_back: usize) -> Option<StateSnapshot> {
        let snapshot = {
            let mut guard = self
                .memory
                .snapshot_manager
                .write()
                .unwrap_or_else(|e| e.into_inner());
            guard.as_mut().and_then(|mgr| mgr.rollback(steps_back))
        };
        let snapshot = snapshot?;
        let mut ctx = self.memory.context.lock().await;
        ctx.clear();
        for msg in &snapshot.messages {
            ctx.push(msg.clone());
        }
        Some(snapshot)
    }

    /// Roll back to the snapshot with the given ID.
    pub async fn rollback_to(&self, snapshot_id: &str) -> Option<StateSnapshot> {
        let snapshot = {
            let mut guard = self
                .memory
                .snapshot_manager
                .write()
                .unwrap_or_else(|e| e.into_inner());
            guard.as_mut().and_then(|mgr| mgr.rollback_to(snapshot_id))
        };
        let snapshot = snapshot?;
        let mut ctx = self.memory.context.lock().await;
        ctx.clear();
        for msg in &snapshot.messages {
            ctx.push(msg.clone());
        }
        Some(snapshot)
    }

    /// Get the list of all snapshots.
    pub fn snapshots(&self) -> Vec<StateSnapshot> {
        let guard = self
            .memory
            .snapshot_manager
            .read()
            .unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .map(|mgr| mgr.list().to_vec())
            .unwrap_or_default()
    }

    /// Get the latest snapshot.
    pub fn latest_snapshot(&self) -> Option<StateSnapshot> {
        let guard = self
            .memory
            .snapshot_manager
            .read()
            .unwrap_or_else(|e| e.into_inner());
        guard.as_ref().and_then(|mgr| mgr.latest().cloned())
    }

    #[cfg(feature = "human-loop")]
    /// Replace the approval provider, enabling runtime switching of the approval channel.
    pub fn set_approval_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.set_human_loop_provider(provider);
    }

    #[cfg(feature = "human-loop")]
    /// Set the human-in-the-loop provider.
    ///
    /// Updates both `approval_provider` (tool approval guard) and the `human_in_loop`
    /// built-in tool (LLM-initiated triggers), keeping both pointing to the same provider.
    pub fn set_human_loop_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.approval.approval_provider = provider.clone();
        if self.tools.tool_manager.get_tool("human_in_loop").is_some() {
            self.tools
                .tool_manager
                .register(Box::new(HumanInLoop::new(provider)));
        }
    }

    // ── Conversation persistence ──────────────────────────────────────────────────

    /// Set the conversation history projection Store.
    ///
    /// When enabled, the agent projects the current transcript into a
    /// `ConversationStore` alongside thread-state persistence, for history
    /// browsing and product-layer queries.
    ///
    /// Note: this feature requires an explicit, separate `conversation_id`;
    /// `session_id` is only used for thread-state recovery, not as a fallback
    /// for history projection.
    pub fn set_conversation_store(
        &mut self,
        store: Arc<dyn crate::memory::conversation::ConversationStore>,
    ) {
        self.memory.conversation_store = Some(store);
    }

    /// Load historical messages into the agent context (replaces existing context).
    ///
    /// Used to restore a conversation from persistent storage so the agent
    /// can continue a previous dialogue. Messages should include the system
    /// prompt as the first entry if needed.
    pub async fn load_messages(&self, messages: Vec<crate::llm::types::Message>) {
        self.memory.context.lock().await.set_messages(messages);
    }

    /// Shut down the agent and release all resources.
    ///
    /// Closes MCP connections, cancels background tasks, and shuts down WebSocket servers.
    /// Call this when the agent is no longer needed, or rely on `Drop` for automatic cleanup.
    pub async fn shutdown(&self) {
        #[cfg(feature = "mcp")]
        {
            self.tools.mcp_manager.close_all().await;
        }
        // Close WebSocket servers if any (placeholder for future WS integration)
        info!(agent = %self.config.agent_name, "Agent shut down complete");
    }
}

// ── Drop implementation for automatic resource cleanup ──

impl Drop for ReactAgent {
    fn drop(&mut self) {
        #[cfg(feature = "mcp")]
        {
            // MCP cleanup is async, but Drop is synchronous.
            // Only spawn cleanup when a Tokio runtime is available.
            let mcp_mgr =
                std::mem::replace(&mut self.tools.mcp_manager, crate::mcp::McpManager::new());
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    mcp_mgr.close_all().await;
                });
            }
        }
    }
}

// ── LLM per-turn output type ───────────────────────────────────────────────────

pub use echo_core::agent::StepType;

// ── impl Agent for ReactAgent ────────────────────────────────────────────────

impl Agent for ReactAgent {
    fn name(&self) -> &str {
        &self.config.agent_name
    }

    fn model_name(&self) -> &str {
        &self.config.model_name
    }

    fn system_prompt(&self) -> &str {
        &self.config.system_prompt
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move {
                #[cfg(feature = "tasks")]
                if self.has_planning_tools() {
                    return self.execute_with_planning(task).await;
                }
                self.run_direct(task).await
            }
            .instrument(info_span!("agent_execute", agent.name = %agent, agent.model = %model)),
        )
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move { self.run_stream(task, run::StreamMode::Execute).await }.instrument(
                info_span!("agent_execute_stream", agent.name = %agent, agent.model = %model),
            ),
        )
    }

    fn chat<'a>(&'a self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move { self.run_chat_direct(message).await }
                .instrument(info_span!("agent_chat", agent.name = %agent, agent.model = %model)),
        )
    }

    fn chat_stream<'a>(
        &'a self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move { self.run_stream(message, run::StreamMode::Chat).await }.instrument(
                info_span!("agent_chat_stream", agent.name = %agent, agent.model = %model),
            ),
        )
    }

    fn chat_stream_with_cancel<'a>(
        &'a self,
        _message: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move {
                *self.cancel_token.lock().await = Some(cancel.clone());
                // Delegate to the Agent trait's default implementation
                // which wraps chat_stream with cancellation
                <Self as Agent>::chat_stream_with_cancel(self, _message, cancel).await
            }
            .instrument(info_span!("agent_chat_stream_with_cancel", agent.name = %agent, agent.model = %model)),
        )
    }

    fn execute_stream_with_cancel<'a>(
        &'a self,
        _task: &'a str,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move {
                *self.cancel_token.lock().await = Some(cancel.clone());
                <Self as Agent>::execute_stream_with_cancel(self, _task, cancel).await
            }
            .instrument(info_span!("agent_execute_stream_with_cancel", agent.name = %agent, agent.model = %model)),
        )
    }

    fn reset(&self) {
        match self.memory.context.try_lock() {
            Ok(mut ctx) => {
                ctx.clear();
                ctx.push(crate::llm::types::Message::system(
                    self.config.system_prompt.clone(),
                ));
            }
            Err(_) => {
                warn!(
                    agent = %self.config.agent_name,
                    "Cannot reset: context locked by active stream"
                );
            }
        }
    }

    fn tool_names(&self) -> Vec<String> {
        self.tools
            .tool_manager
            .list_tools()
            .into_iter()
            .filter(|n| *n != TOOL_FINAL_ANSWER)
            .map(|n| n.to_string())
            .collect()
    }

    /// Get the list of tool definitions (name, description, parameter schema).
    fn tool_definitions(&self) -> Vec<crate::llm::types::ToolDefinition> {
        self.tools
            .tool_manager
            .get_tool_definitions()
            .into_iter()
            .filter(|d| d.function.name != TOOL_FINAL_ANSWER)
            .collect()
    }

    fn skill_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tools
            .skill_registry
            .list()
            .into_iter()
            .map(|s| s.name.clone())
            .collect();
        // Also include file-based skill names
        for desc in self.tools.skill_registry.list_descriptors() {
            if !names.contains(&desc.name) {
                names.push(desc.name.clone());
            }
        }
        names
    }

    fn mcp_server_names(&self) -> Vec<String> {
        #[cfg(feature = "mcp")]
        {
            self.tools
                .mcp_manager
                .server_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect()
        }
        #[cfg(not(feature = "mcp"))]
        {
            vec![]
        }
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(feature = "mcp")]
            self.tools.mcp_manager.close_all().await;
        })
    }
}

// ── ReactAgent multimodal extension methods ─────────────────────────────────────

impl ReactAgent {
    /// Streaming multi-turn conversation (multimodal message version).
    ///
    /// Same as `chat_stream`, but accepts a pre-built `Message` to support
    /// images, files, and other attachments. Preserves context, suitable for
    /// multi-turn multimodal dialogue.
    pub async fn chat_stream_message(
        &self,
        message: crate::llm::types::Message,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream_with_message(message, run::StreamMode::Chat)
            .await
    }

    /// Streaming task execution (multimodal message version).
    ///
    /// Same as `execute_stream`, but accepts a pre-built `Message` to support
    /// images, files, and other attachments. Resets context, suitable for
    /// single-turn multimodal tasks.
    pub async fn execute_stream_message(
        &self,
        message: crate::llm::types::Message,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream_with_message(message, run::StreamMode::Execute)
            .await
    }

    /// Send a message with an image URL (multimodal).
    ///
    /// Sends the image URL directly as an `image_url` part to the LLM.
    /// If you already have a local file or base64 data, use `chat_multimodal()`
    /// and construct `ImageUrl.url` as `data:image/...;base64,...` yourself.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use echo_agent::prelude::*;
    /// # async fn test() -> echo_agent::error::Result<()> {
    /// # let mut agent = ReactAgentBuilder::new().model("qwen3.5-plus").build()?;
    /// let response = agent.chat_with_image_url(
    ///     "Describe this image",
    ///     "https://example.com/image.jpg"
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn chat_with_image_url(&self, text: &str, image_url: &str) -> Result<String> {
        use crate::llm::types::{ContentPart, ImageUrl, Message};

        let message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: image_url.to_string(),
                    detail: None,
                },
            },
        ]);

        self.chat_multimodal(message).await
    }

    /// Send a multimodal message.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use echo_agent::prelude::*;
    /// # async fn test() -> echo_agent::error::Result<()> {
    /// # let mut agent = ReactAgentBuilder::new().model("qwen3.5-plus").build()?;
    /// use echo_agent::llm::types::{ContentPart, ImageUrl, Message};
    ///
    /// let message = Message::user_multimodal(vec![
    ///     ContentPart::Text { text: "Describe these images".to_string() },
    ///     ContentPart::ImageUrl {
    ///         image_url: ImageUrl {
    ///             url: "https://example.com/img1.jpg".to_string(),
    ///             detail: None,
    ///         },
    ///     },
    ///     ContentPart::ImageUrl {
    ///         image_url: ImageUrl {
    ///             url: "data:image/png;base64,iVBORw0KG...".to_string(),
    ///             detail: None,
    ///         },
    ///     },
    /// ]);
    ///
    /// let response = agent.chat_multimodal(message).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn chat_multimodal(&self, message: crate::llm::types::Message) -> Result<String> {
        use crate::llm::{ChatRequest, chat};

        // Ensure context is initialized (includes system prompt)
        {
            let mut ctx = self.memory.context.lock().await;
            if ctx.messages().is_empty() {
                ctx.push(crate::llm::types::Message::system(
                    self.config.system_prompt.clone(),
                ));
            }
            // Add multimodal user message
            ctx.push(message.clone());
        }

        // Prepare message list
        let messages = {
            let ctx = self.memory.context.lock().await;
            ctx.messages().to_vec()
        };

        let content = if let Some(llm_client) = &self.llm_client {
            let response = llm_client
                .chat(ChatRequest {
                    messages: messages.clone(),
                    temperature: None,
                    max_tokens: None,
                    tools: None,
                    tool_choice: None,
                    response_format: None,
                    cancel_token: None,
                })
                .await?;
            response.content().unwrap_or_default()
        } else {
            let response = chat(
                self.client.clone(),
                &self.config.model_name,
                &messages,
                None,        // temperature
                None,        // max_tokens
                Some(false), // stream
                None,        // tools
                None,        // tool_choice
                None,        // response_format
            )
            .await?;

            response
                .choices
                .first()
                .and_then(|c| c.message.content.as_text())
                .unwrap_or_default()
        };

        // Add assistant reply to context
        self.memory
            .context
            .lock()
            .await
            .push(crate::llm::types::Message::assistant(content.clone()));

        Ok(content)
    }

    /// Execute a task with an image URL (single-turn, resets context).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use echo_agent::prelude::*;
    /// # async fn test() -> echo_agent::error::Result<()> {
    /// # let mut agent = ReactAgentBuilder::new().model("qwen3.5-plus").build()?;
    /// let response = agent
    ///     .execute_with_image_url("Analyze this parking receipt", "https://example.com/receipt.jpg")
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_with_image_url(&self, task: &str, image_url: &str) -> Result<String> {
        use crate::llm::types::{ContentPart, ImageUrl, Message};

        // Reset context
        self.reset_messages().await;

        let message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: task.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: image_url.to_string(),
                    detail: None,
                },
            },
        ]);

        self.chat_multimodal(message).await
    }
}
