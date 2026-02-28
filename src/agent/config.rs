use crate::agent::AgentCallback;
use crate::tools::ToolExecutionConfig;
use std::sync::Arc;

/// Agent 角色：区分编排者和执行者
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

pub struct AgentConfig {
    /// 模型名称
    pub(crate) model_name: String,
    /// 系统提示词
    pub(crate) system_prompt: String,
    /// 是否启用详细日志
    verbose: bool,
    /// agent 名称
    pub(crate) agent_name: String,
    /// 最大迭代次数
    pub(crate) max_iterations: usize,
    /// 可使用的工具（为空表示不限制）
    pub(crate) allowed_tools: Vec<String>,
    /// agent 角色
    pub(crate) role: AgentRole,
    /// 是否允许注册并调用业务工具（如数学、天气等）
    pub(crate) enable_tool: bool,
    /// 是否启用任务能力（plan/create_task/update_task）
    pub(crate) enable_task: bool,
    /// 是否启用 human-in-loop 工具
    pub(crate) enable_human_in_loop: bool,
    /// 是否启用 subagent 调度能力（agent_tool）
    pub(crate) enable_subagent: bool,
    /// 上下文 token 上限，超过时触发压缩（`usize::MAX` 表示不限制）
    pub(crate) token_limit: usize,
    /// 事件回调系统
    pub callbacks: Vec<Arc<dyn AgentCallback>>,
    /// LLM 调用失败后的最大重试次数（0 表示不重试，默认 3）
    pub(crate) llm_max_retries: usize,
    /// LLM 重试的初始等待时间（毫秒），每次翻倍指数退避（默认 500）
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
