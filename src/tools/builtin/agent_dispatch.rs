use futures::future::BoxFuture;

use crate::agent::SubAgentMap;
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

pub struct AgentDispatchTool {
    subagents: SubAgentMap,
}

impl AgentDispatchTool {
    pub fn new(subagents: SubAgentMap) -> Self {
        Self { subagents }
    }
}

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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
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

            // 只克隆 Arc 指针（不持有读锁跨越 await），并发同名调用会在 mutex 处自动排队
            let agent_arc = {
                let agents = self
                    .subagents
                    .read()
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "agent_tool".to_string(),
                        message: format!("Lock poisoned: {}", e),
                    })?;
                agents
                    .get(agent_name)
                    .ok_or_else(|| ToolError::ExecutionFailed {
                        tool: "agent_tool".to_string(),
                        message: format!("SubAgent '{}' not found", agent_name),
                    })?
                    .clone()
            };

            info!(
                target_agent = %agent_name,
                task = %task,
                "📡 分派任务到子 Agent"
            );

            // 对同一 agent 的并发调用会在此处排队，不会丢失 agent
            let mut agent = agent_arc.lock().await;
            let result = agent
                .execute(task)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "agent_tool".to_string(),
                    message: format!("SubAgent execution failed: {}", e),
                });

            match &result {
                Ok(answer) => {
                    info!(target_agent = %agent_name, "✅ 子 Agent 执行完成");
                    debug!(target_agent = %agent_name, output = %answer, "子 Agent 返回详情");
                }
                Err(e) => {
                    warn!(target_agent = %agent_name, error = %e, "💥 子 Agent 执行失败");
                }
            }

            Ok(ToolResult::success(result?))
        })
    }
}
