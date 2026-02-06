pub(crate) mod answer;
pub mod math;
pub(crate) mod reasoning;
pub(crate) mod task_management;
pub(crate) mod human_in_loop;
pub mod weather;

use crate::error::{Result, ToolError};
use crate::llm::types::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// 是否成功
    pub success: bool,
    /// 输出内容
    pub output: String,
    /// 错误信息
    pub error: Option<String>,
}

impl ToolResult {
    /// 创建成功结果
    pub fn success(output: String) -> Self {
        Self {
            success: true,
            output,
            error: None,
        }
    }

    /// 创建失败结果
    pub fn error(error: String) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error),
        }
    }
}

pub type ToolParameters = HashMap<String, serde_json::Value>;

pub trait Tool: Send + Sync {
    // 工具名称
    fn name(&self) -> &str;

    // 工具描述
    fn description(&self) -> &str;

    // 工具参数，参数模式（JSON Schema）
    fn parameters(&self) -> serde_json::Value;

    // 执行工具
    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult>;
}

pub struct ToolManager {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolManager {
    pub(crate) fn to_openai_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition::from_tool(&**tool))
            .collect()
    }
}

impl ToolManager {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn list_tools(&self) -> Vec<&str> {
        self.tools.iter().map(|(name, _)| name.as_str()).collect()
    }

    pub fn get_tool(&self, tool_name: &str) -> Option<&dyn Tool> {
        self.tools.get(tool_name).map(|tool| &**tool)
    }

    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition::from_tool(&**tool))
            .collect()
    }

    pub fn execute_tool(&self, tool_name: &str, parameters: ToolParameters) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        tool.execute(parameters)
    }
}
