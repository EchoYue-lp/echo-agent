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
use crate::agent::subagent::SubagentRegistry;
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
use crate::memory::checkpointer::{Checkpointer, FileCheckpointer};
use crate::memory::snapshot::{SnapshotManager, StateSnapshot};
use crate::memory::store::{FileStore, Store};
use crate::sandbox::SandboxManager;
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
use std::sync::Arc;
use tracing::{Instrument, info, info_span, warn};

use crate::agent::react::subsystems::approval::ApprovalSubsystem;
use crate::agent::react::subsystems::guard::GuardSubsystem;
use crate::agent::react::subsystems::memory::MemorySubsystem;
use crate::agent::react::subsystems::tool_exec::ToolExecutionSubsystem;

pub mod builder;
mod capabilities;
mod extract;
#[cfg(feature = "tasks")]
mod planning;
mod run;
pub mod structured;
pub(crate) mod subsystems;
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
        ReactError::Llm(e) => match e.as_ref() {
            LlmError::NetworkError(_) => true,
            LlmError::ApiError { status, .. } => *status == 429 || *status >= 500,
            _ => false,
        },
        _ => false,
    }
}

// ── ReactAgent 结构体 ─────────────────────────────────────────────────────────

/// ReAct（Reasoning + Acting）Agent 实现
///
/// 基于 ReAct 范式的自主 Agent，支持工具调用、任务规划、子代理调度、
/// 长期记忆、思维链、上下文压缩等核心功能。
///
/// # 核心组件
///
/// - **配置管理**：通过 `AgentConfig` 控制 Agent 行为和能力
/// - **上下文管理**：维护对话历史，支持自动压缩和 token 计数
/// - **工具管理**：注册、发现和执行工具，支持权限控制和沙箱执行
/// - **子代理系统**：支持 Sync/Fork/Teammate 三种调度模式
/// - **记忆系统**：长期记忆存储和检索
/// - **技能系统**：代码和文件技能管理
/// - **Hook 系统**：工具调用拦截和修改
pub struct ReactAgent {
    pub(crate) config: AgentConfig,
    /// 工具执行子系统：工具注册/执行、Skill、Hook、MCP、SubAgent、Sandbox
    pub(crate) tools: ToolExecutionSubsystem,
    /// 护栏与安全子系统：护栏、权限策略、审计日志、熔断器
    pub(crate) guard: GuardSubsystem,
    /// 记忆与持久化子系统：上下文管理、长期记忆、快照、Checkpoint
    pub(crate) memory: MemorySubsystem,
    /// 人工介入审批子系统（human-in-the-loop）
    pub(crate) approval: ApprovalSubsystem,
    client: Arc<Client>,
    llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// LLM 配置（可选，不设置时使用环境变量配置）
    llm_config: Option<LlmConfig>,
    /// 当前流式请求的取消令牌，在 `chat_stream_with_cancel` / `execute_stream_with_cancel` 中设置。
    /// `create_llm_stream` 读取此字段并传递给 HTTP 层以支持请求级别的流式中止。
    /// 使用 `tokio::sync::Mutex` 以支持 `&self` 流式方法。
    cancel_token: tokio::sync::Mutex<Option<CancellationToken>>,
}

// ── 构造与初始化 ──────────────────────────────────────────────────────────────

impl ReactAgent {
    #[cfg(feature = "tasks")]
    pub(crate) fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && [TOOL_PLAN, TOOL_CREATE_TASK, TOOL_UPDATE_TASK]
                .iter()
                .all(|name| self.tools.tool_manager.get_tool(name).is_some())
    }

    #[cfg(not(feature = "tasks"))]
    #[allow(dead_code)]
    pub(crate) fn has_planning_tools(&self) -> bool {
        false
    }

    /// 工具调用场景下自动注入的思维链引导语。
    const COT_INSTRUCTION: &'static str = "在调用工具之前，先用文字简述你的分析思路和执行计划。";

    /// 创建新的 ReAct Agent 实例
    ///
    /// # 参数
    /// * `config` - Agent 运行时配置
    ///
    /// # 返回值
    /// 初始化完成的 `ReactAgent` 实例
    ///
    /// # 说明
    /// 该方法会根据配置初始化所有核心组件，包括：
    /// - 上下文管理器
    /// - 工具管理器（根据配置启用工具）
    /// - 子代理系统（根据配置启用子代理调度）
    /// - 记忆系统（根据配置启用长期记忆）
    /// - 技能注册表
    /// - Hook 系统
    pub fn new(config: AgentConfig) -> Self {
        let system_prompt = Self::build_system_prompt(&config);

        let context = Arc::new(tokio::sync::Mutex::new(
            ContextManager::builder(config.token_limit)
                .with_system(system_prompt)
                .build(),
        ));

        let mut tool_manager = ToolManager::new_with_config(config.tool_execution.clone());
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();

        // ── 核心工具 ─────────────────────────────────────────────
        tool_manager.register(Box::new(FinalAnswerTool));

        // ── 子模块初始化 ─────────────────────────────────────────
        #[cfg(feature = "tasks")]
        let task_manager = Arc::new(TaskManager::default());
        let subagent_registry = Arc::new(SubagentRegistry::new());
        let subagent_executor = Arc::new(SubagentExecutor::new(
            subagent_registry.clone(),
            SubagentExecutorConfig::default(),
        ));
        #[cfg(feature = "human-loop")]
        let approval_provider = crate::human_loop::default_provider();

        // ── 按功能注册工具 ───────────────────────────────────────
        // AgentDispatch 由 runtime 配置 enable_subagent 控制
        if config.enable_subagent {
            tool_manager.register(Box::new(AgentDispatchTool::new(
                subagent_executor.clone(),
                config.agent_name.clone(),
                CancellationToken::new(),
            )));
        }

        #[cfg(feature = "human-loop")]
        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop::new(approval_provider.clone())));
        }

        #[cfg(feature = "tasks")]
        if config.enable_task {
            tool_manager.register(Box::new(PlanTool));
            tool_manager.register(Box::new(CreateTaskTool::new(task_manager.clone())));
            tool_manager.register(Box::new(UpdateTaskTool::new(task_manager.clone())));
            tool_manager.register(Box::new(ListTasksTool::new(task_manager.clone())));
            tool_manager.register(Box::new(VisualizeDependenciesTool::new(
                task_manager.clone(),
            )));
            tool_manager.register(Box::new(GetExecutionOrderTool::new(task_manager.clone())));
        }
        Self::register_feature_gated_tools(&config, &mut tool_manager);

        // ── 记忆存储 ─────────────────────────────────────────────
        let store = Self::setup_memory_store(&config, &mut tool_manager);

        // ── 检查点 ───────────────────────────────────────────────
        let checkpointer = Self::setup_checkpointer(&config);

        Self {
            config,
            tools: ToolExecutionSubsystem {
                tool_manager,
                subagent_registry,
                #[cfg(feature = "tasks")]
                task_manager,
                skill_registry: SkillRegistry::new(),
                progressive_skill_registry: None,
                hook_registry: Arc::new(tokio::sync::RwLock::new(HookRegistry::new())),
                #[cfg(feature = "mcp")]
                mcp_manager: McpManager::new(),
                sandbox_manager: None,
            },
            guard: GuardSubsystem {
                guard_manager: None,
                permission_policy: None,
                audit_logger: None,
                circuit_breaker: None,
            },
            memory: MemorySubsystem {
                context,
                store,
                checkpointer,
                snapshot_manager: Arc::new(std::sync::RwLock::new(None)),
                conversation_store: None,
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
            cancel_token: tokio::sync::Mutex::new(None),
        }
    }

    /// 从配置文件创建 Agent
    ///
    /// 搜索 `echo-agent.yaml` 并加载配置。
    ///
    /// ```no_run
    /// use echo_agent::agent::react::ReactAgent;
    /// let agent = ReactAgent::from_config_file(None);
    /// ```
    pub fn from_config_file(path: Option<&str>) -> Self {
        let app_config = crate::config::load_config(path);
        Self::new(app_config.to_agent_config())
    }

    // ── 构造函数辅助方法 ─────────────────────────────────────────────────────────

    fn build_system_prompt(config: &AgentConfig) -> String {
        let mut prompt = if config.enable_tool && config.enable_cot {
            format!(
                "{}\n\n{}",
                config.system_prompt.trim_end(),
                Self::COT_INSTRUCTION,
            )
        } else {
            config.system_prompt.clone()
        };

        #[cfg(feature = "project-rules")]
        if config.auto_project_rules {
            let wd = config
                .working_dir
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            prompt = echo_core::project_rules::inject_rules(&prompt, &wd);
        }

        prompt
    }

    fn register_feature_gated_tools(config: &AgentConfig, tool_manager: &mut ToolManager) {
        #[cfg(feature = "git")]
        if config.enable_tool {
            use crate::tools::builtin::git::{
                GitBlameTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool,
            };
            tool_manager.register(Box::new(GitStatusTool));
            tool_manager.register(Box::new(GitDiffTool));
            tool_manager.register(Box::new(GitLogTool));
            tool_manager.register(Box::new(GitBlameTool));
            tool_manager.register(Box::new(GitBranchTool));
            tool_manager.register(Box::new(GitCommitTool));
        }

        #[cfg(feature = "rag")]
        if config.enable_tool {
            use crate::tools::builtin::rag::{RagChunkDocumentTool, RagIndexTool, RagSearchTool};
            tool_manager.register(Box::new(RagIndexTool));
            tool_manager.register(Box::new(RagSearchTool));
            tool_manager.register(Box::new(RagChunkDocumentTool));
        }

        #[cfg(feature = "chart")]
        if config.enable_tool {
            tool_manager.register(Box::new(crate::tools::builtin::chart::GenerateChartTool));
        }

        #[cfg(feature = "database")]
        if config.enable_tool {
            use crate::tools::builtin::database::{
                DescribeTableTool, ListTablesTool, SqlQueryTool,
            };
            tool_manager.register(Box::new(SqlQueryTool));
            tool_manager.register(Box::new(ListTablesTool));
            tool_manager.register(Box::new(DescribeTableTool));
        }

        #[cfg(feature = "web")]
        if config.enable_tool {
            use crate::tools::builtin::browser::{WebExtractTool, WebFetchTool};
            tool_manager.register(Box::new(WebFetchTool));
            tool_manager.register(Box::new(WebExtractTool));
        }

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

            #[cfg(feature = "data")]
            {
                use crate::tools::builtin::data::{
                    DataAggregateTool, DataBinTool, DataContributionTool, DataExportTool,
                    DataFilterTool, DataProfileTool, DataRatioTool, DataReadTool, DataStatsTool,
                    DataTopNTool, DataTransformTool,
                };

                tool_manager.register(Box::new(DataReadTool));
                tool_manager.register(Box::new(DataFilterTool));
                tool_manager.register(Box::new(DataAggregateTool));
                tool_manager.register(Box::new(DataStatsTool));
                tool_manager.register(Box::new(DataTransformTool));
                tool_manager.register(Box::new(DataExportTool));
                tool_manager.register(Box::new(DataProfileTool));
                tool_manager.register(Box::new(DataTopNTool));
                tool_manager.register(Box::new(DataContributionTool));
                tool_manager.register(Box::new(DataBinTool));
                tool_manager.register(Box::new(DataRatioTool));
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
                tool_manager.register(Box::new(RememberTool::new(
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
                tracing::warn!("⚠️ 长期记忆 Store 初始化失败，记忆功能已禁用: {e}");
                None
            }
        }
    }

    /// 当 embedding 环境变量配置存在时，用 [`EmbeddingStore`] 包装底层 Store，
    /// 使 `remember` 写入自动向量化，`search_memory` 的混合检索真正生效。
    ///
    /// 若未配置 embedding，则直接返回原始 Store，保持原有行为。
    fn wrap_with_embedding_store_if_available(
        inner: Arc<dyn Store>,
        memory_path: &str,
    ) -> Arc<dyn Store> {
        use crate::memory::{EmbeddingStore, HttpEmbedder};

        if std::env::var("EMBEDDING_API_KEY").is_err()
            && std::env::var("OPENAI_API_KEY").is_err()
            && std::env::var("EMBEDDING_APIKEY").is_err()
        {
            tracing::info!("📚 记忆 Store: 纯关键词检索（未配置 embedding 环境变量）");
            return inner;
        }

        let embedder = Arc::new(HttpEmbedder::from_env());
        let vec_path = format!("{}.vecs.json", memory_path.trim_end_matches(".json"));

        match EmbeddingStore::with_persistence(Arc::clone(&inner), embedder, &vec_path) {
            Ok(embedding_store) => {
                tracing::info!(
                    vec_path = %vec_path,
                    "🧠 记忆 Store: 已启用向量索引（语义/混合检索可用）"
                );
                Arc::new(embedding_store)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "⚠️ EmbeddingStore 初始化失败，回退到纯关键词检索"
                );
                inner
            }
        }
    }

    fn setup_checkpointer(config: &AgentConfig) -> Option<Arc<dyn Checkpointer>> {
        config.session_id.as_ref()?;
        match FileCheckpointer::new(&config.checkpointer_path) {
            Ok(cp) => Some(Arc::new(cp)),
            Err(e) => {
                tracing::warn!("⚠️ Checkpointer 初始化失败，会话恢复功能已禁用: {e}");
                None
            }
        }
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
        self.config.model_name = config.model.clone();
        self.llm_config = Some(config);
        self
    }

    /// 注入自定义 LLM 客户端
    pub fn with_llm_client(mut self, client: Arc<dyn crate::llm::LlmClient>) -> Self {
        self.config.model_name = client.model_name().to_string();
        self.llm_client = Some(client);
        self
    }

    /// 设置 LLM 配置
    pub fn set_llm_config(&mut self, config: LlmConfig) {
        self.config.model_name = config.model.clone();
        self.llm_config = Some(config);
    }

    /// 设置自定义 LLM 客户端
    pub fn set_llm_client(&mut self, client: Arc<dyn crate::llm::LlmClient>) {
        self.config.model_name = client.model_name().to_string();
        self.llm_client = Some(client);
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
        self.memory.store = Some(store);
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
        self.tools
            .tool_manager
            .register(Box::new(RememberTool::new(store.clone(), ns.clone())));
        self.tools
            .tool_manager
            .register(Box::new(RecallTool::new(store.clone(), ns.clone())));
        self.tools
            .tool_manager
            .register(Box::new(SearchMemoryTool::new(store.clone(), ns.clone())));
        self.tools
            .tool_manager
            .register(Box::new(ForgetTool::new(store.clone(), ns)));
        self.memory.store = Some(store);
    }

    /// 获取当前长期记忆 Store 的只读引用
    pub fn store(&self) -> Option<&Arc<dyn Store>> {
        self.memory.store.as_ref()
    }

    /// 注入线程状态存储并绑定 session_id，启用跨进程线程恢复
    pub fn set_checkpointer(&mut self, checkpointer: Arc<dyn Checkpointer>, session_id: String) {
        self.memory.checkpointer = Some(checkpointer);
        self.config.session_id = Some(session_id);
    }

    /// `set_checkpointer()` 的语义化别名。
    pub fn set_thread_store(&mut self, store: Arc<dyn Checkpointer>, session_id: String) {
        self.set_checkpointer(store, session_id);
    }

    /// 获取当前线程状态存储的只读引用
    pub fn checkpointer(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.memory.checkpointer.as_ref()
    }

    /// `checkpointer()` 的语义化别名。
    pub fn thread_store(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.memory.checkpointer.as_ref()
    }

    /// 设置对话历史投影使用的 conversation_id。
    pub fn set_conversation_id(&mut self, conversation_id: impl Into<String>) {
        self.config.conversation_id = Some(conversation_id.into());
    }

    /// 获取当前对话历史投影的 conversation_id。
    pub fn conversation_id(&self) -> Option<&str> {
        self.config.get_conversation_id()
    }

    /// 获取当前对话历史消息（只读）
    pub async fn get_messages(&self) -> Vec<crate::llm::types::Message> {
        self.memory.context.lock().await.messages().to_vec()
    }

    /// 获取已注册的工具名称列表
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.tool_manager.list_tools()
    }

    /// 获取已注册的 Skill 名称列表
    pub fn skill_names(&self) -> Vec<&str> {
        self.tools
            .skill_registry
            .list()
            .iter()
            .map(|s| s.name.as_str())
            .collect()
    }

    /// 获取已连接的 MCP 服务端名称列表
    #[cfg(feature = "mcp")]
    pub fn mcp_server_names(&self) -> Vec<&str> {
        self.tools.mcp_manager.server_names()
    }

    #[cfg(not(feature = "mcp"))]
    pub fn mcp_server_names(&self) -> Vec<&str> {
        vec![]
    }

    /// 启用熔断器
    ///
    /// LLM 连续失败达到阈值后自动熔断，等待 timeout 后恢复探测。
    pub fn set_circuit_breaker(&mut self, config: CircuitBreakerConfig) {
        self.guard.circuit_breaker = Some(Arc::new(CircuitBreaker::new(config)));
    }

    /// 设置护栏管理器
    pub fn set_guard_manager(&mut self, manager: GuardManager) {
        self.guard.guard_manager = Some(manager);
    }

    /// 设置权限策略
    pub fn set_permission_policy(
        &mut self,
        policy: Arc<dyn crate::tools::permission::PermissionPolicy>,
    ) {
        self.guard.permission_policy = Some(policy);
    }

    #[cfg(feature = "human-loop")]
    /// 设置统一权限服务
    ///
    /// 一旦设置，`check_tool_approval()` 将优先使用此服务，
    /// 回退到旧的 PermissionPolicy 逻辑。
    pub fn set_permission_service(&mut self, service: Arc<PermissionService>) {
        self.approval.permission_service = Some(service);
    }

    #[cfg(feature = "human-loop")]
    /// 从旧的权限组件构建统一 PermissionService 并设置
    ///
    /// 将当前 `permission_policy` + `approval_provider` 合并为一个
    /// `PermissionService`，保证管线顺序正确（mode → hooks → rules → handler）。
    pub fn build_permission_service(&mut self) {
        use crate::human_loop::service::PermissionService;

        let policy = self.guard.permission_policy.take();
        let provider = self.approval.approval_provider.clone();

        let service = PermissionService::from_provider(provider);
        let service = if let Some(p) = policy {
            service.with_legacy_policy(p)
        } else {
            service
        };

        self.approval.permission_service = Some(Arc::new(service));
    }

    /// 设置审计日志记录器
    pub fn set_audit_logger(&mut self, logger: Arc<dyn crate::audit::AuditLogger>) {
        self.guard.audit_logger = Some(logger);
    }

    // ── 快照 & 回滚 ──────────────────────────────────────────────────────────

    /// 设置沙箱管理器，为 skill 脚本执行提供安全隔离
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

    /// 启用状态快照功能
    pub fn set_snapshot_manager(&self, manager: SnapshotManager) {
        let mut guard = self
            .memory
            .snapshot_manager
            .write()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(manager);
    }

    /// 手动捕获一份当前对话状态的快照，返回快照 ID
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

    /// 回滚到 N 步之前的快照
    ///
    /// `steps_back = 1` 表示回到最近一次快照。
    /// 成功时恢复对话历史并返回快照信息。
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

    /// 回滚到指定 ID 的快照
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

    /// 获取所有快照列表
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

    /// 获取最新快照
    pub fn latest_snapshot(&self) -> Option<StateSnapshot> {
        let guard = self
            .memory
            .snapshot_manager
            .read()
            .unwrap_or_else(|e| e.into_inner());
        guard.as_ref().and_then(|mgr| mgr.latest().cloned())
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
        self.approval.approval_provider = provider.clone();
        if self.tools.tool_manager.get_tool("human_in_loop").is_some() {
            self.tools
                .tool_manager
                .register(Box::new(HumanInLoop::new(provider)));
        }
    }

    // ── 对话持久化 ──────────────────────────────────────────────────────────────

    /// 设置对话历史投影 Store
    ///
    /// 启用后，Agent 会在持久化线程状态的同时，将当前 transcript 投影到
    /// `ConversationStore` 中，供历史浏览与产品层查询使用。
    ///
    /// 注意：该功能要求显式设置独立的 `conversation_id`；
    /// `session_id` 仅用于线程状态恢复，不再作为历史投影的回退标识。
    pub fn set_conversation_store(
        &mut self,
        store: Arc<dyn crate::memory::conversation::ConversationStore>,
    ) {
        self.memory.conversation_store = Some(store);
    }

    /// 加载历史消息到 agent 上下文（替换现有上下文）
    ///
    /// 用于从持久化存储恢复对话，使 agent 可以继续之前的对话。
    /// 消息应包含 system prompt 作为第一条（如需要）。
    pub async fn load_messages(&self, messages: Vec<crate::llm::types::Message>) {
        self.memory.context.lock().await.set_messages(messages);
    }

    /// 关闭 agent 并释放所有资源
    ///
    /// 关闭 MCP 连接、取消后台任务、关闭 WebSocket 服务器。
    /// 当 agent 不再需要时应调用此方法，或依赖 `Drop` 自动调用。
    pub async fn shutdown(&self) {
        #[cfg(feature = "mcp")]
        {
            self.tools.mcp_manager.close_all().await;
        }
        // Close WebSocket servers if any (placeholder for future WS integration)
        info!(agent = %self.config.agent_name, "Agent shut down complete");
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

    fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move {
                #[cfg(feature = "tasks")]
                if self.has_planning_tools() {
                    return self.execute_with_planning(task).await;
                }
                self.run_direct(task).await
            }
            .instrument(info_span!("agent_execute", agent.name = %agent, agent.model = %model)),
        )
    }

    fn execute_stream<'a>(
        &'a self,
        task: &'a str,
    ) -> BoxFuture<'a, Result<BoxStream<'a, Result<AgentEvent>>>> {
        let agent = self.config.agent_name.clone();
        let model = self.config.model_name.clone();
        Box::pin(
            async move { self.run_stream(task, run::StreamMode::Execute).await }.instrument(
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
            async move { self.run_stream(message, run::StreamMode::Chat).await }.instrument(
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
                // Delegate to the Agent trait's default implementation
                // which wraps chat_stream with cancellation
                <Self as Agent>::chat_stream_with_cancel(self, _message, cancel).await
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
                <Self as Agent>::execute_stream_with_cancel(self, _task, cancel).await
            }
            .instrument(info_span!("agent_execute_stream_with_cancel", agent.name = %agent, agent.model = %model)),
        )
    }

    fn reset(&self) {
        match self.memory.context.try_lock() {
            Ok(mut ctx) => {
                ctx.clear();
                ctx.push(crate::llm::types::Message::system(
                    self.config.system_prompt.clone(),
                ));
            }
            Err(_) => {
                warn!(
                    agent = %self.config.agent_name,
                    "Cannot reset: context locked by active stream"
                );
            }
        }
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

    /// 获取工具定义列表（包含名称、描述、参数 Schema）
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

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(feature = "mcp")]
            self.tools.mcp_manager.close_all().await;
        })
    }
}

// ── ReactAgent 多模态扩展方法 ────────────────────────────────────────────────────

impl ReactAgent {
    /// 流式多轮对话（多模态消息版本）
    ///
    /// 与 `chat_stream` 相同，但接受预构建的 `Message` 以支持图片、文件等附件。
    /// 保留上下文，适合多轮多模态对话。
    pub async fn chat_stream_message(
        &self,
        message: crate::llm::types::Message,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream_with_message(message, run::StreamMode::Chat)
            .await
    }

    /// 流式执行任务（多模态消息版本）
    ///
    /// 与 `execute_stream` 相同，但接受预构建的 `Message` 以支持图片、文件等附件。
    /// 重置上下文，适合单轮多模态任务。
    pub async fn execute_stream_message(
        &self,
        message: crate::llm::types::Message,
    ) -> Result<futures::stream::BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream_with_message(message, run::StreamMode::Execute)
            .await
    }

    /// 发送带图片 URL 的消息（多模态）
    ///
    /// 直接将图片 URL 作为 `image_url` part 发送给 LLM。
    /// 如果你已经持有本地文件或 base64 数据，请使用 `chat_multimodal()`
    /// 并自行构造 `data:image/...;base64,...` 的 `ImageUrl.url`。
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
    pub async fn chat_multimodal(&self, message: crate::llm::types::Message) -> Result<String> {
        use crate::llm::{ChatRequest, chat};

        // 确保上下文已初始化（包含 system prompt）
        {
            let mut ctx = self.memory.context.lock().await;
            if ctx.messages().is_empty() {
                ctx.push(crate::llm::types::Message::system(
                    self.config.system_prompt.clone(),
                ));
            }
            // 添加多模态用户消息
            ctx.push(message.clone());
        }

        // 准备消息列表
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
                    cancel_token: None,
                })
                .await?;
            response.content().unwrap_or_default()
        } else {
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

            response
                .choices
                .first()
                .and_then(|c| c.message.content.as_text())
                .unwrap_or_default()
        };

        // 添加助手回复到上下文
        self.memory
            .context
            .lock()
            .await
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
    pub async fn execute_with_image_url(&self, task: &str, image_url: &str) -> Result<String> {
        use crate::llm::types::{ContentPart, ImageUrl, Message};

        // 重置上下文
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
