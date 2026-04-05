//! Agent 构建器

use crate::agent::{AgentCallback, AgentConfig, AgentRole};
use crate::audit::AuditLogger;
use crate::error::Result;
use crate::guard::{Guard, GuardManager};
#[cfg(feature = "human-loop")]
use crate::human_loop::{HumanLoopProvider, PermissionService};
use crate::llm::{LlmClient, LlmConfig, OpenAiClient, ResponseFormat};
use crate::memory::checkpointer::Checkpointer;
use crate::memory::snapshot::{SnapshotManager, SnapshotPolicy};
use crate::memory::store::Store;
use crate::prelude::ReactAgent;
use crate::tools::Tool;
use crate::tools::permission::PermissionPolicy;
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
    #[cfg(feature = "human-loop")]
    approval_provider: Option<Arc<dyn HumanLoopProvider>>,
    #[cfg(feature = "human-loop")]
    permission_service: Option<Arc<PermissionService>>,
    guards: Vec<Arc<dyn Guard>>,
    permission_policy: Option<Arc<dyn PermissionPolicy>>,
    audit_logger: Option<Arc<dyn AuditLogger>>,
    snapshot_policy: Option<SnapshotPolicy>,
    max_snapshots: usize,
    response_format: Option<ResponseFormat>,
    max_tool_output_tokens: Option<usize>,
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
            #[cfg(feature = "human-loop")]
            approval_provider: None,
            #[cfg(feature = "human-loop")]
            permission_service: None,
            guards: Vec::new(),
            permission_policy: None,
            audit_logger: None,
            snapshot_policy: None,
            max_snapshots: 10,
            response_format: None,
            max_tool_output_tokens: None,
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

    // ── 结构化输出 ──────────────────────────────────────────────────────────────

    /// 声明 Agent 的结构化输出类型
    ///
    /// 自动根据 `T` 的 [`JsonSchema`](schemars::JsonSchema) 生成 `response_format`，
    /// 配合 [`ReactAgent::execute_typed`] 使用可直接获得反序列化后的结果。
    ///
    /// # 示例
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

    /// 手动设置响应格式
    pub fn response_format(mut self, fmt: ResponseFormat) -> Self {
        self.response_format = Some(fmt);
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

    /// 设置单次工具输出的最大 token 数
    ///
    /// 工具输出超过此限制时自动截断，并在尾部追加 `[输出已截断，共 N tokens]`。
    /// 防止单次工具调用撑爆上下文窗口。
    pub fn max_tool_output_tokens(mut self, max: usize) -> Self {
        self.max_tool_output_tokens = Some(max);
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

    /// 注入外部 Store 并自动注册 remember / recall / search_memory / forget 四个内置 Tool
    ///
    /// 这是从"有记忆存储"到"Agent 自主使用记忆"的快捷方式，
    /// 等价于 `.store(store).enable_memory()`，但支持传入任意 `Store` 实现
    /// （如 `EmbeddingStore`），无需依赖默认的 `FileStore`。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::prelude::*;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> echo_agent::error::Result<()> {
    /// let store = Arc::new(InMemoryStore::new());
    /// let agent = ReactAgentBuilder::new()
    ///     .model("qwen3-max")
    ///     .with_memory_tools(store)
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_memory_tools(mut self, store: Arc<dyn Store>) -> Self {
        self.store = Some(store);
        self.enable_memory = true;
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

    #[cfg(feature = "human-loop")]
    /// 设置审批 Provider
    pub fn approval_provider(mut self, provider: Arc<dyn HumanLoopProvider>) -> Self {
        self.approval_provider = Some(provider);
        self
    }

    #[cfg(feature = "human-loop")]
    /// 设置统一权限服务
    ///
    /// 一旦设置，将优先使用此服务进行权限检查，
    /// 回退到旧的 PermissionPolicy + HumanApprovalManager 逻辑。
    pub fn permission_service(mut self, service: Arc<PermissionService>) -> Self {
        self.permission_service = Some(service);
        self
    }

    // ── 护栏 & 权限 & 审计 ──────────────────────────────────────────────────────

    /// 添加护栏
    pub fn guard(mut self, guard: Arc<dyn Guard>) -> Self {
        self.guards.push(guard);
        self
    }

    /// 批量添加护栏
    pub fn guards(mut self, guards: Vec<Arc<dyn Guard>>) -> Self {
        self.guards.extend(guards);
        self
    }

    /// 设置工具权限策略
    pub fn permission_policy(mut self, policy: Arc<dyn PermissionPolicy>) -> Self {
        self.permission_policy = Some(policy);
        self
    }

    /// 设置审计日志记录器
    pub fn audit_logger(mut self, logger: Arc<dyn AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    // ── 快照配置 ────────────────────────────────────────────────────────────────

    /// 设置快照策略，启用状态快照功能
    ///
    /// 启用后，ReAct 循环的每轮迭代可自动捕获对话历史快照，
    /// 异常时可通过 `agent.rollback(n)` 回滚到之前的 known-good 状态。
    pub fn snapshot_policy(mut self, policy: SnapshotPolicy) -> Self {
        self.snapshot_policy = Some(policy);
        self
    }

    /// 设置最大快照保留数量（默认 10）
    pub fn max_snapshots(mut self, max: usize) -> Self {
        self.max_snapshots = max;
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

        if let Some(fmt) = self.response_format {
            config = config.response_format(fmt);
        }
        if let Some(max) = self.max_tool_output_tokens {
            config = config.max_tool_output_tokens(max);
        }

        for callback in self.callbacks {
            config = config.with_callback(callback);
        }

        if let Some(session_id) = &self.session_id {
            config = config.session_id(session_id);
        }

        // 当用户通过 with_memory_tools(store) 传入自定义 Store 时，
        // 跳过 ReactAgent::new() 内部的 FileStore 自动初始化，
        // 改由 build() 阶段手动注入用户提供的 Store。
        let has_external_store = self.store.is_some();
        if has_external_store {
            config = config.enable_memory(false);
        }

        let mut agent = crate::agents::react::ReactAgent::new(config);

        // 注入 LLM 配置
        if let Some(llm_config) = self.llm_config {
            agent.set_llm_config(llm_config);
        }

        // 注册自定义工具
        for tool in self.tools {
            agent.add_tool(tool);
        }

        // 设置 Store（同时注册 remember/recall/search_memory/forget 工具）
        if let Some(store) = self.store {
            agent.set_memory_store(store);
        }

        // 设置 Checkpointer
        if let (Some(checkpointer), Some(session_id)) = (self.checkpointer, self.session_id) {
            agent.set_checkpointer(checkpointer, session_id);
        }

        #[cfg(feature = "human-loop")]
        if let Some(provider) = self.approval_provider {
            agent.set_approval_provider(provider);
        }

        #[cfg(feature = "human-loop")]
        if let Some(service) = self.permission_service {
            agent.set_permission_service(service);
        }

        // 设置护栏
        if !self.guards.is_empty() {
            agent.set_guard_manager(GuardManager::from_guards(self.guards));
        }

        // 设置权限策略
        if let Some(policy) = self.permission_policy {
            agent.set_permission_policy(policy);
        }

        // 设置审计日志
        if let Some(logger) = self.audit_logger {
            agent.set_audit_logger(logger);
        }

        // 设置快照管理器
        if let Some(policy) = self.snapshot_policy {
            agent.set_snapshot_manager(SnapshotManager::new(policy, self.max_snapshots));
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
