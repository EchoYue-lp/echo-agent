use crate::agent::Agent;
use crate::error::ParseError::InvalidAction;
use crate::error::{AgentError, ReactError, Result, ToolError};
use crate::llm::chat;
use crate::llm::types::{Message, ToolDefinition};
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

        Self {
            config,
            messages,
            tool_manager: ToolManager::new(),
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
    pub(crate) async fn think(&mut self) -> Result<String> {
        let prompt = format!(
            "{}

        请你一步一步的认真仔细思考，然后决定如何进行下一步工作行动。
        你可以：
        1、认真的思考问题（请以 'Thought:' 开头）
        2、使用工具（格式：'Action: tool_name {{\"param\": \"value\"}}'）
        3、给出最后的运算结果(请以 'Final Answer:' 开头)
",
            self.messages.last().unwrap().content.clone().unwrap(),
        );

        if self.config.verbose {
            println!("======== Think LLM prompt: {} ========", prompt);
        }

        self.messages.push(Message {
            role: "user".to_string(),
            content: Some(prompt),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        });

        let tools = self
            .tool_manager
            .list_tools()
            .iter()
            .map(|tool_name| {
                let tool = self.tool_manager.get_tool(tool_name).unwrap();
                ToolDefinition::from_tool(tool)
            })
            .collect();

        let response = chat(
            self.config.model_name.as_str(),
            self.messages[..self.messages.len()].to_vec(),
            Some(0.7),
            Some(8192u32),
            Some(false),
            Some(tools),
            None,
        )
        .await;

        let content = response.unwrap().content;
        let content = content.unwrap();

        if self.config.verbose {
            println!("=======> Think LLM 响应: {} <=======", content);
        }

        self.messages.push(Message {
            role: "assistant".to_string(),
            content: Some(content.clone()),
            tool_calls: None,
            name: None,
            tool_call_id: None,
        });

        Ok(content)
    }

    pub(crate) fn parse_response(&self, response: &str, step_num: usize) -> Result<ReactStep> {
        let response = response.trim();
        if response.starts_with("Thought:") || response.starts_with("思考:") {
            let thought = response
                .strip_prefix("Thought:")
                .or_else(|| response.strip_prefix("思考:"))
                .unwrap_or(response)
                .trim()
                .to_string();
            Ok(ReactStep {
                step_type: StepType::Thought(thought),
                step_number: step_num,
            })
        } else if response.starts_with("Action:") || response.starts_with("执行:") {
            // 解析 Action: tool_name {"param": "value"}
            let action_str = response
                .strip_prefix("Action:")
                .or_else(|| response.strip_prefix("执行:"))
                .unwrap_or(response)
                .trim();
            // 分割字符串，最多返回指定的元素
            let parts: Vec<&str> = action_str.splitn(2, ' ').collect();

            if parts.len() == 2 {
                let tool = parts[0].to_string();
                let input: Value = serde_json::from_str(parts[1])
                    .unwrap_or_else(|_| Value::String(parts[1].to_string()));
                Ok(ReactStep {
                    step_type: StepType::Action { tool, input },
                    step_number: step_num,
                })
            } else {
                return Err(ReactError::Parse(InvalidAction(
                    "Invalid action".to_string(),
                )));
            }
        } else if response.starts_with("Final Answer:") || response.starts_with("最终结果:") {
            let final_answer = response
                .strip_prefix("Final Answer:")
                .or_else(|| response.strip_prefix("最终结果:"))
                .unwrap_or(response)
                .trim()
                .to_string();
            Ok(ReactStep {
                step_type: StepType::FinalAnswer(final_answer),
                step_number: step_num,
            })
        } else {
            Ok(ReactStep {
                step_type: StepType::Thought("".to_string()),
                step_number: step_num,
            })
        }
    }

    /// 获取所有工具的定义（用于 LLM）
    pub fn get_tool_definitions(&self) -> Result<Vec<Value>> {
        let result = self
            .tool_manager
            .list_tools()
            .iter()
            .map(|tool| {
                let tool = self.tool_manager.get_tool(tool).unwrap();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters()
                    }
                })
            })
            .collect();
        Ok(result)
    }
}

/// ReAct (Reasoning + Acting) 是一种将推理和行动相结合的AI Agent架构模式。
/// ReAct通过以下三个核心阶段形成闭环：
///
///  观察 (Observe): 感知当前环境状态和问题
///  思考 (Think): 基于观察进行推理和策略制定
///  行动 (Act): 执行具体的工具调用和决策
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    // 对应 API 返回的 content 字段
    Thought(String),
    /// 行动（调用工具）
    Action {
        tool: String,
        input: Value,
    },
    /// 观察（工具执行结果）
    Observation(String),
    /// 最终答案
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

            // 1. 思考（Reasoning）：调用 LLM 获取下一步行动
            let response: String = self.think().await?;

            if self.config.verbose {
                println!("---------------->: {}", response);
            }

            // 2. 解析响应，判断是思考、行动还是最终答案
            let step: ReactStep = self.parse_response(&response, iteration)?;

            // 3.执行步骤
            match &step.step_type {
                StepType::Thought(thought) => {
                    // 思考,LLM已经执行了 思考，因此这一步不需要再进行其他操作，仅作记录
                    if self.config.verbose {
                        println!("🤔 思考: {}", thought);
                    }
                }
                StepType::Action { tool, input } => {
                    if self.config.verbose {
                        println!("🚀 执行工具: {}", tool);
                        println!("🛠️ 工具输入: {}", input);
                    }

                    let tool_result = self.execute_tool(tool, input);

                    self.messages.push(Message {
                        role: "assistant".to_string(),
                        content: Option::from(format!("Observation: {:?}", tool_result)),
                        tool_calls: None,
                        name: Some(tool.clone()),
                        tool_call_id: None,
                    });
                }
                StepType::Observation(result) => {
                    if self.config.verbose {
                        println!("🤖 工具执行结果: {}", result);
                    }
                }
                StepType::FinalAnswer(answer) => {
                    return Ok(answer.clone());
                }
            }
            self.steps.push(step);
        }

        Err(ReactError::from(AgentError::MaxIterationsExceeded(
            self.config.max_iterations,
        )))
    }
}
