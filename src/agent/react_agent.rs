pub use crate::agent::config::{AgentConfig, AgentRole};
use crate::agent::{Agent, AgentEvent, SubAgentMap};
use crate::compression::{ContextCompressor, ContextManager};
use crate::error::{AgentError, LlmError, ReactError, Result, ToolError};
use crate::human_loop::{
    HumanApprovalManager, HumanLoopProvider, HumanLoopRequest, HumanLoopResponse,
};
use crate::llm::types::{FunctionCall, Message, ToolCall as LlmToolCall};
use crate::llm::{ResponseFormat, chat, stream_chat};
use crate::memory::checkpointer::{Checkpointer, FileCheckpointer};
use crate::memory::store::{FileStore, Store};
use crate::skills::external::{LoadSkillResourceTool, SkillLoader};
use crate::skills::{Skill, SkillInfo, SkillManager};
use crate::tasks::TaskManager;
use crate::tools::builtin::agent_dispatch::AgentDispatchTool;
use crate::tools::builtin::answer::FinalAnswerTool;
use crate::tools::builtin::human_in_loop::HumanInLoop;
use crate::tools::builtin::memory::{ForgetTool, RecallTool, RememberTool};

use crate::tools::builtin::plan::PlanTool;
use crate::tools::builtin::task::{
    CreateTaskTool, GetExecutionOrderTool, ListTasksTool, UpdateTaskTool, VisualizeDependenciesTool,
};
use crate::tools::{Tool, ToolManager, ToolParameters};
use async_trait::async_trait;
use futures::StreamExt;
use futures::future::join_all;
use futures::stream::BoxStream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, info, warn};

// 内置工具名常量
pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";
pub(crate) const TOOL_CREATE_TASK: &str = "create_task";
pub(crate) const TOOL_PLAN: &str = "plan";
pub(crate) const TOOL_UPDATE_TASK: &str = "update_task";

/// 判断 LLM 错误是否值得重试（网络/超时/限流/服务端 5xx）
fn is_retryable_llm_error(err: &ReactError) -> bool {
    match err {
        ReactError::Llm(LlmError::NetworkError(_)) => true,
        ReactError::Llm(LlmError::ApiError { status, .. }) => *status == 429 || *status >= 500,
        _ => false,
    }
}

pub struct ReactAgent {
    pub(crate) config: AgentConfig,
    /// 上下文管理器：维护对话历史，并在 token 超限时自动触发压缩
    pub(crate) context: ContextManager,
    tool_manager: ToolManager,
    pub(crate) subagents: SubAgentMap,
    client: Arc<Client>,
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
}

impl ReactAgent {
    pub(crate) fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && [TOOL_PLAN, TOOL_CREATE_TASK, TOOL_UPDATE_TASK]
                .iter()
                .all(|name| self.tool_manager.get_tool(name).is_some())
    }

    /// 工具调用场景下自动注入的思维链引导语。
    ///
    /// 替代原来的 `think` 工具——让模型以文本形式在 content 字段输出推理过程，
    /// 从而天然产生流式 Token 事件，同时推理内容也进入对话上下文。
    const COT_INSTRUCTION: &'static str = "在调用工具之前，先用文字简述你的分析思路和执行计划。";

    pub fn new(config: AgentConfig) -> Self {
        // 当工具调用可用且 enable_cot=true 时，自动追加 CoT 引导语
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
            task_manager,
            human_in_loop,
            approval_provider,
            skill_manager: SkillManager::new(),
            store,
            checkpointer,
        }
    }

    /// 获取 AgentConfig 的只读引用
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// 注入自定义长期记忆 Store（仅替换自动注入通道，不重注册工具）
    ///
    /// 若需要同时让 `remember`/`recall`/`forget` 工具也使用新 Store，请改用
    /// [`set_memory_store`](ReactAgent::set_memory_store)。
    pub fn set_store(&mut self, store: Arc<dyn Store>) {
        self.store = Some(store);
    }

    /// 替换长期记忆 Store，并重新注册 `remember` / `recall` / `forget` 工具
    ///
    /// 适用于在 `ReactAgent::new()` 之后切换为 [`EmbeddingStore`](crate::memory::EmbeddingStore)
    /// 或其他自定义 Store 实现。
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
        // HashMap::insert 会覆盖同名工具，直接重注册即可
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
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::memory::checkpointer::{FileCheckpointer, Checkpointer};
    /// use echo_agent::prelude::ReactAgent;
    /// use std::sync::Arc;
    ///
    /// let mut agent = ReactAgent::new(config);
    /// let cp = FileCheckpointer::new("~/.echo-agent/checkpoints.json").unwrap();
    /// agent.set_checkpointer(Arc::new(cp), "alice-session-1".to_string());
    /// ```
    pub fn set_checkpointer(&mut self, checkpointer: Arc<dyn Checkpointer>, session_id: String) {
        self.checkpointer = Some(checkpointer);
        self.config.session_id = Some(session_id);
    }

    /// 获取当前 Checkpointer 的只读引用
    pub fn checkpointer(&self) -> Option<&Arc<dyn Checkpointer>> {
        self.checkpointer.as_ref()
    }

    /// 替换审批 Provider，支持在运行时切换审批渠道。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::human_loop::WebhookApprovalProvider;
    /// use echo_agent::prelude::ReactAgent;
    ///
    /// let mut agent = ReactAgent::new(config);
    /// agent.set_approval_provider(std::sync::Arc::new(
    ///     WebhookApprovalProvider::new("https://your-approval-server/approve"),
    /// ));
    /// ```
    pub fn set_approval_provider(&mut self, provider: Arc<dyn HumanLoopProvider>) {
        self.approval_provider = provider;
    }

    /// 重置消息历史，仅保留 system prompt，确保每次执行互不干扰
    pub(crate) fn reset_messages(&mut self) {
        self.context.clear();
        self.context
            .push(Message::system(self.config.system_prompt.clone()));
    }

    /// 执行工具，保留工具返回的真实错误信息
    pub(crate) async fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        let agent = &self.config.agent_name;
        let callbacks = self.config.callbacks.clone();
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().collect()
        } else {
            HashMap::new()
        };

        for cb in &callbacks {
            cb.on_tool_start(agent, tool_name, input).await;
        }

        info!(agent = %agent, tool = %tool_name, "🔧 开始执行工具");
        debug!(agent = %agent, tool = %tool_name, params = %input, "工具参数详情");

        let needs_approval = {
            let approval_manager = self.human_in_loop.read().unwrap();
            approval_manager.needs_approval(tool_name)
        };

        if needs_approval {
            warn!(agent = %agent, tool = %tool_name, "⚠️ 工具需要人工审批");
            let req = HumanLoopRequest::approval(tool_name, input.clone());
            match self.approval_provider.request(req).await? {
                HumanLoopResponse::Approved => {
                    info!(agent = %agent, tool = %tool_name, "✅ 用户批准执行工具");
                }
                HumanLoopResponse::Rejected { reason } => {
                    warn!(agent = %agent, tool = %tool_name, reason = ?reason, "❌ 用户拒绝执行工具");
                    return Ok(format!(
                        "用户已拒绝执行工具 {}{}",
                        tool_name,
                        reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                    ));
                }
                HumanLoopResponse::Timeout => {
                    warn!(agent = %agent, tool = %tool_name, "⏰ 审批超时，工具未执行");
                    return Ok(format!("工具 {tool_name} 审批超时，已跳过执行"));
                }
                HumanLoopResponse::Text(_) => {
                    // Approval 请求不应收到 Text 响应，视为拒绝
                    warn!(agent = %agent, tool = %tool_name, "⚠️ 审批请求收到意外的 Text 响应，视为拒绝");
                    return Ok(format!("工具 {tool_name} 审批异常，已跳过执行"));
                }
            }
        }

        let result = self.tool_manager.execute_tool(tool_name, params).await?;

        if result.success {
            info!(agent = %agent, tool = %tool_name, "📤 工具执行成功");
            debug!(agent = %agent, tool = %tool_name, output = %result.output, "工具返回详情");
            for cb in &callbacks {
                cb.on_tool_end(agent, tool_name, &result.output).await;
            }
            Ok(result.output)
        } else {
            let error_msg = result.error.unwrap_or_else(|| "工具执行失败".to_string());
            warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 工具执行失败");
            let err = ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: error_msg,
            });
            for cb in &callbacks {
                cb.on_tool_error(agent, tool_name, &err).await;
            }
            Err(err)
        }
    }

    /// 执行工具，并根据 `tool_error_feedback` 配置决定失败时的行为：
    /// - `true`（默认）：将错误信息转换为工具观测值回传给 LLM，让模型自行纠错
    /// - `false`：直接向上抛出 `Err`，与旧行为一致
    ///
    /// `final_answer` 工具始终保持原始错误语义，不会被软化。
    pub(crate) async fn execute_tool_feedback(
        &self,
        tool_name: &str,
        input: &Value,
    ) -> Result<String> {
        match self.execute_tool(tool_name, input).await {
            Ok(result) => Ok(result),
            Err(e) if self.config.tool_error_feedback && tool_name != TOOL_FINAL_ANSWER => {
                warn!(
                    agent = %self.config.agent_name,
                    tool = %tool_name,
                    error = %e,
                    "⚠️ 工具错误已转为观测值回传 LLM"
                );
                Ok(format!(
                    "[工具执行失败] {e}\n提示：请根据错误信息调整参数后重试，或换用其他工具。"
                ))
            }
            Err(e) => Err(e),
        }
    }

    /// 调用 LLM 推理，返回本轮的步骤列表。
    ///
    /// 每次调用前先通过 `ContextManager::prepare` 自动压缩超限的历史消息，
    /// 再将压缩后的消息列表传给 LLM；LLM 的响应追加回 context。
    pub(crate) async fn think(&mut self) -> Result<Vec<StepType>> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();
        let mut res = Vec::new();

        debug!(agent = %agent, model = %self.config.model_name, "🧠 LLM 思考中...");

        let messages = self.context.prepare(None).await?;

        for cb in &callbacks {
            cb.on_think_start(&agent, &messages).await;
        }

        let tools = self.tool_manager.to_openai_tools();
        let max_retries = self.config.llm_max_retries;
        let retry_delay = self.config.llm_retry_delay_ms;

        // 指数退避重试：只对可重试错误（网络/限流/5xx）进行重试
        let mut response_result: Result<_> = Err(ReactError::Agent(AgentError::NoResponse));
        for attempt in 0..=max_retries {
            if attempt > 0 {
                // 延迟 = delay * 2^(attempt-1)，最多放大到 2^5 = 32 倍
                let delay_ms = retry_delay * (1u64 << (attempt - 1).min(5));
                warn!(
                    agent = %agent,
                    attempt = attempt,
                    max = max_retries,
                    delay_ms = delay_ms,
                    "⚠️ LLM 请求失败，{delay_ms}ms 后重试（{attempt}/{max_retries}）"
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
            response_result = chat(
                self.client.clone(),
                self.config.model_name.as_str(),
                messages.clone(),
                Some(0.7),
                Some(8192u32),
                Some(false),
                Some(tools.clone()),
                None,
                self.config.response_format.clone(),
            )
            .await;
            match &response_result {
                Ok(_) => {
                    if attempt > 0 {
                        info!(agent = %agent, attempt, "✅ LLM 重试成功");
                    }
                    break;
                }
                Err(e) if attempt < max_retries && is_retryable_llm_error(e) => {
                    warn!(agent = %agent, error = %e, "LLM 可重试错误");
                }
                Err(_) => break,
            }
        }

        let message = response_result?
            .choices
            .first()
            .ok_or(ReactError::Agent(AgentError::NoResponse))?
            .message
            .clone();

        if let Some(tool_calls) = &message.tool_calls {
            self.context.push(message.clone());
            let tool_names: Vec<&str> = tool_calls
                .iter()
                .map(|c| c.function.name.as_str())
                .collect();
            info!(
                agent = %agent,
                tools = ?tool_names,
                "🧠 LLM 决定调用 {} 个工具",
                tool_calls.len()
            );
            for call in tool_calls {
                res.push(StepType::Call {
                    tool_call_id: call.id.clone(),
                    function_name: call.function.name.clone(),
                    arguments: serde_json::from_str(&call.function.arguments)?,
                });
            }
        } else if let Some(content) = &message.content {
            self.context.push(message.clone());
            debug!(agent = %agent, "🧠 LLM 返回文本响应");
            res.push(StepType::Thought(content.to_string()));
        }

        for cb in &callbacks {
            cb.on_think_end(&agent, &res).await;
        }

        Ok(res)
    }

    /// 处理一轮思考产生的步骤：
    /// - 有工具调用 → 并行执行（需要审批的工具强制串行），`final_answer` 时返回答案
    /// - 无工具调用 → 纯文本响应视为最终答案，直接返回
    pub(crate) async fn process_steps(&mut self, steps: Vec<StepType>) -> Result<Option<String>> {
        let agent = self.config.agent_name.clone();
        let mut tool_calls = Vec::new();
        let mut last_thought: Option<String> = None;

        for step in steps {
            match step {
                StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } => {
                    tool_calls.push((tool_call_id, function_name, arguments));
                }
                StepType::Thought(content) => {
                    debug!(agent = %agent, "🤔 思考: {}", content);
                    last_thought = Some(content);
                }
            }
        }

        // 无工具调用：纯文本响应视为最终答案
        if tool_calls.is_empty() {
            return Ok(last_thought.filter(|s| !s.is_empty()));
        }

        if tool_calls.len() > 1 {
            let tool_names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
            let max_concurrency = self.tool_manager.max_concurrency();
            info!(
                agent = %agent,
                tools = ?tool_names,
                max_concurrency = ?max_concurrency,
                "⚡ 并发执行 {} 个工具调用",
                tool_calls.len()
            );
        }

        // 需要人工审批的工具必须串行，避免并发读取 stdin 导致阻塞或输入串台
        let has_approval_tools = {
            let approval_manager = self.human_in_loop.read().unwrap();
            tool_calls
                .iter()
                .any(|(_, name, _)| approval_manager.needs_approval(name))
        };

        if has_approval_tools {
            info!(agent = %agent, "⚠️ 检测到需人工审批工具，切换为串行执行");
            for (tool_call_id, function_name, arguments) in tool_calls {
                let result = self
                    .execute_tool_feedback(&function_name, &arguments)
                    .await?;
                // 先推入 tool_result，确保上下文完整性
                self.context.push(Message::tool_result(
                    tool_call_id,
                    function_name.clone(),
                    result.clone(),
                ));
                if function_name == TOOL_FINAL_ANSWER {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    return Ok(Some(result));
                }
            }
        } else {
            let futures: Vec<_> = tool_calls
                .iter()
                .map(|(_, name, args)| self.execute_tool_feedback(name, args))
                .collect();
            let results = join_all(futures).await;

            let mut final_answer: Option<String> = None;
            for ((tool_call_id, function_name, _), result) in tool_calls.into_iter().zip(results) {
                let result = result?;
                // 先推入 tool_result，确保上下文完整性
                self.context.push(Message::tool_result(
                    tool_call_id,
                    function_name.clone(),
                    result.clone(),
                ));
                if function_name == TOOL_FINAL_ANSWER {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    final_answer = Some(result);
                }
            }
            if final_answer.is_some() {
                return Ok(final_answer);
            }
        }

        Ok(None)
    }

    /// 直接执行（无规划）：重置/恢复上下文，然后进入 ReAct 循环
    pub(crate) async fn run_direct(&mut self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        // 有 session_id 时尝试从 Checkpointer 恢复上次会话
        if let (Some(cp), Some(tid)) = (&self.checkpointer, &self.config.session_id) {
            match cp.get(tid).await {
                Ok(Some(checkpoint)) => {
                    info!(agent = %agent, session_id = %tid, checkpoint_id = %checkpoint.checkpoint_id, "🔄 从 Checkpoint 恢复会话");
                    self.context.clear();
                    for msg in checkpoint.messages {
                        self.context.push(msg);
                    }
                }
                Ok(None) => {
                    debug!(agent = %agent, session_id = %tid, "新会话，从空上下文开始");
                    self.reset_messages();
                }
                Err(e) => {
                    tracing::warn!(agent = %agent, error = %e, "⚠️ Checkpoint 加载失败，从空上下文开始");
                    self.reset_messages();
                }
            }
        } else {
            self.reset_messages();
        }

        info!(agent = %agent, "🧠 Agent 开始执行任务");
        debug!(
            agent = %agent,
            task = %task,
            tools = ?self.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "执行详情"
        );

        self.run_react_loop(task).await
    }

    /// 多轮对话：不重置上下文，直接追加消息后进入 ReAct 循环
    pub(crate) async fn run_chat_direct(&mut self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        info!(agent = %agent, "💬 Agent 多轮对话中");
        debug!(
            agent = %agent,
            message = %message,
            tools = ?self.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "对话详情"
        );

        self.run_react_loop(message).await
    }

    /// 核心 ReAct 循环（注入记忆 → 追加消息 → think/act 迭代）。
    /// `run_direct` 和 `run_chat_direct` 共享此实现。
    async fn run_react_loop(&mut self, message: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        let callbacks = self.config.callbacks.clone();

        // 搜索 Store 中与当前消息相关的长期记忆，前置注入到对话上下文
        if let Some(store) = &self.store {
            let agent_name = self.config.agent_name.clone();
            let ns = vec![agent_name.as_str(), "memories"];
            match store.semantic_search(&ns, message, 5).await {
                Ok(items) if !items.is_empty() => {
                    debug!(agent = %agent, count = items.len(), "📚 注入相关长期记忆");
                    let mut lines = vec!["[相关历史记忆]".to_string()];
                    for (i, item) in items.iter().enumerate() {
                        let content_str = item
                            .value
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| item.value.to_string());
                        lines.push(format!("{}. {}", i + 1, content_str));
                    }
                    lines.push("[以上记忆供参考，请结合当前问题作答]".to_string());
                    self.context.push(Message::user(lines.join("\n")));
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(agent = %agent, error = %e, "⚠️ 长期记忆检索失败，跳过注入");
                }
            }
        }

        self.context.push(Message::user(message.to_string()));

        for iteration in 0..self.config.max_iterations {
            for cb in &callbacks {
                cb.on_iteration(&agent, iteration).await;
            }

            debug!(agent = %agent, iteration = iteration + 1, "--- 迭代 ---");

            let steps = self.think().await?;
            if steps.is_empty() {
                warn!(agent = %agent, "LLM 没有响应");
                return Err(ReactError::from(AgentError::NoResponse));
            }

            if let Some(answer) = self.process_steps(steps).await? {
                for cb in &callbacks {
                    cb.on_final_answer(&agent, &answer).await;
                }
                info!(agent = %agent, "🏁 执行完毕");

                if let (Some(cp), Some(tid)) = (&self.checkpointer, self.config.session_id.clone())
                {
                    let messages = self.context.messages().to_vec();
                    match cp.put(&tid, messages).await {
                        Ok(cid) => {
                            debug!(agent = %agent, session_id = %tid, checkpoint_id = %cid, "🔖 Checkpoint 已保存")
                        }
                        Err(e) => {
                            tracing::warn!(agent = %agent, error = %e, "⚠️ Checkpoint 保存失败")
                        }
                    }
                }

                return Ok(answer);
            }
        }

        warn!(agent = %agent, max = self.config.max_iterations, "达到最大迭代次数");
        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}

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
        let task = task.to_string();
        let stream = async_stream::try_stream! {
            let agent = self.config.agent_name.clone();
            let callbacks = self.config.callbacks.clone();
            self.reset_messages();

            if let (Some(cp), Some(tid)) = (&self.checkpointer, &self.config.session_id) {
                match cp.get(tid).await {
                    Ok(Some(checkpoint)) => {
                        info!(agent = %agent, session_id = %tid, "🔄 从 Checkpoint 恢复会话（流式）");
                        self.context.clear();
                        for msg in checkpoint.messages {
                            self.context.push(msg);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(agent = %agent, error = %e, "⚠️ Checkpoint 加载失败");
                    }
                }
            }

            if let Some(store) = &self.store {
                let agent_name = self.config.agent_name.clone();
                let ns = vec![agent_name.as_str(), "memories"];
                if let Ok(items) = store.semantic_search(&ns, &task, 5).await &&
                     !items.is_empty() {
                        let mut lines = vec!["[相关历史记忆]".to_string()];
                        for (i, item) in items.iter().enumerate() {
                            let content_str = item.value.get("content")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                                .unwrap_or_else(|| item.value.to_string());
                            lines.push(format!("{}. {}", i + 1, content_str));
                        }
                        lines.push("[以上记忆供参考，请结合当前问题作答]".to_string());
                        self.context.push(Message::user(lines.join("\n")));
                }
            }

            self.context.push(Message::user(task));

            info!(agent = %agent, "🌊 Agent 开始流式执行任务");

            for iteration in 0..self.config.max_iterations {
                for cb in &callbacks {
                    cb.on_iteration(&agent, iteration).await;
                }

                debug!(agent = %agent, iteration = iteration + 1, "--- 流式迭代 ---");

                let messages = self.context.prepare(None).await?;

                for cb in &callbacks {
                    cb.on_think_start(&agent, &messages).await;
                }

                // enable_tool=false 时不传工具，LLM 走纯文本路径；
                // enable_tool=true 时先输出文本推理（Token 事件），再调用工具
                let tools_for_stream: Option<Vec<_>> = if self.config.enable_tool {
                    let tools = self.tool_manager.to_openai_tools();
                    if tools.is_empty() { None } else { Some(tools) }
                } else {
                    None
                };

                let max_retries = self.config.llm_max_retries;
                let retry_delay = self.config.llm_retry_delay_ms;

                // 流式连接阶段的指数退避重试（仅覆盖连接建立失败）
                let mut stream_result: Result<_> =
                    Err(ReactError::Agent(AgentError::NoResponse));
                for attempt in 0..=max_retries {
                    if attempt > 0 {
                        let delay_ms = retry_delay * (1u64 << (attempt - 1).min(5));
                        warn!(
                            agent = %agent,
                            attempt,
                            max = max_retries,
                            delay_ms,
                            "⚠️ 流式 LLM 请求失败，{delay_ms}ms 后重试（{attempt}/{max_retries}）"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                    }
                    stream_result = stream_chat(
                        self.client.clone(),
                        &self.config.model_name,
                        messages.clone(),
                        Some(0.7),
                        Some(8192u32),
                        tools_for_stream.clone(),
                        None,
                        self.config.response_format.clone(),
                    )
                    .await;
                    match &stream_result {
                        Ok(_) => {
                            if attempt > 0 {
                                info!(agent = %agent, attempt, "✅ 流式 LLM 重试成功");
                            }
                            break;
                        }
                        Err(e) if attempt < max_retries && is_retryable_llm_error(e) => {
                            warn!(agent = %agent, error = %e, "流式 LLM 可重试错误");
                        }
                        Err(_) => break,
                    }
                }
                let mut llm_stream = Box::pin(stream_result?);

                let mut content_buffer = String::new();
                // index → (id, name, arguments 拼接缓冲)
                let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
                let mut has_tool_calls = false;

                while let Some(chunk_result) = llm_stream.next().await {
                    let chunk = chunk_result?;

                    if let Some(choice) = chunk.choices.first() {
                        // 文本 token 增量
                        if let Some(content) = &choice.delta.content &&
                             !content.is_empty() {
                                content_buffer.push_str(content);
                                yield AgentEvent::Token(content.clone());
                        }

                        // 工具调用增量（逐 chunk 拼接 arguments）
                        if let Some(delta_calls) = &choice.delta.tool_calls {
                            has_tool_calls = true;
                            for dc in delta_calls {
                                let entry = tool_call_map
                                    .entry(dc.index)
                                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                                if let Some(id) = &dc.id &&
                                     !id.is_empty() {
                                        entry.0 = id.clone();
                                }
                                if let Some(f) = &dc.function {
                                    if let Some(name) = &f.name {
                                        // 某些 API 在后续 chunk 里会重复发 name=""，跳过空值避免覆盖
                                        if !name.is_empty() {
                                            entry.1 = name.clone();
                                        }
                                    }
                                    if let Some(args) = &f.arguments {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }

                if has_tool_calls {
                    let mut sorted_indices: Vec<u32> = tool_call_map.keys().cloned().collect();
                    sorted_indices.sort();

                    let mut msg_tool_calls: Vec<LlmToolCall> = Vec::new();
                    let mut steps: Vec<(String, String, Value)> = Vec::new();

                    for idx in &sorted_indices {
                        let (id, name, args_str) = &tool_call_map[idx];
                        let args: Value =
                            serde_json::from_str(args_str).unwrap_or(Value::Object(Default::default()));

                        yield AgentEvent::ToolCall {
                            name: name.clone(),
                            args: args.clone(),
                        };

                        msg_tool_calls.push(LlmToolCall {
                            id: id.clone(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: name.clone(),
                                arguments: args_str.clone(),
                            },
                        });
                        steps.push((id.clone(), name.clone(), args));
                    }

                    {
                        let think_steps: Vec<StepType> = steps.iter().map(|(id, name, args)| {
                            StepType::Call {
                                tool_call_id: id.clone(),
                                function_name: name.clone(),
                                arguments: args.clone(),
                            }
                        }).collect();
                        for cb in &callbacks {
                            cb.on_think_end(&agent, &think_steps).await;
                        }
                    }

                    self.context.push(Message::assistant_with_tools(msg_tool_calls));

                    let mut done = false;
                    for (tool_call_id, function_name, arguments) in steps {
                        let result = self.execute_tool_feedback(&function_name, &arguments).await?;

                        yield AgentEvent::ToolResult {
                            name: function_name.clone(),
                            output: result.clone(),
                        };

                        // 先推入 tool_result，确保上下文完整性
                        self.context.push(Message::tool_result(
                            tool_call_id,
                            function_name.clone(),
                            result.clone(),
                        ));

                        if function_name == TOOL_FINAL_ANSWER {
                            for cb in &callbacks {
                                cb.on_final_answer(&agent, &result).await;
                            }
                            info!(agent = %agent, "🏁 流式 Agent 执行完毕");
                            yield AgentEvent::FinalAnswer(result);
                            done = true;
                            break;
                        }
                    }

                    if done {
                        return;
                    }
                } else if !content_buffer.is_empty() {
                    // 无工具调用时纯文本响应视为最终答案
                    let think_steps = vec![StepType::Thought(content_buffer.clone())];
                    for cb in &callbacks {
                        cb.on_think_end(&agent, &think_steps).await;
                    }
                    for cb in &callbacks {
                        cb.on_final_answer(&agent, &content_buffer).await;
                    }
                    self.context.push(Message::assistant(content_buffer.clone()));
                    yield AgentEvent::FinalAnswer(content_buffer);
                    return;
                } else {
                    Err(ReactError::Agent(AgentError::NoResponse))?;
                }
            }

            Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
                self.config.max_iterations,
            )))?;
        };

        Ok(Box::pin(stream))
    }

    async fn chat(&mut self, message: &str) -> Result<String> {
        self.run_chat_direct(message).await
    }

    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        let message = message.to_string();
        let stream = async_stream::try_stream! {
            let agent = self.config.agent_name.clone();
            let callbacks = self.config.callbacks.clone();

            if let Some(store) = &self.store {
                let agent_name = self.config.agent_name.clone();
                let ns = vec![agent_name.as_str(), "memories"];
                if let Ok(items) = store.semantic_search(&ns, &message, 5).await &&
                     !items.is_empty() {
                        let mut lines = vec!["[相关历史记忆]".to_string()];
                        for (i, item) in items.iter().enumerate() {
                            let content_str = item.value.get("content")
                                .and_then(|v| v.as_str())
                                .map(String::from)
                                .unwrap_or_else(|| item.value.to_string());
                            lines.push(format!("{}. {}", i + 1, content_str));
                        }
                        lines.push("[以上记忆供参考，请结合当前问题作答]".to_string());
                        self.context.push(Message::user(lines.join("\n")));
                }
            }

            self.context.push(Message::user(message.clone()));

            info!(agent = %agent, "🌊 Agent 开始流式多轮对话");

            for iteration in 0..self.config.max_iterations {
                for cb in &callbacks {
                    cb.on_iteration(&agent, iteration).await;
                }

                debug!(agent = %agent, iteration = iteration + 1, "--- 流式对话迭代 ---");

                let messages = self.context.prepare(None).await?;

                for cb in &callbacks {
                    cb.on_think_start(&agent, &messages).await;
                }

                let tools_for_stream: Option<Vec<_>> = if self.config.enable_tool {
                    let tools = self.tool_manager.to_openai_tools();
                    if tools.is_empty() { None } else { Some(tools) }
                } else {
                    None
                };

                let max_retries = self.config.llm_max_retries;
                let retry_delay = self.config.llm_retry_delay_ms;

                let mut stream_result: Result<_> =
                    Err(ReactError::Agent(AgentError::NoResponse));
                for attempt in 0..=max_retries {
                    if attempt > 0 {
                        let delay_ms = retry_delay * (1u64 << (attempt - 1).min(5));
                        warn!(
                            agent = %agent,
                            attempt,
                            max = max_retries,
                            delay_ms,
                            "⚠️ 流式 LLM 请求失败，{delay_ms}ms 后重试（{attempt}/{max_retries}）"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                    }
                    stream_result = stream_chat(
                        self.client.clone(),
                        &self.config.model_name,
                        messages.clone(),
                        Some(0.7),
                        Some(8192u32),
                        tools_for_stream.clone(),
                        None,
                        self.config.response_format.clone(),
                    )
                    .await;
                    match &stream_result {
                        Ok(_) => {
                            if attempt > 0 {
                                info!(agent = %agent, attempt, "✅ 流式 LLM 重试成功");
                            }
                            break;
                        }
                        Err(e) if attempt < max_retries && is_retryable_llm_error(e) => {
                            warn!(agent = %agent, error = %e, "流式 LLM 可重试错误");
                        }
                        Err(_) => break,
                    }
                }
                let mut llm_stream = Box::pin(stream_result?);

                let mut content_buffer = String::new();
                let mut tool_call_map: HashMap<u32, (String, String, String)> = HashMap::new();
                let mut has_tool_calls = false;

                while let Some(chunk_result) = llm_stream.next().await {
                    let chunk = chunk_result?;

                    if let Some(choice) = chunk.choices.first() {
                        if let Some(content) = &choice.delta.content &&
                             !content.is_empty() {
                                content_buffer.push_str(content);
                                yield AgentEvent::Token(content.clone());
                        }

                        if let Some(delta_calls) = &choice.delta.tool_calls {
                            has_tool_calls = true;
                            for dc in delta_calls {
                                let entry = tool_call_map
                                    .entry(dc.index)
                                    .or_insert_with(|| (String::new(), String::new(), String::new()));
                                if let Some(id) = &dc.id &&
                                     !id.is_empty() {
                                        entry.0 = id.clone();
                                }
                                if let Some(f) = &dc.function {
                                    if let Some(name) = &f.name
                                        && !name.is_empty()
                                    {
                                        entry.1 = name.clone();
                                    }
                                    if let Some(args) = &f.arguments {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }
                    }
                }

                if has_tool_calls {
                    let mut sorted_indices: Vec<u32> = tool_call_map.keys().cloned().collect();
                    sorted_indices.sort();

                    let mut msg_tool_calls: Vec<LlmToolCall> = Vec::new();
                    let mut steps: Vec<(String, String, Value)> = Vec::new();

                    for idx in &sorted_indices {
                        let (id, name, args_str) = &tool_call_map[idx];
                        let args: Value =
                            serde_json::from_str(args_str).unwrap_or(Value::Object(Default::default()));

                        yield AgentEvent::ToolCall {
                            name: name.clone(),
                            args: args.clone(),
                        };

                        msg_tool_calls.push(LlmToolCall {
                            id: id.clone(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: name.clone(),
                                arguments: args_str.clone(),
                            },
                        });
                        steps.push((id.clone(), name.clone(), args));
                    }

                    {
                        let think_steps: Vec<StepType> = steps.iter().map(|(id, name, args)| {
                            StepType::Call {
                                tool_call_id: id.clone(),
                                function_name: name.clone(),
                                arguments: args.clone(),
                            }
                        }).collect();
                        for cb in &callbacks {
                            cb.on_think_end(&agent, &think_steps).await;
                        }
                    }

                    self.context.push(Message::assistant_with_tools(msg_tool_calls));

                    let mut done = false;
                    for (tool_call_id, function_name, arguments) in steps {
                        let result = self.execute_tool_feedback(&function_name, &arguments).await?;

                        yield AgentEvent::ToolResult {
                            name: function_name.clone(),
                            output: result.clone(),
                        };

                        // 先推入 tool_result，确保上下文完整性
                        self.context.push(Message::tool_result(
                            tool_call_id,
                            function_name.clone(),
                            result.clone(),
                        ));

                        if function_name == TOOL_FINAL_ANSWER {
                            for cb in &callbacks {
                                cb.on_final_answer(&agent, &result).await;
                            }
                            info!(agent = %agent, "✅ 流式对话轮次完成");
                            if let (Some(cp), Some(tid)) =
                                (&self.checkpointer, self.config.session_id.clone())
                            {
                                let messages = self.context.messages().to_vec();
                                if let Err(e) = cp.put(&tid, messages).await {
                                    tracing::warn!(agent = %agent, error = %e, "⚠️ Checkpoint 保存失败");
                                }
                            }
                            yield AgentEvent::FinalAnswer(result);
                            done = true;
                            break;
                        }
                    }

                    if done {
                        return;
                    }
                } else if !content_buffer.is_empty() {
                    let think_steps = vec![StepType::Thought(content_buffer.clone())];
                    for cb in &callbacks {
                        cb.on_think_end(&agent, &think_steps).await;
                    }
                    for cb in &callbacks {
                        cb.on_final_answer(&agent, &content_buffer).await;
                    }
                    self.context.push(Message::assistant(content_buffer.clone()));
                    if let (Some(cp), Some(tid)) =
                        (&self.checkpointer, self.config.session_id.clone())
                    {
                        let messages = self.context.messages().to_vec();
                        if let Err(e) = cp.put(&tid, messages).await {
                            tracing::warn!(agent = %agent, error = %e, "⚠️ Checkpoint 保存失败");
                        }
                    }
                    yield AgentEvent::FinalAnswer(content_buffer);
                    return;
                } else {
                    Err(ReactError::Agent(AgentError::NoResponse))?;
                }
            }

            Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
                self.config.max_iterations,
            )))?;
        };

        Ok(Box::pin(stream))
    }

    fn reset(&mut self) {
        self.reset_messages();
    }
}

impl ReactAgent {
    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        if !self.config.enable_tool {
            warn!(
                agent = %self.config.agent_name,
                tool = %tool.name(),
                "⚠️ tool 能力已禁用，忽略工具注册"
            );
            return;
        }
        self.tool_manager.register(tool)
    }

    pub fn add_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        if !self.config.enable_tool {
            warn!(
                agent = %self.config.agent_name,
                "⚠️ tool 能力已禁用，忽略批量工具注册"
            );
            return;
        }
        let allowed = &self.config.allowed_tools;
        if allowed.is_empty() {
            self.tool_manager.register_tools(tools);
        } else {
            for tool in tools {
                if allowed.contains(&tool.name().to_string()) {
                    self.tool_manager.register(tool);
                }
            }
        }
    }

    /// 注册需要人工审批的工具：执行前会在控制台弹出 y/n 确认
    pub fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>) {
        if !self.config.enable_tool {
            warn!(
                agent = %self.config.agent_name,
                tool = %tool.name(),
                "⚠️ tool 能力已禁用，忽略需要审批工具注册"
            );
            return;
        }
        if !self.config.enable_human_in_loop {
            warn!(
                agent = %self.config.agent_name,
                tool = %tool.name(),
                "⚠️ human_in_loop 能力已禁用，工具将注册但不会进入人工审批"
            );
            self.tool_manager.register(tool);
            return;
        }
        let tool_name = tool.name().to_string();
        self.tool_manager.register(tool);
        self.human_in_loop
            .write()
            .map_err(|e| {
                warn!("human_in_loop lock poisoned: {}", e);
            })
            .map(|mut guard| guard.mark_need_approval(tool_name))
            .ok();
    }

    /// 设置上下文压缩器。
    ///
    /// 配合 `AgentConfig::token_limit` 使用：token 超限时自动在 `think()` 前压缩消息历史。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::compression::compressor::{SlidingWindowCompressor, SummaryCompressor, DefaultSummaryPrompt};
    /// use echo_agent::llm::DefaultLlmClient;
    /// use reqwest::Client;
    /// use std::sync::Arc;
    ///
    /// # fn example(agent: &mut echo_agent::agent::react_agent::ReactAgent) {
    /// // 纯滑动窗口（无需 LLM）
    /// agent.set_compressor(SlidingWindowCompressor::new(20));
    ///
    /// // 或摘要压缩（需要 LLM 调用）
    /// let llm = Arc::new(DefaultLlmClient::new(Arc::new(Client::new()), "qwen3-max"));
    /// agent.set_compressor(SummaryCompressor::new(llm, DefaultSummaryPrompt, 8));
    /// # }
    /// ```
    pub fn set_compressor(&mut self, compressor: impl ContextCompressor + 'static) {
        self.context.set_compressor(compressor);
    }

    /// 返回当前上下文的（消息条数，估算 token 数）
    pub fn context_stats(&self) -> (usize, usize) {
        (self.context.messages().len(), self.context.token_estimate())
    }

    /// 使用指定压缩器强制压缩上下文（不影响已安装的默认压缩器）
    pub async fn force_compress_with(
        &mut self,
        compressor: &dyn crate::compression::ContextCompressor,
    ) -> crate::error::Result<crate::compression::ForceCompressStats> {
        self.context.force_compress_with(compressor).await
    }

    pub fn list_tools(&self) -> Vec<&str> {
        self.tool_manager.list_tools()
    }

    pub fn register_agent(&mut self, agent: Box<dyn Agent>) {
        if !self.config.enable_subagent {
            warn!(
                agent = %self.config.agent_name,
                subagent = %agent.name(),
                "⚠️ subagent 能力已禁用，忽略子 agent 注册"
            );
            return;
        }
        let name = agent.name().to_string();
        match self.subagents.write() {
            Ok(mut agents) => {
                agents.insert(name, Arc::new(AsyncMutex::new(agent)));
            }
            Err(e) => {
                warn!(
                    agent = %self.config.agent_name,
                    subagent = %name,
                    "⚠️ subagents lock poisoned，无法注册子 agent: {}",
                    e
                );
            }
        }
    }

    pub fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    pub fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }

    /// 运行时注册事件回调
    pub fn add_callback(&mut self, callback: std::sync::Arc<dyn crate::agent::AgentCallback>) {
        self.config.callbacks.push(callback);
    }

    // ── 外部 Skill 文件系统加载 ───────────────────────────────────────────────

    /// 扫描指定目录下的所有外部技能（SKILL.md），并将它们安装到 Agent
    ///
    /// # 整体流程
    ///
    /// ```text
    /// 1. 扫描 skills_dir/ 下的每个子目录
    /// 2. 解析 SKILL.md 的 YAML Frontmatter → SkillMeta
    /// 3. 将 meta.instructions 注入 system_prompt
    /// 4. 预加载 load_on_startup: true 的资源并追加到 system_prompt
    /// 5. 注册 LoadSkillResourceTool（LLM 按需调用懒加载其余资源）
    /// 6. 在 SkillManager 中记录元数据
    /// ```
    ///
    /// # 参数
    /// - `skills_dir`: 技能根目录路径（绝对或相对路径均可）
    ///
    /// # 返回
    /// 成功加载的技能名称列表
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// agent.load_skills_from_dir("./skills").await?;
    /// ```
    pub async fn load_skills_from_dir(
        &mut self,
        skills_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Vec<String>> {
        let loader = std::sync::Arc::new(tokio::sync::Mutex::new(SkillLoader::new(skills_dir)));

        // 扫描并加载所有 SKILL.md
        let loaded = {
            let mut l = loader.lock().await;
            l.scan().await?
        };

        if loaded.is_empty() {
            tracing::warn!(
                agent = %self.config.agent_name,
                "外部技能目录扫描完毕，未找到任何有效 SKILL.md"
            );
            return Ok(vec![]);
        }

        let mut loaded_names = Vec::new();
        let mut has_resources = false;

        for skill in &loaded {
            let meta = &skill.meta;

            // 跳过已安装的技能（避免重复注入）
            if self.skill_manager.is_installed(&meta.name) {
                tracing::warn!(
                    agent = %self.config.agent_name,
                    skill = %meta.name,
                    "Skill 已安装，跳过"
                );
                continue;
            }

            // 注入 instructions 到 system prompt
            let prompt_block = meta.to_prompt_block();
            self.config.system_prompt.push_str(&prompt_block);

            // 若有 load_on_startup 资源，追加其内容
            {
                let l = loader.lock().await;
                for res_ref in meta.startup_resources() {
                    if l.is_cached(&meta.name, &res_ref.name) {
                        // 内容已在 scan() 中预加载到 loader，这里只需要把内容再注入到 prompt
                        // （实际上 scan() 已缓存，由 load_resource 提供，此处仅记录）
                        tracing::debug!(
                            "预加载资源 '{}/{}' 已就绪，可通过工具访问",
                            meta.name,
                            res_ref.name
                        );
                    }
                }
            }

            // 检查是否有任何资源需要懒加载工具
            if meta.resources.as_ref().is_some_and(|r| !r.is_empty()) {
                has_resources = true;
            }

            // 记录到 SkillManager
            let tool_names = if has_resources {
                vec!["load_skill_resource".to_string()]
            } else {
                vec![]
            };
            self.skill_manager.record(SkillInfo {
                name: meta.name.clone(),
                description: meta.description.clone(),
                tool_names,
                has_prompt_injection: true,
            });

            tracing::info!(
                agent = %self.config.agent_name,
                skill = %meta.name,
                version = %meta.version.as_deref().unwrap_or("?"),
                resources = meta.resources.as_ref().map_or(0, |r| r.len()),
                "🎯 外部 Skill 已加载"
            );

            loaded_names.push(meta.name.clone());
        }

        // 同步更新 context 中的 system message
        self.context
            .update_system(self.config.system_prompt.clone());

        // 注册资源懒加载工具（只注册一次，即使有多个 skill 有资源）
        if has_resources && self.tool_manager.get_tool("load_skill_resource").is_none() {
            // 构建资源目录描述，帮助 LLM 选择正确的参数
            let catalog_desc = {
                let l = loader.lock().await;
                l.resource_catalog()
                    .iter()
                    .map(|(sname, rref)| {
                        format!(
                            "  - {}/{}: {}",
                            sname,
                            rref.name,
                            rref.description.as_deref().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            let tool = LoadSkillResourceTool::new(loader).with_catalog_desc(catalog_desc);
            self.tool_manager.register(Box::new(tool));

            tracing::info!(
                agent = %self.config.agent_name,
                "已注册 load_skill_resource 工具"
            );
        }

        Ok(loaded_names)
    }

    // ── Skill API ─────────────────────────────────────────────────────────────

    /// 为 Agent 安装一个 Skill
    ///
    /// 安装过程：
    /// 1. 将 Skill 提供的所有工具注册到 ToolManager
    /// 2. 若 Skill 有 system_prompt_injection，追加到 system_prompt
    /// 3. 记录 Skill 元数据到 SkillManager
    ///
    /// # 示例
    /// ```rust
    /// agent.add_skill(Box::new(CalculatorSkill));
    /// agent.add_skill(Box::new(FileSystemSkill::with_base_dir("/workspace")));
    /// ```
    pub fn add_skill(&mut self, skill: Box<dyn Skill>) {
        let name = skill.name().to_string();

        if self.skill_manager.is_installed(&name) {
            warn!(
                agent = %self.config.agent_name,
                skill = %name,
                "⚠️ Skill 已安装，跳过重复注册"
            );
            return;
        }

        // Step 1: 收集 Skill 工具信息（在 move 之前）
        let tools = skill.tools();
        let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

        // Step 2: 注册工具
        for tool in tools {
            self.tool_manager.register(tool);
        }

        // Step 3: 注入系统提示词
        let has_injection = skill.system_prompt_injection().is_some();
        if let Some(injection) = skill.system_prompt_injection() {
            self.config.system_prompt.push_str(&injection);
            // 同步更新 context 中的 system 消息
            self.context
                .update_system(self.config.system_prompt.clone());
        }

        // Step 4: 记录元数据
        self.skill_manager.record(SkillInfo {
            name: name.clone(),
            description: skill.description().to_string(),
            tool_names,
            has_prompt_injection: has_injection,
        });

        info!(
            agent = %self.config.agent_name,
            skill = %name,
            description = %skill.description(),
            "🎯 Skill 已安装"
        );
    }

    /// 批量安装多个 Skill
    pub fn add_skills(&mut self, skills: Vec<Box<dyn Skill>>) {
        for skill in skills {
            self.add_skill(skill);
        }
    }

    /// 列出所有已安装的 Skill 元数据
    pub fn list_skills(&self) -> Vec<&SkillInfo> {
        self.skill_manager.list()
    }

    /// 查询某个 Skill 是否已安装
    pub fn has_skill(&self, name: &str) -> bool {
        self.skill_manager.is_installed(name)
    }

    /// 已安装的 Skill 数量
    pub fn skill_count(&self) -> usize {
        self.skill_manager.count()
    }

    /// 一次性结构化 JSON 提取，不走 ReAct 循环。
    ///
    /// 直接向 LLM 发一次请求，要求按 `schema` 返回 JSON，
    /// 返回解析后的 [`serde_json::Value`]。
    ///
    /// 适合"提取 / 分类 / 格式转换"等不需要工具调用的场景。
    ///
    /// # 参数
    /// - `prompt`：用户输入或待处理文本
    /// - `schema`：目标 JSON Schema（`ResponseFormat::json_schema(name, schema)` 快速构建）
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::llm::ResponseFormat;
    /// use serde_json::json;
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// # use echo_agent::prelude::*;
    /// # let config = AgentConfig::new("gpt-4o", "extractor", "你是一个提取助手");
    /// # let agent = ReactAgent::new(config);
    /// let result = agent.extract_json(
    ///     "从文本中提取人名和年龄：张三，28岁",
    ///     ResponseFormat::json_schema(
    ///         "person",
    ///         json!({ "type": "object",
    ///                 "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
    ///                 "required": ["name", "age"],
    ///                 "additionalProperties": false }),
    ///     ),
    /// ).await?;
    /// println!("{}", result["name"]);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract_json(
        &self,
        prompt: &str,
        schema: ResponseFormat,
    ) -> Result<serde_json::Value> {
        let messages = vec![
            Message::system(self.config.system_prompt.clone()),
            Message::user(prompt.to_string()),
        ];

        let response = chat(
            self.client.clone(),
            &self.config.model_name,
            messages,
            Some(0.0),
            Some(4096),
            Some(false),
            None,
            None,
            Some(schema),
        )
        .await?;

        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| ReactError::Other("LLM 返回空内容".to_string()))?;

        serde_json::from_str(&text)
            .map_err(|e| ReactError::Other(format!("JSON 解析失败: {e}\n原始响应: {text}")))
    }

    /// 一次性结构化提取，自动将 JSON 结果反序列化为指定类型 `T`。
    ///
    /// 与 [`extract_json`](Self::extract_json) 相同，但额外执行 `serde` 反序列化。
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_agent::llm::ResponseFormat;
    /// use serde::{Deserialize, Serialize};
    /// use serde_json::json;
    ///
    /// #[derive(Debug, Deserialize)]
    /// struct Person { name: String, age: u32 }
    ///
    /// # async fn run() -> echo_agent::error::Result<()> {
    /// # use echo_agent::prelude::*;
    /// # let config = AgentConfig::new("gpt-4o", "extractor", "你是一个提取助手");
    /// # let agent = ReactAgent::new(config);
    /// let person: Person = agent.extract(
    ///     "张三，28岁",
    ///     ResponseFormat::json_schema(
    ///         "person",
    ///         json!({ "type": "object",
    ///                 "properties": { "name": { "type": "string" }, "age": { "type": "integer" } },
    ///                 "required": ["name", "age"],
    ///                 "additionalProperties": false }),
    ///     ),
    /// ).await?;
    /// println!("姓名: {}, 年龄: {}", person.name, person.age);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extract<T>(&self, prompt: &str, schema: ResponseFormat) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = self.extract_json(prompt, schema).await?;
        serde_json::from_value(value).map_err(|e| ReactError::Other(format!("反序列化失败: {e}")))
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::llm::types::Message;
    use crate::testing::{FailingMockAgent, MockAgent};

    // ── ReactAgent::reset() ───────────────────────────────────────────────────

    /// reset() 应清除所有消息，仅保留 system prompt（1 条）
    #[test]
    fn react_agent_reset_clears_to_system_only() {
        let config = AgentConfig::new("test-model", "test_agent", "你是测试助手");
        let mut agent = ReactAgent::new(config);

        // 初始只有 system prompt
        let (count, _) = agent.context_stats();
        assert_eq!(count, 1, "初始应只有 1 条 system 消息");

        // 手动追加几条消息
        agent.context.push(Message::user("你好".to_string()));
        agent.context.push(Message::assistant("你好！".to_string()));
        agent.context.push(Message::user("再见".to_string()));
        let (count_after_push, _) = agent.context_stats();
        assert_eq!(count_after_push, 4, "追加后应有 4 条消息");

        // reset() 后回到只有 system prompt
        agent.reset();
        let (count_after_reset, _) = agent.context_stats();
        assert_eq!(count_after_reset, 1, "reset() 后应只剩 1 条 system 消息");
    }

    /// 连续 reset() 多次应幂等，不产生重复的 system prompt
    #[test]
    fn react_agent_reset_is_idempotent() {
        let config = AgentConfig::new("test-model", "test_agent", "系统提示词");
        let mut agent = ReactAgent::new(config);

        agent.reset();
        agent.reset();
        agent.reset();

        let (count, _) = agent.context_stats();
        assert_eq!(count, 1, "多次 reset() 后应仍只有 1 条 system 消息");
    }

    /// reset() 后 system prompt 内容应保持不变
    #[test]
    fn react_agent_reset_preserves_system_prompt() {
        let system = "这是一个自定义的系统提示词";
        let config = AgentConfig::new("test-model", "agent", system);
        let mut agent = ReactAgent::new(config);

        agent
            .context
            .push(Message::user("随便什么消息".to_string()));
        agent.reset();

        let messages = agent.context.messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content.as_deref().unwrap_or(""), system);
    }

    // ── Agent trait 合约 ──────────────────────────────────────────────────────

    /// reset() 可通过 &mut dyn Agent 调用（trait 对象安全性验证）
    #[tokio::test]
    async fn trait_reset_callable_via_dyn_agent() {
        let mut agent: Box<dyn Agent> = Box::new(
            MockAgent::new("mock")
                .with_response("r1")
                .with_response("r2"),
        );

        let r1 = agent.chat("msg1").await.unwrap();
        assert_eq!(r1, "r1");

        agent.reset(); // 通过 dyn Agent 调用 reset()

        let r2 = agent.chat("msg2").await.unwrap();
        assert_eq!(r2, "r2");
    }

    // ── MockAgent 合约 ────────────────────────────────────────────────────────

    /// chat() 应记录调用，并消费预设响应队列
    #[tokio::test]
    async fn mock_agent_chat_records_calls_and_consumes_responses() {
        let mut agent = MockAgent::new("test")
            .with_response("回复1")
            .with_response("回复2")
            .with_response("回复3");

        let r1 = agent.chat("消息1").await.unwrap();
        let r2 = agent.chat("消息2").await.unwrap();
        let r3 = agent.chat("消息3").await.unwrap();

        assert_eq!(r1, "回复1");
        assert_eq!(r2, "回复2");
        assert_eq!(r3, "回复3");
        assert_eq!(agent.call_count(), 3);
        assert_eq!(agent.calls(), vec!["消息1", "消息2", "消息3"]);
    }

    /// reset() 应清空 MockAgent 的调用历史（模拟对话重置语义）
    #[tokio::test]
    async fn mock_agent_reset_clears_call_history() {
        let mut agent = MockAgent::new("test")
            .with_response("r1")
            .with_response("r2")
            .with_response("r3");

        agent.chat("第一轮消息1").await.unwrap();
        agent.chat("第一轮消息2").await.unwrap();
        assert_eq!(agent.call_count(), 2, "reset 前应有 2 条记录");

        agent.reset();
        assert_eq!(agent.call_count(), 0, "reset 后调用历史应清空");

        agent.chat("第二轮消息1").await.unwrap();
        assert_eq!(agent.call_count(), 1, "reset 后第二轮应从 1 开始计数");
        assert_eq!(agent.calls(), vec!["第二轮消息1"]);
    }

    /// execute() 和 chat() 共享同一个响应队列
    #[tokio::test]
    async fn mock_agent_execute_and_chat_share_response_queue() {
        let mut agent = MockAgent::new("test")
            .with_response("execute回复")
            .with_response("chat回复");

        let r1 = agent.execute("任务").await.unwrap();
        let r2 = agent.chat("对话").await.unwrap();

        assert_eq!(r1, "execute回复");
        assert_eq!(r2, "chat回复");
        assert_eq!(agent.call_count(), 2);
    }

    /// 响应队列耗尽后，chat() 应返回默认响应
    #[tokio::test]
    async fn mock_agent_chat_falls_back_to_default_when_queue_empty() {
        let mut agent = MockAgent::new("test"); // 无预设响应

        let r = agent.chat("任意消息").await.unwrap();
        assert_eq!(r, "mock agent response", "队列空时应返回默认响应");
    }

    /// FailingMockAgent::reset() 清空调用历史
    #[tokio::test]
    async fn failing_mock_agent_reset_clears_calls() {
        let mut agent = FailingMockAgent::new("failing", "总是失败");

        agent.execute("任务1").await.unwrap_err();
        agent.chat("任务2").await.unwrap_err();
        assert_eq!(agent.call_count(), 2);

        agent.reset();
        assert_eq!(agent.call_count(), 0, "reset 后应清空调用记录");
    }

    // ── chat + reset 完整生命周期 ─────────────────────────────────────────────

    /// 模拟典型多轮对话生命周期：chat → reset → chat
    #[tokio::test]
    async fn mock_agent_full_chat_lifecycle() {
        let mut agent = MockAgent::new("assistant").with_responses([
            "轮1回复1",
            "轮1回复2",
            "轮2回复1",
            "轮2回复2",
        ]);

        // 第一轮对话
        agent.chat("第1轮：问题A").await.unwrap();
        agent.chat("第1轮：问题B").await.unwrap();
        assert_eq!(agent.call_count(), 2);

        // 重置，开启第二轮
        agent.reset();
        assert_eq!(agent.call_count(), 0);

        // 第二轮对话
        agent.chat("第2轮：问题C").await.unwrap();
        agent.chat("第2轮：问题D").await.unwrap();
        assert_eq!(agent.call_count(), 2);
        assert_eq!(agent.calls(), vec!["第2轮：问题C", "第2轮：问题D"]);
    }
}
