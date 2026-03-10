//! Agent 配置

use crate::agent::AgentCallback;
use crate::llm::ResponseFormat;
use crate::tools::ToolExecutionConfig;
use std::sync::Arc;

/// Agent 角色，决定其在多 Agent 系统中的职责
#[derive(Default, Debug, Clone, PartialEq)]
pub enum AgentRole {
    /// 编排者：负责任务规划、分配和协调子 agent，不持有具体业务工具
    Orchestrator,
    /// 执行者：专注于具体任务执行，只携带业务工具，不持有任务管理/子 agent 调度能力
    #[default]
    Worker,
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
    pub(crate) enable_cot: bool,
    /// 工具执行配置：超时、重试策略、并行并发度
    pub(crate) tool_execution: ToolExecutionConfig,
    /// 是否启用长期记忆 Store（remember/recall/forget 工具 + 上下文自动注入）
    pub(crate) enable_memory: bool,
    /// 长期记忆 Store 文件路径（默认 `~/.echo-agent/store.json`）
    pub(crate) memory_path: String,
    /// 会话标识，用于 Checkpointer 在跨进程启动时恢复同一对话的历史上下文。
    pub(crate) session_id: Option<String>,
    /// Checkpointer 文件路径（默认 `~/.echo-agent/checkpoints.json`）
    pub(crate) checkpointer_path: String,
    /// 结构化输出格式（None = 默认文本）
    pub(crate) response_format: Option<ResponseFormat>,
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
            response_format: None,
        }
    }

    // ── 预设配置（易用性优化）──────────────────────────────────────────────────────

    /// 创建最小配置的 Agent（无工具、无记忆）
    ///
    /// 适用于简单的对话场景。
    pub fn minimal(model_name: &str, system_prompt: &str) -> Self {
        Self::new(model_name, "assistant", system_prompt)
            .enable_tool(false)
            .enable_memory(false)
            .enable_cot(false)
    }

    /// 创建标准配置的 Agent（启用工具、思维链）
    ///
    /// 适用于大多数 Agent 场景。
    pub fn standard(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self::new(model_name, agent_name, system_prompt)
            .enable_tool(true)
            .enable_cot(true)
    }

    /// 创建完整功能的 Agent（工具、记忆、规划）
    ///
    /// 适用于复杂的自主 Agent 场景。
    pub fn full_featured(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self::new(model_name, agent_name, system_prompt)
            .enable_tool(true)
            .enable_memory(true)
            .enable_task(true)
            .enable_cot(true)
    }

    /// 启用所有功能（工具、记忆、规划）- Builder 链式调用版本
    pub fn with_full_features(mut self) -> Self {
        self.enable_tool = true;
        self.enable_memory = true;
        self.enable_task = true;
        self.enable_cot = true;
        self
    }

    /// 启用基本工具功能（工具 + 思维链）- Builder 链式调用版本
    pub fn with_tools(mut self) -> Self {
        self.enable_tool = true;
        self.enable_cot = true;
        self
    }

    // ── 原有 Builder 方法 ──────────────────────────────────────────────────────────

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

    pub fn token_limit(mut self, limit: usize) -> Self {
        self.token_limit = limit;
        self
    }

    pub fn with_callback(mut self, callback: Arc<dyn AgentCallback>) -> Self {
        self.callbacks.push(callback);
        self
    }

    pub fn llm_max_retries(mut self, retries: usize) -> Self {
        self.llm_max_retries = retries;
        self
    }

    pub fn llm_retry_delay_ms(mut self, delay_ms: u64) -> Self {
        self.llm_retry_delay_ms = delay_ms;
        self
    }

    pub fn tool_error_feedback(mut self, enabled: bool) -> Self {
        self.tool_error_feedback = enabled;
        self
    }

    pub fn get_session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn get_llm_max_retries(&self) -> usize {
        self.llm_max_retries
    }

    pub fn get_llm_retry_delay_ms(&self) -> u64 {
        self.llm_retry_delay_ms
    }

    pub fn get_tool_error_feedback(&self) -> bool {
        self.tool_error_feedback
    }

    pub fn enable_cot(mut self, enabled: bool) -> Self {
        self.enable_cot = enabled;
        self
    }

    pub fn enable_memory(mut self, enabled: bool) -> Self {
        self.enable_memory = enabled;
        self
    }

    pub fn memory_path(mut self, path: &str) -> Self {
        self.memory_path = path.to_string();
        self
    }

    pub fn session_id(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
        self
    }

    pub fn checkpointer_path(mut self, path: &str) -> Self {
        self.checkpointer_path = path.to_string();
        self
    }

    pub fn tool_execution(mut self, config: ToolExecutionConfig) -> Self {
        self.tool_execution = config;
        self
    }

    pub fn response_format(mut self, fmt: ResponseFormat) -> Self {
        self.response_format = Some(fmt);
        self
    }
}
