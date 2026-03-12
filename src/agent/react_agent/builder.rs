//! Agent 构建器

use crate::agent::{AgentCallback, AgentConfig, AgentRole};
use crate::error::Result;
use crate::human_loop::HumanLoopProvider;
use crate::llm::{LlmClient, LlmConfig, OpenAiClient};
use crate::memory::checkpointer::Checkpointer;
use crate::memory::store::Store;
use crate::prelude::ReactAgent;
use crate::tools::Tool;
use std::sync::Arc;

/// Agent 构建器
///
/// 提供流畅的 API 来配置和构建 Agent。
/// 通过 [`AgentKind`] 指定具体类型，返回 `Box<dyn Agent>` 抽象。
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
    max_iterations: usize,
    token_limit: usize,
    callbacks: Vec<Arc<dyn AgentCallback>>,
    store: Option<Arc<dyn Store>>,
    checkpointer: Option<Arc<dyn Checkpointer>>,
    session_id: Option<String>,
    approval_provider: Option<Arc<dyn HumanLoopProvider>>,
}

impl Default for ReactAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactAgentBuilder {
    /// 创建新的构建器（默认 ReAct 模式）
    pub fn new() -> Self {
        Self {
            name: "assistant".to_string(),
            model: String::new(),
            system_prompt: "你是一个有帮助的助手".to_string(),
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
            max_iterations: 10,
            token_limit: usize::MAX,
            callbacks: Vec::new(),
            store: None,
            checkpointer: None,
            session_id: None,
            approval_provider: None,
        }
    }

    // ── 预设配置 ────────────────────────────────────────────────────────────────

    /// 创建简单对话 Agent（无工具、无记忆）
    ///
    /// 适用于简单的问答场景。
    pub fn simple(model: &str, system_prompt: &str) -> Result<ReactAgent> {
        Self::new()
            .model(model)
            .system_prompt(system_prompt)
            .build()
    }

    /// 创建标准 Agent（启用工具、思维链）
    ///
    /// 适用于大多数 Agent 场景。
    pub fn standard(model: &str, name: &str, system_prompt: &str) -> Result<ReactAgent> {
        Self::new()
            .model(model)
            .name(name)
            .system_prompt(system_prompt)
            .enable_tools()
            .build()
    }

    /// 创建完整功能 Agent（工具、记忆、规划）
    ///
    /// 适用于复杂的自主 Agent 场景。
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
    // ── 基本配置 ────────────────────────────────────────────────────────────────

    /// 设置 Agent 名称
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 设置模型名称
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 设置系统提示词
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// 设置 Agent 角色
    pub fn role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    // ── LLM 配置 ────────────────────────────────────────────────────────────────

    /// 设置自定义 LLM 客户端
    ///
    /// 使用此方法可以：
    /// - 注入 Mock 客户端进行测试
    /// - 使用自定义 LLM 实现
    /// - 共享 LLM 客户端实例
    pub fn llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    /// 设置 LLM 配置（依赖注入）
    ///
    /// 用于动态配置 API 地址、密钥等，不使用环境变量。
    pub fn llm_config(mut self, config: LlmConfig) -> Self {
        self.llm_config = Some(config);
        self
    }

    /// 使用 OpenAI 客户端（便捷方法）
    ///
    /// 从环境变量读取配置。
    pub fn with_openai(mut self, model: &str) -> Result<Self> {
        let client = Arc::new(OpenAiClient::from_env(model)?);
        self.llm_client = Some(client);
        self.model = model.to_string();
        Ok(self)
    }

    // ── 工具配置 ────────────────────────────────────────────────────────────────

    /// 启用内置工具（通过 `enable_tool` 标志）
    pub fn enable_tools(mut self) -> Self {
        self.enable_builtin_tools = true;
        self
    }

    /// 禁用内置工具
    pub fn disable_tools(mut self) -> Self {
        self.enable_builtin_tools = false;
        self
    }

    /// 注册单个工具
    pub fn tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// 批量注册工具
    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools.extend(tools);
        self
    }

    // ── 功能开关 ────────────────────────────────────────────────────────────────

    /// 启用长期记忆
    pub fn enable_memory(mut self) -> Self {
        self.enable_memory = true;
        self
    }

    /// 启用任务规划
    pub fn enable_planning(mut self) -> Self {
        self.enable_task = true;
        self
    }

    /// 启用人工介入
    pub fn enable_human_in_loop(mut self) -> Self {
        self.enable_human_in_loop = true;
        self
    }

    /// 启用子 Agent 调度
    pub fn enable_subagent(mut self) -> Self {
        self.enable_subagent = true;
        self
    }

    /// 启用思维链引导
    pub fn enable_cot(mut self) -> Self {
        self.enable_cot = true;
        self
    }

    /// 禁用思维链引导
    pub fn disable_cot(mut self) -> Self {
        self.enable_cot = false;
        self
    }

    // ── 执行参数 ────────────────────────────────────────────────────────────────

    /// 设置最大迭代次数
    pub fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// 设置 token 上限
    pub fn token_limit(mut self, limit: usize) -> Self {
        self.token_limit = limit;
        self
    }

    // ── 回调与扩展 ──────────────────────────────────────────────────────────────

    /// 添加回调
    pub fn callback(mut self, callback: Arc<dyn AgentCallback>) -> Self {
        self.callbacks.push(callback);
        self
    }

    /// 设置长期记忆 Store
    pub fn store(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// 设置 Checkpointer（同时设置 session_id）
    pub fn checkpointer(
        mut self,
        checkpointer: Arc<dyn Checkpointer>,
        session_id: impl Into<String>,
    ) -> Self {
        self.checkpointer = Some(checkpointer);
        self.session_id = Some(session_id.into());
        self
    }

    /// 设置 Checkpointer（使用已设置的 session_id）
    /// 需要先调用 session_id() 设置会话标识
    pub fn checkpointer_only(mut self, checkpointer: Arc<dyn Checkpointer>) -> Self {
        self.checkpointer = Some(checkpointer);
        self
    }

    /// 设置 session_id（会话标识）
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// 设置审批 Provider
    pub fn approval_provider(mut self, provider: Arc<dyn HumanLoopProvider>) -> Self {
        self.approval_provider = Some(provider);
        self
    }

    // ── 构建 ────────────────────────────────────────────────────────────────────

    /// 构建 ReAct Agent（内部方法）
    pub fn build(self) -> Result<ReactAgent> {
        let mut config = AgentConfig::new(&self.model, &self.name, &self.system_prompt)
            .role(self.role)
            .enable_tool(self.enable_builtin_tools)
            .enable_memory(self.enable_memory)
            .enable_task(self.enable_task)
            .enable_human_in_loop(self.enable_human_in_loop)
            .enable_subagent(self.enable_subagent)
            .enable_cot(self.enable_cot)
            .max_iterations(self.max_iterations)
            .token_limit(self.token_limit);

        for callback in self.callbacks {
            config = config.with_callback(callback);
        }

        if let Some(session_id) = &self.session_id {
            config = config.session_id(session_id);
        }

        let mut agent = crate::agent::react_agent::ReactAgent::new(config);

        // 注入 LLM 配置
        if let Some(llm_config) = self.llm_config {
            agent.set_llm_config(llm_config);
        }

        // 注册自定义工具
        for tool in self.tools {
            agent.add_tool(tool);
        }

        // 设置 Store
        if let Some(store) = self.store {
            agent.set_memory_store(store);
        }

        // 设置 Checkpointer
        if let (Some(checkpointer), Some(session_id)) = (self.checkpointer, self.session_id) {
            agent.set_checkpointer(checkpointer, session_id);
        }

        // 设置审批 Provider
        if let Some(provider) = self.approval_provider {
            agent.set_approval_provider(provider);
        }

        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_basic() {
        let builder = ReactAgentBuilder::new()
            .name("test-agent")
            .model("qwen3-max")
            .system_prompt("测试");

        assert_eq!(builder.name, "test-agent");
        assert_eq!(builder.model, "qwen3-max");
        assert_eq!(builder.system_prompt, "测试");
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
            .system_prompt("测试")
            .enable_tools();

        assert!(builder.enable_builtin_tools);
    }
}
