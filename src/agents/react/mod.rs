//! ReAct Agent 核心模块
//!
//! ## 模块结构
//!
//! | 文件 | 职责 |
//! |------|------|
//! | `mod.rs` | 结构体定义、`new()`、`impl Agent` trait |
//! | `run.rs` | 执行引擎（`think` / `process_steps` / `run_react_loop`） |
//! | `capabilities.rs` | 能力配置（工具 / Skill / MCP / SubAgent 注册） |
//! | `extract.rs` | 结构化 JSON 提取（`extract_json` / `extract`） |

pub use crate::agent::config::{AgentConfig, AgentRole};
use crate::agent::{Agent, AgentEvent, CancellationToken};
use crate::agents::subagent::SubagentRegistry;
use crate::agents::subagent::executor::{SubagentExecutor, SubagentExecutorConfig};
use crate::compression::ContextManager;
use crate::error::{LlmError, ReactError, Result};
use crate::guard::GuardManager;
#[cfg(feature = "human-loop")]
#[allow(deprecated)] // HumanApprovalManager kept for backward compatibility
use crate::human_loop::{HumanApprovalManager, HumanLoopProvider, PermissionService};
use crate::llm::config::LlmConfig;
#[cfg(feature = "mcp")]
use crate::mcp::McpManager;
use crate::memory::checkpointer::{Checkpointer, FileCheckpointer};
use crate::memory::snapshot::{SnapshotManager, StateSnapshot};
use crate::memory::store::{FileStore, Store};
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
use std::sync::{Arc, RwLock};

pub mod builder;
mod capabilities;
mod extract;
#[cfg(feature = "tasks")]
mod planning;
mod run;
pub mod structured;
#[cfg(test)]
mod tests;
// ── 内置工具名常量 ─────────────────────────────────────────────────────────────

pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";
#[cfg(feature = "tasks")]
pub(crate) const TOOL_CREATE_TASK: &str = "create_task";
#[cfg(feature = "tasks")]
pub(crate) const TOOL_PLAN: &str = "plan";
#[cfg(feature = "tasks")]
pub(crate) const TOOL_UPDATE_TASK: &str = "update_task";

/// 判断 LLM 错误是否值得重试（网络/超时/限流/服务端 5xx）
pub(crate) fn is_retryable_llm_error(err: &ReactError) -> bool {
    match err {
        ReactError::Llm(LlmError::NetworkError(_)) => true,
        ReactError::Llm(LlmError::ApiError { status, .. }) => *status == 429 || *status >= 500,
        _ => false,
    }
}

// ── ReactAgent 结构体 ─────────────────────────────────────────────────────────

pub struct ReactAgent {
    pub(crate) config: AgentConfig,
    /// 上下文管理器：维护对话历史，并在 token 超限时自动触发压缩
    pub(crate) context: ContextManager,
    tool_manager: ToolManager,
    /// Subagent 注册表：管理子代理的定义、发现和生命周期
    pub(crate) subagent_registry: Arc<SubagentRegistry>,
    /// Subagent 执行器：统一调度 Sync/Fork/Teammate 模式
    #[allow(dead_code)] // Used by AgentDispatchTool at construction; accessor TBD
    pub(crate) subagent_executor: Arc<SubagentExecutor>,
    client: Arc<Client>,
    /// LLM 配置（可选，不设置时使用环境变量配置）
    llm_config: Option<LlmConfig>,
    #[cfg(feature = "tasks")]
    pub(crate) task_manager: Arc<TaskManager>,
    #[cfg(feature = "human-loop")]
    #[allow(deprecated)]
    human_in_loop: Arc<RwLock<HumanApprovalManager>>,
    #[cfg(feature = "human-loop")]
    approval_provider: Arc<dyn HumanLoopProvider>,
    /// Skill 注册表：管理 code-based 和 file-based skills
    skill_registry: SkillRegistry,
    /// Hook registry for skill-defined tool call interception
    pub(crate) hook_registry: Arc<tokio::sync::RwLock<HookRegistry>>,
    /// 长期记忆 Store，通过 `remember`/`recall`/`forget` 工具访问
    store: Option<Arc<dyn Store>>,
    /// 短期会话 Checkpointer，按 session_id 持久化对话历史
    checkpointer: Option<Arc<dyn Checkpointer>>,
    #[cfg(feature = "mcp")]
    mcp_manager: McpManager,
    /// 护栏管理器：对输入/输出进行安全过滤
    pub(crate) guard_manager: Option<GuardManager>,
    /// 权限策略：控制工具执行权限
    pub(crate) permission_policy: Option<Arc<dyn crate::tools::permission::PermissionPolicy>>,
    /// 统一权限服务：整合 mode/rules/hooks/classifier/handler
    #[cfg(feature = "human-loop")]
    pub(crate) permission_service: Option<Arc<PermissionService>>,
    /// 审计日志记录器
    pub(crate) audit_logger: Option<Arc<dyn crate::audit::AuditLogger>>,
    /// 状态快照管理器，支持每轮迭代自动快照和回滚
    pub(crate) snapshot_manager: Option<SnapshotManager>,
    /// 对话持久化 Store，支持对话保存、恢复和历史管理
    pub(crate) conversation_store: Option<Arc<dyn crate::memory::conversation::ConversationStore>>,
    /// 熔断器：LLM 持续不可用时快速失败，防止无效重试
    pub(crate) circuit_breaker: Option<Arc<CircuitBreaker>>,
}

// ── 构造与初始化 ──────────────────────────────────────────────────────────────

impl ReactAgent {
    #[cfg(feature = "tasks")]
    pub(crate) fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && [TOOL_PLAN, TOOL_CREATE_TASK, TOOL_UPDATE_TASK]
                .iter()
                .all(|name| self.tool_manager.get_tool(name).is_some())
    }

    #[cfg(not(feature = "tasks"))]
    #[allow(dead_code)]
    pub(crate) fn has_planning_tools(&self) -> bool {
        false
    }

    /// 工具调用场景下自动注入的思维链引导语。
    const COT_INSTRUCTION: &'static str = "在调用工具之前，先用文字简述你的分析思路和执行计划。";

    pub fn new(config: AgentConfig) -> Self {
        let system_prompt = if config.enable_tool && config.enable_cot {
            format!(
                "{}\n\n{}",
                config.system_prompt.trim_end(),
                Self::COT_INSTRUCTION,
            )
        } else {
            config.system_prompt.clone()
        };

        let context = ContextManager::builder(config.token_limit)
            .with_system(system_prompt)
            .build();

        let mut tool_manager = ToolManager::new_with_config(config.tool_execution.clone());
        let client = reqwest::Client::new();

        tool_manager.register(Box::new(FinalAnswerTool));

        #[cfg(feature = "tasks")]
        let task_manager = Arc::new(TaskManager::default());
        #[cfg(feature = "human-loop")]
        #[allow(deprecated)]
        let human_in_loop = Arc::new(RwLock::new(HumanApprovalManager::default()));
        let subagent_registry = Arc::new(SubagentRegistry::new());
        let subagent_executor = Arc::new(SubagentExecutor::new(
            subagent_registry.clone(),
            SubagentExecutorConfig::default(),
        ));
        #[cfg(feature = "human-loop")]
        let approval_provider = crate::human_loop::default_provider();

        #[cfg(feature = "human-loop")]
        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop::new(approval_provider.clone())));
        }

        #[cfg(feature = "tasks")]
        if config.enable_task {
            tool_manager.register(Box::new(PlanTool));
            tool_manager.register(Box::new(CreateTaskTool::new(task_manager.clone())));
            tool_manager.register(Box::new(ListTasksTool::new(task_manager.clone())));
            tool_manager.register(Box::new(UpdateTaskTool::new(task_manager.clone())));
            tool_manager.register(Box::new(VisualizeDependenciesTool::new(
                task_manager.clone(),
            )));
            tool_manager.register(Box::new(GetExecutionOrderTool::new(task_manager.clone())));
        }
        if config.enable_subagent {
            tool_manager.register(Box::new(AgentDispatchTool::new(
                subagent_executor.clone(),
                config.agent_name.clone(),
                CancellationToken::new(),
            )));
        }

        // 注册媒体工具（图片分析、PDF 处理、Excel、Word）— 仅在 enable_tool 时
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

            // 注册数据处理工具
            #[cfg(feature = "data")]
            {
                use crate::tools::builtin::data::{
                    DataAggregateTool, DataExportTool, DataFilterTool, DataReadTool, DataStatsTool,
                    DataTransformTool,
                };

                tool_manager.register(Box::new(DataReadTool));
                tool_manager.register(Box::new(DataFilterTool));
                tool_manager.register(Box::new(DataAggregateTool));
                tool_manager.register(Box::new(DataStatsTool));
                tool_manager.register(Box::new(DataTransformTool));
                tool_manager.register(Box::new(DataExportTool));
            }
        }

        let store: Option<Arc<dyn Store>> = if config.enable_memory {
            match FileStore::new(&config.memory_path) {
                Ok(s) => {
                    let store = Arc::new(s) as Arc<dyn Store>;
                    let agent_name = config.agent_name.clone();
                    let namespace = vec![agent_name, "memories".to_string()];
                    tool_manager.register(Box::new(RememberTool::new(
                        store.clone(),
                        namespace.clone(),
                    )));
                    tool_manager
                        .register(Box::new(RecallTool::new(store.clone(), namespace.clone())));
                    tool_manager.register(Box::new(SearchMemoryTool::new(
                        store.clone(),
                        namespace.clone(),
                    )));
                    tool_manager.register(Box::new(ForgetTool::new(store.clone(), namespace)));
                    Some(store)
                }
                Err(e) => {
                    tracing::warn!("⚠️ 长期记忆 Store 初始化失败，记忆功能已禁用: {e}");
                    None
                }
            }
        } else {
            None
        };

        let checkpointer: Option<Arc<dyn Checkpointer>> = if config.session_id.is_some() {
            match FileCheckpointer::new(&config.checkpointer_path) {
                Ok(cp) => Some(Arc::new(cp)),
                Err(e) => {
                    tracing::warn!("⚠️ Checkpointer 初始化失败，会话恢复功能已禁用: {e}");
                    None
                }
            }
        } else {
            None
        };

        Self {
            config,
            context,
            tool_manager,
            subagent_registry,
            subagent_executor,
            client: Arc::new(client),
            llm_config: None,
            #[cfg(feature = "tasks")]
            task_manager,
            #[cfg(feature = "human-loop")]
            human_in_loop,
            #[cfg(feature = "human-loop")]
            approval_provider,
            skill_registry: SkillRegistry::new(),
            hook_registry: Arc::new(tokio::sync::RwLock::new(HookRegistry::new())),
            store,
            checkpointer,
            #[cfg(feature = "mcp")]
            mcp_manager: McpManager::new(),
            guard_manager: None,
            permission_policy: None,
            #[cfg(feature = "human-loop")]
            permission_service: None,
            audit_logger: None,
            snapshot_manager: None,
            conversation_store: None,
            circuit_breaker: None,
        }
    }

    /// 从配置文件创建 Agent
    ///
    /// 搜索 `echo-agent.yaml` 并加载配置，自动应用环境变量覆盖。
    ///
    /// ```no_run
    /// use echo_agent::agents::react::ReactAgent;
    /// let agent = ReactAgent::from_config_file(None);
    /// ```
    pub fn from_config_file(path: Option<&str>) -> Self {
        let mut app_config = crate::config::load_config(path);
        crate::config::apply_env_overrides(&mut app_config);
        Self::new(app_config.to_agent_config())
    }

    // ── LLM 配置注入 ─────────────────────────────────────────────────────────────

    /// 注入自定义 LLM 配置（依赖注入模式）
    ///
    /// 使用此方法可以：
    /// - 动态切换 API 配置
    /// - 支持多租户场景
    /// - 方便测试
    ///
    /// # 示例
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
    ///     AgentConfig::standard("qwen3-max", "assistant", "你是一个助手")
    /// ).with_llm_config(llm_config);
    /// ```
    pub fn with_llm_config(mut self, config: LlmConfig) -> Self {
        self.llm_config = Some(config);
        self
    }

    /// 设置 LLM 配置
    pub fn set_llm_config(&mut self, config: LlmConfig) {
        self.llm_config = Some(config);
    }

    /// 获取当前 LLM 配置
    pub fn llm_config(&self) -> Option<&LlmConfig> {
        self.llm_config.as_ref()
    }

    // ── 访问器 & 设置器 ────────────────────────────────────────────────────────

    /// 获取 AgentConfig 的只读引用
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// 注入自定义长期记忆 Store（仅替换自动注入通道，不重注册工具）
    pub fn set_store(&mut self, store: Arc<dyn Store>) {
        self.store = Some(store);
    }

    /// 替换长期记忆 Store，并重新注册 `remember` / `recall` / `forget` 工具
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
        self.tool_manager
            .register(Box::new(RememberTool::new(store.clone(), ns.clone())));
        self.tool_manager
            .register(Box::new(RecallTool::new(store.clone(), ns.clone())));
        self.tool_manager
            .register(Box::new(SearchMemoryTool::new(store.clone(), ns.clone())));
        self.tool_manager
            .register(Box::new(ForgetTool::new(store.clone(), ns)));
        self.store = Some(store);
    }

    /// 获取当前长期记忆 Store 的只读引用
    pub fn store(&self) -> Option<&Arc<dyn Store>> {
        self.store.as_ref()
    }

    /// 注入 Checkpointer 并绑定 session_id，启用跨进程会话恢复
    pub fn set_checkpointer(&mut self, checkpointer: Arc<dyn Checkpointer>, session_id: String) {
        self.checkpointer = Some(checkpointer);
        self.config.session_id = Some(session_id);
    }

    /// 获取当前 Checkpointer 的只读引用
    pub fn checkpointer(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.checkpointer.as_ref()
    }

    /// 获取当前对话历史消息（只读）
    pub fn get_messages(&self) -> &[crate::llm::types::Message] {
        self.context.messages()
    }

    /// 获取已注册的工具名称列表
    pub fn tool_names(&self) -> Vec<&str> {
        self.tool_manager.list_tools()
    }

    /// 获取已注册的 Skill 名称列表
    pub fn skill_names(&self) -> Vec<&str> {
        self.skill_registry
            .list()
            .iter()
            .map(|s| s.name.as_str())
            .collect()
    }

    /// 获取已连接的 MCP 服务端名称列表
    #[cfg(feature = "mcp")]
    pub fn mcp_server_names(&self) -> Vec<&str> {
        self.mcp_manager.server_names()
    }

    #[cfg(not(feature = "mcp"))]
    pub fn mcp_server_names(&self) -> Vec<&str> {
        vec![]
    }

    /// 启用熔断器
    ///
    /// LLM 连续失败达到阈值后自动熔断，等待 timeout 后恢复探测。
    pub fn set_circuit_breaker(&mut self, config: CircuitBreakerConfig) {
        self.circuit_breaker = Some(Arc::new(CircuitBreaker::new(config)));
    }

    /// 设置护栏管理器
    pub fn set_guard_manager(&mut self, manager: GuardManager) {
        self.guard_manager = Some(manager);
    }

    /// 设置权限策略
    pub fn set_permission_policy(
        &mut self,
        policy: Arc<dyn crate::tools::permission::PermissionPolicy>,
    ) {
        self.permission_policy = Some(policy);
    }

    #[cfg(feature = "human-loop")]
    /// 设置统一权限服务
    ///
    /// 一旦设置，`check_tool_approval()` 将优先使用此服务，
    /// 回退到旧的 PermissionPolicy + HumanApprovalManager 逻辑。
    pub fn set_permission_service(&mut self, service: Arc<PermissionService>) {
        self.permission_service = Some(service);
    }

    #[cfg(feature = "human-loop")]
    /// 从旧的权限组件构建统一 PermissionService 并设置
    ///
    /// 将当前 `permission_policy` + `approval_provider` 合并为一个
    /// `PermissionService`，保证管线顺序正确（mode → hooks → rules → handler）。
    pub fn build_permission_service(&mut self) {
        use crate::human_loop::service::PermissionService;

        let policy = self.permission_policy.take();
        let provider = self.approval_provider.clone();

        let service = PermissionService::from_provider(provider);
        let service = if let Some(p) = policy {
            service.with_legacy_policy(p)
        } else {
            service
        };

        self.permission_service = Some(Arc::new(service));
    }

    /// 设置审计日志记录器
    pub fn set_audit_logger(&mut self, logger: Arc<dyn crate::audit::AuditLogger>) {
        self.audit_logger = Some(logger);
    }

    // ── 快照 & 回滚 ──────────────────────────────────────────────────────────

    /// 启用状态快照功能
    pub fn set_snapshot_manager(&mut self, manager: SnapshotManager) {
        self.snapshot_manager = Some(manager);
    }

    /// 手动捕获一份当前对话状态的快照，返回快照 ID
    pub fn snapshot(&mut self) -> Option<String> {
        let messages = self.context.messages();
        self.snapshot_manager
            .as_mut()
            .map(|mgr| mgr.capture(0, messages))
    }

    /// 回滚到 N 步之前的快照
    ///
    /// `steps_back = 1` 表示回到最近一次快照。
    /// 成功时恢复对话历史并返回快照信息。
    pub fn rollback(&mut self, steps_back: usize) -> Option<StateSnapshot> {
        let snapshot = self
            .snapshot_manager
            .as_mut()
            .and_then(|mgr| mgr.rollback(steps_back))?;
        self.context.clear();
        for msg in &snapshot.messages {
            self.context.push(msg.clone());
        }
        Some(snapshot)
    }

    /// 回滚到指定 ID 的快照
    pub fn rollback_to(&mut self, snapshot_id: &str) -> Option<StateSnapshot> {
        let snapshot = self
            .snapshot_manager
            .as_mut()
            .and_then(|mgr| mgr.rollback_to(snapshot_id))?;
        self.context.clear();
        for msg in &snapshot.messages {
            self.context.push(msg.clone());
        }
        Some(snapshot)
    }

    /// 获取所有快照列表
    pub fn snapshots(&self) -> &[StateSnapshot] {
        self.snapshot_manager
            .as_ref()
            .map(|mgr| mgr.list())
            .unwrap_or(&[])
    }

    /// 获取最新快照
    pub fn latest_snapshot(&self) -> Option<&StateSnapshot> {
        self.snapshot_manager.as_ref().and_then(|mgr| mgr.latest())
    }

    #[cfg(feature = "human-loop")]
    /// 替换审批 Provider，支持在运行时切换审批渠道。
    pub fn set_approval_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.set_human_loop_provider(provider);
    }

    #[cfg(feature = "human-loop")]
    /// 设置人工介入 Provider
    ///
    /// 同时更新 `approval_provider`（工具审批 guard）和 `human_in_loop` 内置工具（LLM
    /// 主动触发），保证两者始终指向同一个 provider。
    pub fn set_human_loop_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.approval_provider = provider.clone();
        if self.tool_manager.get_tool("human_in_loop").is_some() {
            self.tool_manager
                .register(Box::new(HumanInLoop::new(provider)));
        }
    }

    // ── 对话持久化 ──────────────────────────────────────────────────────────────

    /// 设置对话持久化 Store
    ///
    /// 启用后，对话将自动保存到 Store，支持跨会话恢复。
    pub fn set_conversation_store(
        &mut self,
        store: Arc<dyn crate::memory::conversation::ConversationStore>,
    ) {
        self.conversation_store = Some(store);
    }

    /// 加载历史消息到 agent 上下文（替换现有上下文）
    ///
    /// 用于从持久化存储恢复对话，使 agent 可以继续之前的对话。
    /// 消息应包含 system prompt 作为第一条（如需要）。
    pub fn load_messages(&mut self, messages: Vec<crate::llm::types::Message>) {
        self.context.set_messages(messages);
    }
}

// ── LLM 每轮推理的输出类型 ────────────────────────────────────────────────────

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

    fn execute<'a>(&'a mut self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            #[cfg(feature = "tasks")]
            if self.has_planning_tools() {
                return self.execute_with_planning(task).await;
            }
            self.run_direct(task).await
        })
    }

    fn execute_stream<'a>(
        &'a mut self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move { self.run_stream(task, run::StreamMode::Execute).await })
    }

    fn chat<'a>(&'a mut self, message: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move { self.run_chat_direct(message).await })
    }

    fn chat_stream<'a>(
        &'a mut self,
        message: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        Box::pin(async move { self.run_stream(message, run::StreamMode::Chat).await })
    }

    fn reset(&mut self) {
        self.reset_messages();
    }

    fn tool_names(&self) -> Vec<String> {
        self.tool_manager
            .list_tools()
            .into_iter()
            .filter(|n| *n != TOOL_FINAL_ANSWER)
            .map(|n| n.to_string())
            .collect()
    }

    /// 获取工具定义列表（包含名称、描述、参数 Schema）
    fn tool_definitions(&self) -> Vec<crate::llm::types::ToolDefinition> {
        self.tool_manager
            .get_tool_definitions()
            .into_iter()
            .filter(|d| d.function.name != TOOL_FINAL_ANSWER)
            .collect()
    }

    fn skill_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .skill_registry
            .list()
            .into_iter()
            .map(|s| s.name.clone())
            .collect();
        // Also include file-based skill names
        for desc in self.skill_registry.list_descriptors() {
            if !names.contains(&desc.name) {
                names.push(desc.name.clone());
            }
        }
        names
    }

    fn mcp_server_names(&self) -> Vec<String> {
        #[cfg(feature = "mcp")]
        {
            self.mcp_manager
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

    fn close(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(feature = "mcp")]
            self.mcp_manager.close_all().await;
        })
    }
}

// ── ReactAgent 多模态扩展方法 ────────────────────────────────────────────────────

impl ReactAgent {
    /// 发送带图片 URL 的消息（多模态）
    ///
    /// 自动下载图片并转换为 base64 发送给 LLM。
    /// 部分云厂商（如阿里云 Qwen）不支持直接访问外部 URL，需要下载后转 base64。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use echo_agent::prelude::*;
    /// # async fn test() -> echo_agent::error::Result<()> {
    /// # let mut agent = ReactAgentBuilder::new().model("qwen3.5-plus").build()?;
    /// let response = agent.chat_with_image_url(
    ///     "描述这张图片",
    ///     "https://example.com/image.jpg"
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn chat_with_image_url(&mut self, text: &str, image_url: &str) -> Result<String> {
        use crate::llm::types::{ContentPart, ImageUrl, Message};

        // 下载图片并转换为 base64 data URL
        let data_url = fetch_image_as_base64(image_url).await?;

        let message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: text.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: data_url,
                    detail: None,
                },
            },
        ]);

        self.chat_multimodal(message).await
    }

    /// 发送多模态消息
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use echo_agent::prelude::*;
    /// # async fn test() -> echo_agent::error::Result<()> {
    /// # let mut agent = ReactAgentBuilder::new().model("qwen3.5-plus").build()?;
    /// use echo_agent::llm::types::{ContentPart, ImageUrl, Message};
    ///
    /// let message = Message::user_multimodal(vec![
    ///     ContentPart::Text { text: "描述这些图片".to_string() },
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
    pub async fn chat_multimodal(&mut self, message: crate::llm::types::Message) -> Result<String> {
        use crate::llm::chat;

        // 确保上下文已初始化（包含 system prompt）
        if self.context.messages().is_empty() {
            self.context.push(crate::llm::types::Message::system(
                self.config.system_prompt.clone(),
            ));
        }

        // 添加多模态用户消息
        self.context.push(message.clone());

        // 准备消息列表
        let messages = self.context.messages().to_vec();

        // 调用 LLM（不使用工具模式，因为这是直接对话）
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

        let content = response
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        // 添加助手回复到上下文
        self.context
            .push(crate::llm::types::Message::assistant(content.clone()));

        Ok(content)
    }

    /// 执行带图片 URL 的任务（单轮，重置上下文）
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// # use echo_agent::prelude::*;
    /// # async fn test() -> echo_agent::error::Result<()> {
    /// # let mut agent = ReactAgentBuilder::new().model("qwen3.5-plus").build()?;
    /// let response = agent
    ///     .execute_with_image_url("分析这张停车缴费单", "https://example.com/receipt.jpg")
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_with_image_url(&mut self, task: &str, image_url: &str) -> Result<String> {
        use crate::llm::types::{ContentPart, ImageUrl, Message};

        // 重置上下文
        self.reset_messages();

        // 下载图片并转换为 base64
        let data_url = fetch_image_as_base64(image_url).await?;

        let message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: task.to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: data_url,
                    detail: None,
                },
            },
        ]);

        self.chat_multimodal(message).await
    }
}

/// 下载图片并转换为 base64 data URL
///
/// 支持检测 Content-Type 并生成正确的 data URI 格式：
/// `data:image/jpeg;base64,...`
async fn fetch_image_as_base64(url: &str) -> Result<String> {
    use crate::error::{ReactError, ToolError};
    use base64::Engine;
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| {
            ReactError::Agent(crate::error::AgentError::InitializationFailed(format!(
                "Failed to build HTTP client: {}",
                e
            )))
        })?;

    let response = client.get(url).send().await.map_err(|e| {
        ReactError::Tool(ToolError::ExecutionFailed {
            tool: "fetch_image".to_string(),
            message: format!("下载图片失败: {}", e),
        })
    })?;

    if !response.status().is_success() {
        return Err(ReactError::Tool(ToolError::ExecutionFailed {
            tool: "fetch_image".to_string(),
            message: format!("HTTP 错误: {}", response.status()),
        }));
    }

    // 获取 MIME 类型（在消费 response 之前）
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    // 提取主类型（如 image/jpeg -> jpeg）
    let mime_subtype = content_type.split('/').nth(1).unwrap_or("jpeg");

    // 下载二进制数据
    let bytes = response.bytes().await.map_err(|e| {
        ReactError::Tool(ToolError::ExecutionFailed {
            tool: "fetch_image".to_string(),
            message: format!("读取图片数据失败: {}", e),
        })
    })?;

    // 转换为 base64
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(format!(
        "data:image/{};base64,{}",
        mime_subtype, base64_data
    ))
}
