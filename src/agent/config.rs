//! Agent configuration

use crate::agent::AgentCallback;
use crate::agent::react::loop_detector::LoopDetectorConfig;
use crate::llm::ResponseFormat;
use crate::tools::ToolExecutionConfig;
use echo_core::budget::TokenBudgetConfig;
use std::path::PathBuf;
use std::sync::Arc;

/// Default token limit for agent context (128000 tokens, matches TokenBudget default).
pub const DEFAULT_TOKEN_LIMIT: usize = 128_000;

/// Agent role enum, determining its responsibility scope in a multi-agent system.
///
/// # Current usage
///
/// - `Orchestrator`: Used in `TaskExecutor::build_execute_fn` (`react/planning.rs`).
///   The orchestrator prioritizes dispatching tasks to registered SubAgents
///   rather than calling the LLM directly.
///   Suitable for the "leader" role in multi-agent collaboration scenarios.
///
/// - `Worker` (default): Executes tasks directly via LLM without dispatching to SubAgents.
///   Suitable for agents that independently perform specific tasks.
///
/// # Note
///
/// This role field currently **only** affects behavior in the TaskExecutor's execution logic.
/// It has no additional effect in other modules (ReactAgent, PlanExecute, etc.).
#[derive(Default, Debug, Clone, PartialEq)]
pub enum AgentRole {
    /// Orchestrator: responsible for task planning, allocation, and coordinating sub-agents; does not hold business tools.
    /// Prioritizes dispatching to SubAgents in TaskExecutor.
    Orchestrator,
    /// Worker (default): focuses on specific task execution, only carries business tools,
    /// does not hold task management/sub-agent scheduling capabilities. Executes tasks directly via LLM.
    #[default]
    Worker,
}

/// Agent runtime configuration
///
/// Configure parameters via builder chain calls, then pass to `ReactAgent::new`.
pub struct AgentConfig {
    pub(crate) model_name: String,
    pub(crate) system_prompt: String,
    pub(crate) agent_name: String,
    /// Maximum iteration rounds, prevents infinite loops (default: 100, effectively unlimited for most tasks)
    pub(crate) max_iterations: usize,
    /// Tool allowlist (empty = no restriction, all registered tools can be called)
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) role: AgentRole,
    /// Whether to allow registering and calling business tools (e.g., math, weather, etc.)
    pub(crate) enable_tool: bool,
    /// Whether to enable task planning capability (plan/create_task/update_task tools)
    pub(crate) enable_task: bool,
    /// Whether to enable human-in-loop tool
    pub(crate) enable_human_in_loop: bool,
    /// Whether to enable subagent dispatch tool (agent_tool)
    pub(crate) enable_subagent: bool,
    /// Context token limit; auto-triggers compression when exceeded (`usize::MAX` means no limit)
    pub(crate) token_limit: usize,
    /// Streaming channel buffer size (default 256). When full, events are dropped with a warning.
    pub(crate) stream_buffer_size: usize,
    pub(crate) callbacks: Vec<Arc<dyn AgentCallback>>,
    /// Maximum retry count after LLM call failure (0 = no retry, default 3)
    pub(crate) llm_max_retries: usize,
    /// LLM retry initial delay (ms), doubles with exponential backoff (default 500)
    pub(crate) llm_retry_delay_ms: u64,
    /// On tool execution failure, feed the error back to the LLM instead of failing the Agent directly (default true)
    pub(crate) tool_error_feedback: bool,
    /// Enable chain-of-thought (CoT) system prompt injection (default false).
    pub(crate) enable_cot: bool,
    /// Require that a file be explicitly read (via `read_file`) before any
    /// write/edit/delete operation on it. When enabled, tools like
    /// `edit_file`, `write_file`, and `delete_file` will reject paths
    /// that haven't been read in the current conversation turn. (default false)
    pub(crate) force_read_before_edit: bool,
    /// Whether the agent is in plan mode (read-only tools only).
    pub(crate) plan_mode: bool,
    /// Reasoning effort: low(quick)/medium(standard)/high(thorough).
    pub(crate) _reasoning_effort: String,
    /// Tool execution config: timeout, retry strategy, parallel concurrency
    pub(crate) tool_execution: ToolExecutionConfig,
    /// Whether to enable long-term memory Store (remember/recall/forget tools + automatic context injection)
    pub(crate) enable_memory: bool,
    /// Long-term memory Store file path (default `~/.echo-agent/store.json`)
    pub(crate) memory_path: String,
    /// Session identifier for process-local/logical run grouping.
    ///
    /// Runtime crash recovery uses `conversation_id` together with
    /// [`crate::state::RuntimeStateStore`]; this field is purely an
    /// in-process label and does not drive restore behavior.
    pub(crate) session_id: Option<String>,
    /// Conversation identifier, used by ConversationStore to persist transcript/history projections.
    pub(crate) conversation_id: Option<String>,
    /// Structured output format (None = default text)
    pub(crate) response_format: Option<ResponseFormat>,
    /// Maximum token count for a single tool output (None = no limit).
    /// Automatically truncated when exceeded, with a `[Output truncated, N tokens total]` hint appended.
    pub(crate) max_tool_output_tokens: Option<usize>,
    /// When available token ratio falls below this threshold, proactively trigger compression before think().
    /// Value range 0.0–1.0, default 0.2 (i.e., triggers when less than 20% remains).
    pub(crate) compress_threshold_ratio: f64,
    /// LLM temperature parameter (0.0–2.0, None means use model default)
    pub(crate) temperature: Option<f32>,
    /// Maximum generation token count (None means use model default)
    pub(crate) max_tokens: Option<u32>,
    /// Stable user identifier sent to providers for KVCache isolation.
    /// DeepSeek etc. use this to partition prompt cache. A stable, session-scoped
    /// value enables >90% cache hit rate; without it, every request caches separately.
    pub(crate) cache_user_id: Option<String>,
    /// Whether to automatically load project rules file (`.echo-agent/AGENT.md`), default true
    pub(crate) auto_project_rules: bool,
    /// Working directory (for searching project rules files), None means use current directory.
    /// Mutex enables `&self` updates during resume (BUG-3).
    pub(crate) working_dir: std::sync::Mutex<Option<PathBuf>>,
    /// Token budget configuration for fine-grained context window management
    pub(crate) token_budget_config: TokenBudgetConfig,
    /// Whether to enable notebook tracking for reproducibility (default false).
    /// When enabled, each tool invocation is recorded as a NotebookCell
    /// that can be exported as Markdown or JSON.
    pub(crate) enable_notebook: bool,
    /// Loop detection configuration.
    pub(crate) loop_detector_config: LoopDetectorConfig,
    /// Permission mode for tool execution (default, auto-edit, full-auto, strict).
    pub(crate) permission_mode: String,
    /// How often to checkpoint the React loop state (in iterations).
    /// 0 = only checkpoint at end of execution (default).
    /// 1 = checkpoint every iteration.
    /// N = checkpoint every N iterations.
    ///
    /// When a background task crashes mid-execution, the Agent can resume
    /// from the last checkpointed iteration because the conversation history
    /// (including all tool calls and results) is preserved.
    pub(crate) react_checkpoint_interval: usize,

    /// Whether the verifier (Critic) is enabled for final_answer validation.
    pub(crate) verifier_enabled: bool,
    /// Minimum score (0.0-10.0) required for the verifier to pass.
    pub(crate) verifier_min_score: f64,
    /// Maximum number of verifier retry attempts before accepting the answer.
    pub(crate) verifier_max_retries: usize,

    /// Planning policy for framework-driven planning mode triggers.
    pub(crate) planning_policy: echo_orchestration::planning::PlanningPolicy,
}

impl AgentConfig {
    /// Create a new Agent configuration
    ///
    /// # Parameters
    /// * `model_name` - LLM model name to use (corresponds to model identifier in config)
    /// * `agent_name` - Agent name, used for identification and logging
    /// * `system_prompt` - System prompt, defines the Agent's role and capabilities
    ///
    /// # Returns
    /// Returns a default AgentConfig instance; further configuration via chain calls
    pub fn new(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self {
            model_name: model_name.to_string(),
            system_prompt: system_prompt.to_string(),
            agent_name: agent_name.to_string(),
            max_iterations: 100,
            allowed_tools: Vec::new(),
            role: AgentRole::default(),
            enable_tool: false,
            enable_task: false,
            enable_human_in_loop: false,
            enable_subagent: false,
            token_limit: DEFAULT_TOKEN_LIMIT,
            stream_buffer_size: 256,
            callbacks: Vec::new(),
            llm_max_retries: 3,
            llm_retry_delay_ms: 500,
            tool_error_feedback: true,
            enable_cot: false,
            force_read_before_edit: false,
            plan_mode: false,
            _reasoning_effort: "medium".to_string(),
            tool_execution: ToolExecutionConfig::default(),
            enable_memory: false,
            memory_path: "~/.echo-agent/store.json".to_string(),
            session_id: None,
            conversation_id: None,
            response_format: None,
            max_tool_output_tokens: None,
            compress_threshold_ratio: 0.2,
            temperature: None,
            max_tokens: None,
            cache_user_id: None,
            auto_project_rules: true,
            working_dir: std::sync::Mutex::new(None),
            token_budget_config: TokenBudgetConfig::default(),
            enable_notebook: false,
            loop_detector_config: LoopDetectorConfig::default(),
            permission_mode: "default".to_string(),
            react_checkpoint_interval: 0,
            verifier_enabled: false,
            verifier_min_score: 7.0,
            verifier_max_retries: 2,
            planning_policy: echo_orchestration::planning::PlanningPolicy::default(),
        }
    }

    // ── Preset Configurations (usability optimizations) ───────────────────────────────

    /// Create a minimal Agent (no tools, no memory)
    ///
    /// Suitable for simple conversation scenarios.
    pub fn minimal(model_name: &str, system_prompt: &str) -> Self {
        Self::new(model_name, "assistant", system_prompt)
            .enable_tool(false)
            .enable_memory(false)
            .enable_cot(false)
    }

    /// Create a standard Agent (tools + chain-of-thought enabled)
    ///
    /// Suitable for most Agent scenarios.
    pub fn standard(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self::new(model_name, agent_name, system_prompt)
            .enable_tool(true)
            .enable_cot(true)
    }

    /// Create a full-featured Agent (tools, memory, planning)
    ///
    /// Suitable for complex autonomous Agent scenarios.
    pub fn full_featured(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self::new(model_name, agent_name, system_prompt)
            .enable_tool(true)
            .enable_memory(true)
            .enable_task(true)
            .enable_cot(true)
    }

    /// Enable all features (tools, memory, planning) - Builder chain call version
    pub fn with_full_features(mut self) -> Self {
        self.enable_tool = true;
        self.enable_memory = true;
        self.enable_task = true;
        self.enable_cot = true;
        self
    }

    /// Enable basic tool features (tools + chain-of-thought) - Builder chain call version
    pub fn with_tools(mut self) -> Self {
        self.enable_tool = true;
        self.enable_cot = true;
        self
    }

    // ── Original Builder Methods ─────────────────────────────────────────────────────

    /// Set Agent role
    ///
    /// # Parameters
    /// * `role` - Agent role (`AgentRole::Orchestrator` or `AgentRole::Worker`)
    ///
    /// # Description
    /// - `Orchestrator`: orchestrator role, responsible for task planning, allocation, and coordinating sub-agents
    /// - `Worker`: worker role, focused on specific task execution
    pub fn role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    /// Enable or disable tool calling
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable tool calling, `false` to disable
    ///
    /// # Description
    /// When enabled, the Agent can call registered business tools (e.g., math, file operations, etc.)
    pub fn enable_tool(mut self, enabled: bool) -> Self {
        self.enable_tool = enabled;
        self
    }

    /// Enable or disable task planning capability
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable task planning, `false` to disable
    ///
    /// # Description
    /// When enabled, the Agent can use task management tools such as `plan`, `create_task`, `update_task`
    pub fn enable_task(mut self, enabled: bool) -> Self {
        self.enable_task = enabled;
        self
    }

    /// Enable or disable Human-in-the-Loop functionality
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable human interaction, `false` to disable
    ///
    /// # Description
    /// When enabled, the Agent can request human intervention via the `human_in_loop` tool when approval or confirmation is needed
    pub fn enable_human_in_loop(mut self, enabled: bool) -> Self {
        self.enable_human_in_loop = enabled;
        self
    }

    /// Enable or disable subagent dispatch
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable subagent dispatch, `false` to disable
    ///
    /// # Description
    /// When enabled, the Agent can use the `agent_tool` tool to dispatch other sub-agents for task execution
    pub fn enable_subagent(mut self, enabled: bool) -> Self {
        self.enable_subagent = enabled;
        self
    }

    /// Set tool allowlist
    ///
    /// # Parameters
    /// * `tools` - List of allowed tool names
    ///
    /// # Description
    /// - If the list is empty, no restriction; all registered tools can be called
    /// - If the list is non-empty, only tools in the list can be called
    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools.extend(tools);
        self
    }

    /// Get tool allowlist
    ///
    /// # Returns
    /// Returns a slice of currently allowed tool names
    pub fn get_allowed_tools(&self) -> &[String] {
        &self.allowed_tools
    }

    /// Check if tool calling is enabled
    ///
    /// # Returns
    /// `true` if tool calling is enabled, `false` if disabled
    pub fn is_tool_enabled(&self) -> bool {
        self.enable_tool
    }

    /// Check if task planning is enabled
    ///
    /// # Returns
    /// `true` if task planning is enabled, `false` if disabled
    pub fn is_task_enabled(&self) -> bool {
        self.enable_task
    }

    /// Check if Human-in-the-Loop is enabled
    ///
    /// # Returns
    /// `true` if human-in-the-loop is enabled, `false` if disabled
    pub fn is_human_in_loop_enabled(&self) -> bool {
        self.enable_human_in_loop
    }

    /// Check if subagent dispatch is enabled
    ///
    /// # Returns
    /// `true` if subagent dispatch is enabled, `false` if disabled
    pub fn is_subagent_enabled(&self) -> bool {
        self.enable_subagent
    }

    /// Set maximum iteration rounds
    ///
    /// # Parameters
    /// * `max_iterations` - Maximum iteration count, prevents infinite loops (default: 100)
    ///
    /// # Description
    /// The Agent performs at most the specified number of iterations during execution; exceeding this limit terminates execution.
    /// Default is 100, which is effectively unlimited for most tasks. Set to a lower value for stricter control.
    pub fn max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// Set Agent name
    ///
    /// # Parameters
    /// * `agent_name` - Agent name, used for identification and logging
    pub fn agent_name(mut self, agent_name: &str) -> Self {
        self.agent_name = agent_name.to_string();
        self
    }

    /// Set LLM model name
    ///
    /// # Parameters
    /// * `model_name` - LLM model name to use (corresponds to model identifier in config)
    pub fn model_name(mut self, model_name: &str) -> Self {
        self.model_name = model_name.to_string();
        self
    }

    /// Set model name at runtime (mutable reference version)
    pub fn set_model_name(&mut self, model_name: &str) {
        self.model_name = model_name.to_string();
    }

    /// Set a stable cache user_id for KVCache isolation (DeepSeek, etc.).
    /// A stable session-scoped id enables the provider to reuse prompt cache
    /// across requests. Without this, cache hit rate can drop to <1%.
    pub fn cache_user_id(mut self, id: impl Into<String>) -> Self {
        self.cache_user_id = Some(id.into());
        self
    }

    /// Set cache_user_id at runtime.
    pub fn set_cache_user_id(&mut self, id: impl Into<String>) {
        self.cache_user_id = Some(id.into());
    }

    /// Set system prompt
    ///
    /// # Parameters
    /// * `system_prompt` - System prompt, defines the Agent's role and capabilities
    pub fn system_prompt(mut self, system_prompt: &str) -> Self {
        self.system_prompt = system_prompt.to_string();
        self
    }

    /// Set context token limit
    ///
    /// # Parameters
    /// * `limit` - Context token limit; auto-triggers compression when exceeded (`usize::MAX` means no limit)
    pub fn token_limit(mut self, limit: usize) -> Self {
        self.token_limit = limit;
        self
    }

    /// Add Agent callback
    ///
    /// # Parameters
    /// * `callback` - Callback instance implementing the `AgentCallback` trait
    ///
    /// # Description
    /// Callbacks are invoked when different events are triggered during Agent execution, for monitoring, logging, etc.
    pub fn with_callback(mut self, callback: Arc<dyn AgentCallback>) -> Self {
        self.callbacks.push(callback);
        self
    }

    /// Set maximum retry count after LLM call failure
    ///
    /// # Parameters
    /// * `retries` - Maximum retry count (0 = no retry, default 3)
    pub fn llm_max_retries(mut self, retries: usize) -> Self {
        self.llm_max_retries = retries;
        self
    }

    /// Set LLM retry initial delay
    ///
    /// # Parameters
    /// * `delay_ms` - Initial delay (milliseconds), doubles with exponential backoff (default 500)
    pub fn llm_retry_delay_ms(mut self, delay_ms: u64) -> Self {
        self.llm_retry_delay_ms = delay_ms;
        self
    }

    /// Enable or disable tool error feedback
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable tool error feedback, `false` to disable
    ///
    /// # Description
    /// When enabled, tool execution failures feed the error back to the LLM instead of failing the Agent directly
    pub fn tool_error_feedback(mut self, enabled: bool) -> Self {
        self.tool_error_feedback = enabled;
        self
    }

    /// Get session identifier
    ///
    /// # Returns
    /// Reference to session identifier, or `None` if not set
    ///
    /// # Description
    /// The session identifier is a lightweight run/thread label. Durable runtime
    /// recovery uses `conversation_id` with `RuntimeStateStore`.
    pub fn get_session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Get conversation identifier
    ///
    /// # Returns
    /// Reference to conversation identifier, or `None` if not set
    ///
    /// # Description
    /// The conversation identifier is used for `ConversationStore` transcript/history projections
    /// and `RuntimeStateStore` crash-recovery checkpoints. It is distinct from
    /// `session_id`, which is only a run/thread label.
    pub fn get_conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    /// Get maximum retry count after LLM call failure
    ///
    /// # Returns
    /// Maximum retry count
    pub fn get_llm_max_retries(&self) -> usize {
        self.llm_max_retries
    }

    /// Get LLM retry initial delay
    ///
    /// # Returns
    /// Initial delay (milliseconds)
    pub fn get_llm_retry_delay_ms(&self) -> u64 {
        self.llm_retry_delay_ms
    }

    /// Get tool error feedback setting
    ///
    /// # Returns
    /// `true` if tool error feedback is enabled, `false` if disabled
    pub fn get_tool_error_feedback(&self) -> bool {
        self.tool_error_feedback
    }

    /// Get maximum iteration rounds
    ///
    /// # Returns
    /// Maximum iteration count
    pub fn get_max_iterations(&self) -> usize {
        self.max_iterations
    }

    /// Get context token limit
    ///
    /// # Returns
    /// Context token limit, `usize::MAX` means no limit
    pub fn get_token_limit(&self) -> usize {
        self.token_limit
    }

    /// Check if chain-of-thought (CoT) is enabled
    ///
    /// # Returns
    /// `true` if CoT is enabled, `false` if disabled
    pub fn is_cot_enabled(&self) -> bool {
        self.enable_cot
    }

    /// Check if long-term memory is enabled
    ///
    /// # Returns
    /// `true` if long-term memory is enabled, `false` if disabled
    pub fn is_memory_enabled(&self) -> bool {
        self.enable_memory
    }

    /// Get long-term memory store file path
    ///
    /// # Returns
    /// Long-term memory store file path
    pub fn get_memory_path(&self) -> &str {
        &self.memory_path
    }

    /// Get tool execution configuration
    ///
    /// # Returns
    /// Reference to tool execution configuration (includes timeout, retry strategy, parallel concurrency, etc.)
    pub fn get_tool_execution(&self) -> &crate::tools::ToolExecutionConfig {
        &self.tool_execution
    }

    /// Get structured output format
    ///
    /// # Returns
    /// Reference to structured output format, or `None` if not set
    pub fn get_response_format(&self) -> Option<&crate::llm::ResponseFormat> {
        self.response_format.as_ref()
    }

    /// Get LLM model name
    ///
    /// # Returns
    /// LLM model name
    pub fn get_model_name(&self) -> &str {
        &self.model_name
    }

    /// Get system prompt
    ///
    /// # Returns
    /// System prompt
    pub fn get_system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Get Agent name
    ///
    /// # Returns
    /// Agent name
    pub fn get_agent_name(&self) -> &str {
        &self.agent_name
    }

    /// Enable or disable chain-of-thought (CoT)
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable CoT, `false` to disable
    ///
    /// # Description
    /// When CoT is enabled, the Agent injects CoT-related instructions into the system prompt
    pub fn enable_cot(mut self, enabled: bool) -> Self {
        self.enable_cot = enabled;
        self
    }

    /// Enable or disable long-term memory
    ///
    /// # Parameters
    /// * `enabled` - `true` to enable long-term memory, `false` to disable
    ///
    /// # Description
    /// When enabled, the Agent can use remember/recall/forget tools, with automatic context injection support
    pub fn enable_memory(mut self, enabled: bool) -> Self {
        self.enable_memory = enabled;
        self
    }

    /// Enable or disable notebook tracking for reproducibility
    ///
    /// # Parameters
    /// * `enable` - `true` to enable notebook tracking, `false` to disable
    ///
    /// # Description
    /// When enabled, each tool invocation is recorded as a `NotebookCell`,
    /// and the full session can be exported as Markdown or JSON.
    pub fn enable_notebook(mut self, enable: bool) -> Self {
        self.enable_notebook = enable;
        self
    }

    /// Set long-term memory store file path
    ///
    /// # Parameters
    /// * `path` - Long-term memory store file path
    pub fn memory_path(mut self, path: &str) -> Self {
        self.memory_path = path.to_string();
        self
    }

    /// Set session identifier
    ///
    /// # Parameters
    /// * `id` - Session identifier
    ///
    /// # Description
    /// The session identifier is a lightweight run/thread label. Durable runtime
    /// recovery uses `conversation_id` with `RuntimeStateStore`.
    pub fn session_id(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
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

    /// Enable or disable the verifier (Critic) for final_answer validation.
    pub fn verifier_enabled(mut self, enabled: bool) -> Self {
        self.verifier_enabled = enabled;
        self
    }

    /// Set the minimum score (0.0-10.0) required for the verifier to pass.
    pub fn verifier_min_score(mut self, score: f64) -> Self {
        self.verifier_min_score = score;
        self
    }

    /// Set the maximum number of verifier retry attempts.
    pub fn verifier_max_retries(mut self, retries: usize) -> Self {
        self.verifier_max_retries = retries;
        self
    }

    /// Set the planning policy for framework-driven planning mode triggers.
    pub fn planning_policy(mut self, policy: echo_orchestration::planning::PlanningPolicy) -> Self {
        self.planning_policy = policy;
        self
    }

    /// Set conversation identifier
    ///
    /// # Parameters
    /// * `id` - Conversation identifier
    ///
    /// # Description
    /// The conversation identifier is used by `ConversationStore` to persist transcript/history projections.
    /// Unlike `session_id`, it does not handle thread state restoration.
    pub fn conversation_id(mut self, id: &str) -> Self {
        self.conversation_id = Some(id.to_string());
        self
    }

    /// Set tool execution configuration
    ///
    /// # Parameters
    /// * `config` - Tool execution configuration (includes timeout, retry strategy, parallel concurrency, etc.)
    pub fn tool_execution(mut self, config: ToolExecutionConfig) -> Self {
        self.tool_execution = config;
        self
    }

    /// Set structured output format
    ///
    /// # Parameters
    /// * `fmt` - Structured output format
    pub fn response_format(mut self, fmt: ResponseFormat) -> Self {
        self.response_format = Some(fmt);
        self
    }

    /// Set maximum token count for a single tool output; automatically truncated when exceeded
    pub fn max_tool_output_tokens(mut self, max: usize) -> Self {
        self.max_tool_output_tokens = Some(max);
        self
    }

    /// Get maximum token count for a single tool output
    ///
    /// # Returns
    /// Maximum token count, `None` means no limit
    pub fn get_max_tool_output_tokens(&self) -> Option<usize> {
        self.max_tool_output_tokens
    }

    /// Set proactive compression threshold ratio (0.0–1.0), default 0.2
    pub fn compress_threshold_ratio(mut self, ratio: f64) -> Self {
        self.compress_threshold_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// Get proactive compression threshold ratio
    ///
    /// # Returns
    /// Compression threshold ratio (0.0–1.0), default 0.2
    pub fn get_compress_threshold_ratio(&self) -> f64 {
        self.compress_threshold_ratio
    }

    /// Enable or disable automatic project rules loading
    ///
    /// # Parameters
    /// * `enabled` - `true` to automatically search for `.echo-agent/AGENT.md` in the working directory and inject into system prompt
    pub fn auto_project_rules(mut self, enabled: bool) -> Self {
        self.auto_project_rules = enabled;
        self
    }

    /// Set working directory (for searching project rules files)
    ///
    /// # Parameters
    /// * `path` - Working directory path, None means use current directory
    pub fn working_dir(self, path: Option<PathBuf>) -> Self {
        *self.working_dir.lock().unwrap() = path;
        self
    }

    /// Set token budget configuration for fine-grained context window management.
    pub fn token_budget(mut self, config: TokenBudgetConfig) -> Self {
        self.token_budget_config = config;
        self
    }

    /// Set LLM temperature parameter
    ///
    /// # Parameters
    /// * `temperature` - Temperature value (0.0–2.0, None means use model default)
    pub fn temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Get LLM temperature parameter
    ///
    /// # Returns
    /// Temperature value, `None` means use model default
    pub fn get_temperature(&self) -> Option<f32> {
        self.temperature
    }

    /// Set maximum generation token count
    ///
    /// # Parameters
    /// * `max_tokens` - Maximum token count (None means use model default)
    pub fn max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Get maximum generation token count
    ///
    /// # Returns
    /// Maximum token count, `None` means use model default
    pub fn get_max_tokens(&self) -> Option<u32> {
        self.max_tokens
    }

    /// Set loop detector configuration.
    pub fn loop_detector(mut self, config: LoopDetectorConfig) -> Self {
        self.loop_detector_config = config;
        self
    }

    /// Get loop detector configuration.
    pub fn get_loop_detector_config(&self) -> &LoopDetectorConfig {
        &self.loop_detector_config
    }

    /// Set the permission mode (default, auto-edit, full-auto, strict).
    pub fn permission_mode(mut self, mode: &str) -> Self {
        self.permission_mode = normalize_permission_mode(mode).to_string();
        self
    }

    /// Get the current permission mode.
    pub fn get_permission_mode(&self) -> &str {
        &self.permission_mode
    }

    /// Set the permission mode at runtime (mutable reference).
    pub fn set_permission_mode(&mut self, mode: &str) {
        self.permission_mode = normalize_permission_mode(mode).to_string();
    }
}

fn normalize_permission_mode(mode: &str) -> &str {
    match mode {
        "plan" | "auto" => "default",
        _ => mode,
    }
}

// ── Unit Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_new() {
        let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");

        assert_eq!(config.get_model_name(), "qwen3-max");
        assert_eq!(config.get_agent_name(), "assistant");
        assert_eq!(config.get_system_prompt(), "You are a helpful assistant");
        assert_eq!(config.get_max_iterations(), 100);
        assert_eq!(config.get_token_limit(), DEFAULT_TOKEN_LIMIT);
        assert!(!config.is_tool_enabled());
        assert!(!config.is_task_enabled());
        assert!(!config.is_human_in_loop_enabled());
        assert!(!config.is_subagent_enabled());
    }

    #[test]
    fn test_agent_config_minimal() {
        let config = AgentConfig::minimal("qwen3-max", "Be helpful");

        assert_eq!(config.get_model_name(), "qwen3-max");
        assert!(!config.is_tool_enabled());
        assert!(!config.is_memory_enabled());
        assert!(!config.is_cot_enabled());
    }

    #[test]
    fn test_agent_config_standard() {
        let config = AgentConfig::standard("qwen3-max", "agent1", "You are helpful");

        assert!(config.is_tool_enabled());
        assert!(config.is_cot_enabled());
    }

    #[test]
    fn test_agent_config_full_featured() {
        let config = AgentConfig::full_featured("qwen3-max", "agent1", "You are helpful");

        assert!(config.is_tool_enabled());
        assert!(config.is_memory_enabled());
        assert!(config.is_task_enabled());
        assert!(config.is_cot_enabled());
    }

    #[test]
    fn test_agent_config_builder_chain() {
        let config = AgentConfig::new("model", "agent", "prompt")
            .max_iterations(20)
            .token_limit(DEFAULT_TOKEN_LIMIT)
            .enable_tool(true)
            .enable_task(true)
            .enable_human_in_loop(true)
            .enable_subagent(true)
            .enable_memory(true)
            .enable_cot(false)
            .llm_max_retries(5)
            .llm_retry_delay_ms(1000)
            .tool_error_feedback(false);

        assert_eq!(config.get_max_iterations(), 20);
        assert_eq!(config.get_token_limit(), DEFAULT_TOKEN_LIMIT);
        assert!(config.is_tool_enabled());
        assert!(config.is_task_enabled());
        assert!(config.is_human_in_loop_enabled());
        assert!(config.is_subagent_enabled());
        assert!(config.is_memory_enabled());
        assert!(!config.is_cot_enabled());
        assert_eq!(config.get_llm_max_retries(), 5);
        assert_eq!(config.get_llm_retry_delay_ms(), 1000);
        assert!(!config.get_tool_error_feedback());
    }

    #[test]
    fn test_permission_mode_legacy_aliases_normalize_to_default() {
        let mut config = AgentConfig::new("model", "agent", "prompt").permission_mode("auto");
        assert_eq!(config.get_permission_mode(), "default");

        config.set_permission_mode("plan");
        assert_eq!(config.get_permission_mode(), "default");
    }

    #[test]
    fn test_agent_config_allowed_tools() {
        let config = AgentConfig::new("model", "agent", "prompt")
            .allowed_tools(vec!["tool1".to_string(), "tool2".to_string()]);

        assert_eq!(config.get_allowed_tools(), &["tool1", "tool2"]);
    }

    #[test]
    fn test_agent_config_session_id() {
        let config = AgentConfig::new("model", "agent", "prompt").session_id("session-123");

        assert_eq!(config.get_session_id(), Some("session-123"));
    }

    #[test]
    fn test_agent_config_conversation_id() {
        let config =
            AgentConfig::new("model", "agent", "prompt").conversation_id("conversation-123");

        assert_eq!(config.get_conversation_id(), Some("conversation-123"));
    }

    #[test]
    fn test_agent_config_role() {
        let config = AgentConfig::new("model", "agent", "prompt").role(AgentRole::Orchestrator);

        assert_eq!(config.role, AgentRole::Orchestrator);
    }

    #[test]
    fn test_agent_config_model_name_mutation() {
        let mut config = AgentConfig::new("model1", "agent", "prompt");

        config.set_model_name("model2");
        assert_eq!(config.get_model_name(), "model2");
    }

    #[test]
    fn test_agent_config_with_full_features() {
        let config = AgentConfig::new("model", "agent", "prompt").with_full_features();

        assert!(config.is_tool_enabled());
        assert!(config.is_memory_enabled());
        assert!(config.is_task_enabled());
        assert!(config.is_cot_enabled());
    }

    #[test]
    fn test_agent_config_with_tools() {
        let config = AgentConfig::new("model", "agent", "prompt").with_tools();

        assert!(config.is_tool_enabled());
        assert!(config.is_cot_enabled());
    }

    #[test]
    fn test_agent_config_memory_path() {
        let config =
            AgentConfig::new("model", "agent", "prompt").memory_path("/custom/path/store.json");

        assert_eq!(config.get_memory_path(), "/custom/path/store.json");
    }

    #[test]
    fn test_agent_role_default() {
        assert_eq!(AgentRole::default(), AgentRole::Worker);
    }
}
