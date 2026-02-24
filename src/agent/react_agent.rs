use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::human_loop::HumanApprovalManager;
use crate::llm::chat;
use crate::llm::types::Message;
use crate::tasks::{TaskManager, TaskStatus};
use crate::tools::agent::AgentDispatchTool;
use crate::tools::answer::FinalAnswerTool;
use crate::tools::human_in_loop::HumanInLoop;
use crate::tools::reasoning::ThinkTool;
use crate::tools::task_management::{
    CreateTaskTool, GetExecutionOrderTool, ListTasksTool, PlanTool, UpdateTaskTool,
    VisualizeDependenciesTool,
};
use crate::tools::{Tool, ToolManager, ToolParameters};
use async_trait::async_trait;
use futures::future::join_all;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::option::Option;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

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
    model_name: String,
    /// 系统提示词
    system_prompt: String,
    /// 是否启用详细日志
    verbose: bool,
    /// agent 名称
    agent_name: String,
    /// 最大迭代次数
    max_iterations: usize,
    /// 可使用的工具（为空表示不限制）
    allowed_tools: Vec<String>,
    /// agent 角色
    role: AgentRole,
    /// 是否允许注册并调用业务工具（如数学、天气等）
    enable_tool: bool,
    /// 是否启用任务能力（plan/create_task/update_task）
    enable_task: bool,
    /// 是否启用 human-in-loop 工具
    enable_human_in_loop: bool,
    /// 是否启用 subagent 调度能力（agent_tool）
    enable_subagent: bool,
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

    pub fn get_allowed_tools(&self) -> &Vec<String> {
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

}

pub struct ReactAgent {
    config: AgentConfig,
    messages: Vec<Message>,
    tool_manager: ToolManager,
    subagents: Arc<RwLock<HashMap<String, Box<dyn Agent>>>>,
    steps: Vec<ReactStep>,
    client: Arc<Client>,
    task_manager: Arc<RwLock<TaskManager>>,
    human_in_loop: Arc<RwLock<HumanApprovalManager>>,
}

impl ReactAgent {
    fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && ["plan", "create_task", "update_task"]
                .iter()
                .all(|name| self.tool_manager.get_tool(name).is_some())
    }

    pub fn new(config: AgentConfig) -> Self {
        let system_message = Message {
            role: "system".to_string(),
            content: Option::from(config.system_prompt.clone()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        };
        let messages = vec![system_message];
        let mut tool_manager = ToolManager::new();
        let client = reqwest::Client::new();

        // 基础工具：所有 agent 共享
        tool_manager.register(Box::new(FinalAnswerTool));
        tool_manager.register(Box::new(ThinkTool));
        if config.enable_human_in_loop {
            tool_manager.register(Box::new(HumanInLoop));
        }

        let task_manager = Arc::new(RwLock::new(TaskManager::default()));
        let human_in_loop = Arc::new(RwLock::new(HumanApprovalManager::new()));
        let subagents = Arc::new(RwLock::new(HashMap::new()));

        if config.enable_task {
            // 规划能力：任务管理工具
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
            // 子 agent 编排能力
            tool_manager.register(Box::new(AgentDispatchTool::new(subagents.clone())));
        }

        Self {
            config,
            messages,
            tool_manager,
            subagents,
            steps: Vec::new(),
            client: Arc::new(client),
            task_manager,
            human_in_loop,
        }
    }

    /// 重置消息历史，仅保留 system prompt，确保每次执行互不干扰
    fn reset_messages(&mut self) {
        let system_message = Message {
            role: "system".to_string(),
            content: Option::from(self.config.system_prompt.clone()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        };
        self.messages = vec![system_message];
    }

    /// 执行工具
    async fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        let agent = &self.config.agent_name;

        // 将 JSON Value 转换为 ToolParameters
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().map(|(k, v)| (k, v)).collect()
        } else {
            HashMap::new()
        };

        info!(agent = %agent, tool = %tool_name, "🔧 开始执行工具");
        debug!(agent = %agent, tool = %tool_name, params = %input, "工具参数详情");

        let needs_approval = {
            let approval_manager = self.human_in_loop.read().unwrap();
            approval_manager.needs_approval(tool_name)
        };

        if needs_approval {
            warn!(agent = %agent, tool = %tool_name, "⚠️ 工具需要人工审批，是否批准？(y/n)");

            let mut user_input = String::new();
            std::io::stdin()
                .read_line(&mut user_input)
                .expect("读取输入失败");

            if user_input.trim() != "y" && user_input.trim() != "Y" {
                warn!(agent = %agent, tool = %tool_name, "❌ 用户拒绝执行工具");
                return Ok(format!("用户已拒绝执行工具 {}", tool_name));
            }
            info!(agent = %agent, tool = %tool_name, "✅ 用户批准执行工具");
        }

        let result = self.tool_manager.execute_tool(tool_name, params).await?;

        if result.success {
            info!(agent = %agent, tool = %tool_name, "📤 工具执行成功");
            debug!(agent = %agent, tool = %tool_name, output = %result.output, "工具返回详情");
            Ok(result.output)
        } else {
            warn!(agent = %agent, tool = %tool_name, "💥 工具执行失败");
            Err(ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: "工具执行失败".to_string(),
            }))
        }
    }

    pub(crate) async fn think(&mut self) -> Result<Vec<StepType>> {
        let agent = self.config.agent_name.clone();
        let mut res = Vec::new();

        debug!(agent = %agent, model = %self.config.model_name, "🧠 LLM 思考中...");

        // 第一步，构建 tools 定义
        let tools = self.tool_manager.to_openai_tools();

        let response = chat(
            self.client.clone(),
            self.config.model_name.as_str(),
            self.messages.clone(),
            Some(0.7),
            Some(8192u32),
            Some(false),
            Some(tools),
            None,
        )
        .await;

        let message = response?
            .choices
            .first()
            .ok_or(ReactError::Agent(AgentError::NoResponse))?
            .message
            .clone();

        if let Some(tool_calls) = &message.tool_calls {
            self.messages.push(message.clone());
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
            // 没有工具调用，是纯文本响应（思考或最终答案）
            self.messages.push(message.clone());
            debug!(agent = %agent, "🧠 LLM 返回文本响应");
            res.push(StepType::Thought(content.to_string()));
        }
        Ok(res)
    }

    /// 处理一轮思考产生的所有步骤（工具调用并行执行），返回 final_answer 结果（如有）
    async fn process_steps(&mut self, steps: Vec<StepType>) -> Result<Option<String>> {
        let agent = self.config.agent_name.clone();
        // 分离工具调用和其他步骤
        let mut tool_calls = Vec::new();

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
                }
                _ => {}
            }
        }

        if tool_calls.is_empty() {
            return Ok(None);
        }

        if tool_calls.len() > 1 {
            let tool_names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
            info!(
                agent = %agent,
                tools = ?tool_names,
                "⚡ 并行执行 {} 个工具调用",
                tool_calls.len()
            );
        }

        // 对需要人工审批的工具，必须串行执行，避免并发读取 stdin 导致阻塞或输入串台
        let has_approval_tools = {
            let approval_manager = self.human_in_loop.read().unwrap();
            tool_calls
                .iter()
                .any(|(_, function_name, _)| approval_manager.needs_approval(function_name))
        };

        if has_approval_tools {
            info!(
                agent = %agent,
                "⚠️ 检测到需人工审批工具，切换为串行执行"
            );
            for (tool_call_id, function_name, arguments) in tool_calls {
                let result = self.execute_tool(&function_name, &arguments).await?;

                if function_name == "final_answer" {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    return Ok(Some(result));
                }

                self.messages
                    .push(Message::tool_result(tool_call_id, function_name, result));
            }
        } else {
            // 并行执行所有工具调用
            let futures: Vec<_> = tool_calls
                .iter()
                .map(|(_, name, args)| self.execute_tool(name, args))
                .collect();
            let results = join_all(futures).await;

            // 收集结果并推入消息
            for ((tool_call_id, function_name, _), result) in tool_calls.into_iter().zip(results) {
                let result = result?;

                if function_name == "final_answer" {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    return Ok(Some(result));
                }

                self.messages
                    .push(Message::tool_result(tool_call_id, function_name, result));
            }
        }

        Ok(None)
    }

    pub async fn execute_with_planning(&mut self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        // 重置消息历史和任务管理器，确保每次规划都是干净的 session
        self.reset_messages();
        if let Ok(mut manager) = self.task_manager.write() {
            *manager = TaskManager::default();
        }

        info!(agent = %agent, "🎯 启动任务规划模式");
        info!(agent = %agent, task = %task, "📋 用户任务");

        // 未启用规划能力或未注册规划工具时，降级到普通执行，避免卡在规划流程
        if !self.has_planning_tools() {
            warn!(
                agent = %agent,
                "⚠️ 当前 agent 未启用规划能力或未注册完整规划工具集，自动降级为普通执行模式"
            );
            return self.execute(task).await;
        }

        // ── 第一阶段：让 Agent 制定计划 ──────────────────────
        info!(agent = %agent, phase = "planning", "📐 阶段1: 制定计划");

        let planning_prompt = format!(
            "{}\n\n\
            请先使用 think 工具分析问题，然后用 plan 工具制定计划，最后用 create_task 逐个创建所有子任务。\n\n\
            **重要：任务拆分规则**\n\
            - 将问题拆分为尽可能细粒度的子任务，每个子任务只做一件事\n\
            - 互相独立的子任务不要设置依赖关系，让它们可以并行执行\n\
            - 只有当一个任务真正需要另一个任务的结果时，才设置 dependencies\n\
            - 尽量构建宽而浅的 DAG（有向无环图），而非线性链\n\
            - **必须创建全部子任务后规划才算完成，不要只创建部分就停止**",
            task
        );

        self.messages.push(Message::user(planning_prompt));

        // 执行直到所有子任务创建完毕（LLM 停止调用 create_task 时视为规划结束）
        let planning_max_rounds = self.config.max_iterations;
        let mut has_created_tasks = false;

        for round in 0..planning_max_rounds {
            debug!(agent = %agent, round = round + 1, "📐 规划轮次");
            let steps = self.think().await?;
            let mut created_task_this_round = false;

            for step in steps {
                if let StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } = step
                {
                    if function_name == "create_task" {
                        created_task_this_round = true;
                    }
                    let result = self.execute_tool(&function_name, &arguments).await?;
                    if function_name == "final_answer" {
                        info!(agent = %agent, "🏁 规划阶段已生成最终答案");
                        return Ok(result);
                    }
                    self.messages
                        .push(Message::tool_result(tool_call_id, function_name, result));
                }
            }

            if created_task_this_round {
                has_created_tasks = true;
            }

            // 已经创建过任务，但本轮没有继续创建 → 规划完成
            if has_created_tasks && !created_task_this_round {
                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
                let task_count = manager.get_all_tasks().len();
                info!(
                    agent = %agent,
                    task_count = task_count,
                    "📐 规划完成，共创建 {} 个子任务",
                    task_count
                );
                break;
            }
        }

        // 规划阶段结束后仍无任务，说明模型未按规划协议工作，回退普通执行
        let planned_task_count = self
            .task_manager
            .read()
            .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?
            .get_all_tasks()
            .len();
        if planned_task_count == 0 {
            warn!(
                agent = %agent,
                "⚠️ 规划阶段未创建任务，自动降级为普通执行模式"
            );
            return self.execute(task).await;
        }

        // ── 第二阶段：并行执行就绪任务 ──────────────────────
        info!(agent = %agent, phase = "execution", "🚀 阶段2: 执行任务");

        loop {
            let ready_tasks = {
                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;

                // 检查是否全部完成
                if manager.is_all_completed() {
                    info!(agent = %agent, "✅ 所有子任务已完成");
                    break;
                }

                // 获取所有依赖已满足的就绪任务
                manager
                    .get_ready_tasks()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if ready_tasks.is_empty() {
                warn!(agent = %agent, "⏳ 没有可执行的任务，等待依赖完成");
                self.messages.push(Message::user(
                    "没有可执行的任务。请检查任务状态并继续。".to_string(),
                ));
                self.think().await?;
                continue;
            }

            // 构建批量执行提示：一次性告知 LLM 所有就绪任务
            let task_list: Vec<String> = ready_tasks
                .iter()
                .map(|t| format!("  - [{}]: {}", t.id, t.description))
                .collect();

            let batch_ids: Vec<String> = ready_tasks.iter().map(|t| t.id.clone()).collect();

            info!(
                agent = %agent,
                tasks = ?batch_ids,
                "⚡ 开始执行 {} 个就绪任务",
                ready_tasks.len()
            );

            // 构建 SubAgent 分派提示（仅编排模式且启用 subagent 能力）
            let dispatch_hint =
                if self.config.role == AgentRole::Orchestrator
                    && self.config.enable_subagent
                {
                let subagent_names: Vec<String> = self
                    .subagents
                    .read()
                    .map(|agents| agents.keys().cloned().collect())
                    .unwrap_or_default();
                if !subagent_names.is_empty() {
                    format!(
                        "\n\n**重要**：你是编排者，请使用 agent_tool 将任务分派给合适的 SubAgent 执行，\
                        不要自己直接计算或猜测结果。\n\
                        可用的 SubAgent: {}\n\
                        完成后使用 update_task 标记完成，并将 SubAgent 返回的结果写入 result 字段。",
                        subagent_names.join(", ")
                    )
                } else {
                    "\n完成后使用 update_task 标记完成。".to_string()
                }
            } else {
                "\n完成后使用 update_task 标记完成。".to_string()
            };

            if ready_tasks.len() == 1 {
                self.messages.push(Message::user(format!(
                    "请执行任务 [{}]: {}{}",
                    ready_tasks[0].id, ready_tasks[0].description, dispatch_hint
                )));
            } else {
                self.messages.push(Message::user(format!(
                    "以下 {} 个任务的依赖已全部满足，请**同时**执行所有任务：\n{}{}",
                    ready_tasks.len(),
                    task_list.join("\n"),
                    dispatch_hint
                )));
            }

            // 多轮 think 直到本批任务全部完成
            for iteration in 0..self.config.max_iterations {
                debug!(
                    agent = %agent,
                    tasks = ?batch_ids,
                    iteration = iteration + 1,
                    "任务批次迭代"
                );
                let steps = self.think().await?;
                if let Some(answer) = self.process_steps(steps).await? {
                    return Ok(answer);
                }

                // 检查本批任务是否全部完成
                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
                let batch_done = batch_ids.iter().all(|id| {
                    manager.get_all_tasks().iter().any(|t| {
                        t.id == *id
                            && matches!(
                                t.status,
                                TaskStatus::Completed
                                    | TaskStatus::Cancelled
                                    | TaskStatus::Failed(_)
                            )
                    })
                });
                if batch_done {
                    info!(agent = %agent, tasks = ?batch_ids, "✅ 任务批次执行完成");
                    break;
                }
            }
        }

        // ── 第三阶段：总结结果 ──────────────────────────────
        info!(agent = %agent, phase = "summary", "📝 阶段3: 生成最终答案");

        // 收集所有任务的执行结果，便于 LLM 生成准确的最终答案
        let task_results_summary = {
            let manager = self
                .task_manager
                .read()
                .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
            manager
                .get_all_tasks()
                .iter()
                .map(|t| {
                    let result_str = t.result.as_deref().unwrap_or("无结果");
                    format!(
                        "  - [{}] {:?}: {} → {}",
                        t.id, t.status, t.description, result_str
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        self.messages.push(Message::user(format!(
            "所有任务已完成。以下是各任务的执行结果：\n{}\n\n\
            请根据以上结果，使用 final_answer 工具给出最终答案。\n\
            **注意**：不要再创建新任务或执行其他操作，直接给出最终答案。",
            task_results_summary
        )));

        for _ in 0..self.config.max_iterations {
            let steps = self.think().await?;
            if let Some(answer) = self.process_steps(steps).await? {
                info!(agent = %agent, "🏁 任务规划模式执行完毕");
                return Ok(answer);
            }
        }

        warn!(agent = %agent, max = self.config.max_iterations, "达到最大迭代次数");
        Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }

    pub async fn execute_loop(&mut self) -> Result<()> {
        let agent = self.config.agent_name.clone();
        info!(agent = %agent, "🔄 Agent 进入循环执行模式");

        loop {
            let steps = self.think().await?;

            for step in steps {
                match step {
                    StepType::Call {
                        tool_call_id,
                        function_name,
                        arguments,
                    } => {
                        info!("Calling tool: {}", function_name);
                        let result = self.execute_tool(&function_name, &arguments).await?;
                        let tool_msg = Message {
                            role: "tool".to_string(),
                            content: Option::from(result),
                            tool_call_id: Some(tool_call_id),
                            name: Option::from(function_name.clone()),
                            ..Default::default()
                        };
                        self.messages.push(tool_msg);
                    }
                    StepType::Thought(content) => {
                        debug!(agent = %agent, "🤔 思考: {}", content);
                        continue;
                    }
                    StepType::FinalAnswer(content) => {
                        info!(agent = %agent, "🏁 最终答案: {}", content);
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}

// 现在的 StepType 更贴合 OpenAI/Llama3 的 API 结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    // 对应 API 返回的 content 字段
    Thought(String),

    // 对应 API 返回的 tool_calls 字段
    // 注意：一次响应可能包含多个工具调用（并行调用），所以这里可能是一个列表
    Call {
        tool_call_id: String, // 重要：后续回传 observation 需要这个 ID
        function_name: String,
        arguments: Value,
    },

    // 对应 role: tool 的消息
    Observation {
        tool_call_id: String, // 必须匹配 Call 中的 ID
        output: String,
    },

    FinalAnswer(String),
}

/// ReAct 执行步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactStep {
    /// 步骤类型
    pub step_type: StepType,
    /// 步骤序号
    pub step_number: usize,
}

#[async_trait]
impl Agent for ReactAgent {
    fn name(&self) -> &str {
        &self.config.agent_name
    }

    fn model_name(&self) -> &str {
        &self.config.model_name
    }

    fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }

    fn system_prompt(&self) -> &str {
        &self.config.system_prompt
    }

    fn add_tool(&mut self, tool: Box<dyn Tool>) {
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

    fn add_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        if !self.config.enable_tool {
            warn!(
                agent = %self.config.agent_name,
                "⚠️ tool 能力已禁用，忽略批量工具注册"
            );
            return;
        }
        let allowed = &self.config.allowed_tools;
        if allowed.is_empty() {
            // 无限制，注册所有工具
            self.tool_manager.register_tools(tools);
        } else {
            // 只注册白名单中的工具
            for tool in tools {
                if allowed.contains(&tool.name().to_string()) {
                    self.tool_manager.register(tool);
                }
            }
        }
    }

    fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>) {
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
        // 工具照常注册，LLM 需要知道它的存在
        self.tool_manager.register(tool);
        // 同时标记为危险，执行时会触发 y/n 确认
        self.human_in_loop
            .write()
            .unwrap()
            .mark_need_approval(tool_name);
    }

    fn list_tools(&self) -> Vec<&str> {
        self.tool_manager.list_tools()
    }

    fn register_agent(&mut self, agent: Box<dyn Agent>) {
        if !self.config.enable_subagent {
            warn!(
                agent = %self.config.agent_name,
                subagent = %agent.name(),
                "⚠️ subagent 能力已禁用，忽略子 agent 注册"
            );
            return;
        }
        self.subagents
            .write()
            .unwrap()
            .insert(agent.name().to_string(), agent);
    }

    fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    fn list_subagent(&self) -> Vec<String> {
        self.subagents.read().unwrap().keys().cloned().collect()
    }

    async fn execute(&mut self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();

        // 重置消息历史，确保每次执行都是干净的 session
        self.reset_messages();

        info!(agent = %agent, "🧠 Agent 开始执行任务");
        debug!(
            agent = %agent,
            task = %task,
            tools = ?self.list_tools(),
            max_iterations = self.config.max_iterations,
            "执行详情"
        );

        let user_message = Message {
            role: "user".to_string(),
            content: Option::from(task.to_string()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        };
        self.messages.push(user_message);

        for iteration in 0..self.config.max_iterations {
            debug!(agent = %agent, iteration = iteration + 1, "--- 迭代 ---");

            // 调用 LLM 思考
            let steps = self.think().await?;

            // 如果没有返回任何步骤，说明LLM没有响应
            if steps.is_empty() {
                warn!(agent = %agent, "LLM 没有响应");
                return Err(ReactError::from(AgentError::NoResponse));
            }

            // 处理每个步骤
            let mut has_tool_call = false;

            for step in steps {
                match step {
                    StepType::Call {
                        tool_call_id,
                        function_name,
                        arguments,
                    } => {
                        has_tool_call = true;

                        let result = self.execute_tool(&function_name, &arguments).await?;

                        if function_name == "final_answer" {
                            info!(agent = %agent, "🏁 最终答案已生成");
                            return Ok(result);
                        }

                        self.messages.push(Message {
                            role: "tool".to_string(),
                            content: Some(result),
                            tool_calls: None,
                            name: Some(function_name),
                            tool_call_id: Some(tool_call_id),
                        });
                    }
                    StepType::Thought(content) => {
                        debug!(agent = %agent, "🤔 思考: {}", content);

                        // 如果没有工具调用且有内容，可能是最终答案
                        if !has_tool_call && !content.is_empty() {
                            info!(agent = %agent, "🏁 Agent 执行完毕（文本响应）");
                            return Ok(content);
                        }
                    }
                    _ => {}
                }
            }
        }

        warn!(agent = %agent, max = self.config.max_iterations, "达到最大迭代次数");
        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}
