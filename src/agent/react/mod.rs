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
#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentRegistry;
#[cfg(feature = "subagent")]
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
use crate::memory::snapshot::{SnapshotManager, StateSnapshot};
use crate::memory::store::{FileStore, Store};
use crate::sandbox::SandboxManager;
use crate::skills::SkillRegistry;
use crate::skills::hooks::HookRegistry;
#[cfg(feature = "tasks")]
use crate::tasks::TaskSpawner;
use crate::tools::ToolManager;
#[cfg(feature = "subagent")]
use crate::tools::builtin::agent_dispatch::AgentDispatchTool;
use crate::tools::builtin::answer::FinalAnswerTool;
#[cfg(feature = "tasks")]
use crate::tools::builtin::check_task::{CheckTaskStatusTool, ListBackgroundTasksTool};
#[cfg(feature = "human-loop")]
use crate::tools::builtin::human_in_loop::HumanInLoop;
use crate::tools::builtin::memory::{
    ForgetTool, LayeredForgetTool, LayeredRecallTool, LayeredRememberTool, LayeredSearchMemoryTool,
    LegacyStoreRememberTool, RecallTool, SearchMemoryTool,
};
#[cfg(feature = "tasks")]
use crate::tools::builtin::spawn_task::SpawnBackgroundTaskTool;
use echo_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{Instrument, info, info_span};

use crate::agent::react::subsystems::approval::ApprovalSubsystem;
use crate::agent::react::subsystems::guard::GuardSubsystem;
use crate::agent::react::subsystems::memory::MemorySubsystem;
use crate::agent::react::subsystems::tool_exec::ToolExecutionSubsystem;

pub mod builder;
mod capabilities;
mod extract;
pub mod loop_detector;
pub mod run;
pub mod structured;
pub(crate) mod subsystems;
#[cfg(test)]
mod tests;
// ── Built-in tool name constants ────────────────────────────────────────────────

pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";

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
    /// Runtime-mutable system prompt override (set via `set_system_prompt` trait method).
    /// When set, this overrides `config.system_prompt` for subsequent turns.
    pub(crate) mutable_system_prompt: std::sync::RwLock<Option<String>>,
    /// Tool execution subsystem: tool registry/execution, Skill, Hook, MCP, SubAgent, Sandbox
    pub(crate) tools: ToolExecutionSubsystem,
    /// Guard & safety subsystem: guards, permission policy, audit logging, circuit breaker
    pub(crate) guard: GuardSubsystem,
    /// Memory & persistence subsystem: context management, long-term memory, snapshots, transcript projection
    pub(crate) memory: MemorySubsystem,
    /// Human-in-the-loop approval subsystem
    #[allow(dead_code)]
    pub(crate) approval: ApprovalSubsystem,
    client: Arc<Client>,
    llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// LLM configuration (optional; falls back to environment variables when not set)
    llm_config: Option<LlmConfig>,
    /// Per-agent thinking-depth / reasoning config, applied to every chat
    /// request issued by this agent (think phase, react loop). `None` means
    /// "use the model's default" — no thinking field is sent. Set from config
    /// (`ModelConfig.thinking`) at agent construction.
    thinking: Option<crate::llm::ThinkingConfig>,
    /// Cancellation token for the current streaming request, set in
    /// `chat_stream_with_cancel` / `execute_stream_with_cancel`.
    /// `create_llm_stream` reads this field and passes it to the HTTP layer
    /// to support request-level stream cancellation.
    /// Uses `tokio::sync::Mutex` to support `&self` streaming methods.
    pub(crate) cancel_token: tokio::sync::Mutex<Option<CancellationToken>>,

    /// Shared handle to the `AgentDispatchTool`'s cancel token (P1-11).
    ///
    /// Mirrors [`cancel_token`] into the LLM-callable dispatch tool so that a
    /// subagent dispatched via the `agent_tool` is cancelled when the parent
    /// run is. Updated alongside `cancel_token` at run start. `None` when
    /// subagents are disabled (`AgentDispatchTool` never registered).
    pub(crate) dispatch_cancel_handle: Option<Arc<tokio::sync::Mutex<Option<CancellationToken>>>>,
    #[cfg(feature = "subagent")]
    pub(crate) dispatch_catalog_handle: Option<
        Arc<std::sync::RwLock<Vec<crate::tools::builtin::agent_dispatch::SubagentCatalogEntry>>>,
    >,

    /// Optional run store for persisting execution traces.
    /// When set, each streaming execution records a [`Run`](crate::trace::Run)
    /// with events, token usage, and timings.
    pub run_store: Option<Arc<dyn crate::trace::RunStore>>,

    /// The currently active run ID. Set at the start of `run_react_loop` or
    /// streaming execution; cleared when the run completes. Used to associate
    /// trace events with the correct run.
    pub current_run_id: std::sync::Mutex<Option<String>>,

    /// 外部 run 级上下文（跨 spawn 安全，值传递）。
    ///
    /// `tokio::task_local!` 不会跨 `tokio::spawn` 继承——worker agent 在框架层
    /// 的 dispatch_fork spawn 里执行时，应用层经 task_local 注入的 run_id /
    /// cancel / trace_sink 全部丢失。这里改用 Mutex 字段承载
    /// （set_external_context 设置，pipeline 构造 ToolContext 时读取），是跨
    /// spawn 安全的值传递通路。
    pub external_cancel:
        std::sync::Mutex<Option<std::sync::Arc<tokio_util::sync::CancellationToken>>>,
    pub external_trace_sink: std::sync::Mutex<Option<echo_core::tools::TraceSinkFn>>,

    /// Optional tool execution pipeline. When set, `execute_tool_feedback_raw`
    /// delegates to this pipeline instead of the inline implementation.
    pub(crate) tool_execution_pipeline: Option<Arc<run::pipeline::ToolExecutionPipeline>>,

    /// Optional prompt template engine for variable substitution.
    /// When set, system prompts can use template syntax (`{{variable}}`)
    /// and the engine resolves them dynamically.
    pub(crate) prompt_template_engine: Option<Arc<echo_core::agent::PromptTemplateManager>>,

    /// Current agent turn (if one is in progress).
    pub(crate) current_turn: std::sync::Mutex<Option<crate::agent::turn::AgentTurn>>,

    /// Tracks absolute file paths that have been successfully read during the
    /// current conversation turn, along with the instant of the read.
    /// Used by the read-before-edit enforcement when `config.force_read_before_edit`
    /// is true. Entries are evicted when they exceed the TTL (30 min) or when
    /// the set exceeds `MAX_READ_FILES`.
    pub(crate) recently_read_files: Arc<std::sync::Mutex<HashMap<String, std::time::Instant>>>,

    /// Serializes all execution (chat, execute, stream) on this agent.
    ///
    /// Only one execution can be active at a time. Both non-streaming
    /// (`run_react_loop`) and streaming (`AgentRunSnapshot::run_loop`)
    /// acquire this mutex at their entry point and hold it for the
    /// full duration. This prevents concurrent access to the
    /// `ContextManager` and other internal mutable state.
    pub(crate) execution_mutex: Arc<tokio::sync::Mutex<()>>,

    /// Thread-safe token usage tracker that accumulates prompt/completion
    /// tokens across all LLM calls in this agent's lifetime.
    /// Prefer reading real usage from API responses; falls back to estimation.
    pub(crate) token_tracker: Arc<echo_core::tokenizer::TokenUsageTracker>,

    /// Self-calibrating tokenizer wrapper that improves estimation accuracy
    /// using actual API token counts. Wrapped in Arc for shared access.
    /// The calibration factor is updated after each LLM call using EMA smoothing.
    pub(crate) calibrated_tokenizer: Arc<echo_core::tokenizer::CalibratedTokenizer>,

    /// Optional intent router for pre-ReAct classification and routing.
    pub(crate) intent_router: Option<crate::intent::IntentRouter>,

    /// Current plan text (set by PlanTool, captured in checkpoints).
    pub(crate) plan_state: Arc<tokio::sync::RwLock<Option<String>>>,

    /// Optional Critic for final_answer verification.
    pub(crate) critic: Option<Arc<dyn echo_core::agent::Critic>>,

    /// Layered memory manager used by runtime-triggered memory writes.
    pub(crate) memory_layer_manager: Option<Arc<crate::evolution::MemoryLayerManager>>,

    /// Runtime state consumed by TriggerDetector between turns.
    pub(crate) memory_trigger_state: Arc<std::sync::Mutex<MemoryTriggerRuntimeState>>,

    /// Shared slot for hook→classifier communication.
    /// Written by prepare phase after UserPromptSubmit hooks resolve
    /// (fire_lifecycle_hook → activate_skill); read by TriggerSupervisor
    /// during intent classification. Consumed once per turn (take).
    pub(crate) hook_activation_cache: Arc<std::sync::Mutex<Option<(String, String)>>>,
}

#[derive(Default)]
pub(crate) struct MemoryTriggerRuntimeState {
    pub last_tool_failure: Option<crate::evolution::ToolFailureRecord>,
    pub last_tool_success: Option<crate::evolution::ToolSuccessRecord>,
    pub tool_sequences: Vec<crate::evolution::ToolSequenceRecord>,
}

// ── Construction & initialization ──────────────────────────────────────────────

impl ReactAgent {
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
    ///
    /// Prefer [`ReactAgentBuilder`] for construction — it handles subsystem
    /// initialization and provides sensible defaults. Direct construction with
    /// [`new`](Self::new) initialises every subsystem eagerly.
    pub fn new(config: AgentConfig) -> Self {
        let system_prompt = Self::build_system_prompt(&config);

        let sp_for_canonical = system_prompt.clone();
        #[cfg(feature = "subagent")]
        let sp_for_subagent = system_prompt.clone();

        // ── CalibratedTokenizer setup ──
        // Wrap HeuristicTokenizer with CalibratedTokenizer for self-improving
        // token estimation. The same Arc is shared with ReactAgent so that
        // runtime calibration (from actual API usage) flows into ContextManager.
        let calibrated_tokenizer = Arc::new(echo_core::tokenizer::CalibratedTokenizer::new(
            Arc::new(echo_core::tokenizer::HeuristicTokenizer),
        ));
        let mut ctx_builder = ContextManager::builder(config.token_limit)
            .with_system(system_prompt)
            .tokenizer(calibrated_tokenizer.clone() as Arc<dyn echo_core::tokenizer::Tokenizer>);

        // Wire TokenBudget if configured
        if config.token_budget_config.enabled {
            let budget = config.token_budget_config.build(config.token_limit);
            ctx_builder = ctx_builder.budget(budget);
        }

        // Set default compressor when token_limit is configured or token budget is enabled.
        // This ensures compression can actually be triggered when the budget check fires.
        if config.token_limit < usize::MAX || config.token_budget_config.enabled {
            use crate::compression::compressor::SlidingWindowCompressor;
            // Keep the most recent messages that fit within the token limit
            // Use a conservative window: keep last 40 messages (roughly 20 turns)
            let compressor = SlidingWindowCompressor::new(40);
            ctx_builder = ctx_builder.compressor(compressor);
        }

        let mut ctx = ctx_builder.build();

        // ── Canonical context auto-wiring ──
        // Build canonical context so system prompt, rules, and skills survive compression
        let canonical = crate::compression::CanonicalContext {
            system_prompt: Some(sp_for_canonical),
            project_rules: {
                #[cfg(feature = "project-rules")]
                {
                    let wd = config
                        .working_dir
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    echo_core::project_rules::rules_injection(&wd)
                }
                #[cfg(not(feature = "project-rules"))]
                None
            },
            skill_injections: Vec::new(),
            active_skill_names: Vec::new(),
        };
        ctx.set_canonical_context(canonical.clone());

        let context = Arc::new(tokio::sync::Mutex::new(ctx));

        let mut tool_manager = ToolManager::new_with_config(config.tool_execution.clone());
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        // ── Core tools ─────────────────────────────────────────────
        tool_manager.register(Box::new(FinalAnswerTool));
        tool_manager.register(Box::new(crate::tools::builtin::todo::TodoWriteTool));

        // ── Subsystem initialization ──────────────────────────────
        #[cfg(feature = "tasks")]
        let task_spawner = Arc::new(TaskSpawner::new(crate::tasks::TaskSpawnerConfig::default()));
        #[cfg(feature = "subagent")]
        let subagent_registry = Arc::new(SubagentRegistry::new());

        // Create hook_registry early so the subagent executor can reference it
        let hook_registry = Arc::new(tokio::sync::RwLock::new(HookRegistry::new()));

        #[cfg(feature = "subagent")]
        let subagent_executor = {
            let hr_clone = hook_registry.clone();
            let unified_executor: crate::skills::hooks::UnifiedHookExecutorFn =
                Arc::new(move |ctx: crate::skills::hooks::HookContext| {
                    let hr = hr_clone.clone();
                    Box::pin(async move {
                        let registry = hr.read().await.clone();
                        registry.run_lifecycle_hooks(&ctx).await
                    })
                });
            Arc::new(SubagentExecutor::new(
                subagent_registry.clone(),
                SubagentExecutorConfig {
                    unified_hook_executor: Some(unified_executor),
                    default_timeout_secs: config.subagent_timeout_secs,
                    worktree_factory: config.subagent_worktree_factory.clone(),
                    ..SubagentExecutorConfig::default()
                },
            ))
        };
        #[cfg(not(feature = "subagent"))]
        let _ = &hook_registry; // suppress unused warning
        #[cfg(feature = "human-loop")]
        let approval_provider = crate::human_loop::default_provider();

        // ── Feature-gated tool registration ───────────────────────
        #[cfg(feature = "human-loop")]
        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop::new(approval_provider.clone())));
        }

        #[cfg(feature = "tasks")]
        if config.enable_task {
            // Background task tools (long-running task support)
            tool_manager.register(Box::new(SpawnBackgroundTaskTool::new(task_spawner.clone())));
            tool_manager.register(Box::new(CheckTaskStatusTool::new(task_spawner.clone())));
            tool_manager.register(Box::new(ListBackgroundTasksTool::new(task_spawner.clone())));
        }
        Self::register_feature_gated_tools(&config, &mut tool_manager);

        // ── Memory store ──────────────────────────────────────────
        let store = Self::setup_memory_store(&config, &mut tool_manager);

        // Wrap tool_manager in Arc for sharing with subsystems and context factory
        let tool_manager = Arc::new(tool_manager);

        // ── AgentDispatch tool (after all other tools + store are ready) ──
        // Context inheritance factory needs the final tool_manager Arc and store.
        // Shared cancel handle so the parent run can push its token into the
        // LLM-callable dispatch tool (P1-11). `None` when subagents disabled.
        #[cfg(feature = "subagent")]
        let mut dispatch_cancel_handle: Option<
            Arc<tokio::sync::Mutex<Option<CancellationToken>>>,
        > = None;
        #[cfg(feature = "subagent")]
        let mut dispatch_catalog_handle: Option<
            Arc<
                std::sync::RwLock<Vec<crate::tools::builtin::agent_dispatch::SubagentCatalogEntry>>,
            >,
        > = None;
        #[cfg(feature = "subagent")]
        if config.register_agent_dispatch_tool {
            let factory = Arc::new(
                crate::tools::builtin::agent_dispatch::ParentContextFactory {
                    system_prompt: sp_for_subagent.clone(),
                    tool_manager: tool_manager.clone(),
                    context: context.clone(),
                    store: store.clone(),
                },
            );
            let dispatch_tool = AgentDispatchTool::new(
                subagent_executor.clone(),
                config.agent_name.clone(),
                CancellationToken::new(),
            )
            .with_parent_context(factory);
            // Capture the shared handle before the tool is moved into the
            // tool_manager, so the agent can update it at run start.
            dispatch_cancel_handle = Some(dispatch_tool.cancel_handle());
            dispatch_catalog_handle = Some(dispatch_tool.catalog_handle());
            tool_manager.register(Box::new(dispatch_tool));
        }

        let model_name = config.model_name.clone();

        Self {
            config,
            tools: ToolExecutionSubsystem {
                tool_manager: tool_manager.clone(),
                #[cfg(feature = "subagent")]
                subagent_registry,
                #[cfg(feature = "subagent")]
                subagent_executor,
                skill_registry: SkillRegistry::new(),
                progressive_skill_registry: None,
                hook_registry,
                #[cfg(feature = "mcp")]
                mcp_manager: McpManager::new(),
                sandbox_manager: None,
                intervention_callbacks: Vec::new(),
            },
            guard: GuardSubsystem {
                guard_manager: None,
                audit_logger: None,
                circuit_breaker: None,
            },
            memory: MemorySubsystem {
                context,
                store,
                snapshot_manager: Arc::new(std::sync::RwLock::new(None)),
                conversation_store: None,
                state_store: None,
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
            thinking: None,
            cancel_token: tokio::sync::Mutex::new(None),
            #[cfg(feature = "subagent")]
            dispatch_cancel_handle,
            #[cfg(not(feature = "subagent"))]
            dispatch_cancel_handle: None,
            #[cfg(feature = "subagent")]
            dispatch_catalog_handle,
            run_store: None,
            current_run_id: std::sync::Mutex::new(None),
            external_cancel: std::sync::Mutex::new(None),
            external_trace_sink: std::sync::Mutex::new(None),
            tool_execution_pipeline: None,
            prompt_template_engine: None,
            current_turn: std::sync::Mutex::new(None),
            recently_read_files: Arc::new(std::sync::Mutex::new(HashMap::new())),
            mutable_system_prompt: std::sync::RwLock::new(None),
            execution_mutex: Arc::new(tokio::sync::Mutex::new(())),
            token_tracker: Arc::new(echo_core::tokenizer::TokenUsageTracker::new(model_name)),
            calibrated_tokenizer: Arc::new(echo_core::tokenizer::CalibratedTokenizer::new(
                Arc::new(echo_core::tokenizer::HeuristicTokenizer),
            )),
            intent_router: None,
            plan_state: Arc::new(tokio::sync::RwLock::new(None)),
            critic: None,
            memory_layer_manager: None,
            memory_trigger_state: Arc::new(std::sync::Mutex::new(
                MemoryTriggerRuntimeState::default(),
            )),
            hook_activation_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Replace default memory tools with layered memory tools backed by the
    /// main runtime memory layer manager.
    pub fn install_memory_layer_manager(
        &mut self,
        layer_manager: Arc<crate::evolution::MemoryLayerManager>,
    ) {
        self.tools
            .tool_manager
            .register(Box::new(LayeredRememberTool::new(layer_manager.clone())));
        self.tools
            .tool_manager
            .register(Box::new(LayeredRecallTool::new(layer_manager.clone())));
        self.tools
            .tool_manager
            .register(Box::new(LayeredSearchMemoryTool::new(
                layer_manager.clone(),
            )));
        self.tools
            .tool_manager
            .register(Box::new(LayeredForgetTool::new(layer_manager.clone())));
        self.memory_layer_manager = Some(layer_manager);
    }

    /// Whether this agent has the layered memory runtime installed.
    pub fn has_memory_layer_manager(&self) -> bool {
        self.memory_layer_manager.is_some()
    }

    /// (stage4 F1) Access the shared layer manager so app-side write paths
    /// (e.g. session-end auto-memory) route through the same instance the agent
    /// uses — shared security guard, audit log, and write counter/observer
    /// (割裂点 6: previously app paths built a fresh per-call manager that
    /// bypassed the agent's shared instance).
    pub fn memory_layer_manager(&self) -> Option<&Arc<crate::evolution::MemoryLayerManager>> {
        self.memory_layer_manager.as_ref()
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
        // ── Stable prefix: NEVER include per-request or per-session data ──
        // CWD, workspace info, timestamps, run_id, memory recall etc. belong
        // in the user message, not the system prompt. The system prefix is
        // what provider-side prompt caching (DeepSeek KVCache, Anthropic
        // prompt cache, OpenAI prefix cache) keys on — any change to it
        // invalidates the entire cache.
        let prompt = if config.enable_tool && config.enable_cot {
            format!(
                "{}\n\n{}",
                config.system_prompt.trim_end(),
                Self::COT_INSTRUCTION,
            )
        } else {
            config.system_prompt.clone()
        };

        // Project rules are loaded from the workspace and are stable per
        // project. They can stay in the system prompt since they don't
        // change between requests within the same workspace.
        #[cfg(feature = "project-rules")]
        let mut prompt = prompt;
        #[cfg(feature = "project-rules")]
        if config.auto_project_rules {
            let wd = config
                .working_dir
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            prompt = echo_core::project_rules::inject_rules(&prompt, &wd);
        }

        prompt
    }

    /// Build a workspace context block to be injected into the first user message
    /// (NOT the system prompt). This keeps the system prefix cache-stable across
    /// workspace changes.
    pub fn build_workspace_context_block(working_dir: Option<&std::path::PathBuf>) -> String {
        let mut parts = Vec::new();
        if let Some(wd) = working_dir {
            parts.push(format!("- root: {}", wd.display()));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("[workspace]\n{}\n[/workspace]\n", parts.join("\n"))
        }
    }

    fn register_feature_gated_tools(config: &AgentConfig, tool_manager: &mut ToolManager) {
        if config.enable_tool {
            if config.readonly_tools {
                echo_tools::register_readonly_tools(tool_manager);
            } else {
                echo_tools::register_all_tools(tool_manager);
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
                tool_manager.register(Box::new(LegacyStoreRememberTool::new(
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
        self.set_llm_config(config);
        self
    }

    /// Inject a custom LLM client.
    pub fn with_llm_client(mut self, client: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.config.model_name = client.model_name().to_string();
        self.llm_client = Some(client);
        self
    }

    /// Set the LLM configuration.
    ///
    /// Builds an LLM client from the config and sets it so that subsequent API
    /// calls use the provided credentials instead of falling back to environment
    /// variables or YAML model configuration files.
    pub fn set_llm_config(&mut self, config: LlmConfig) {
        self.config.model_name = config.model.clone();
        // Try to build a client from the config. If it succeeds, set it so the
        // runtime uses these credentials. If it fails (e.g. invalid API key),
        // leave llm_client as None so the runtime falls back to env vars /
        // echo-agent-models.yaml.
        match config.build_client() {
            Ok(client) => {
                tracing::info!(
                    model = %config.model,
                    "LLM client built from LlmConfig, credential injection active"
                );
                self.llm_client = Some(Arc::from(client));
            }
            Err(e) => {
                tracing::warn!(
                    model = %config.model,
                    error = %e,
                    "Failed to build LLM client from LlmConfig, will fall back to env vars / models.yaml"
                );
            }
        }
        self.llm_config = Some(config);
    }

    /// Set a custom LLM client.
    pub fn set_llm_client(&mut self, client: Arc<dyn crate::llm::LlmClient>) {
        self.config.model_name = client.model_name().to_string();
        self.llm_client = Some(client);
    }

    /// Set the working directory for the agent.
    ///
    /// Affects project-rules file lookup AND, via `RuntimeConfig` →
    /// `ExecuteStage` → `ToolContext`, the cwd of every tool call (shell,
    /// file, git) — so binding a worktree path here isolates that agent's
    /// file operations. Pass `None` to clear (fall back to process cwd).
    pub fn set_working_dir(&self, path: Option<std::path::PathBuf>) {
        let updated = match self.config.working_dir.lock() {
            Ok(mut working_dir) => {
                *working_dir = path;
                true
            }
            Err(err) => {
                tracing::warn!(error = %err, "Could not update agent working directory");
                false
            }
        };
        if updated {
            self.refresh_root_system_prompt();
        }
    }

    fn refresh_root_system_prompt(&self) {
        let system_prompt = Self::build_system_prompt(&self.config);
        if let Ok(mut ctx) = self.memory.context.try_lock() {
            let mut messages = ctx.messages().to_vec();
            if let Some(system) = messages
                .iter_mut()
                .find(|message| matches!(message.role, echo_core::llm::types::Role::System))
            {
                *system = echo_core::llm::types::Message::system(system_prompt.clone());
            } else {
                messages.insert(
                    0,
                    echo_core::llm::types::Message::system(system_prompt.clone()),
                );
            }
            ctx.set_messages(messages);
            ctx.set_canonical_system_prompt(Some(system_prompt));
        } else {
            tracing::warn!(
                "Could not acquire ContextManager lock to refresh working-dir system prompt; \
                 the next agent rebuild will include the updated cwd"
            );
        }
    }

    /// Get the current LLM configuration.
    pub fn llm_config(&self) -> Option<&LlmConfig> {
        self.llm_config.as_ref()
    }

    /// Get a reference to the LLM client (if set).
    pub fn llm_client(&self) -> Option<&Arc<dyn crate::llm::LlmClient>> {
        self.llm_client.as_ref()
    }

    /// Get the agent's thinking-depth config (if any).
    pub fn thinking(&self) -> Option<&crate::llm::ThinkingConfig> {
        self.thinking.as_ref()
    }

    /// Set the agent's thinking-depth config. Applied to every chat request
    /// issued by this agent. Set from `ModelConfig.thinking` at construction
    /// or when the user changes the model's reasoning depth at runtime.
    pub fn set_thinking(&mut self, thinking: Option<crate::llm::ThinkingConfig>) {
        self.thinking = thinking;
    }

    // ── Accessors & setters ──────────────────────────────────────────────────────

    /// Get a read-only reference to the AgentConfig.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Public read-only access to the hook activation cache.
    /// Used by bootstrap (echo-agent-cli) to pass the slot to TriggerSupervisor
    /// so the supervisor can read UserPromptSubmit hook activation requests.
    pub fn hook_activation_cache(&self) -> Arc<std::sync::Mutex<Option<(String, String)>>> {
        self.hook_activation_cache.clone()
    }

    /// Public read-only access to the effective system prompt.
    /// Returns the runtime override (mutable_system_prompt) if set, else
    /// config.system_prompt. Used by bootstrap (echo-agent-cli) to read the
    /// current prompt before injecting methodology baseline bodies.
    /// 返回 owned String 以避开 RwLock 借用问题。
    pub fn current_system_prompt(&self) -> String {
        if let Ok(guard) = self.mutable_system_prompt.read()
            && let Some(ref override_prompt) = *guard
        {
            return override_prompt.clone();
        }
        self.config.system_prompt.clone()
    }

    /// Mutable reference to the agent config for runtime adjustments.
    pub fn config_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }

    /// Get the cumulative token usage summary for this agent.
    ///
    /// Tracks real token counts from API responses (OpenAI, Anthropic, Ollama, etc.).
    /// Falls back to zero when the provider does not return usage information.
    pub fn token_usage_summary(&self) -> echo_core::tokenizer::UsageSummary {
        self.token_tracker.summary()
    }

    /// Get a reference to the token usage tracker for external recording.
    pub fn token_tracker(&self) -> &Arc<echo_core::tokenizer::TokenUsageTracker> {
        &self.token_tracker
    }

    /// Get a reference to the self-calibrating tokenizer.
    ///
    /// The calibration factor improves over time as actual API token counts
    /// are fed back via `calibrate()`.
    pub fn calibrated_tokenizer(&self) -> &Arc<echo_core::tokenizer::CalibratedTokenizer> {
        &self.calibrated_tokenizer
    }

    /// Inject a custom long-term memory Store (replaces the injection channel only; does not re-register tools).
    ///
    /// Also rewires L3 compression promotion to the same Store so evicted context
    /// and future recall queries stay on one backing store. This synchronous
    /// setter is best called during construction; if the context lock is held,
    /// use [`Self::install_store`] from async code.
    pub fn set_store(&mut self, store: Arc<dyn Store>) {
        self.memory.store = Some(store.clone());
        if let Ok(mut ctx) = self.memory.context.try_lock() {
            let promoter = Arc::new(crate::memory_promoter::StoreMemoryPromoter::new(store));
            ctx.set_memory_promoter(promoter);
        } else {
            tracing::warn!(
                "Could not acquire ContextManager lock to set memory promoter; \
                 use install_store() from an async context if the agent is already running"
            );
        }
    }

    /// Async variant of [`Self::set_store`] that safely rewires L3 promotion
    /// after the agent has started running.
    pub async fn install_store(&mut self, store: Arc<dyn Store>) {
        self.memory.store = Some(store.clone());
        let promoter = Arc::new(crate::memory_promoter::StoreMemoryPromoter::new(store));
        self.memory
            .context
            .lock()
            .await
            .set_memory_promoter(promoter);
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
        if let Some(layer_manager) = &self.memory_layer_manager {
            self.tools
                .tool_manager
                .register(Box::new(LayeredRememberTool::new(layer_manager.clone())));
            self.tools
                .tool_manager
                .register(Box::new(LayeredRecallTool::new(layer_manager.clone())));
            self.tools
                .tool_manager
                .register(Box::new(LayeredSearchMemoryTool::new(
                    layer_manager.clone(),
                )));
            self.tools
                .tool_manager
                .register(Box::new(LayeredForgetTool::new(layer_manager.clone())));
        } else {
            self.tools
                .tool_manager
                .register(Box::new(LegacyStoreRememberTool::new(
                    store.clone(),
                    ns.clone(),
                )));
            self.tools
                .tool_manager
                .register(Box::new(RecallTool::new(store.clone(), ns.clone())));
            self.tools
                .tool_manager
                .register(Box::new(SearchMemoryTool::new(store.clone(), ns.clone())));
        }
        self.tools
            .tool_manager
            .register(Box::new(ForgetTool::new(store.clone(), ns)));
        self.memory.store = Some(store.clone());

        // ── L3 Memory Promotion ──
        // Wire a StoreMemoryPromoter into the ContextManager so that
        // messages evicted during compression are promoted to long-term memory.
        //
        // `try_lock` is correct here — this synchronous setter is meant to be
        // called during build / before any task holds the context lock. If an
        // agent has already started running, callers should use
        // [`Self::install_memory_store`] instead, which awaits the lock.
        if let Ok(mut ctx) = self.memory.context.try_lock() {
            let promoter = Arc::new(crate::memory_promoter::StoreMemoryPromoter::new(store));
            ctx.set_memory_promoter(promoter);
        } else {
            tracing::warn!(
                "Could not acquire ContextManager lock to set memory promoter; \
                 use install_memory_store() from an async context if the agent is already running"
            );
        }
    }

    /// Async variant of [`Self::set_memory_store`] safe to call after the
    /// agent has started running (e.g. while another task is holding the
    /// context lock). Awaits the `ContextManager` mutex instead of using
    /// `try_lock`, so the `MemoryPromoter` and tool registrations always
    /// take effect — no silent fallback.
    pub async fn install_memory_store(&mut self, store: Arc<dyn Store>) {
        let ns = vec![self.config.agent_name.clone(), "memories".to_string()];
        if let Some(layer_manager) = &self.memory_layer_manager {
            self.tools
                .tool_manager
                .register(Box::new(LayeredRememberTool::new(layer_manager.clone())));
            self.tools
                .tool_manager
                .register(Box::new(LayeredRecallTool::new(layer_manager.clone())));
            self.tools
                .tool_manager
                .register(Box::new(LayeredSearchMemoryTool::new(
                    layer_manager.clone(),
                )));
            self.tools
                .tool_manager
                .register(Box::new(LayeredForgetTool::new(layer_manager.clone())));
        } else {
            self.tools
                .tool_manager
                .register(Box::new(LegacyStoreRememberTool::new(
                    store.clone(),
                    ns.clone(),
                )));
            self.tools
                .tool_manager
                .register(Box::new(RecallTool::new(store.clone(), ns.clone())));
            self.tools
                .tool_manager
                .register(Box::new(SearchMemoryTool::new(store.clone(), ns.clone())));
        }
        self.tools
            .tool_manager
            .register(Box::new(ForgetTool::new(store.clone(), ns)));
        self.memory.store = Some(store.clone());

        let promoter = Arc::new(crate::memory_promoter::StoreMemoryPromoter::new(store));
        self.memory
            .context
            .lock()
            .await
            .set_memory_promoter(promoter);
    }

    /// Set canonical context sources for re-injection after compression.
    ///
    /// When set, system prompt, project rules, and skill injections are
    /// automatically re-injected if compression evicts them from context.
    ///
    /// Synchronous — best called during build. If the agent is already
    /// running, use [`Self::install_canonical_context`] from async code.
    pub fn set_canonical_context(&self, context: crate::compression::CanonicalContext) {
        if let Ok(mut ctx) = self.memory.context.try_lock() {
            ctx.set_canonical_context(context);
        } else {
            tracing::warn!(
                "Could not acquire ContextManager lock to set canonical context; \
                 use install_canonical_context() from an async context"
            );
        }
    }

    /// Async variant of [`Self::set_canonical_context`].
    pub async fn install_canonical_context(&self, context: crate::compression::CanonicalContext) {
        self.memory
            .context
            .lock()
            .await
            .set_canonical_context(context);
    }

    /// Get a read-only reference to the current long-term memory Store.
    pub fn store(&self) -> Option<&Arc<dyn Store>> {
        self.memory.store.as_ref()
    }

    /// Set the conversation_id used for conversation history projection.
    pub fn set_conversation_id(&mut self, conversation_id: impl Into<String>) {
        self.config.conversation_id = Some(conversation_id.into());
    }

    /// Get the current conversation_id for conversation history projection.
    pub fn conversation_id(&self) -> Option<&str> {
        self.config.get_conversation_id()
    }

    /// Clear the working directory binding (e.g. `/worktree exit`). Subsequent
    /// tool calls fall back to the process cwd. (Setting it uses
    /// [`set_working_dir`](Self::set_working_dir) with `Some(path)`.)
    pub fn clear_working_dir(&self) {
        self.set_working_dir(None);
    }

    /// The current working directory binding, if any.
    pub fn working_dir(&self) -> Option<std::path::PathBuf> {
        self.config.working_dir.lock().unwrap().clone()
    }

    /// Get the current conversation history messages (read-only).
    pub async fn get_messages(&self) -> Vec<crate::llm::types::Message> {
        self.memory.context.lock().await.messages().to_vec()
    }

    /// Get the list of registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.tool_manager.list_tools()
    }

    /// Get the list of registered Skill names.
    pub fn skill_names(&self) -> Vec<String> {
        self.tools
            .skill_registry
            .list()
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    /// Get all registered file-based skill descriptors.
    ///
    /// Returns the full [`SkillDescriptor`] list including triggers,
    /// allowed-tools, and other frontmatter metadata.
    pub fn skill_descriptors(
        &self,
    ) -> Vec<echo_execution::skills::external::types::SkillDescriptor> {
        self.tools
            .skill_registry
            .list_descriptors()
            .into_iter()
            .cloned()
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

    /// Wire up the MCP tool executor for `HookAction::McpTool` hook actions
    /// (see `echo_execution::skills::hooks::HookAction`).
    ///
    /// Call this **after** connecting MCP servers via
    /// [`connect_mcp_from_config`](Self::connect_mcp_from_config) or
    /// [`load_mcp_from_file`](Self::load_mcp_from_file).
    /// Call again after connecting additional servers to refresh the client snapshot.
    #[cfg(feature = "mcp")]
    pub fn setup_hook_mcp_executor(&mut self) {
        use crate::skills::hooks::McpExecutorFn;
        use std::sync::Arc;

        let clients = self.tools.mcp_manager.get_clients();
        let executor: McpExecutorFn = Arc::new(move |server, tool, args| {
            let client = clients.get(&server).cloned();
            Box::pin(async move {
                match client {
                    Some(c) => match c.call_tool(&tool, args.unwrap_or_default()).await {
                        Ok(result) => {
                            let mut hook_result = crate::skills::hooks::HookResult::default();
                            let output = serde_json::to_string(&result).unwrap_or_default();
                            hook_result
                                .messages
                                .push(format!("McpTool {}::{} => {}", server, tool, output));
                            hook_result.metadata =
                                Some(serde_json::to_value(&result).unwrap_or_default());
                            hook_result
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %server,
                                tool = %tool,
                                error = %e,
                                "McpTool hook call failed"
                            );
                            crate::skills::hooks::HookResult::default()
                        }
                    },
                    None => {
                        tracing::warn!(
                            server = %server,
                            tool = %tool,
                            "McpTool hook: server not found"
                        );
                        crate::skills::hooks::HookResult::default()
                    }
                }
            })
        });

        if let Ok(mut hooks) = self.tools.hook_registry.try_write() {
            hooks.set_mcp_executor(executor);
        } else {
            tracing::error!("Failed to acquire hook_registry lock for MCP executor setup");
        }
    }

    /// Enable the circuit breaker.
    ///
    /// Automatically trips after consecutive LLM failures reach the threshold,
    /// then probes for recovery after the configured timeout.
    pub fn set_circuit_breaker(&mut self, config: CircuitBreakerConfig) {
        self.guard.circuit_breaker = Some(Arc::new(CircuitBreaker::new(config)));
    }

    /// Set the prompt template engine for dynamic prompt variable substitution.
    ///
    /// When set, the agent can use template syntax (`{{variable}}`) in system
    /// prompts and the engine resolves them dynamically at render time.
    pub fn set_prompt_template_engine(
        &mut self,
        engine: Arc<echo_core::agent::PromptTemplateManager>,
    ) {
        self.prompt_template_engine = Some(engine);
    }

    /// Set the guard manager.
    pub fn set_guard_manager(&mut self, manager: GuardManager) {
        self.guard.guard_manager = Some(manager);
    }

    #[cfg(feature = "human-loop")]
    /// Set the unified permission service.
    pub fn set_permission_service(&mut self, service: Arc<PermissionService>) {
        self.approval.permission_service = Some(service);
    }

    #[cfg(feature = "human-loop")]
    /// Build and set a unified PermissionService from the approval provider.
    pub fn build_permission_service(&mut self) {
        use crate::human_loop::service::PermissionService;

        let provider = self.approval.approval_provider.clone();
        let service = PermissionService::from_provider(provider);
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

    // ── Pool resource injection setters ─────────────────────────────────────
    //
    // These setters allow the product layer to replace internally-created
    // resources with shared (Arc-cloned) instances from an AgentPool.

    /// Replace the tool manager with a shared instance (for AgentPool).
    pub fn set_tool_manager(&mut self, tm: Arc<echo_execution::tools::ToolManager>) {
        self.tools.tool_manager = tm;
    }

    /// Get the subagent registry (for the Tauri subagent-event bridge to
    /// forward dispatch lifecycle events to the frontend).
    #[cfg(feature = "subagent")]
    pub fn subagent_registry(&self) -> &Arc<crate::agent::subagent::SubagentRegistry> {
        &self.tools.subagent_registry
    }

    /// Replace the hook registry with a shared instance (for AgentPool).
    pub fn set_hook_registry(
        &mut self,
        hr: Arc<tokio::sync::RwLock<crate::skills::hooks::HookRegistry>>,
    ) {
        self.tools.hook_registry = hr;
    }

    /// Replace the token usage tracker with a shared instance (for AgentPool).
    pub fn set_token_tracker(&mut self, tt: Arc<echo_core::tokenizer::TokenUsageTracker>) {
        self.token_tracker = tt;
    }

    /// Replace the tool execution pipeline with a shared instance (for AgentPool).
    pub fn set_tool_execution_pipeline(
        &mut self,
        pipeline: Arc<run::pipeline::ToolExecutionPipeline>,
    ) {
        self.tool_execution_pipeline = Some(pipeline);
    }

    /// Replace the runtime state store with a shared instance (for AgentPool).
    pub fn set_state_store(&mut self, store: Arc<dyn crate::state::RuntimeStateStore>) {
        self.memory.state_store = Some(store);
    }

    /// Replace the run store with a shared instance (for AgentPool).
    pub fn set_run_store(&mut self, store: Arc<dyn crate::trace::RunStore>) {
        self.run_store = Some(store);
    }

    /// Get the run store, if configured.
    pub fn run_store(&self) -> Option<&Arc<dyn crate::trace::RunStore>> {
        self.run_store.as_ref()
    }

    /// Set the intent router for pre-ReAct intent classification.
    pub fn set_intent_router(&mut self, router: crate::intent::IntentRouter) {
        self.intent_router = Some(router);
    }

    /// Set the Critic for final_answer verification.
    ///
    /// When set and `config.verifier_enabled` is true, the agent will evaluate
    /// each final_answer with the Critic before accepting it. If the score is
    /// below `config.verifier_min_score`, the feedback is injected as a system
    /// message and the agent continues iterating to self-correct.
    pub fn set_critic(&mut self, critic: Arc<dyn echo_core::agent::Critic>) {
        self.critic = Some(critic);
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
    ///
    /// Uses `PermissionService::replace_provider` to swap the handler **in place**,
    /// preserving all existing configuration (mode, bypass_disabled, classifier,
    /// audit_sink, protected_paths, rules) and clearing the approval cache so
    /// stale approvals from the old provider don't carry over.
    pub fn set_human_loop_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.approval.approval_provider = provider.clone();
        // 原地替换 PermissionService 的 handler，不重建整个服务——
        // 避免丢失 mode/bypass_disabled/classifier/audit_sink 等配置。
        #[cfg(feature = "human-loop")]
        if let Some(ref service) = self.approval.permission_service {
            service.replace_provider(provider.clone());
        }
        if self.tools.tool_manager.get_tool("human_in_loop").is_some() {
            self.tools
                .tool_manager
                .register(Box::new(HumanInLoop::new(provider)));
        }
    }

    #[cfg(feature = "human-loop")]
    /// Replace the HumanLoopProvider transport without clearing session approvals.
    ///
    /// Desktop GUI installs a run-scoped provider for each message so approval
    /// responses can be routed back to the right window/conversation. That
    /// transport swap should not invalidate "approve for this session".
    pub fn set_human_loop_provider_preserving_approvals(
        &mut self,
        provider: Arc<dyn HumanLoopProvider>,
    ) {
        self.approval.approval_provider = provider.clone();
        if let Some(ref service) = self.approval.permission_service {
            service.replace_provider_preserving_cache(provider.clone());
        }
        if self.tools.tool_manager.get_tool("human_in_loop").is_some() {
            self.tools
                .tool_manager
                .register(Box::new(HumanInLoop::new(provider)));
        }
    }

    // ── Conversation persistence ──────────────────────────────────────────────────

    /// Add an intervention callback that can influence agent behavior.
    ///
    /// Intervention callbacks are checked before tool execution, LLM reasoning,
    /// and final answers. They can block actions, inject context, redirect
    /// execution, modify tool arguments, or cancel the entire run.
    ///
    /// Unlike `AgentCallback` (which is observational), `InterventionCallback`
    /// can *influence* the agent's decisions.
    pub fn add_intervention_callback(
        &mut self,
        callback: Arc<dyn crate::agent::InterventionCallback>,
    ) {
        self.tools.intervention_callbacks.push(callback);
    }

    /// Set the conversation history projection Store.
    ///
    /// When enabled, the agent projects the current transcript into a
    /// `ConversationStore` alongside thread-state persistence, for history
    /// browsing and product-layer queries.
    ///
    /// Note: this feature requires an explicit, separate `conversation_id`;
    /// `session_id` is only used for thread-state recovery, not as a fallback
    /// for history projection.
    pub fn set_conversation_store(&mut self, store: Arc<dyn crate::memory::ConversationStore>) {
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

    /// Resume agent state from a [`RuntimeStateStore`](crate::state::RuntimeStateStore) checkpoint.
    ///
    /// Loads the most recent [`AgentCheckpoint`](crate::state::AgentCheckpoint) for
    /// the configured `conversation_id`, deserializes the saved messages, and
    /// restores them into the context manager.
    ///
    /// Returns the checkpoint metadata (plan, skills, blocked_reason) if a
    /// checkpoint was found and restored, or `None` if no state store is
    /// configured or no checkpoint exists.
    pub async fn resume_from_state_store(&self) -> Result<Option<crate::state::AgentCheckpoint>> {
        let Some(ref store) = self.memory.state_store else {
            return Ok(None);
        };
        let Some(ref conv_id) = self.config.conversation_id else {
            tracing::debug!("resume_from_state_store: no conversation_id configured");
            return Ok(None);
        };

        let checkpoint = store.get_checkpoint(conv_id).await?;
        if let Some(ref cp) = checkpoint {
            let messages: Vec<crate::llm::types::Message> = serde_json::from_str(&cp.messages_json)
                .map_err(|e| {
                    crate::error::ReactError::RuntimeState(Box::new(
                        echo_core::error::RuntimeStateError::SerializationError(format!(
                            "Failed to deserialize checkpoint messages: {}",
                            e
                        )),
                    ))
                })?;

            let msg_count = messages.len();
            self.memory.context.lock().await.set_messages(messages);

            // Restore plan state
            if let Some(ref plan) = cp.current_plan {
                *self.plan_state.write().await = Some(plan.clone());
                tracing::debug!(plan_len = plan.len(), "Restored plan state from checkpoint");
            }

            // Re-activate skills
            for skill_name in &cp.active_skills {
                self.tools.skill_registry.mark_activated(skill_name);
            }
            if !cp.active_skills.is_empty() {
                tracing::debug!(
                    skills = ?cp.active_skills,
                    "Re-activated skills from checkpoint"
                );
            }

            // Hydrate any Running TaskNodes (mark them as Hydrated for resume)
            let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(self);
            snapshot.hydrate_running_nodes().await;

            // Restore working directory for worktree-isolated sessions (N-P1-7, BUG-3)
            if let Some(ref wd) = cp.working_dir {
                self.set_working_dir(Some(wd.clone()));
                tracing::debug!(?wd, "Restored working_dir from checkpoint");
            }

            // Log blocked reason if any
            if let Some(ref reason) = cp.blocked_reason {
                tracing::info!(
                    reason = %reason,
                    "Resumed with blocked state from checkpoint"
                );
            }

            tracing::info!(
                conversation_id = conv_id.as_str(),
                message_count = msg_count,
                blocked_reason = ?cp.blocked_reason,
                "Resumed from RuntimeStateStore checkpoint"
            );
        } else {
            tracing::debug!(
                conversation_id = conv_id.as_str(),
                "No checkpoint found in RuntimeStateStore"
            );
        }
        Ok(checkpoint)
    }

    /// Force-save a runtime checkpoint immediately.
    ///
    /// Useful for user-initiated checkpoint saves (e.g., `/checkpoint` command).
    /// Silently no-ops if no state store or conversation_id is configured.
    pub async fn force_checkpoint(&self) {
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(self);
        snapshot
            .save_runtime_checkpoint(&self.memory.context, None)
            .await;
    }

    #[allow(dead_code)]
    const MAX_READ_FILES: usize = 1024;
    /// TTL for recently-read-file entries (30 minutes).
    #[allow(dead_code)]
    const READ_FILES_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

    /// Record that a file was successfully read (for read-before-edit enforcement).
    /// Caps at MAX_READ_FILES entries to prevent unbounded growth in long sessions.
    /// Entries exceeding the TTL are lazily evicted.
    #[allow(dead_code)]
    pub(crate) fn record_file_read(&self, path: &str) {
        if self.config.force_read_before_edit {
            let mut files = self
                .recently_read_files
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            files.insert(path.to_string(), std::time::Instant::now());
            // Evict expired entries first
            let ttl = Self::READ_FILES_TTL;
            files.retain(|_, instant| instant.elapsed() < ttl);
            // Cap: if still over limit, remove oldest entries
            if files.len() > Self::MAX_READ_FILES {
                let mut entries: Vec<(String, std::time::Instant)> = files.drain().collect();
                entries.sort_by_key(|(_, t)| *t);
                let keep = entries.into_iter().rev().take(Self::MAX_READ_FILES);
                files.extend(keep);
            }
        }
    }

    /// Check whether a file was read in the current conversation turn and is
    /// still within the TTL window.
    /// Returns `true` if read-before-edit is disabled, or if the path was
    /// previously recorded via [`record_file_read`] and hasn't expired.
    #[allow(dead_code)]
    pub(crate) fn was_file_read(&self, path: &str) -> bool {
        if !self.config.force_read_before_edit {
            return true; // enforcement disabled — allow all
        }
        let mut files = self
            .recently_read_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match files.get(path) {
            Some(instant) if instant.elapsed() < Self::READ_FILES_TTL => true,
            Some(_) => {
                // Expired — remove it
                files.remove(path);
                false
            }
            None => false,
        }
    }

    /// Clear the read-files set at the start of a new conversation turn.
    pub(crate) fn clear_read_files(&self) {
        self.recently_read_files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Entry point for text-based streaming: build `StreamInit` and delegate
    /// to `run_stream_channel`.
    ///
    /// `clear_read_files` is now done inside `prepare_stream_context`
    /// (converged with the non-streaming path), so it is no longer cleared
    /// here to avoid a double clear.
    async fn run_stream_entry(
        &self,
        input: &str,
        mode: run::StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        self.run_stream_channel(
            run::types::StreamInit {
                text: input.to_string(),
                message: None,
                label: String::new(),
            },
            mode,
        )
        .await
    }

    /// Entry point for multimodal streaming: build `StreamInit` and delegate
    /// to `run_stream_channel`. (`clear_read_files` is done in prepare.)
    async fn run_stream_message_entry(
        &self,
        message: crate::llm::types::Message,
        mode: run::StreamMode,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let text = message.content.as_text().unwrap_or_default();
        self.run_stream_channel(
            run::types::StreamInit {
                text,
                message: Some(message),
                label: "(multimodal)".to_string(),
            },
            mode,
        )
        .await
    }

    // ── Trace / run recording ─────────────────────────────────────────────────

    /// Record a trace event to the current run (if a run store is attached).
    /// Also publishes trace lifecycle to global event bus for audit subscribers.
    pub(crate) async fn record_trace_event(&self, event: crate::trace::RunEvent) {
        let run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let (Some(store), Some(run_id)) = (&self.run_store, &run_id)
            && let Err(e) = store.append_event(run_id, event).await
        {
            tracing::warn!(error = %e, run_id = %run_id, "Failed to append trace event");
        }
    }

    /// Start a new trace run and set it as the current run.
    ///
    /// **Does NOT overwrite** a run_id already set by `set_external_context`.
    /// The product layer (e.g. `launch_unified_run`) calls
    /// `set_external_context` to inject the TaskRuntime run_id BEFORE the
    /// agent's ReAct loop starts. If we overwrote it here with a trace-only
    /// `run_{uuid}`, downstream tools (`task_create`, `execute_plan`) would
    /// read the trace id from `current_run_id` and operate on the wrong
    /// TaskRuntime store entry (no matching `RunCreated` event → plan.json
    /// never materialises → "stuck in executing" bug).
    pub(crate) async fn start_trace_run(&self, input: &str) {
        if let Some(ref store) = self.run_store {
            // If the product layer already set a run_id via set_external_context,
            // keep it — trace events will be recorded under that id, which is
            // the correct TaskRuntime run_id.
            let existing = self
                .current_run_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if existing.is_some() {
                return;
            }
            let run_id = format!("run_{}", uuid::Uuid::new_v4());
            let run = crate::trace::Run {
                run_id: run_id.clone(),
                parent_run_id: None,
                session_id: self.config.session_id.clone().unwrap_or_default(),
                status: crate::trace::RunStatus::Running,
                input: input.to_string(),
                events: vec![],
                final_output: None,
                error: None,
                token_usage: crate::trace::TokenUsage::default(),
                timings: crate::trace::RunTimings::default(),
                started_at: chrono::Utc::now(),
                finished_at: None,
            };
            *self
                .current_run_id
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(run_id);
            if let Err(e) = store.save(run).await {
                tracing::warn!(error = %e, "Failed to save trace run on start");
            }
        }
    }

    /// Finalize the current trace run (completed or failed).
    #[allow(dead_code)]
    pub(crate) async fn finalize_trace_run(
        &self,
        status: crate::trace::RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) {
        let run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let (Some(store), Some(run_id)) = (&self.run_store, run_id)
            && let Ok(Some(mut run)) = store.load(&run_id).await
        {
            run.status = status;
            run.final_output = output.map(|s| s.to_string());
            run.error = error.map(|s| s.to_string());
            run.finished_at = Some(chrono::Utc::now());
            if let Err(e) = store.save(run).await {
                tracing::warn!(error = %e, "Failed to save trace run on finalize");
            }
        }
    }

    /// Shut down the agent and release all resources.
    ///
    /// Closes MCP connections, cancels background tasks, and shuts down WebSocket servers.
    /// Call this when the agent is no longer needed, or rely on `Drop` for automatic cleanup.
    ///
    /// This is a convenience wrapper around [`Agent::close()`]. Prefer `close()` for
    /// trait-object usage; `shutdown()` is retained for backward compatibility.
    pub async fn shutdown(&self) {
        // Fire SessionEnd hook before cleanup
        self.fire_lifecycle_hook(crate::skills::hooks::HookEvent::SessionEnd, Some("other"))
            .await;
        let _ = self.close().await;
    }

    /// Set the maximum number of ReAct loop iterations at runtime.
    ///
    /// This allows dynamic adjustment of the agent's reasoning depth — for example,
    /// `/think low` sets a low iteration count for quick responses, while
    /// `/think high` allows more reasoning steps.
    ///
    /// Passing 0 means unlimited; the run loop will continue until the task
    /// completes, is cancelled, or another runtime safety mechanism stops it.
    /// Get a reference to the shared context manager (for stats/display).
    pub fn context(&self) -> &Arc<tokio::sync::Mutex<crate::compression::ContextManager>> {
        &self.memory.context
    }

    /// Get the maximum number of iterations.
    pub fn max_iterations(&self) -> usize {
        self.config.max_iterations
    }

    /// Enable or disable plan mode (read-only tools).
    pub fn set_plan_mode(&mut self, enabled: bool) {
        self.config.plan_mode = enabled;
    }

    /// Check if plan mode is active.
    pub fn is_plan_mode(&self) -> bool {
        self.config.plan_mode
    }

    /// Set the permission mode at runtime.
    ///
    /// Accepted values: "default", "auto-edit", "full-auto", "strict".
    /// Legacy aliases "plan" and "auto" normalize to "default"; read-only
    /// planning and Auto routing are controlled by separate runtime modes.
    /// Read-only planning is controlled separately via `set_plan_mode`.
    /// Also propagates to `PermissionService` if wired (sync, non-blocking).
    pub fn set_permission_mode(&mut self, mode: &str) {
        let normalized_mode = match mode {
            "plan" | "auto" => "default",
            _ => mode,
        };
        self.config.permission_mode = normalized_mode.to_string();

        // Propagate to PermissionService (if wired)
        #[cfg(feature = "human-loop")]
        if let Some(ref service) = self.approval.permission_service {
            use echo_core::tools::permission::PermissionMode;
            let pm = match normalized_mode {
                "full-auto" => PermissionMode::BypassPermissions,
                "auto-edit" | "accept-edits" => PermissionMode::AcceptEdits,
                "strict" | "strict-confirm" | "strict-confirmation" => {
                    PermissionMode::StrictConfirm
                }
                _ => PermissionMode::Default,
            };
            // Security: make bypass mode loud. BypassPermissions auto-allows every
            // tool (shell, write, MCP) with no per-action approval. Surface this in
            // the trace/audit log so it is never silently enabled via config or env.
            if matches!(pm, PermissionMode::BypassPermissions) {
                tracing::warn!(
                    agent = %self.config.agent_name,
                    "Permission mode set to full-auto/BypassPermissions: ALL tools                      (including shell and file writes) will be auto-approved without                      per-action confirmation. Use the admin bypass_disabled switch to                      deny this in shared/CI environments."
                );
            }
            service.set_mode_sync(pm);
            // F2 修复：切换权限模式时清除审批缓存。旧模式下的审批决策
            //（如 Default 模式批准的 shell）不应延续到新模式（如 Plan）。
            service.clear_cache();
        }
    }

    /// Get the current permission mode.
    pub fn get_permission_mode(&self) -> &str {
        &self.config.permission_mode
    }

    pub fn set_max_iterations(&mut self, max: usize) {
        self.config.max_iterations = max;
    }

    /// Delegate a task to a subagent by name.
    ///
    /// This is a convenience method that creates a `DispatchRequest` and
    /// dispatches it through the subagent executor. The subagent must have
    /// been previously registered via `register_subagent()`.
    ///
    /// If no subagent is registered, falls back to executing the task
    /// directly with `self.chat()`.
    #[cfg(feature = "subagent")]
    pub async fn delegate_task(&self, task: &str) -> Result<String> {
        self.delegate_task_with_depth(task, 0).await
    }

    /// Like [`delegate_task`] but with an explicit delegation depth.
    /// Called by the ReAct loop when processing internal delegate markers.
    /// Top-level calls pass 0; nested subagent ReAct loops increment.
    #[cfg(feature = "subagent")]
    pub async fn delegate_task_with_depth(&self, task: &str, depth: u32) -> Result<String> {
        use crate::agent::subagent::executor::DispatchRequest;
        use crate::agent::subagent::types::ExecutionMode;

        // Check if there are any registered subagents
        let agents = self.tools.subagent_registry.list_available().await;

        if !agents.is_empty() {
            let agent_name = agents
                .first()
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "default".to_string());

            let req = DispatchRequest {
                agent_name,
                task: task.to_string(),
                mode_override: Some(ExecutionMode::Fork),
                // Inherit the parent run's cancel token (P1-11): a bare
                // `CancellationToken::new()` here would detach the subagent
                // from the parent, so cancelling the parent run would not
                // propagate to the delegated subagent. Fall back to a fresh
                // token only if no run is active (cancel_token not set).
                cancel: self
                    .cancel_token
                    .lock()
                    .await
                    .as_ref()
                    .map(|t| t.child_token())
                    .unwrap_or_else(CancellationToken::new),
                parent_agent: self.config.agent_name.clone(),
                parent_context: self.build_parent_context(&ExecutionMode::Fork).await,
                delegate_depth: depth,
                runtime_context: self.build_runtime_context(),
                message: None,
            };

            // Reuse the stored executor (with hook configuration)
            let result = self.tools.subagent_executor.dispatch(req).await?;
            Ok(result.output)
        } else {
            // Fallback: execute directly with the current agent
            <Self as Agent>::chat(self, task).await
        }
    }

    /// Delegate a task to a specific subagent by name.
    ///
    /// Unlike [`delegate_task`](Self::delegate_task) which picks the first
    /// registered subagent, this method routes to the specified `target` agent.
    /// Returns an error if the target agent is not registered.
    ///
    /// Context inheritance is applied automatically: the subagent receives
    /// the parent's system prompt, tools, and recent conversation history
    /// based on the Fork mode default policy.
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent(&self, target: &str, task: &str) -> Result<String> {
        self.delegate_to_agent_with_depth(target, task, 0).await
    }

    /// Like [`delegate_to_agent`] but with an explicit delegation depth.
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent_with_depth(
        &self,
        target: &str,
        task: &str,
        depth: u32,
    ) -> Result<String> {
        use crate::agent::subagent::executor::DispatchRequest;
        use crate::agent::subagent::types::ExecutionMode;

        // Verify the target agent exists in the registry
        let agents = self.tools.subagent_registry.list_available().await;
        if !agents.iter().any(|d| d.name == target) {
            return Err(echo_core::error::ReactError::Other(format!(
                "Subagent '{}' not found. Available agents: {:?}",
                target,
                agents.iter().map(|d| &d.name).collect::<Vec<_>>()
            )));
        }

        let mode = ExecutionMode::Fork;
        let req = DispatchRequest {
            agent_name: target.to_string(),
            task: task.to_string(),
            mode_override: Some(mode.clone()),
            // Inherit the parent run's cancel token (P1-11) — see delegate_task.
            cancel: self
                .cancel_token
                .lock()
                .await
                .as_ref()
                .map(|t| t.child_token())
                .unwrap_or_else(CancellationToken::new),
            parent_agent: self.config.agent_name.clone(),
            parent_context: self.build_parent_context(&mode).await,
            delegate_depth: depth,
            runtime_context: self.build_runtime_context(),
            message: None,
        };

        let result = self.tools.subagent_executor.dispatch(req).await?;
        Ok(result.output)
    }

    /// Delegate a task to a specific subagent by name, with caller-supplied
    /// cancellation.
    ///
    /// This is the cancel-aware counterpart of
    /// [`delegate_to_agent`](Self::delegate_to_agent). It exists so that
    /// product-layer runtimes (e.g. a TaskRuntime DAG executor) can fan out
    /// work to the registered subagents AND propagate parent-run cancellation
    /// into each worker — the plain `delegate_to_agent` hard-codes a fresh,
    /// never-cancelled token, which makes parent→child cancellation
    /// impossible and is unsuitable for parallel DAG dispatch.
    ///
    /// The caller typically passes a *child* of the parent run's token
    /// (`parent_cancel.child_token()`); cancelling the parent then cancels
    /// every worker dispatched via this method.
    ///
    /// Returns the worker's full [`SubagentResult`] (including usage data),
    /// or an error if the target agent is not registered.
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent_with_cancel(
        &self,
        target: &str,
        task: &str,
        cancel: CancellationToken,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_parent_and_cancel(
            target,
            task,
            self.config.agent_name.as_str(),
            cancel,
            0,
        )
        .await
    }

    /// Delegate a task to a specific subagent with caller-supplied parent
    /// label and cancellation.
    ///
    /// Product-layer runtimes use `parent_label` to correlate subagent events
    /// with a top-level run id, instead of the static parent agent name.
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent_with_parent_and_cancel(
        &self,
        target: &str,
        task: &str,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        use crate::agent::subagent::executor::DispatchRequest;
        use crate::agent::subagent::types::ExecutionMode;

        // Verify the target agent exists in the registry
        let agents = self.tools.subagent_registry.list_available().await;
        if !agents.iter().any(|d| d.name == target) {
            return Err(echo_core::error::ReactError::Other(format!(
                "Subagent '{}' not found. Available agents: {:?}",
                target,
                agents.iter().map(|d| &d.name).collect::<Vec<_>>()
            )));
        }

        let mode = ExecutionMode::Fork;
        // 从当前 agent 的 external_* 构造 runtime_context 透传给 worker
        // (主 agent → worker、worker → sub-worker 自动继承,嵌套自然)。
        let runtime_context = self.build_runtime_context();
        let req = DispatchRequest {
            agent_name: target.to_string(),
            task: task.to_string(),
            mode_override: Some(mode.clone()),
            cancel,
            parent_agent: parent_label.to_string(),
            parent_context: self.build_parent_context(&mode).await,
            delegate_depth: depth,
            runtime_context,
            message: None,
        };

        let result = self.tools.subagent_executor.dispatch(req).await?;
        Ok(result)
    }

    /// Delegate a multimodal task to a subagent (images/files included).
    ///
    /// Like [`delegate_to_agent_with_parent_and_cancel`](Self::delegate_to_agent_with_parent_and_cancel)
    /// but carries a [`Message`] so the worker sees user-uploaded attachments.
    /// The `task` text is also kept (used for hooks/events/fallback).
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent_with_parent_cancel_and_message(
        &self,
        target: &str,
        task: &str,
        message: crate::llm::types::Message,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        use crate::agent::subagent::executor::DispatchRequest;
        use crate::agent::subagent::types::ExecutionMode;

        let agents = self.tools.subagent_registry.list_available().await;
        if !agents.iter().any(|d| d.name == target) {
            return Err(echo_core::error::ReactError::Other(format!(
                "Subagent '{}' not found. Available agents: {:?}",
                target,
                agents.iter().map(|d| &d.name).collect::<Vec<_>>()
            )));
        }

        let mode = ExecutionMode::Fork;
        let runtime_context = self.build_runtime_context();
        let req = DispatchRequest {
            agent_name: target.to_string(),
            task: task.to_string(),
            mode_override: Some(mode.clone()),
            cancel,
            parent_agent: parent_label.to_string(),
            parent_context: self.build_parent_context(&mode).await,
            delegate_depth: depth,
            runtime_context,
            message: Some(message),
        };

        let result = self.tools.subagent_executor.dispatch(req).await?;
        Ok(result)
    }

    /// Build parent context for subagent dispatch based on execution mode.
    ///
    /// Shared by `delegate_task()`, `delegate_to_agent()`, and conceptually
    /// mirrors `ParentContextFactory::build()` (used by `AgentDispatchTool`).
    #[cfg(feature = "subagent")]
    /// 从当前 agent 的 external_* 字段构造 ExternalRunContext（透传给 worker）。
    ///
    /// 这样主 agent 委派 worker、worker 委派 sub-worker 时,run context 自动继承
    /// （嵌套自然继承)。current_run_id 为 None 时返回 None（无 run 上下文,旧行为）。
    #[cfg(feature = "subagent")]
    fn build_runtime_context(&self) -> Option<echo_core::tools::ExternalRunContext> {
        let run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        Some(echo_core::tools::ExternalRunContext {
            run_id,
            cancel: self
                .external_cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            trace_sink: self
                .external_trace_sink
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        })
    }

    #[cfg(feature = "subagent")]
    async fn build_parent_context(
        &self,
        mode: &crate::agent::subagent::types::ExecutionMode,
    ) -> Option<crate::agent::subagent::context::SubagentContext> {
        use crate::agent::subagent::context::{ContextInheritance, SubagentContext};

        let inheritance = ContextInheritance::for_mode(mode);
        let system_prompt = self.system_prompt().to_string();
        let tool_defs = self.tool_definitions();
        let messages = self.memory.context.lock().await.messages().to_vec();
        let store = self.memory.store.clone();

        let ctx = SubagentContext::from_parent(
            &system_prompt,
            &tool_defs,
            &messages,
            store,
            &inheritance,
        );
        if ctx.has_content() { Some(ctx) } else { None }
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

// ── Internal accessors ───────────────────────────────────────────────────────

impl ReactAgent {
    /// Access the shared HTTP client (for StreamRunner construction).
    pub(crate) fn client(&self) -> &Arc<Client> {
        &self.client
    }
}

// ── impl Agent for ReactAgent ────────────────────────────────────────────────

impl Agent for ReactAgent {
    fn name(&self) -> &str {
        &self.config.agent_name
    }

    fn model_name(&self) -> &str {
        &self.config.model_name
    }

    fn current_run_id(&self) -> Option<String> {
        self.current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_external_context(&self, ctx: &echo_core::tools::ExternalRunContext) {
        *self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(ctx.run_id.clone());
        *self
            .external_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.cancel.clone();
        *self
            .external_trace_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.trace_sink.clone();
    }

    fn clear_external_context(&self) {
        *self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .external_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .external_trace_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn set_working_dir(&self, path: Option<std::path::PathBuf>) {
        // Delegate to the inherent method (sets config.working_dir + refreshes
        // root-system-prompt so the cwd-in-prompt stays accurate).
        ReactAgent::set_working_dir(self, path);
    }

    fn clear_working_dir(&self) {
        ReactAgent::clear_working_dir(self);
    }

    fn system_prompt(&self) -> &str {
        // Check for runtime override first
        if self
            .mutable_system_prompt
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            // We can't return a reference to the RwLock contents directly,
            // so fall back to config prompt. The override is picked up at
            // the start of each new turn via build_system_prompt().
            // This method returns the "base" prompt for inspection.
            return &self.config.system_prompt;
        }
        &self.config.system_prompt
    }

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        let agent_name = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        let agent_name_for_span = agent_name.clone();
        let model_for_span = model.clone();
        Box::pin(
            async move {
                self.run_direct(task).await
            }
            .instrument(info_span!("agent_execute", agent.name = %agent_name_for_span, agent.model = %model_for_span)),
        )
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move { self.run_stream_entry(task, run::StreamMode::Execute).await }.instrument(
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
            async move { self.run_stream_entry(message, run::StreamMode::Chat).await }.instrument(
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
                // Mirror the active run's token into the LLM-callable dispatch
                // tool (P1-11): subagents dispatched via `agent_tool` derive a
                // child_token from this, so they're cancelled with the parent.
                if let Some(handle) = &self.dispatch_cancel_handle {
                    *handle.lock().await = Some(cancel);
                }
                self.run_stream_entry(_message, run::StreamMode::Chat).await
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
                // Mirror the active run's token into the LLM-callable dispatch
                // tool (P1-11): subagents dispatched via `agent_tool` derive a
                // child_token from this, so they're cancelled with the parent.
                if let Some(handle) = &self.dispatch_cancel_handle {
                    *handle.lock().await = Some(cancel);
                }
                self.run_stream_entry(_task, run::StreamMode::Execute).await
            }
            .instrument(info_span!("agent_execute_stream_with_cancel", agent.name = %agent, agent.model = %model)),
        )
    }

    fn execute_stream_message_with_cancel<'a>(
        &'a self,
        message: crate::llm::types::Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move {
                *self.cancel_token.lock().await = Some(cancel.clone());
                if let Some(handle) = &self.dispatch_cancel_handle {
                    *handle.lock().await = Some(cancel);
                }
                self.run_stream_message_entry(message, run::StreamMode::Execute)
                    .await
            }
            .instrument(info_span!(
                "agent_execute_stream_message_with_cancel",
                agent.name = %agent,
                agent.model = %model
            )),
        )
    }

    fn reset(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.reset_messages().await;
        })
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

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            #[cfg(feature = "mcp")]
            self.tools.mcp_manager.close_all().await;
            info!(agent = %self.config.agent_name, "Agent shut down complete");
            Ok(())
        })
    }

    fn messages(&self) -> Vec<crate::llm::types::Message> {
        // context_manager uses tokio::sync::Mutex, requiring async.
        // Return empty for sync method. Use async get_messages_async() for full data.
        vec![]
    }

    fn register_tool(&self, tool: Box<dyn crate::tools::Tool>) {
        self.tools.tool_manager.register(tool);
    }

    fn remove_tool(&self, name: &str) -> bool {
        self.tools.tool_manager.unregister(name).is_some()
    }

    fn set_system_prompt(&self, prompt: &str) {
        *self
            .mutable_system_prompt
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(prompt.to_string());
    }

    fn delegate_to<'a>(&'a self, target: &'a str, task: &'a str) -> BoxFuture<'a, Result<String>> {
        #[cfg(feature = "subagent")]
        {
            Box::pin(self.delegate_to_agent(target, task))
        }
        #[cfg(not(feature = "subagent"))]
        {
            let _ = (target, task);
            Box::pin(async {
                Err(echo_core::error::ReactError::Other(
                    "delegation not supported (subagent feature disabled)".into(),
                ))
            })
        }
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
        self.run_stream_message_entry(message, run::StreamMode::Chat)
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
        self.run_stream_message_entry(message, run::StreamMode::Execute)
            .await
    }

    /// Streaming multi-turn conversation with cancellation (multimodal version).
    ///
    /// Combines [`chat_stream_message`](Self::chat_stream_message) with the
    /// cancel-token + dispatch-handle mirroring that
    /// [`chat_stream_with_cancel`](Agent::chat_stream_with_cancel) does for
    /// plain text. Use this for UIs that send images/files AND need
    /// cooperative cancellation (the Tauri chat path).
    pub fn chat_stream_message_with_cancel<'a>(
        &'a self,
        message: crate::llm::types::Message,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move {
                *self.cancel_token.lock().await = Some(cancel.clone());
                // Mirror the active run's token into the LLM-callable dispatch
                // tool so subagents are cancelled with the parent (P1-11).
                if let Some(handle) = &self.dispatch_cancel_handle {
                    *handle.lock().await = Some(cancel);
                }
                self.run_stream_message_entry(message, run::StreamMode::Chat)
                    .await
            }
            .instrument(info_span!(
                "agent_chat_stream_message_with_cancel",
                agent.name = %agent,
                agent.model = %model
            )),
        )
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

        // ★ Serialize execution — multimodal mutates context and calls LLM
        let _execution_guard = self.execution_mutex.lock().await;

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
                    thinking: self.thinking.clone(),
                    cancel_token: None,
                    user_id: self.config.cache_user_id.clone(),
                    cache_hints: None,
                })
                .await?;
            response.content().unwrap_or_default()
        } else {
            let response = chat(
                self.client.clone(),
                &self.config.model_name,
                &messages,
                None,                              // temperature
                None,                              // max_tokens
                Some(false),                       // stream
                None,                              // tools
                None,                              // tool_choice
                None,                              // response_format
                self.config.cache_user_id.clone(), // user_id (E4 fix)
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
