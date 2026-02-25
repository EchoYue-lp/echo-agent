use crate::agent::Agent;
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

pub struct AgentDispatchTool {
    subagents: Arc<RwLock<HashMap<String, Box<dyn Agent>>>>,
}

impl AgentDispatchTool {
    pub fn new(subagents: Arc<RwLock<HashMap<String, Box<dyn Agent>>>>) -> Self {
        Self { subagents }
    }
}

#[async_trait::async_trait]
impl Tool for AgentDispatchTool {
    fn name(&self) -> &str {
        "agent_tool"
    }

    fn description(&self) -> &str {
        "将任务分派给专用 SubAgent 执行。作为编排者，应优先使用此工具将计算、数据获取等任务委托给专业的 SubAgent，而不是自己直接回答。"
    }

    fn parameters(&self) -> Value {
        let agent_names: Vec<String> = self
            .subagents
            .read()
            .map(|agents| agents.keys().cloned().collect())
            .unwrap_or_default();
        let agent_desc = if agent_names.is_empty() {
            "子 Agent 名称".to_string()
        } else {
            format!("子 Agent 名称，可用: {}", agent_names.join(", "))
        };
        json!({
            "type": "object",
            "properties": {
                "agent_name": { "type": "string", "description": agent_desc },
                "task": { "type": "string", "description": "要分配给子 Agent 的具体任务描述，应包含必要的上下文信息" }
            },
            "required": ["agent_name", "task"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let agent_name = parameters
            .get("agent_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParameter {
                name: "agent_name".to_string(),
                message: "agent_name is required".to_string(),
            })?;

        let task = parameters
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParameter {
                name: "task".to_string(),
                message: "task is required".to_string(),
            })?;

        // 从 map 中取出 agent，立即释放写锁，避免跨 await 持有锁
        let mut agent = {
            let mut agents = self
                .subagents
                .write()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "agent_tool".to_string(),
                    message: format!("Lock poisoned: {}", e),
                })?;
            agents
                .remove(agent_name)
                .ok_or_else(|| ToolError::ExecutionFailed {
                    tool: "agent_tool".to_string(),
                    message: format!("SubAgent '{}' not found", agent_name),
                })?
        }; // 写锁在此释放

        info!(
            target_agent = %agent_name,
            task = %task,
            "📡 分派任务到子 Agent"
        );

        // 在锁外执行 agent（安全地跨 await）
        let result = agent
            .execute(task)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "agent_tool".to_string(),
                message: format!("SubAgent execution failed: {}", e),
            });

        // 记录子 agent 执行结果
        match &result {
            Ok(answer) => {
                info!(target_agent = %agent_name, "✅ 子 Agent 执行完成");
                debug!(target_agent = %agent_name, output = %answer, "子 Agent 返回详情");
            }
            Err(e) => {
                warn!(target_agent = %agent_name, error = %e, "💥 子 Agent 执行失败");
            }
        }

        // 无论成功失败，都将 agent 放回 map
        {
            let mut agents = self
                .subagents
                .write()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "agent_tool".to_string(),
                    message: format!("Lock poisoned: {}", e),
                })?;
            agents.insert(agent_name.to_string(), agent);
        }

        Ok(ToolResult::success(result?))
    }
}
