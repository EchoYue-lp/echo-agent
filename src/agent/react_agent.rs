use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::human_loop::HumanApprovalManager;
use crate::llm::chat;
use crate::llm::types::Message;
use crate::tasks::{TaskManager, TaskStatus};
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
use tracing::{debug, info};

pub struct ReactConfig {
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
}

impl ReactConfig {
    pub fn new(model_name: &str, agent_name: &str, system_prompt: &str) -> Self {
        Self {
            model_name: model_name.to_string(),
            system_prompt: system_prompt.to_string(),
            verbose: false,
            agent_name: agent_name.to_string(),
            max_iterations: 10,
        }
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
    config: ReactConfig,
    messages: Vec<Message>,
    tool_manager: ToolManager,
    subagents: HashMap<String, Box<dyn Agent>>,
    steps: Vec<ReactStep>,
    client: Arc<Client>,
    task_manager: Arc<RwLock<TaskManager>>,
    human_in_loop: Arc<RwLock<HumanApprovalManager>>,
}

impl ReactAgent {
    pub fn new(config: ReactConfig) -> Self {
        let system_message = Message {
            role: "system".to_string(),
            content: Option::from(config.system_prompt.clone()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        };
        let mut messages = Vec::new();
        messages.push(system_message);
        let mut tool_manager = ToolManager::new();
        tool_manager.register(Box::new(FinalAnswerTool));
        tool_manager.register(Box::new(ThinkTool));
        tool_manager.register(Box::new(HumanInLoop));
        let client = reqwest::Client::new();

        let task_manager = Arc::new(RwLock::new(TaskManager::default()));

        // 注册基础任务管理工具
        tool_manager.register(Box::new(PlanTool));
        tool_manager.register(Box::new(CreateTaskTool::new(task_manager.clone())));
        tool_manager.register(Box::new(ListTasksTool::new(task_manager.clone())));
        tool_manager.register(Box::new(UpdateTaskTool::new(task_manager.clone())));
        let human_in_loop = Arc::new(RwLock::new(HumanApprovalManager::new()));

        // 注册新增的高级任务管理工具
        tool_manager.register(Box::new(VisualizeDependenciesTool::new(
            task_manager.clone(),
        )));
        tool_manager.register(Box::new(GetExecutionOrderTool::new(task_manager.clone())));

        Self {
            config,
            messages,
            tool_manager,
            subagents: HashMap::new(),
            steps: Vec::new(),
            client: Arc::new(client),
            task_manager,
            human_in_loop,
        }
    }

    /// 执行工具
    async fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        // 将 JSON Value 转换为 ToolParameters
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().map(|(k, v)| (k, v)).collect()
        } else {
            HashMap::new()
        };

        let needs_approval = {
            let approval_manager = self.human_in_loop.read().unwrap();
            approval_manager.needs_approval(tool_name)
        };

        if needs_approval {
            info!("\n⚠️  即将执行危险操作: {}", tool_name);
            info!("   参数: {}", input);
            info!("   是否批准该工具执行？(y/n): ");

            let mut input = String::new();
            std::io::stdin()
                .read_line(&mut input)
                .expect("读取输入失败");

            if input.trim() != "y" && input.trim() != "Y" {
                // 拒绝 → 直接返回文字结果给 LLM，让它知道被拒绝了
                return Ok(format!("用户已拒绝执行工具 {}", tool_name));
            }
            // 批准 → 继续往下正常执行
            info!("用户已批准执行工具 {}", tool_name);
        }

        let result = self.tool_manager.execute_tool(tool_name, params).await?;

        if result.success {
            Ok(result.output)
        } else {
            Err(ReactError::from(ToolError::ExecutionFailed {
                tool: tool_name.to_string(),
                message: "工具执行失败".to_string(),
            }))
        }
    }

    pub(crate) async fn think(&mut self) -> Result<Vec<StepType>> {
        let mut res = Vec::new();

        // 第一步，构建 tools 定义
        let tools = self.tool_manager.to_openai_tools();

        let response = chat(
            self.client.clone(),
            self.config.model_name.as_str(),
            self.messages.clone(),
            Some(0.7),
            Some(8192u32),
            Some(false),
            Some(tools), // 开启 Native Tool Calling
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
            res.push(StepType::Thought(content.to_string()));
        }
        debug!("think result: {:?}", res);
        Ok(res)
    }

    /// 处理一轮思考产生的所有步骤（工具调用并行执行），返回 final_answer 结果（如有）
    async fn process_steps(&mut self, steps: Vec<StepType>) -> Result<Option<String>> {
        // 分离工具调用和其他步骤
        let mut tool_calls = Vec::new();

        for step in steps {
            match step {
                StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } => {
                    if self.config.verbose {
                        info!("🚀 准备调用工具: {} , 参数: {}", function_name, arguments);
                    }
                    tool_calls.push((tool_call_id, function_name, arguments));
                }
                StepType::Thought(content) => {
                    if self.config.verbose {
                        info!("🤔 思考: {}", content);
                    }
                }
                _ => {}
            }
        }

        if tool_calls.is_empty() {
            return Ok(None);
        }

        if self.config.verbose && tool_calls.len() > 1 {
            info!("⚡ 并行执行 {} 个工具调用", tool_calls.len());
        }

        // 并行执行所有工具调用
        let futures: Vec<_> = tool_calls
            .iter()
            .map(|(_, name, args)| self.execute_tool(name, args))
            .collect();
        let results = join_all(futures).await;

        // 收集结果并推入消息
        for ((tool_call_id, function_name, _), result) in tool_calls.into_iter().zip(results) {
            let result = result?;

            if self.config.verbose {
                info!("🚀 工具: {} 📤 结果: {}", function_name, result);
            }

            if function_name == "final_answer" {
                return Ok(Some(result));
            }

            self.messages
                .push(Message::tool_result(tool_call_id, function_name, result));
        }

        Ok(None)
    }

    pub async fn execute_with_planning(&mut self, task: &str) -> Result<String> {
        if self.config.verbose {
            info!("🎯 启动任务规划模式");
        }

        // 第一阶段：让 Agent 制定计划
        let planning_prompt = format!(
            "{}\n\n\
            请先使用 plan 工具分析问题，然后用 create_task 创建子任务列表。\n\n\
            **重要：任务拆分规则**\n\
            - 将问题拆分为尽可能细粒度的子任务，每个子任务只做一件事\n\
            - 互相独立的子任务不要设置依赖关系，让它们可以并行执行\n\
            - 只有当一个任务真正需要另一个任务的结果时，才设置 dependencies\n\
            - 尽量构建宽而浅的 DAG（有向无环图），而非线性链\n\
           请一次性创建所有子任务。",
            task
        );

        self.messages.push(Message::user(planning_prompt));

        // 执行直到创建完任务
        for _ in 0..3 {
            // 最多3轮规划
            let steps = self.think().await?;

            for step in steps {
                if let StepType::Call {
                    tool_call_id,
                    function_name,
                    arguments,
                } = step
                {
                    let result = self.execute_tool(&function_name, &arguments).await?;
                    self.messages
                        .push(Message::tool_result(tool_call_id, function_name, result));
                }
            }

            // 检查是否已创建任务
            let manager = self
                .task_manager
                .read()
                .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;
            if !manager.get_all_tasks().is_empty() {
                break;
            }
        }

        // 第二阶段：并行执行就绪任务
        loop {
            let ready_tasks = {
                let manager = self
                    .task_manager
                    .read()
                    .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))?;

                // 检查是否全部完成
                if manager.is_all_completed() {
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
                // 没有可执行的任务，可能有依赖未满足或需要等待
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

            if self.config.verbose {
                info!(
                    "⚡ 并行执行 {} 个就绪任务: {:?}",
                    ready_tasks.len(),
                    batch_ids
                );
            }

            if ready_tasks.len() == 1 {
                self.messages.push(Message::user(format!(
                    "请执行任务 [{}]: {}。完成后使用 update_task 标记完成。",
                    ready_tasks[0].id, ready_tasks[0].description
                )));
            } else {
                self.messages.push(Message::user(format!(
                    "以下 {} 个任务的依赖已全部满足，请**同时**执行所有任务。\n\
                    完成后分别使用 update_task 标记完成：\n{}",
                    ready_tasks.len(),
                    task_list.join("\n")
                )));
            }

            // 多轮 think 直到本批任务全部完成
            for _ in 0..self.config.max_iterations {
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
                    break;
                }
            }
        }

        // 第三阶段：总结结果（直接进入 think 循环，不再调用 self.execute 避免重复推入 user message）
        self.messages.push(Message::user(
            "所有任务已完成，请使用 final_answer 给出最终答案。".to_string(),
        ));

        for _ in 0..self.config.max_iterations {
            let steps = self.think().await?;
            if let Some(answer) = self.process_steps(steps).await? {
                return Ok(answer);
            }
        }

        Err(ReactError::Agent(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }

    pub async fn execute_loop(&mut self) -> Result<()> {
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
                        info!("Thought: {}", content);
                        continue;
                    }
                    StepType::FinalAnswer(content) => {
                        info!("Final Answer: {}", content);
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
        self.tool_manager.register(tool)
    }

    fn add_need_appeal_tool(&mut self, tool: Box<dyn Tool>) {
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
        self.subagents.insert(agent.name().to_string(), agent);
    }

    fn list_subagent(&self) -> Vec<&str> {
        self.subagents.keys().map(|s| s.as_str()).collect()
    }

    async fn execute(&mut self, task: &str) -> Result<String> {
        if self.config.verbose {
            info!("🧠 ReAct Agent 开始执行任务");
            info!("📋 任务: {}", task);
            info!("🔧 可用工具: {:?}", self.list_tools());
            info!("🔄 最大迭代次数: {}", self.config.max_iterations);
        }
        let user_message = Message {
            role: "user".to_string(),
            content: Option::from(task.to_string()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        };
        self.messages.push(user_message);

        for iteration in 0..self.config.max_iterations {
            if self.config.verbose {
                info!("--- 迭代 {} ---", iteration + 1);
            }

            // 调用 LLM 思考
            let steps = self.think().await?;

            // 如果没有返回任何步骤，说明LLM没有响应
            if steps.is_empty() {
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
                        if self.config.verbose {
                            info!("🚀 调用工具: {} , 参数: {}", function_name, arguments);
                        }

                        let result = self.execute_tool(&function_name, &arguments).await?;

                        if self.config.verbose {
                            info!("🚀 调用工具: {} ,📤 结果: {}", function_name, result);
                        }

                        if function_name == "final_answer" {
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
                        if self.config.verbose {
                            info!("🤔 思考: {}", content);
                        }

                        // 如果没有工具调用且有内容，可能是最终答案
                        if !has_tool_call && !content.is_empty() {
                            return Ok(content);
                        }
                    }
                    _ => {}
                }
            }
        }

        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}
