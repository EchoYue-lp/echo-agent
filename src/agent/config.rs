//! Agent 配置

use crate::agent::AgentCallback;
use crate::tools::ToolExecutionConfig;
use std::sync::Arc;

/// Agent 角色，决定其在多 Agent 系统中的职责
#[derive(Debug, Clone, PartialEq)]
pub enum AgentRole {
    /// 编排者：负责任务规划、分配和协调子 agent，不持有具体业务工具
    Orchestrator,
    /// 执行者：专注于具体任务执行，只携带业务工具，不持有任务管理/子 agent 调度能力
    Worker,
}

impl Default for AgentRole {
    fn default() -> Self {
        AgentRole::Worker
    }
}

/// Agent 运行时配置
///
/// 通过构建器链式调用设置各项参数，再传入 [`ReactAgent::new`]。
pub struct AgentConfig {
    pub(crate) model_name: String,
    pub(crate) system_prompt: String,
    verbose: bool,
    pub(crate) agent_name: String,
    /// 最大迭代轮次，防止死循环
    pub(crate) max_iterations: usize,
    /// 工具白名单（空 = 不限制，可调用所有已注册工具）
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) role: AgentRole,
    /// 是否允许注册并调用业务工具（如数学、天气等）
    pub(crate) enable_tool: bool,
    /// 是否启用任务规划能力（plan/create_task/update_task 工具）
    pub(crate) enable_task: bool,
    /// 是否启用 human-in-loop 工具
    pub(crate) enable_human_in_loop: bool,
    /// 是否启用 subagent 调度工具（agent_tool）
    pub(crate) enable_subagent: bool,
    /// 上下文 token 上限，超过时自动触发压缩（`usize::MAX` 表示不限制）
    pub(crate) token_limit: usize,
    pub callbacks: Vec<Arc<dyn AgentCallback>>,
    /// LLM 调用失败后最大重试次数（0 = 不重试，默认 3）
    pub(crate) llm_max_retries: usize,
    /// LLM 重试初始等待（毫秒），指数退避翻倍（默认 500）
    pub(crate) llm_retry_delay_ms: u64,
    /// 工具执行失败时将错误信息回传给 LLM，而非直接让 Agent 失败（默认 true）
    pub(crate) tool_error_feedback: bool,
    /// 启用思维链（CoT）系统提示注入（默认 true）。
    ///
    /// 当 `enable_tool=true` 且本字段为 `true` 时，框架自动在系统提示末尾追加一行
    /// 引导模型先推理再行动的指令，无需在每个 Agent 的 system_prompt 中手写。
    /// 设为 `false` 可完全由调用方自行控制推理引导。
    pub(crate) enable_cot: bool,
    /// 工具执行配置：超时、重试策略、并行并发度
    pub(crate) tool_execution: ToolExecutionConfig,
    /// 是否启用长期记忆 Store（remember/recall/forget 工具 + 上下文自动注入）
    pub(crate) enable_memory: bool,
    /// 长期记忆 Store 文件路径（默认 `~/.echo-agent/store.json`）
    pub(crate) memory_path: String,
    /// 会话标识，用于 Checkpointer 在跨进程启动时恢复同一对话的历史上下文。
    ///
    /// 设置后，每次 `execute()` 调用前会自动加载该会话的最新快照，
    /// 结束后自动将本轮消息写入快照。
    pub(crate) session_id: Option<String>,
    /// Checkpointer 文件路径（默认 `~/.echo-agent/checkpoints.json`）
    pub(crate) checkpointer_path: String,
}

impl AgentConfig {
    pub fn new(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self {
            model_name: model_name.to_string(),
            system_prompt: system_prompt.to_string(),
            verbose: false,
            agent_name: agent_name.to_string(),
            max_iterations: 10,
            allowed_tools: Vec::new(),
            role: AgentRole::default(),
            enable_tool: false,
            enable_task: false,
            enable_human_in_loop: false,
            enable_subagent: false,
            token_limit: usize::MAX,
            callbacks: Vec::new(),
            llm_max_retries: 3,
            llm_retry_delay_ms: 500,
            tool_error_feedback: true,
            enable_cot: true,
            tool_execution: ToolExecutionConfig::default(),
            enable_memory: false,
            memory_path: "~/.echo-agent/store.json".to_string(),
            session_id: None,
            checkpointer_path: "~/.echo-agent/checkpoints.json".to_string(),
        }
    }

    pub fn role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    pub fn enable_tool(mut self, enabled: bool) -> Self {
        self.enable_tool = enabled;
        self
    }

    pub fn enable_task(mut self, enabled: bool) -> Self {
        self.enable_task = enabled;
        self
    }

    pub fn enable_human_in_loop(mut self, enabled: bool) -> Self {
        self.enable_human_in_loop = enabled;
        self
    }

    pub fn enable_subagent(mut self, enabled: bool) -> Self {
        self.enable_subagent = enabled;
        self
    }

    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools.extend(tools);
        self
    }

    pub fn get_allowed_tools(&self) -> &[String] {
        &self.allowed_tools
    }

    pub fn is_tool_enabled(&self) -> bool {
        self.enable_tool
    }

    pub fn is_task_enabled(&self) -> bool {
        self.enable_task
    }

    pub fn is_human_in_loop_enabled(&self) -> bool {
        self.enable_human_in_loop
    }

    pub fn is_subagent_enabled(&self) -> bool {
        self.enable_subagent
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    pub fn agent_name(mut self, agent_name: &str) -> Self {
        self.agent_name = agent_name.to_string();
        self
    }

    pub fn model_name(mut self, model_name: &str) -> Self {
        self.model_name = model_name.to_string();
        self
    }

    pub fn system_prompt(mut self, system_prompt: &str) -> Self {
        self.system_prompt = system_prompt.to_string();
        self
    }

    /// 设置上下文 token 上限，超出后 `ReactAgent` 在每次 LLM 调用前自动触发压缩。
    /// 需配合 `ReactAgent::set_compressor` 一起使用，否则仅估算不压缩。
    pub fn token_limit(mut self, limit: usize) -> Self {
        self.token_limit = limit;
        self
    }

    /// 注册一个事件回调，支持链式调用
    pub fn with_callback(mut self, callback: Arc<dyn AgentCallback>) -> Self {
        self.callbacks.push(callback);
        self
    }

    /// LLM 调用失败后的最大重试次数（0 = 不重试）
    pub fn llm_max_retries(mut self, retries: usize) -> Self {
        self.llm_max_retries = retries;
        self
    }

    /// LLM 首次重试前等待的毫秒数，后续每次翻倍（指数退避）
    pub fn llm_retry_delay_ms(mut self, delay_ms: u64) -> Self {
        self.llm_retry_delay_ms = delay_ms;
        self
    }

    /// 工具失败时是否将错误信息回传 LLM（true = 让 LLM 自行纠错，false = 直接抛出异常）
    pub fn tool_error_feedback(mut self, enabled: bool) -> Self {
        self.tool_error_feedback = enabled;
        self
    }

    /// 读取当前配置的 session_id
    pub fn get_session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// 读取当前配置的 LLM 最大重试次数
    pub fn get_llm_max_retries(&self) -> usize {
        self.llm_max_retries
    }

    /// 读取当前配置的 LLM 首次重试延迟（毫秒）
    pub fn get_llm_retry_delay_ms(&self) -> u64 {
        self.llm_retry_delay_ms
    }

    /// 读取工具错误回传开关状态
    pub fn get_tool_error_feedback(&self) -> bool {
        self.tool_error_feedback
    }

    /// 启用/禁用思维链（CoT）系统提示自动注入（默认 true）。
    ///
    /// 禁用后，框架不会追加任何推理引导，完全由 system_prompt 控制。
    pub fn enable_cot(mut self, enabled: bool) -> Self {
        self.enable_cot = enabled;
        self
    }

    /// 启用/禁用长期记忆 Store（默认 false）。
    ///
    /// 启用后 Agent 自动注册 `remember`/`recall`/`forget` 三个工具，
    /// 并在每轮执行前将相关历史记忆注入上下文。
    pub fn enable_memory(mut self, enabled: bool) -> Self {
        self.enable_memory = enabled;
        self
    }

    /// 自定义长期记忆 Store 文件路径（默认 `~/.echo-agent/store.json`）
    pub fn memory_path(mut self, path: &str) -> Self {
        self.memory_path = path.to_string();
        self
    }

    /// 设置会话标识，启用 Checkpointer 跨进程对话恢复。
    ///
    /// 相同的 `session_id` 在不同进程/重启后都能恢复到同一对话历史。
    /// 若未同时配置 Checkpointer 文件路径，可通过 `ReactAgent::set_checkpointer` 注入自定义后端。
    pub fn session_id(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
        self
    }

    /// 自定义 Checkpointer 文件路径（默认 `~/.echo-agent/checkpoints.json`）
    pub fn checkpointer_path(mut self, path: &str) -> Self {
        self.checkpointer_path = path.to_string();
        self
    }

    /// 设置工具执行配置（超时、重试、并发度）。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use echo_agent::agent::react_agent::AgentConfig;
    /// use echo_agent::tools::ToolExecutionConfig;
    ///
    /// let config = AgentConfig::new("qwen3-max", "my-agent", "你是一个助手")
    ///     .enable_tool(true)
    ///     .tool_execution(ToolExecutionConfig {
    ///         timeout_ms: 10_000,   // 10 秒超时
    ///         retry_on_fail: true,
    ///         max_retries: 2,
    ///         retry_delay_ms: 300,
    ///         max_concurrency: Some(3), // 最多 3 个工具并发
    ///     });
    /// ```
    pub fn tool_execution(mut self, config: ToolExecutionConfig) -> Self {
        self.tool_execution = config;
        self
    }
}
