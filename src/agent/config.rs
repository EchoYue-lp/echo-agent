//! Agent 配置

use crate::agent::AgentCallback;
use crate::llm::ResponseFormat;
use crate::tools::ToolExecutionConfig;
use std::sync::Arc;

/// Agent 角色枚举，决定其在多 Agent 系统中的职责范围。
///
/// # 当前用途
///
/// - `Orchestrator`：在 `TaskExecutor::build_execute_fn`（`react/planning.rs`）中使用。
///   编排者会优先将任务分派给已注册的 SubAgent 执行，而非直接调用 LLM。
///   适用于多 Agent 协作场景中的"领导者"角色。
///
/// - `Worker`（默认）：直接通过 LLM 执行任务，不尝试分派给 SubAgent。
///   适用于独立执行具体任务的 Agent。
///
/// # 注意
///
/// 该角色字段目前**仅**在 TaskExecutor 的执行逻辑中影响行为。
/// 在其他模块（ReactAgent、PlanExecute 等）中不产生额外效果。
#[derive(Default, Debug, Clone, PartialEq)]
pub enum AgentRole {
    /// 编排者：负责任务规划、分配和协调子 agent，不持有具体业务工具。
    /// 在 TaskExecutor 中会优先分派给 SubAgent。
    Orchestrator,
    /// 执行者（默认）：专注于具体任务执行，只携带业务工具，
    /// 不持有任务管理/子 agent 调度能力。直接通过 LLM 执行任务。
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
    pub(crate) callbacks: Vec<Arc<dyn AgentCallback>>,
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
    /// 对话标识，用于 ConversationStore 持久化 transcript/history 投影。
    pub(crate) conversation_id: Option<String>,
    /// Checkpointer 文件路径（默认 `~/.echo-agent/checkpoints.json`）
    pub(crate) checkpointer_path: String,
    /// 结构化输出格式（None = 默认文本）
    pub(crate) response_format: Option<ResponseFormat>,
    /// 单次工具输出的最大 token 数（None = 不限制）。
    /// 超限时自动截断并在末尾追加 `[输出已截断，共 N tokens]` 提示。
    pub(crate) max_tool_output_tokens: Option<usize>,
    /// 当可用 token 余量低于此比例时，在 think() 前主动触发压缩。
    /// 取值 0.0–1.0，默认 0.2（即剩余不到 20% 时触发）。
    pub(crate) compress_threshold_ratio: f64,
}

impl AgentConfig {
    /// 创建新的 Agent 配置
    ///
    /// # 参数
    /// * `model_name` - 使用的 LLM 模型名称（对应配置中的模型标识）
    /// * `agent_name` - Agent 的名称，用于标识和日志输出
    /// * `system_prompt` - 系统提示词，定义 Agent 的角色和能力
    ///
    /// # 返回值
    /// 返回默认配置的 AgentConfig 实例，后续可通过链式调用进一步配置
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
            conversation_id: None,
            checkpointer_path: "~/.echo-agent/checkpoints.json".to_string(),
            response_format: None,
            max_tool_output_tokens: None,
            compress_threshold_ratio: 0.2,
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

    /// 设置 Agent 角色
    ///
    /// # 参数
    /// * `role` - Agent 角色（`AgentRole::Orchestrator` 或 `AgentRole::Worker`）
    ///
    /// # 说明
    /// - `Orchestrator`：编排者角色，负责任务规划、分配和协调子 agent
    /// - `Worker`：执行者角色，专注于具体任务执行
    pub fn role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    /// 启用或禁用工具调用功能
    ///
    /// # 参数
    /// * `enabled` - `true` 启用工具调用，`false` 禁用
    ///
    /// # 说明
    /// 启用后 Agent 可以调用已注册的业务工具（如数学计算、文件操作等）
    pub fn enable_tool(mut self, enabled: bool) -> Self {
        self.enable_tool = enabled;
        self
    }

    /// 启用或禁用任务规划能力
    ///
    /// # 参数
    /// * `enabled` - `true` 启用任务规划，`false` 禁用
    ///
    /// # 说明
    /// 启用后 Agent 可以使用 `plan`、`create_task`、`update_task` 等任务管理工具
    pub fn enable_task(mut self, enabled: bool) -> Self {
        self.enable_task = enabled;
        self
    }

    /// 启用或禁用人工介入（Human-in-the-Loop）功能
    ///
    /// # 参数
    /// * `enabled` - `true` 启用人机交互，`false` 禁用
    ///
    /// # 说明
    /// 启用后 Agent 在需要审批或确认时可以通过 `human_in_loop` 工具请求人工介入
    pub fn enable_human_in_loop(mut self, enabled: bool) -> Self {
        self.enable_human_in_loop = enabled;
        self
    }

    /// 启用或禁用子代理调度功能
    ///
    /// # 参数
    /// * `enabled` - `true` 启用子代理调度，`false` 禁用
    ///
    /// # 说明
    /// 启用后 Agent 可以使用 `agent_tool` 工具调度其他子 Agent 执行任务
    pub fn enable_subagent(mut self, enabled: bool) -> Self {
        self.enable_subagent = enabled;
        self
    }

    /// 设置工具白名单
    ///
    /// # 参数
    /// * `tools` - 允许调用的工具名称列表
    ///
    /// # 说明
    /// - 如果列表为空，则不限制，可调用所有已注册工具
    /// - 如果列表非空，则只能调用列表中的工具
    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools.extend(tools);
        self
    }

    /// 获取工具白名单
    ///
    /// # 返回值
    /// 返回当前允许调用的工具名称切片
    pub fn get_allowed_tools(&self) -> &[String] {
        &self.allowed_tools
    }

    /// 检查工具调用功能是否启用
    ///
    /// # 返回值
    /// `true` 表示工具调用已启用，`false` 表示已禁用
    pub fn is_tool_enabled(&self) -> bool {
        self.enable_tool
    }

    /// 检查任务规划能力是否启用
    ///
    /// # 返回值
    /// `true` 表示任务规划已启用，`false` 表示已禁用
    pub fn is_task_enabled(&self) -> bool {
        self.enable_task
    }

    /// 检查人工介入（Human-in-the-Loop）功能是否启用
    ///
    /// # 返回值
    /// `true` 表示人工介入功能已启用，`false` 表示已禁用
    pub fn is_human_in_loop_enabled(&self) -> bool {
        self.enable_human_in_loop
    }

    /// 检查子代理调度功能是否启用
    ///
    /// # 返回值
    /// `true` 表示子代理调度功能已启用，`false` 表示已禁用
    pub fn is_subagent_enabled(&self) -> bool {
        self.enable_subagent
    }

    /// 启用或禁用详细日志输出
    ///
    /// # 参数
    /// * `verbose` - `true` 启用详细日志，`false` 禁用
    ///
    /// # 说明
    /// 启用后 Agent 会输出更详细的执行过程日志
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// 设置最大迭代轮次
    ///
    /// # 参数
    /// * `max_iterations` - 最大迭代次数，防止死循环
    ///
    /// # 说明
    /// Agent 在执行过程中最多进行指定次数的迭代，超过此限制会终止执行
    pub fn max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// 设置 Agent 名称
    ///
    /// # 参数
    /// * `agent_name` - Agent 的名称，用于标识和日志输出
    pub fn agent_name(mut self, agent_name: &str) -> Self {
        self.agent_name = agent_name.to_string();
        self
    }

    /// 设置 LLM 模型名称
    ///
    /// # 参数
    /// * `model_name` - 使用的 LLM 模型名称（对应配置中的模型标识）
    pub fn model_name(mut self, model_name: &str) -> Self {
        self.model_name = model_name.to_string();
        self
    }

    /// 运行时设置模型名称（可变引用版本）
    pub fn set_model_name(&mut self, model_name: &str) {
        self.model_name = model_name.to_string();
    }

    /// 设置系统提示词
    ///
    /// # 参数
    /// * `system_prompt` - 系统提示词，定义 Agent 的角色和能力
    pub fn system_prompt(mut self, system_prompt: &str) -> Self {
        self.system_prompt = system_prompt.to_string();
        self
    }

    /// 设置上下文 token 上限
    ///
    /// # 参数
    /// * `limit` - 上下文 token 上限，超过时自动触发压缩（`usize::MAX` 表示不限制）
    pub fn token_limit(mut self, limit: usize) -> Self {
        self.token_limit = limit;
        self
    }

    /// 添加 Agent 回调
    ///
    /// # 参数
    /// * `callback` - 实现了 `AgentCallback` trait 的回调实例
    ///
    /// # 说明
    /// 回调会在 Agent 执行过程中触发不同事件时被调用，用于监控、日志记录等
    pub fn with_callback(mut self, callback: Arc<dyn AgentCallback>) -> Self {
        self.callbacks.push(callback);
        self
    }

    /// 设置 LLM 调用失败后最大重试次数
    ///
    /// # 参数
    /// * `retries` - 最大重试次数（0 = 不重试，默认 3）
    pub fn llm_max_retries(mut self, retries: usize) -> Self {
        self.llm_max_retries = retries;
        self
    }

    /// 设置 LLM 重试初始等待时间
    ///
    /// # 参数
    /// * `delay_ms` - 初始等待时间（毫秒），指数退避翻倍（默认 500）
    pub fn llm_retry_delay_ms(mut self, delay_ms: u64) -> Self {
        self.llm_retry_delay_ms = delay_ms;
        self
    }

    /// 启用或禁用工具错误反馈
    ///
    /// # 参数
    /// * `enabled` - `true` 启用工具错误反馈，`false` 禁用
    ///
    /// # 说明
    /// 启用后，工具执行失败时将错误信息回传给 LLM，而非直接让 Agent 失败
    pub fn tool_error_feedback(mut self, enabled: bool) -> Self {
        self.tool_error_feedback = enabled;
        self
    }

    /// 获取会话标识
    ///
    /// # 返回值
    /// 会话标识的引用，如果没有设置则返回 `None`
    ///
    /// # 说明
    /// 会话标识用于 Checkpointer 在跨进程启动时恢复同一对话的历史上下文
    pub fn get_session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// 获取对话标识
    ///
    /// # 返回值
    /// 对话标识的引用，如果没有设置则返回 `None`
    ///
    /// # 说明
    /// 对话标识用于 `ConversationStore` 中的 transcript/history 投影；
    /// 它与用于恢复线程状态的 `session_id` 是两个独立概念。
    pub fn get_conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    /// 获取 LLM 调用失败后最大重试次数
    ///
    /// # 返回值
    /// 最大重试次数
    pub fn get_llm_max_retries(&self) -> usize {
        self.llm_max_retries
    }

    /// 获取 LLM 重试初始等待时间
    ///
    /// # 返回值
    /// 初始等待时间（毫秒）
    pub fn get_llm_retry_delay_ms(&self) -> u64 {
        self.llm_retry_delay_ms
    }

    /// 获取工具错误反馈设置状态
    ///
    /// # 返回值
    /// `true` 表示工具错误反馈已启用，`false` 表示已禁用
    pub fn get_tool_error_feedback(&self) -> bool {
        self.tool_error_feedback
    }

    /// 获取最大迭代轮次
    ///
    /// # 返回值
    /// 最大迭代次数
    pub fn get_max_iterations(&self) -> usize {
        self.max_iterations
    }

    /// 获取上下文 token 上限
    ///
    /// # 返回值
    /// 上下文 token 上限，`usize::MAX` 表示不限制
    pub fn get_token_limit(&self) -> usize {
        self.token_limit
    }

    /// 检查思维链（CoT）是否启用
    ///
    /// # 返回值
    /// `true` 表示思维链已启用，`false` 表示已禁用
    pub fn is_cot_enabled(&self) -> bool {
        self.enable_cot
    }

    /// 检查长期记忆是否启用
    ///
    /// # 返回值
    /// `true` 表示长期记忆已启用，`false` 表示已禁用
    pub fn is_memory_enabled(&self) -> bool {
        self.enable_memory
    }

    /// 获取长期记忆存储文件路径
    ///
    /// # 返回值
    /// 长期记忆存储文件路径
    pub fn get_memory_path(&self) -> &str {
        &self.memory_path
    }

    /// 获取检查点文件路径
    ///
    /// # 返回值
    /// 检查点文件路径
    pub fn get_checkpointer_path(&self) -> &str {
        &self.checkpointer_path
    }

    /// 获取工具执行配置
    ///
    /// # 返回值
    /// 工具执行配置的引用（包含超时、重试策略、并行并发度等设置）
    pub fn get_tool_execution(&self) -> &crate::tools::ToolExecutionConfig {
        &self.tool_execution
    }

    /// 获取结构化输出格式
    ///
    /// # 返回值
    /// 结构化输出格式的引用，如果没有设置则返回 `None`
    pub fn get_response_format(&self) -> Option<&crate::llm::ResponseFormat> {
        self.response_format.as_ref()
    }

    /// 获取 LLM 模型名称
    ///
    /// # 返回值
    /// LLM 模型名称
    pub fn get_model_name(&self) -> &str {
        &self.model_name
    }

    /// 获取系统提示词
    ///
    /// # 返回值
    /// 系统提示词
    pub fn get_system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// 获取 Agent 名称
    ///
    /// # 返回值
    /// Agent 名称
    pub fn get_agent_name(&self) -> &str {
        &self.agent_name
    }

    /// 检查是否启用详细日志输出
    ///
    /// # 返回值
    /// `true` 表示详细日志已启用，`false` 表示已禁用
    pub fn is_verbose(&self) -> bool {
        self.verbose
    }

    /// 启用或禁用思维链（CoT）
    ///
    /// # 参数
    /// * `enabled` - `true` 启用思维链，`false` 禁用
    ///
    /// # 说明
    /// 启用思维链后，Agent 会在系统提示词中注入 CoT 相关指令
    pub fn enable_cot(mut self, enabled: bool) -> Self {
        self.enable_cot = enabled;
        self
    }

    /// 启用或禁用长期记忆
    ///
    /// # 参数
    /// * `enabled` - `true` 启用长期记忆，`false` 禁用
    ///
    /// # 说明
    /// 启用长期记忆后，Agent 可以使用 remember/recall/forget 工具，并支持上下文自动注入
    pub fn enable_memory(mut self, enabled: bool) -> Self {
        self.enable_memory = enabled;
        self
    }

    /// 设置长期记忆存储文件路径
    ///
    /// # 参数
    /// * `path` - 长期记忆存储文件路径
    pub fn memory_path(mut self, path: &str) -> Self {
        self.memory_path = path.to_string();
        self
    }

    /// 设置会话标识
    ///
    /// # 参数
    /// * `id` - 会话标识
    ///
    /// # 说明
    /// 会话标识用于 Checkpointer 在跨进程启动时恢复同一对话的历史上下文
    pub fn session_id(mut self, id: &str) -> Self {
        self.session_id = Some(id.to_string());
        self
    }

    /// 设置对话标识
    ///
    /// # 参数
    /// * `id` - 对话标识
    ///
    /// # 说明
    /// 对话标识用于 `ConversationStore` 持久化 transcript/history 投影，
    /// 与 `session_id` 不同，它不承担线程状态恢复职责。
    pub fn conversation_id(mut self, id: &str) -> Self {
        self.conversation_id = Some(id.to_string());
        self
    }

    /// 设置检查点文件路径
    ///
    /// # 参数
    /// * `path` - 检查点文件路径
    pub fn checkpointer_path(mut self, path: &str) -> Self {
        self.checkpointer_path = path.to_string();
        self
    }

    /// 设置工具执行配置
    ///
    /// # 参数
    /// * `config` - 工具执行配置（包含超时、重试策略、并行并发度等设置）
    pub fn tool_execution(mut self, config: ToolExecutionConfig) -> Self {
        self.tool_execution = config;
        self
    }

    /// 设置结构化输出格式
    ///
    /// # 参数
    /// * `fmt` - 结构化输出格式
    pub fn response_format(mut self, fmt: ResponseFormat) -> Self {
        self.response_format = Some(fmt);
        self
    }

    /// 设置单次工具输出的最大 token 数，超限自动截断
    pub fn max_tool_output_tokens(mut self, max: usize) -> Self {
        self.max_tool_output_tokens = Some(max);
        self
    }

    /// 获取单次工具输出的最大 token 数
    ///
    /// # 返回值
    /// 最大 token 数，`None` 表示不限制
    pub fn get_max_tool_output_tokens(&self) -> Option<usize> {
        self.max_tool_output_tokens
    }

    /// 设置主动压缩阈值比例（0.0–1.0），默认 0.2
    pub fn compress_threshold_ratio(mut self, ratio: f64) -> Self {
        self.compress_threshold_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// 获取主动压缩阈值比例
    ///
    /// # 返回值
    /// 压缩阈值比例（0.0–1.0），默认 0.2
    pub fn get_compress_threshold_ratio(&self) -> f64 {
        self.compress_threshold_ratio
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_new() {
        let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");

        assert_eq!(config.get_model_name(), "qwen3-max");
        assert_eq!(config.get_agent_name(), "assistant");
        assert_eq!(config.get_system_prompt(), "You are a helpful assistant");
        assert_eq!(config.get_max_iterations(), 10);
        assert_eq!(config.get_token_limit(), usize::MAX);
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
            .token_limit(8000)
            .enable_tool(true)
            .enable_task(true)
            .enable_human_in_loop(true)
            .enable_subagent(true)
            .enable_memory(true)
            .enable_cot(false)
            .llm_max_retries(5)
            .llm_retry_delay_ms(1000)
            .tool_error_feedback(false)
            .verbose(true);

        assert_eq!(config.get_max_iterations(), 20);
        assert_eq!(config.get_token_limit(), 8000);
        assert!(config.is_tool_enabled());
        assert!(config.is_task_enabled());
        assert!(config.is_human_in_loop_enabled());
        assert!(config.is_subagent_enabled());
        assert!(config.is_memory_enabled());
        assert!(!config.is_cot_enabled());
        assert_eq!(config.get_llm_max_retries(), 5);
        assert_eq!(config.get_llm_retry_delay_ms(), 1000);
        assert!(!config.get_tool_error_feedback());
        assert!(config.is_verbose());
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
    fn test_agent_config_checkpointer_path() {
        let config = AgentConfig::new("model", "agent", "prompt")
            .checkpointer_path("/custom/path/checkpoints.json");

        assert_eq!(
            config.get_checkpointer_path(),
            "/custom/path/checkpoints.json"
        );
    }

    #[test]
    fn test_agent_role_default() {
        assert_eq!(AgentRole::default(), AgentRole::Worker);
    }
}
