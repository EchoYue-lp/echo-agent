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
use crate::agent::{Agent, AgentEvent, SubAgentMap};
use crate::compression::ContextManager;
use crate::error::{LlmError, ReactError, Result};
use crate::human_loop::{HumanApprovalManager, HumanLoopProvider};
use crate::llm::config::LlmConfig;
use crate::mcp::McpManager;
use crate::memory::checkpointer::{Checkpointer, FileCheckpointer};
use crate::memory::store::{FileStore, Store};
use crate::skills::SkillManager;
use crate::tasks::TaskManager;
use crate::tools::ToolManager;
use crate::tools::builtin::agent_dispatch::AgentDispatchTool;
use crate::tools::builtin::answer::FinalAnswerTool;
use crate::tools::builtin::human_in_loop::HumanInLoop;
use crate::tools::builtin::memory::{ForgetTool, RecallTool, RememberTool};
use crate::tools::builtin::plan::PlanTool;
use crate::tools::builtin::task::{
    CreateTaskTool, GetExecutionOrderTool, ListTasksTool, UpdateTaskTool, VisualizeDependenciesTool,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub mod builder;
mod capabilities;
mod extract;
mod run;
#[cfg(test)]
mod tests;
// ── 内置工具名常量 ─────────────────────────────────────────────────────────────

pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";
pub(crate) const TOOL_CREATE_TASK: &str = "create_task";
pub(crate) const TOOL_PLAN: &str = "plan";
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
    pub(crate) subagents: SubAgentMap,
    client: Arc<Client>,
    /// LLM 配置（可选，不设置时使用环境变量配置）
    llm_config: Option<LlmConfig>,
    pub(crate) task_manager: Arc<RwLock<TaskManager>>,
    human_in_loop: Arc<RwLock<HumanApprovalManager>>,
    /// 人工介入 Provider：支持命令行、HTTP Webhook、WebSocket 等多种渠道
    approval_provider: Arc<dyn HumanLoopProvider>,
    /// Skill 管理器：记录已安装的所有 Skill 元数据
    skill_manager: SkillManager,
    /// 长期记忆 Store，通过 `remember`/`recall`/`forget` 工具访问
    store: Option<Arc<dyn Store>>,
    /// 短期会话 Checkpointer，按 session_id 持久化对话历史
    checkpointer: Option<Arc<dyn Checkpointer>>,
    /// MCP 连接管理器：持有所有 MCP 服务端的客户端，保证连接生命周期与 Agent 一致
    mcp_manager: McpManager,
}

// ── 构造与初始化 ──────────────────────────────────────────────────────────────

impl ReactAgent {
    pub(crate) fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && [TOOL_PLAN, TOOL_CREATE_TASK, TOOL_UPDATE_TASK]
                .iter()
                .all(|name| self.tool_manager.get_tool(name).is_some())
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

        let task_manager = Arc::new(RwLock::new(TaskManager::default()));
        let human_in_loop = Arc::new(RwLock::new(HumanApprovalManager::default()));
        let subagents = Arc::new(RwLock::new(HashMap::new()));
        let approval_provider = crate::human_loop::default_provider();

        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop::new(approval_provider.clone())));
        }

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
            tool_manager.register(Box::new(AgentDispatchTool::new(subagents.clone())));
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
            subagents,
            client: Arc::new(client),
            llm_config: None,
            task_manager,
            human_in_loop,
            approval_provider,
            skill_manager: SkillManager::new(),
            store,
            checkpointer,
            mcp_manager: McpManager::new(),
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
        self.skill_manager
            .list()
            .iter()
            .map(|s| s.name.as_str())
            .collect()
    }

    /// 获取已连接的 MCP 服务端名称列表
    pub fn mcp_server_names(&self) -> Vec<&str> {
        self.mcp_manager.server_names()
    }

    /// 替换审批 Provider，支持在运行时切换审批渠道。
    ///
    /// 等同于 [`set_human_loop_provider`]，两者均会同步更新 `human_in_loop` 工具。
    pub fn set_approval_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.set_human_loop_provider(provider);
    }

    /// 设置人工介入 Provider（`set_approval_provider` 的别名）
    ///
    /// 同时更新 `approval_provider`（工具审批 guard）和 `human_in_loop` 内置工具（LLM
    /// 主动触发），保证两者始终指向同一个 provider，避免"新旧 provider 割裂"的问题。
    ///
    /// 推荐使用 [`HumanLoopManager`] 作为 provider，支持事件驱动模式：
    ///
    /// ```rust,no_run
    /// use echo_agent::human_loop::{HumanLoopManager, HumanLoopEvent, ApprovalDecision};
    /// use echo_agent::prelude::*;
    /// use std::sync::Arc;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> echo_agent::error::Result<()> {
    /// let manager = Arc::new(HumanLoopManager::new());
    ///
    /// let config = AgentConfig::standard("qwen3-max", "assistant", "你是一个助手")
    ///     .enable_human_in_loop(true);
    /// let mut agent = ReactAgent::new(config);
    /// // 调用后，HumanInLoop 工具与 approval_provider 均指向 manager
    /// agent.set_human_loop_provider(manager);
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_human_loop_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.approval_provider = provider.clone();
        // 若 human_in_loop 工具已注册，用新 provider 重新注册，保持同步
        if self.tool_manager.get_tool("human_in_loop").is_some() {
            self.tool_manager
                .register(Box::new(HumanInLoop::new(provider)));
        }
    }
}

// ── LLM 每轮推理的输出类型 ────────────────────────────────────────────────────

/// LLM 每轮推理的输出类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    /// LLM 返回的纯文本响应（无工具调用时）
    Thought(String),

    /// LLM 发起的工具调用（一次响应可能包含多个，支持并行执行）
    Call {
        /// 工具调用唯一 ID，回传 observation 时需要匹配
        tool_call_id: String,
        function_name: String,
        arguments: Value,
    },
}

// ── impl Agent for ReactAgent ────────────────────────────────────────────────

#[async_trait]
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

    /// 统一执行入口：`enable_task=true` 时自动路由到规划模式，否则直接执行
    async fn execute(&mut self, task: &str) -> Result<String> {
        if self.has_planning_tools() {
            self.execute_with_planning(task).await
        } else {
            self.run_direct(task).await
        }
    }

    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream(task, run::StreamMode::Execute).await
    }

    async fn chat(&mut self, message: &str) -> Result<String> {
        self.run_chat_direct(message).await
    }

    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        self.run_stream(message, run::StreamMode::Chat).await
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
        self.skill_manager
            .list()
            .into_iter()
            .map(|s| s.name.clone())
            .collect()
    }

    fn mcp_server_names(&self) -> Vec<String> {
        self.mcp_manager
            .server_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    async fn close(&mut self) {
        self.mcp_manager.close_all().await;
    }
}
