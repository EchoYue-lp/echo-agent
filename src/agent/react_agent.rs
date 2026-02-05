use crate::agent::Agent;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::llm::chat;
use crate::llm::types::Message;
use crate::tools::answer::FinalAnswerTool;
use crate::tools::reasoning::ThinkTool;
use crate::tools::{Tool, ToolManager, ToolParameters};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::option::Option;

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
    pub fn new(agent_name: &str, model_name: &str, system_prompt: &str) -> Self {
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

        Self {
            config,
            messages,
            tool_manager,
            subagents: HashMap::new(),
            steps: Vec::new(),
        }
    }

    /// 执行工具
    fn execute_tool(&self, tool_name: &str, input: &Value) -> Result<String> {
        // 将 JSON Value 转换为 ToolParameters
        let params: ToolParameters = if let Value::Object(map) = input {
            map.clone().into_iter().map(|(k, v)| (k, v)).collect()
        } else {
            HashMap::new()
        };

        let result = self.tool_manager.execute_tool(tool_name, params)?;

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
            self.config.model_name.as_str(),
            self.messages.clone(),
            Some(0.7),
            Some(8192u32),
            Some(false),
            Some(tools), // 开启 Native Tool Calling
            None,
        )
        .await;

        let message = response?.choices[0].message.clone();

        if let Some(tool_calls) = &message.tool_calls {
            self.messages.push(message.clone());
            for call in tool_calls {
                // 将 Assistant 消息存入历史（必须存，否则 API 会报错断连）
                // self.messages.push(Message::from_assistant_tool(msg));

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
        Ok(res)
    }

    pub async fn execute_loop(&mut self) {
        loop {
            let steps = self.think().await.unwrap();

            for step in steps {
                match step {
                    StepType::Call {
                        tool_call_id,
                        function_name,
                        arguments,
                    } => {
                        println!("Calling tool: {}", function_name);
                        let result = self.execute_tool(&function_name, &arguments).unwrap();
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
                        println!("Thought: {}", content);
                        continue;
                    }
                    StepType::FinalAnswer(content) => {
                        println!("Final Answer: {}", content);
                        break;
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
            println!("\n🧠 ReAct Agent 开始执行任务");
            println!("📋 任务: {}", task);
            println!("🔧 可用工具: {:?}", self.list_tools());
            println!("🔄 最大迭代次数: {}\n", self.config.max_iterations);
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
                println!("--- 迭代 {} ---", iteration + 1);
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
                            println!("🚀 调用工具: {}", function_name);
                            println!("📥 参数: {}", arguments);
                        }

                        let result = self.execute_tool(&function_name, &arguments)?;

                        if self.config.verbose {
                            println!("📤 结果: {}", result);
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
                            println!("🤔 思考: {}", content);
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
