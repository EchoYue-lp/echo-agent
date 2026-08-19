//! ReAct Agent core module
//!
//! ## Module Structure
//!
//! | File | Responsibility |
//! |------|----------------|
//! | `mod.rs` | Struct definition, `new()`, `impl Agent` trait |
//! | `run.rs` | Canonical ReAct execution engine |
//! | `capabilities.rs` | Capability configuration (tool / skill / MCP / subagent registration) |
//! | `extract.rs` | Structured JSON extraction (`extract_json` / `extract`) |

pub use crate::agent::config::AgentConfig;
#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentRegistry;
#[cfg(feature = "subagent")]
use crate::agent::subagent::executor::{DispatchRequest, SubagentExecutor, SubagentExecutorConfig};
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
#[cfg(feature = "subagent")]
use crate::tools::builtin::agent_dispatch::AgentDispatchTool;
use crate::tools::builtin::answer::FinalAnswerTool;
use crate::tools::builtin::cell_tools::{ListCellsTool, StopCellTool, WaitCellTool};
#[cfg(feature = "human-loop")]
use crate::tools::builtin::human_in_loop::HumanInLoop;
use crate::tools::builtin::memory::{
    ForgetTool, LayeredForgetTool, LayeredRecallTool, LayeredRememberTool, LayeredSearchMemoryTool,
    LegacyStoreRememberTool, RecallTool, SearchMemoryTool,
};
use crate::tools::{ToolManager, ToolSearchTool};
use echo_core::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use reqwest::Client;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{Instrument, info, info_span};

#[cfg(feature = "subagent")]
struct HookSubagentCancelGuard(CancellationToken);

#[cfg(feature = "subagent")]
impl Drop for HookSubagentCancelGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(feature = "human-loop")]
use crate::agent::react::subsystems::approval::ApprovalSubsystem;
use crate::agent::react::subsystems::guard::GuardSubsystem;
use crate::agent::react::subsystems::memory::MemorySubsystem;
use crate::agent::react::subsystems::tool_exec::ToolExecutionSubsystem;

pub mod builder;
mod capabilities;
pub use capabilities::{
    PreparedAgentModelDeactivation, PreparedAgentModelGeneration, PreparedCriticUpdate,
    PreparedTokenLimit,
};
mod extract;
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
    /// Tool execution subsystem: tool registry/execution, Skill, Hook, MCP, Subagent, Sandbox
    pub(crate) tools: ToolExecutionSubsystem,
    /// Guard & safety subsystem: guards, permission policy, audit logging, circuit breaker
    pub(crate) guard: GuardSubsystem,
    /// Memory & persistence subsystem: context management, long-term memory, snapshots, transcript projection
    pub(crate) memory: MemorySubsystem,
    /// Optional application-supplied projection refreshed before every model call.
    pub(crate) pre_model_context_projector:
        std::sync::RwLock<Option<Arc<dyn crate::compression::PreModelContextProjector>>>,
    /// Human-in-the-loop approval subsystem
    #[cfg(feature = "human-loop")]
    pub(crate) approval: ApprovalSubsystem,
    client: Arc<Client>,
    llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Application-injected LLM configuration.
    llm_config: Option<LlmConfig>,
    /// Per-agent thinking-depth / reasoning config, applied to every chat
    /// request issued by this agent (think phase, react loop). `None` means
    /// "use the model's default" — no thinking field is sent.
    thinking: Option<crate::llm::ThinkingConfig>,
    /// Cancellation token for the current streaming request, set in
    /// `chat_stream_with_cancel` / `execute_stream_with_cancel`.
    /// `create_llm_stream` reads this field and passes it to the HTTP layer
    /// to support request-level stream cancellation.
    /// Uses `tokio::sync::Mutex` to support `&self` streaming methods.
    pub(crate) cancel_token: tokio::sync::Mutex<Option<CancellationToken>>,
    /// Same-turn user input mailbox shared with streaming snapshots.
    pub(crate) turn_steer_mailbox: Arc<crate::agent::steer::TurnSteerMailbox>,

    /// Shared handle to the `AgentDispatchTool`'s cancel token (P1-11).
    ///
    /// Mirrors [`cancel_token`] into the LLM-callable dispatch tool so that a
    /// subagent dispatched via the `agent_tool` is cancelled when the parent
    /// run is. Updated alongside `cancel_token` at run start. `None` when
    /// subagents are disabled (`AgentDispatchTool` never registered).
    pub(crate) dispatch_cancel_handle: Option<Arc<tokio::sync::Mutex<Option<CancellationToken>>>>,

    /// Optional run store for persisting execution traces.
    /// When set, each streaming execution records a [`Run`](crate::trace::Run)
    /// with events, token usage, and timings.
    pub run_store: Option<Arc<dyn crate::trace::RunStore>>,

    /// Product/business run ID propagated into tools and projections.
    pub current_run_id: std::sync::Mutex<Option<String>>,

    /// Unique trace invocation ID used only by the framework run store.
    pub current_trace_run_id: std::sync::Mutex<Option<String>>,

    /// 外部 run 级上下文（跨 spawn 安全，值传递）。
    ///
    /// `tokio::task_local!` 不会跨 `tokio::spawn` 继承——subagent 在框架层
    /// 的 dispatch_fork spawn 里执行时，应用层经 task_local 注入的 run_id /
    /// cancel / trace_sink 全部丢失。这里改用 Mutex 字段承载
    /// （set_external_context 设置，pipeline 构造 ToolContext 时读取），是跨
    /// spawn 安全的值传递通路。
    pub external_cancel:
        std::sync::Mutex<Option<std::sync::Arc<tokio_util::sync::CancellationToken>>>,
    pub external_trace_sink: std::sync::Mutex<Option<echo_core::tools::TraceSinkFn>>,
    pub external_delegation_policy:
        std::sync::Mutex<Option<echo_core::tools::NestedDelegationPolicy>>,
    /// Stable execution id (`{task_id}:{attempt}`) set by the app layer before
    /// dispatching a subagent, so `SubagentEvent.execution_id` carries a stable
    /// identifier instead of bridge-side temp allocation. Carried as a Mutex
    /// field (same cross-spawn pattern as the other external_* fields).
    pub external_execution_id: std::sync::Mutex<Option<String>>,
    /// Stable identity for reusable worktree/workspace isolation resources.
    pub external_isolation_id: std::sync::Mutex<Option<String>>,
    pub external_turn_id: std::sync::Mutex<Option<String>>,
    /// Chat message id that triggered the run, forwarded to
    /// `SubagentEvent::DispatchStarted.message_id` so the frontend can pin a
    /// subagent stream to the right chat message block.
    pub external_message_id: std::sync::Mutex<Option<String>>,

    /// Optional tool execution pipeline. When absent, the standard pipeline is used.
    pub(crate) tool_execution_pipeline: Option<Arc<run::pipeline::ToolExecutionPipeline>>,

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
    /// Named owner allowed to refresh the current critic during a prepared
    /// model-generation publication. `None` denotes an unowned/custom critic.
    critic_owner: Option<String>,

    /// Layered memory manager used by runtime-triggered memory writes.
    pub(crate) memory_layer_manager: Option<Arc<crate::evolution::MemoryLayerManager>>,

    /// Runtime state consumed by TriggerDetector between turns.
    pub(crate) memory_trigger_state: Arc<std::sync::Mutex<MemoryTriggerRuntimeState>>,

    /// Optional product-owned sink for proposal and review workflows.
    pub(crate) memory_trigger_sink: Option<Arc<dyn crate::evolution::MemoryTriggerSink>>,

    /// Optional product-owned authority for file-based skill discovery.
    pub(crate) skill_load_policy: Option<Arc<dyn crate::skills::external::SkillLoadPolicy>>,

    /// Optional consumer-supplied skill lifecycle curator.
    ///
    /// Framework consumers that scope skill lifecycle state per project can
    /// replace the default user-level curator without coupling the framework to
    /// a specific workspace layout.
    pub(crate) skill_curator: Option<crate::evolution::Curator>,

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
    /// Inject user input into the currently active regular ReAct turn.
    pub fn steer_input(
        &self,
        expected_turn_id: Option<&str>,
        message: crate::llm::types::Message,
    ) -> std::result::Result<String, crate::agent::TurnSteerError> {
        self.turn_steer_mailbox.steer(expected_turn_id, message)
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
    ///
    /// Prefer [`crate::agent::ReactAgentBuilder`] for construction — it handles subsystem
    /// initialization and provides sensible defaults. Direct construction with
    /// [`new`](Self::new) initialises every subsystem eagerly.
    pub fn new(config: AgentConfig) -> Self {
        #[cfg(feature = "subagent")]
        {
            Self::new_inner(config, None)
        }
        #[cfg(not(feature = "subagent"))]
        {
            Self::new_inner(config)
        }
    }

    #[cfg(feature = "subagent")]
    pub(crate) fn new_with_subagent_registry(
        config: AgentConfig,
        registry: Arc<SubagentRegistry>,
    ) -> Self {
        Self::new_inner(config, Some(registry))
    }

    fn new_inner(
        mut config: AgentConfig,
        #[cfg(feature = "subagent")] provided_subagent_registry: Option<Arc<SubagentRegistry>>,
    ) -> Self {
        if !config.token_limit_explicit
            && let Some(profile_window) = config
                .model_profile
                .as_ref()
                .and_then(|profile| profile.context_window)
                .and_then(|window| usize::try_from(window).ok())
        {
            config.token_limit = profile_window;
        }
        let system_prompt = Self::build_system_prompt(&config);

        let sp_for_canonical = system_prompt.clone();
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
        if config.token_budget_config.enabled
            && let Ok(budget) = config.token_budget_config.build(config.token_limit)
        {
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
                        .ok()
                        .and_then(|guard| guard.clone())
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    echo_core::project_rules::rules_injection_with_root(
                        &wd,
                        config.project_root.as_deref(),
                    )
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
        let task_revision_service = Arc::new(echo_orchestration::tasks::TaskRevisionService::new(
            Arc::new(echo_orchestration::tasks::InMemoryRevisionedTaskStore::new()),
            Arc::new(echo_orchestration::tasks::DefaultTaskToolPolicy::default()),
        ));
        tool_manager.register_tools(echo_orchestration::tasks::build_task_tools(
            task_revision_service,
        ));

        // ── Subsystem initialization ──────────────────────────────
        #[cfg(feature = "subagent")]
        let subagent_registry =
            provided_subagent_registry.unwrap_or_else(|| Arc::new(SubagentRegistry::new()));

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
                    data_workspace_factory: config.subagent_data_workspace_factory.clone(),
                    prompt_compiler: config.subagent_prompt_compiler.clone(),
                    ..SubagentExecutorConfig::default()
                },
            ))
        };
        #[cfg(feature = "subagent")]
        {
            use crate::skills::hooks::SubagentExecutorFn;

            let executor = subagent_executor.clone();
            let parent_agent = config.agent_name.clone();
            let hook_executor: SubagentExecutorFn = Arc::new(move |name, task| {
                let executor = executor.clone();
                let parent_agent = parent_agent.clone();
                Box::pin(async move {
                    let cancel = CancellationToken::new();
                    let _cancel_guard = HookSubagentCancelGuard(cancel.clone());
                    let result = executor
                        .dispatch(DispatchRequest {
                            agent_name: name,
                            task,
                            mode_override: None,
                            cancel,
                            parent_agent,
                            parent_context: None,
                            delegation_policy: crate::tasks::NestedDelegationPolicy {
                                can_spawn_subagents: false,
                                delegate_depth: 0,
                                max_delegate_depth: 0,
                            },
                            runtime_context: None,
                            message: None,
                            prompt_payload: None,
                            constraints: Vec::new(),
                            background: false,
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                    if result.outcome.status
                        != crate::agent::subagent::types::SubagentStatus::Completed
                    {
                        return Err(format!(
                            "Subagent finished with status {:?}: {}",
                            result.outcome.status, result.output
                        ));
                    }
                    Ok(result.output)
                })
            });
            if let Ok(mut registry) = hook_registry.try_write() {
                registry.set_subagent_executor(hook_executor);
            } else {
                tracing::error!("Failed to initialize the hook subagent executor");
            }
        }
        #[cfg(not(feature = "subagent"))]
        let _ = &hook_registry; // suppress unused warning
        #[cfg(feature = "human-loop")]
        let approval_provider = crate::human_loop::default_provider();

        // ── Feature-gated tool registration ───────────────────────
        #[cfg(feature = "human-loop")]
        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop::new(approval_provider.clone())));
        }

        // Background command cells (shell background=true + wait/stop/list).
        // Registered whenever a shared registry is injected — one process-wide
        // registry lets the main agent and its subagents observe the same cells.
        if let Some(cells) = config.command_cells.clone() {
            tool_manager.register(Box::new(WaitCellTool::new(cells.clone())));
            tool_manager.register(Box::new(StopCellTool::new(cells.clone())));
            tool_manager.register(Box::new(ListCellsTool::new(cells)));
        }
        Self::register_feature_gated_tools(&config, &mut tool_manager);

        // ── Memory store ──────────────────────────────────────────
        let store = Self::setup_memory_store(&config, &mut tool_manager);

        // Wrap tool_manager in Arc for sharing with subsystems and context factory
        let tool_manager = Arc::new(tool_manager);
        tool_manager.register(Box::new(ToolSearchTool::new(Arc::downgrade(&tool_manager))));

        // ── AgentDispatch tool (after all other tools + store are ready) ──
        // Context inheritance factory needs the final tool_manager Arc and store.
        // Shared cancel handle so the parent run can push its token into the
        // LLM-callable dispatch tool (P1-11). `None` when subagents disabled.
        #[cfg(feature = "subagent")]
        let mut dispatch_cancel_handle: Option<
            Arc<tokio::sync::Mutex<Option<CancellationToken>>>,
        > = None;
        #[cfg(feature = "subagent")]
        if config.register_agent_dispatch_tool {
            let factory = Arc::new(
                crate::tools::builtin::agent_dispatch::ParentContextFactory {
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
                disabled_tools: Arc::new(std::sync::RwLock::new(None)),
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
            pre_model_context_projector: std::sync::RwLock::new(None),
            #[cfg(feature = "human-loop")]
            approval: ApprovalSubsystem {
                approval_provider,
                permission_service: None,
                pending_permission_rules: std::sync::Mutex::new(Vec::new()),
            },
            client: Arc::new(client),
            llm_client: None,
            llm_config: None,
            thinking: None,
            cancel_token: tokio::sync::Mutex::new(None),
            turn_steer_mailbox: Arc::new(crate::agent::steer::TurnSteerMailbox::default()),
            #[cfg(feature = "subagent")]
            dispatch_cancel_handle,
            #[cfg(not(feature = "subagent"))]
            dispatch_cancel_handle: None,
            run_store: None,
            current_run_id: std::sync::Mutex::new(None),
            current_trace_run_id: std::sync::Mutex::new(None),
            external_cancel: std::sync::Mutex::new(None),
            external_trace_sink: std::sync::Mutex::new(None),
            external_delegation_policy: std::sync::Mutex::new(None),
            external_execution_id: std::sync::Mutex::new(None),
            external_isolation_id: std::sync::Mutex::new(None),
            external_turn_id: std::sync::Mutex::new(None),
            external_message_id: std::sync::Mutex::new(None),
            tool_execution_pipeline: None,
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
            critic_owner: None,
            memory_layer_manager: None,
            memory_trigger_state: Arc::new(std::sync::Mutex::new(
                MemoryTriggerRuntimeState::default(),
            )),
            memory_trigger_sink: None,
            skill_load_policy: None,
            skill_curator: None,
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
        self.memory_layer_manager = Some(layer_manager.clone());
        if let Ok(mut context) = self.memory.context.try_lock() {
            context.set_memory_promoter(Arc::new(
                crate::memory_promoter::StoreMemoryPromoter::new(layer_manager),
            ));
        } else {
            tracing::warn!(
                "Could not acquire ContextManager lock to install layered memory promoter; \
                 install the memory layer manager before running the agent"
            );
        }
    }

    /// Whether this agent has the layered memory runtime installed.
    pub fn has_memory_layer_manager(&self) -> bool {
        self.memory_layer_manager.is_some()
    }

    /// Set or clear the application-supplied pre-model context projector.
    pub fn set_pre_model_context_projector(
        &self,
        projector: Option<Arc<dyn crate::compression::PreModelContextProjector>>,
    ) {
        *self
            .pre_model_context_projector
            .write()
            .unwrap_or_else(|error| error.into_inner()) = projector;
    }

    /// (stage4 F1) Access the shared layer manager so app-side write paths
    /// (e.g. session-end auto-memory) route through the same instance the agent
    /// uses — shared security guard, audit log, and write observer
    /// (割裂点 6: previously app paths built a fresh per-call manager that
    /// bypassed the agent's shared instance).
    pub fn memory_layer_manager(&self) -> Option<&Arc<crate::evolution::MemoryLayerManager>> {
        self.memory_layer_manager.as_ref()
    }

    /// Route runtime memory triggers through an application-owned sink.
    pub fn set_memory_trigger_sink(
        &mut self,
        sink: Option<Arc<dyn crate::evolution::MemoryTriggerSink>>,
    ) {
        self.memory_trigger_sink = sink;
    }

    /// Set the authority consulted by all subsequent file-based skill discovery.
    pub fn set_skill_load_policy(
        &mut self,
        policy: Option<Arc<dyn crate::skills::external::SkillLoadPolicy>>,
    ) {
        self.skill_load_policy = policy;
    }

    /// Set or clear the curator used to record skill usage lifecycle data.
    pub fn set_skill_curator(&mut self, curator: Option<crate::evolution::Curator>) {
        self.skill_curator = curator;
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
        let mut prompt = if config.enable_tool && config.enable_cot {
            format!(
                "{}\n\n{}",
                config.system_prompt.trim_end(),
                Self::COT_INSTRUCTION,
            )
        } else {
            config.system_prompt.clone()
        };

        if let Some(suffix) = config
            .model_profile
            .as_ref()
            .and_then(|profile| profile.prompt_suffix.as_deref())
            .filter(|suffix| !suffix.trim().is_empty())
        {
            prompt.push_str("\n\n");
            prompt.push_str(suffix.trim());
        }

        // Project rules are loaded from the workspace and are stable per
        // project. They can stay in the system prompt since they don't
        // change between requests within the same workspace.
        #[cfg(feature = "project-rules")]
        if config.auto_project_rules {
            let wd = config
                .working_dir
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            prompt = echo_core::project_rules::inject_rules_with_root(
                &prompt,
                &wd,
                config.project_root.as_deref(),
            );
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
                // The injected cell registry (if any) enables ShellTool's
                // background=true mode alongside the regular tool surface.
                echo_tools::register_all_tools_with_cells(
                    tool_manager,
                    config.command_cells.clone(),
                );
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
                let namespace = crate::evolution::layer::WARM_NAMESPACE
                    .iter()
                    .map(|part| (*part).to_string())
                    .collect::<Vec<_>>();
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
    /// # fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    /// let llm_config = LlmConfig::for_provider(
    ///     "my-provider",
    ///     "https://api.example.com/v1",
    ///     "sk-...",
    ///     "qwen3-max",
    ///     LlmApiProtocol::ChatCompletions,
    /// )?;
    ///
    /// let agent = ReactAgent::new(
    ///     AgentConfig::standard("qwen3-max", "assistant", "You are a helpful assistant")
    /// ).with_llm_config(llm_config);
    /// # Ok(())
    /// # }
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
    /// calls use the provided credentials and explicit protocol.
    pub fn set_llm_config(&mut self, config: LlmConfig) {
        self.config.model_name = config.model.clone();
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
                    "Failed to build LLM client from explicit LlmConfig"
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
    /// issued by this agent. Applications set it at runtime after resolving the
    /// active model's [`crate::llm::core::capabilities::ThinkingProfile`].
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
            if let Some(layer_manager) = &self.memory_layer_manager {
                ctx.set_memory_promoter(Arc::new(
                    crate::memory_promoter::StoreMemoryPromoter::new(layer_manager.clone()),
                ));
            } else {
                ctx.remove_memory_promoter();
            }
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
        let mut context = self.memory.context.lock().await;
        if let Some(layer_manager) = &self.memory_layer_manager {
            context.set_memory_promoter(Arc::new(
                crate::memory_promoter::StoreMemoryPromoter::new(layer_manager.clone()),
            ));
        } else {
            context.remove_memory_promoter();
        }
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
        let ns = crate::evolution::layer::WARM_NAMESPACE
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>();
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
            self.tools
                .tool_manager
                .register(Box::new(ForgetTool::new(store.clone(), ns)));
        }
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
            if let Some(layer_manager) = &self.memory_layer_manager {
                ctx.set_memory_promoter(Arc::new(
                    crate::memory_promoter::StoreMemoryPromoter::new(layer_manager.clone()),
                ));
            } else {
                ctx.remove_memory_promoter();
            }
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
        let ns = crate::evolution::layer::WARM_NAMESPACE
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>();
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
            self.tools
                .tool_manager
                .register(Box::new(ForgetTool::new(store.clone(), ns)));
        }
        self.memory.store = Some(store.clone());

        let mut context = self.memory.context.lock().await;
        if let Some(layer_manager) = &self.memory_layer_manager {
            context.set_memory_promoter(Arc::new(
                crate::memory_promoter::StoreMemoryPromoter::new(layer_manager.clone()),
            ));
        } else {
            context.remove_memory_promoter();
        }
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
        self.config.working_dir.lock().ok().and_then(|g| g.clone())
    }

    /// Update the application-selected tool-output artifact policy.
    pub fn set_tool_output_artifacts(
        &self,
        config: Option<echo_core::tools::artifact::ToolOutputArtifactConfig>,
    ) {
        match self.config.tool_output_artifacts.lock() {
            Ok(mut current) => *current = config,
            Err(error) => tracing::warn!(
                error = %error,
                "Could not update tool-output artifact configuration"
            ),
        }
    }

    pub fn tool_output_artifacts(
        &self,
    ) -> Option<echo_core::tools::artifact::ToolOutputArtifactConfig> {
        self.config.get_tool_output_artifacts()
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
    /// Returns the full [`echo_execution::skills::external::types::SkillDescriptor`] list including triggers,
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
    /// Connection and disconnection methods call this automatically so hook
    /// actions always observe the current MCP client set.
    #[cfg(feature = "mcp")]
    pub async fn setup_hook_mcp_executor(&self) {
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
                            let output = output.chars().take(10_000).collect::<String>();
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
                            let mut result = crate::skills::hooks::HookResult::default();
                            result
                                .messages
                                .push(format!("McpTool hook {}::{} failed: {}", server, tool, e));
                            result
                        }
                    },
                    None => {
                        tracing::warn!(
                            server = %server,
                            tool = %tool,
                            "McpTool hook: server not found"
                        );
                        let mut result = crate::skills::hooks::HookResult::default();
                        result.messages.push(format!(
                            "McpTool hook {}::{} failed: server is not connected",
                            server, tool
                        ));
                        result
                    }
                }
            })
        });

        self.tools
            .hook_registry
            .write()
            .await
            .set_mcp_executor(executor);
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
        self.tools.tool_manager.apply_sandbox(manager.clone());
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

    /// Shared Subagent executor and its process-scoped attempt control plane.
    #[cfg(feature = "subagent")]
    pub fn subagent_executor(&self) -> &Arc<crate::agent::subagent::SubagentExecutor> {
        &self.tools.subagent_executor
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
        self.critic_owner = None;
    }

    /// Install a critic that may be refreshed by the same named owner during a
    /// prepared runtime-model publication.
    pub fn set_owned_critic(
        &mut self,
        owner: impl Into<String>,
        critic: Arc<dyn echo_core::agent::Critic>,
    ) {
        self.critic = Some(critic);
        self.critic_owner = Some(owner.into());
    }

    /// Return the named critic owner, if the current critic permits refresh.
    pub fn critic_owner(&self) -> Option<&str> {
        self.critic_owner.as_deref()
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
            let messages = cp.restore_messages()?;

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

            let completed_tool_call_ids = cp.completed_tool_call_ids()?;
            self.record_trace_event(crate::trace::RunEvent::CheckpointResumed {
                conversation_id: conv_id.clone(),
                completed_tool_call_ids: completed_tool_call_ids.clone(),
                checkpoint_timestamp: cp.timestamp,
            })
            .await;
            tracing::info!(
                conversation_id = conv_id.as_str(),
                message_count = msg_count,
                completed_tool_calls = completed_tool_call_ids.len(),
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
    pub async fn force_checkpoint(&self) -> Result<()> {
        let snapshot = crate::agent::snapshot::AgentRunSnapshot::from_agent(self);
        snapshot
            .save_runtime_checkpoint(&self.memory.context, None)
            .await
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
        invocation: Option<echo_core::agent::AgentInvocationContext>,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        self.run_stream_channel(
            run::types::StreamInit {
                text: input.to_string(),
                message: None,
                label: String::new(),
                invocation,
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
        invocation: Option<echo_core::agent::AgentInvocationContext>,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent>>> {
        let text = message.content.as_text().unwrap_or_default();
        self.run_stream_channel(
            run::types::StreamInit {
                text,
                message: Some(message),
                label: "(multimodal)".to_string(),
                invocation,
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
            .current_trace_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let (Some(store), Some(run_id)) = (&self.run_store, &run_id)
            && let Err(e) = store.append_event(run_id, event).await
        {
            tracing::warn!(error = %e, run_id = %run_id, "Failed to append trace event");
        }
    }

    /// Start a unique trace invocation while preserving any product run ID.
    pub(crate) async fn start_trace_run(&self, input: &str) -> Option<String> {
        let parent_run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let trace_run_id = self
            .start_scoped_trace_run(input, parent_run_id.as_deref(), None, None, None)
            .await?;
        *self
            .current_trace_run_id
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(trace_run_id.clone());
        Some(trace_run_id)
    }

    /// Start a trace run without mutating the agent-wide product run id.
    pub(crate) async fn start_scoped_trace_run(
        &self,
        input: &str,
        parent_run_id: Option<&str>,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        execution_id: Option<&str>,
    ) -> Option<String> {
        let store = self.run_store.as_ref()?;
        let run_id = format!("run_{}", uuid::Uuid::new_v4());
        let run = crate::trace::Run {
            run_id: run_id.clone(),
            parent_run_id: parent_run_id.map(str::to_string),
            agent_name: self.config.agent_name.clone(),
            model: self.config.model_name.clone(),
            provider: self
                .config
                .model_profile
                .as_ref()
                .map(|profile| profile.provider.clone()),
            turn_id: turn_id.map(str::to_string),
            execution_id: execution_id.map(str::to_string),
            session_id: conversation_id
                .map(str::to_string)
                .or_else(|| self.config.session_id.clone())
                .unwrap_or_default(),
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
        if let Err(error) = store.save(run).await {
            tracing::warn!(error = %error, "Failed to save scoped trace run on start");
        }
        Some(run_id)
    }

    pub(crate) async fn finalize_scoped_trace_run(
        &self,
        trace_run_id: Option<&str>,
        status: crate::trace::RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) {
        let Some(run_id) = trace_run_id else {
            return;
        };
        let Some(store) = self.run_store.as_ref() else {
            return;
        };
        if let Ok(Some(mut run)) = store.load(run_id).await {
            run.status = status;
            run.final_output = output.map(str::to_string);
            run.error = error.map(str::to_string);
            run.finished_at = Some(chrono::Utc::now());
            if let Err(error) = store.save(run).await {
                tracing::warn!(error = %error, run_id, "Failed to finalize scoped trace run");
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

    /// Update the hard iteration ceiling.
    ///
    /// This allows dynamic adjustment of the agent's reasoning depth while
    /// preserving a finite safety cap.
    ///
    /// # Errors
    /// Returns a configuration error when `max` is zero.
    pub fn set_max_iterations(&mut self, max: usize) -> Result<()> {
        if max == 0 {
            return Err(crate::error::ConfigError::ConfigFileError(
                "max_iterations must be greater than zero".to_string(),
            )
            .into());
        }
        self.config.max_iterations = max;
        Ok(())
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

    /// Like [`Self::delegate_task`] but with an explicit delegation depth.
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
                parent_context: self
                    .build_parent_context_with(
                        &crate::agent::subagent::context::ContextInheritance::fresh_default(),
                    )
                    .await,
                delegation_policy: DispatchRequest::policy_from_depth(depth),
                runtime_context: self.build_runtime_context(),
                message: None,
                prompt_payload: None,
                constraints: Vec::new(),
                background: false,
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
    /// Default inheritance is **fresh** (no parent system/history/memory).
    /// Execution still uses Fork mode so worktree/workspace isolation works.
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent(&self, target: &str, task: &str) -> Result<String> {
        self.delegate_to_agent_with_depth(target, task, 0).await
    }

    /// Like [`Self::delegate_to_agent`] but with an explicit delegation depth.
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
        let inheritance = crate::agent::subagent::context::ContextInheritance::fresh_default();
        let req = DispatchRequest {
            agent_name: target.to_string(),
            task: task.to_string(),
            mode_override: Some(mode),
            // Inherit the parent run's cancel token (P1-11) — see delegate_task.
            cancel: self
                .cancel_token
                .lock()
                .await
                .as_ref()
                .map(|t| t.child_token())
                .unwrap_or_else(CancellationToken::new),
            parent_agent: self.config.agent_name.clone(),
            parent_context: self.build_parent_context_with(&inheritance).await,
            delegation_policy: DispatchRequest::policy_from_depth(depth),
            runtime_context: self.build_runtime_context(),
            message: None,
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
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
    /// into each subagent — the plain `delegate_to_agent` hard-codes a fresh,
    /// never-cancelled token, which makes parent→child cancellation
    /// impossible and is unsuitable for parallel DAG dispatch.
    ///
    /// The caller typically passes a *child* of the parent run's token
    /// (`parent_cancel.child_token()`); cancelling the parent then cancels
    /// every subagent dispatched via this method.
    ///
    /// Returns the subagent's full [`crate::agent::subagent::SubagentResult`] (including usage data),
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
        self.delegate_to_agent_with_parent_context_and_cancel(
            target,
            task,
            parent_label,
            cancel,
            depth,
            self.build_runtime_context(),
        )
        .await
    }

    /// Delegate a task with an explicit runtime context.
    ///
    /// This is the concurrency-safe product-layer entry point: callers that
    /// already have a TaskRuntime context should pass it as a value on the
    /// dispatch request instead of first writing it into this agent's shared
    /// external-context fields. Multiple parallel dispatches can then carry
    /// different execution ids without overwriting each other.
    #[cfg(feature = "subagent")]
    pub async fn delegate_to_agent_with_parent_context_and_cancel(
        &self,
        target: &str,
        task: &str,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_parent_context_cancel_and_tools(
            target,
            task,
            parent_label,
            cancel,
            depth,
            runtime_context,
            None,
        )
        .await
    }

    /// Delegate with an invocation-scoped tool allowlist. An empty allowlist
    /// preserves the role's default tools; a non-empty list is enforced by
    /// hiding every other tool from both the model and execution pipeline.
    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_to_agent_with_parent_context_cancel_and_tools(
        &self,
        target: &str,
        task: &str,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_prompt_payload(
            target,
            task,
            parent_label,
            cancel,
            depth,
            runtime_context,
            allowed_tools,
            None,
        )
        .await
    }

    /// Delegate with an opaque structured payload for the configured prompt compiler.
    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_to_agent_with_prompt_payload(
        &self,
        target: &str,
        task: &str,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
        prompt_payload: Option<serde_json::Value>,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_prompt_payload_inner(
            target,
            task,
            parent_label,
            cancel,
            depth,
            runtime_context,
            allowed_tools,
            prompt_payload,
            None,
        )
        .await
    }

    /// Attempt-scoped form of [`Self::delegate_to_agent_with_prompt_payload`].
    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_to_agent_attempt_with_prompt_payload(
        &self,
        target: &str,
        task: &str,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
        prompt_payload: Option<serde_json::Value>,
        identity: crate::agent::subagent::SubagentAttemptIdentity,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_prompt_payload_inner(
            target,
            task,
            parent_label,
            cancel,
            depth,
            runtime_context,
            allowed_tools,
            prompt_payload,
            Some(identity),
        )
        .await
    }

    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    async fn delegate_to_agent_with_prompt_payload_inner(
        &self,
        target: &str,
        task: &str,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
        prompt_payload: Option<serde_json::Value>,
        identity: Option<crate::agent::subagent::SubagentAttemptIdentity>,
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

        // Keep Fork execution for worktree/workspace isolation, but default to
        // fresh inheritance (no parent system/history/memory) — Claude/Cursor.
        let mode = ExecutionMode::Fork;
        let inheritance = crate::agent::subagent::context::ContextInheritance::fresh_default();
        let mut parent_context = self.build_parent_context_with(&inheritance).await;
        if allowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
        {
            let context = parent_context
                .get_or_insert_with(crate::agent::subagent::context::SubagentContext::empty);
            context.allowed_tools = allowed_tools;
        }
        let req = DispatchRequest {
            agent_name: target.to_string(),
            task: task.to_string(),
            mode_override: Some(mode),
            cancel,
            parent_agent: parent_label.to_string(),
            parent_context,
            delegation_policy: DispatchRequest::policy_from_depth(depth),
            runtime_context,
            message: None,
            prompt_payload,
            constraints: Vec::new(),
            background: false,
        };

        let result = if let Some(identity) = identity {
            self.tools
                .subagent_executor
                .dispatch_attempt(req, identity)
                .await?
        } else {
            self.tools.subagent_executor.dispatch(req).await?
        };
        Ok(result)
    }

    /// Delegate a multimodal task to a subagent (images/files included).
    ///
    /// Like [`delegate_to_agent_with_parent_and_cancel`](Self::delegate_to_agent_with_parent_and_cancel)
    /// but carries a [`crate::llm::types::Message`] so the subagent sees user-uploaded attachments.
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
        self.delegate_to_agent_with_parent_context_cancel_and_message(
            target,
            task,
            message,
            parent_label,
            cancel,
            depth,
            self.build_runtime_context(),
        )
        .await
    }

    /// Delegate a multimodal task with an explicit runtime context.
    ///
    /// See [`Self::delegate_to_agent_with_parent_context_and_cancel`] for why
    /// product runtimes should prefer value-passing the context for parallel
    /// dispatches.
    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)] // Public delegation boundary carries explicit cancellation and run context.
    pub async fn delegate_to_agent_with_parent_context_cancel_and_message(
        &self,
        target: &str,
        task: &str,
        message: crate::llm::types::Message,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_parent_context_cancel_message_and_tools(
            target,
            task,
            message,
            parent_label,
            cancel,
            depth,
            runtime_context,
            None,
        )
        .await
    }

    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_to_agent_with_parent_context_cancel_message_and_tools(
        &self,
        target: &str,
        task: &str,
        message: crate::llm::types::Message,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_message_and_prompt_payload(
            target,
            task,
            message,
            parent_label,
            cancel,
            depth,
            runtime_context,
            allowed_tools,
            None,
        )
        .await
    }

    /// Multimodal delegation with an opaque structured prompt payload.
    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_to_agent_with_message_and_prompt_payload(
        &self,
        target: &str,
        task: &str,
        message: crate::llm::types::Message,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
        prompt_payload: Option<serde_json::Value>,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_message_and_prompt_payload_inner(
            target,
            task,
            message,
            parent_label,
            cancel,
            depth,
            runtime_context,
            allowed_tools,
            prompt_payload,
            None,
        )
        .await
    }

    /// Multimodal attempt-scoped delegation with structured prompt payload.
    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    pub async fn delegate_to_agent_attempt_with_message_and_prompt_payload(
        &self,
        target: &str,
        task: &str,
        message: crate::llm::types::Message,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
        prompt_payload: Option<serde_json::Value>,
        identity: crate::agent::subagent::SubagentAttemptIdentity,
    ) -> Result<crate::agent::subagent::SubagentResult> {
        self.delegate_to_agent_with_message_and_prompt_payload_inner(
            target,
            task,
            message,
            parent_label,
            cancel,
            depth,
            runtime_context,
            allowed_tools,
            prompt_payload,
            Some(identity),
        )
        .await
    }

    #[cfg(feature = "subagent")]
    #[allow(clippy::too_many_arguments)]
    async fn delegate_to_agent_with_message_and_prompt_payload_inner(
        &self,
        target: &str,
        task: &str,
        message: crate::llm::types::Message,
        parent_label: &str,
        cancel: CancellationToken,
        depth: u32,
        runtime_context: Option<echo_core::tools::ExternalRunContext>,
        allowed_tools: Option<Vec<String>>,
        prompt_payload: Option<serde_json::Value>,
        identity: Option<crate::agent::subagent::SubagentAttemptIdentity>,
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

        // Keep Fork execution for worktree/workspace; fresh inheritance by default.
        let mode = ExecutionMode::Fork;
        let inheritance = crate::agent::subagent::context::ContextInheritance::fresh_default();
        let mut parent_context = self.build_parent_context_with(&inheritance).await;
        if allowed_tools
            .as_ref()
            .is_some_and(|tools| !tools.is_empty())
        {
            let context = parent_context
                .get_or_insert_with(crate::agent::subagent::context::SubagentContext::empty);
            context.allowed_tools = allowed_tools;
        }
        let req = DispatchRequest {
            agent_name: target.to_string(),
            task: task.to_string(),
            mode_override: Some(mode),
            cancel,
            parent_agent: parent_label.to_string(),
            parent_context,
            delegation_policy: DispatchRequest::policy_from_depth(depth),
            runtime_context,
            message: Some(message),
            prompt_payload,
            constraints: Vec::new(),
            background: false,
        };

        let result = if let Some(identity) = identity {
            self.tools
                .subagent_executor
                .dispatch_attempt(req, identity)
                .await?
        } else {
            self.tools.subagent_executor.dispatch(req).await?
        };
        Ok(result)
    }

    /// 从当前 agent 的 external_* 字段构造 ExternalRunContext（透传给 subagent）。
    ///
    /// 这样主 agent 委派 subagent、subagent 委派 sub-subagent 时,run context 自动继承
    /// （嵌套自然继承)。current_run_id 为 None 时返回 None（无 run 上下文,旧行为）。
    #[cfg(feature = "subagent")]
    fn build_runtime_context(&self) -> Option<echo_core::tools::ExternalRunContext> {
        let run_id = self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let turn_id = self
            .external_turn_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if run_id.is_none() && turn_id.is_none() {
            return None;
        }
        Some(echo_core::tools::ExternalRunContext {
            conversation_id: self.config.conversation_id.clone(),
            run_id,
            turn_id,
            execution_id: self
                .external_execution_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            isolation_id: self
                .external_isolation_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            message_id: self
                .external_message_id
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
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
            delegation_policy: *self
                .external_delegation_policy
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        })
    }

    /// Build parent context using an explicit inheritance policy.
    ///
    /// Product defaults use [`ContextInheritance::fresh_default`]. Pass
    /// [`ContextInheritance::fork_default`] (or `ContextInheritance::for_mode`)
    /// when the caller explicitly wants parent conversation inheritance.
    /// Mirrors `ParentContextFactory::build_with_inheritance`.
    #[cfg(feature = "subagent")]
    async fn build_parent_context_with(
        &self,
        inheritance: &crate::agent::subagent::context::ContextInheritance,
    ) -> Option<crate::agent::subagent::context::SubagentContext> {
        use crate::agent::subagent::context::SubagentContext;

        let tool_defs = self.tool_definitions();
        let messages = self.memory.context.lock().await.messages().to_vec();
        let store = self.memory.store.clone();

        let ctx = SubagentContext::from_parent(&tool_defs, &messages, store, inheritance);
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

    fn token_usage_summary(&self) -> echo_core::tokenizer::UsageSummary {
        ReactAgent::token_usage_summary(self)
    }

    fn steer_input(
        &self,
        expected_turn_id: Option<&str>,
        message: crate::llm::types::Message,
    ) -> std::result::Result<String, echo_core::agent::AgentSteerError> {
        ReactAgent::steer_input(self, expected_turn_id, message)
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
            .unwrap_or_else(|e| e.into_inner()) = ctx.run_id.clone();
        *self
            .external_turn_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.turn_id.clone();
        *self
            .external_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.cancel.clone();
        *self
            .external_trace_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.trace_sink.clone();
        *self
            .external_delegation_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.delegation_policy;
        *self
            .external_execution_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.execution_id.clone();
        *self
            .external_isolation_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.isolation_id.clone();
        *self
            .external_message_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = ctx.message_id.clone();
    }

    fn clear_external_context(&self) {
        *self
            .current_run_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .external_turn_id
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
        *self
            .external_delegation_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .external_execution_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .external_isolation_id
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .external_message_id
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
            async move {
                self.run_stream_entry(task, run::StreamMode::Execute, None)
                    .await
            }
            .instrument(
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
            async move {
                self.run_stream_entry(message, run::StreamMode::Chat, None)
                    .await
            }
            .instrument(info_span!("agent_chat_stream", agent.name = %agent, agent.model = %model)),
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
                self.run_stream_entry(_message, run::StreamMode::Chat, None)
                    .await
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
                self.run_stream_entry(_task, run::StreamMode::Execute, None)
                    .await
            }
            .instrument(info_span!("agent_execute_stream_with_cancel", agent.name = %agent, agent.model = %model)),
        )
    }

    fn execute_stream_with_invocation_context<'a>(
        &'a self,
        task: &'a str,
        cancel: CancellationToken,
        mut invocation: echo_core::agent::AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        invocation.cancel = Some(cancel);
        Box::pin(
            async move {
                self.run_stream_entry(task, run::StreamMode::Execute, Some(invocation))
                    .await
            }
            .instrument(info_span!(
                "agent_execute_stream_with_invocation_context",
                agent.name = %agent,
                agent.model = %model
            )),
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
                self.run_stream_message_entry(message, run::StreamMode::Execute, None)
                    .await
            }
            .instrument(info_span!(
                "agent_execute_stream_message_with_cancel",
                agent.name = %agent,
                agent.model = %model
            )),
        )
    }

    fn execute_stream_message_with_invocation_context<'a>(
        &'a self,
        message: crate::llm::types::Message,
        cancel: CancellationToken,
        mut invocation: echo_core::agent::AgentInvocationContext,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        invocation.cancel = Some(cancel);
        Box::pin(
            async move {
                self.run_stream_message_entry(message, run::StreamMode::Execute, Some(invocation))
                    .await
            }
            .instrument(info_span!(
                "agent_execute_stream_message_with_invocation_context",
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
        let incompatible = self
            .llm_config
            .as_ref()
            .map(|config| {
                self.tools
                    .tool_manager
                    .incompatible_tool_names(&config.input_modalities)
            })
            .unwrap_or_default();
        self.tools
            .tool_manager
            .list_tools()
            .into_iter()
            .filter(|name| *name != TOOL_FINAL_ANSWER && !incompatible.contains(name))
            .map(|n| n.to_string())
            .collect()
    }

    /// Get the list of tool definitions (name, description, parameter schema).
    fn tool_definitions(&self) -> Vec<crate::llm::types::ToolDefinition> {
        let definitions = match self.llm_config.as_ref() {
            Some(config) => self
                .tools
                .tool_manager
                .get_tool_definitions_for_modalities(&config.input_modalities),
            None => self.tools.tool_manager.get_tool_definitions(),
        };
        definitions
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
        self.run_stream_message_entry(message, run::StreamMode::Chat, None)
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
        self.run_stream_message_entry(message, run::StreamMode::Execute, None)
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
                self.run_stream_message_entry(message, run::StreamMode::Chat, None)
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
        use futures::StreamExt;

        let mut stream = self.chat_stream_message(message).await?;
        let mut final_content = None;
        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::FinalAnswer(content) => final_content = Some(content),
                AgentEvent::Cancelled => {
                    return Err(
                        crate::error::AgentError::Cancelled("multimodal chat".to_string()).into(),
                    );
                }
                AgentEvent::Error { message, .. } => {
                    return Err(crate::error::ReactError::Other(message));
                }
                _ => {}
            }
        }
        final_content.ok_or_else(|| {
            crate::error::AgentError::NoResponse {
                model: self.config.model_name.clone(),
                agent: self.config.agent_name.clone(),
            }
            .into()
        })
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
