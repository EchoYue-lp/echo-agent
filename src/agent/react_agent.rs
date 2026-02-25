use crate::agent::Agent;
pub use crate::agent::config::{AgentConfig, AgentRole};
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::human_loop::HumanApprovalManager;
use crate::llm::chat;
use crate::llm::types::Message;
use crate::tasks::TaskManager;
use crate::tools::builtin::agent_dispatch::AgentDispatchTool;
use crate::tools::builtin::answer::FinalAnswerTool;
use crate::tools::builtin::human_in_loop::HumanInLoop;
use crate::tools::builtin::plan::PlanTool;
use crate::tools::builtin::task::{
    CreateTaskTool, GetExecutionOrderTool, ListTasksTool, UpdateTaskTool, VisualizeDependenciesTool,
};
use crate::tools::builtin::think::ThinkTool;
use crate::tools::{Tool, ToolManager, ToolParameters};
use async_trait::async_trait;
use futures::future::join_all;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

// 内置工具名常量，统一定义避免魔法字符串散落各处
pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";
pub(crate) const TOOL_CREATE_TASK: &str = "create_task";
pub(crate) const TOOL_PLAN: &str = "plan";
pub(crate) const TOOL_UPDATE_TASK: &str = "update_task";

pub struct ReactAgent {
    pub(crate) config: AgentConfig,
    pub(crate) messages: Vec<Message>,
    tool_manager: ToolManager,
    pub(crate) subagents: Arc<RwLock<HashMap<String, Box<dyn Agent>>>>,
    client: Arc<Client>,
    pub(crate) task_manager: Arc<RwLock<TaskManager>>,
    human_in_loop: Arc<RwLock<HumanApprovalManager>>,
}

impl ReactAgent {
    pub(crate) fn has_planning_tools(&self) -> bool {
        self.config.enable_task
            && [TOOL_PLAN, TOOL_CREATE_TASK, TOOL_UPDATE_TASK]
                .iter()
                .all(|name| self.tool_manager.get_tool(name).is_some())
    }

    pub fn new(config: AgentConfig) -> Self {
        let messages = vec![Message::system(config.system_prompt.clone())];
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

        Self {
            config,
            messages,
            tool_manager,
            subagents,
            client: Arc::new(client),
            task_manager,
            human_in_loop,
        }
    }

    /// 重置消息历史，仅保留 system prompt，确保每次执行互不干扰
    pub(crate) fn reset_messages(&mut self) {
        self.messages = vec![Message::system(self.config.system_prompt.clone())];
    }

    /// 执行工具，保留工具返回的真实错误信息
    pub(crate) async fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        let agent = &self.config.agent_name;
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().collect()
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
            let error_msg = result.error.unwrap_or_else(|| "工具执行失败".to_string());
            warn!(agent = %agent, tool = %tool_name, error = %error_msg, "💥 工具执行失败");
            Err(ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: error_msg,
            }))
        }
    }

    /// 调用 LLM 推理，返回本轮的步骤列表
    pub(crate) async fn think(&mut self) -> Result<Vec<StepType>> {
        let agent = self.config.agent_name.clone();
        let mut res = Vec::new();

        debug!(agent = %agent, model = %self.config.model_name, "🧠 LLM 思考中...");

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
            self.messages.push(message.clone());
            debug!(agent = %agent, "🧠 LLM 返回文本响应");
            res.push(StepType::Thought(content.to_string()));
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
            info!(
                agent = %agent,
                tools = ?tool_names,
                "⚡ 并行执行 {} 个工具调用",
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
                let result = self.execute_tool(&function_name, &arguments).await?;
                if function_name == TOOL_FINAL_ANSWER {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    return Ok(Some(result));
                }
                self.messages
                    .push(Message::tool_result(tool_call_id, function_name, result));
            }
        } else {
            let futures: Vec<_> = tool_calls
                .iter()
                .map(|(_, name, args)| self.execute_tool(name, args))
                .collect();
            let results = join_all(futures).await;

            for ((tool_call_id, function_name, _), result) in tool_calls.into_iter().zip(results) {
                let result = result?;
                if function_name == TOOL_FINAL_ANSWER {
                    info!(agent = %agent, "🏁 最终答案已生成");
                    return Ok(Some(result));
                }
                self.messages
                    .push(Message::tool_result(tool_call_id, function_name, result));
            }
        }

        Ok(None)
    }

    /// 直接执行模式（无规划），复用 `process_steps` 以获得并行工具调用能力
    pub(crate) async fn run_direct(&mut self, task: &str) -> Result<String> {
        let agent = self.config.agent_name.clone();
        self.reset_messages();

        info!(agent = %agent, "🧠 Agent 开始执行任务");
        debug!(
            agent = %agent,
            task = %task,
            tools = ?self.tool_manager.list_tools(),
            max_iterations = self.config.max_iterations,
            "执行详情"
        );

        self.messages.push(Message::user(task.to_string()));

        for iteration in 0..self.config.max_iterations {
            debug!(agent = %agent, iteration = iteration + 1, "--- 迭代 ---");

            let steps = self.think().await?;
            if steps.is_empty() {
                warn!(agent = %agent, "LLM 没有响应");
                return Err(ReactError::from(AgentError::NoResponse));
            }

            if let Some(answer) = self.process_steps(steps).await? {
                info!(agent = %agent, "🏁 Agent 执行完毕");
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
            .unwrap()
            .mark_need_approval(tool_name);
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
        self.subagents
            .write()
            .unwrap()
            .insert(agent.name().to_string(), agent);
    }

    pub fn register_agents(&mut self, agents: Vec<Box<dyn Agent>>) {
        for agent in agents {
            self.register_agent(agent)
        }
    }

    pub fn set_model(&mut self, model_name: &str) {
        self.config.model_name = model_name.to_string();
    }
}
