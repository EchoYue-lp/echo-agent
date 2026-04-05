use futures::future::BoxFuture;

use crate::agents::subagent::executor::SubagentExecutor;
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_core::agent::CancellationToken;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{debug, info, warn};

pub struct AgentDispatchTool {
    executor: Arc<SubagentExecutor>,
    parent_agent: String,
    cancel: CancellationToken,
}

impl AgentDispatchTool {
    pub fn new(
        executor: Arc<SubagentExecutor>,
        parent_agent: impl Into<String>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            executor,
            parent_agent: parent_agent.into(),
            cancel,
        }
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
        // NOTE: agent_names would require async, so we provide a generic description
        json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "子 Agent 名称"
                },
                "task": {
                    "type": "string",
                    "description": "要分配给子 Agent 的具体任务描述，应包含必要的上下文信息"
                },
                "mode": {
                    "type": "string",
                    "enum": ["sync", "fork", "teammate"],
                    "description": "执行模式：sync-同步等待（默认）, fork-继承上下文独立运行, teammate-并行协作"
                }
            },
            "required": ["agent_name", "task"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let executor = self.executor.clone();
        let parent_agent = self.parent_agent.clone();
        let cancel = self.cancel.clone();

        Box::pin(async move {
            let agent_name = parameters
                .get("agent_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("agent_name".to_string()))?;

            let task = parameters
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task".to_string()))?;

            let mode_override =
                parameters
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .and_then(|m| match m {
                        "sync" => Some(crate::agents::subagent::ExecutionMode::Sync),
                        "fork" => Some(crate::agents::subagent::ExecutionMode::Fork),
                        "teammate" => Some(crate::agents::subagent::ExecutionMode::Teammate),
                        _ => None,
                    });

            info!(
                target_agent = %agent_name,
                task = %task,
                mode = ?mode_override,
                "Dispatching task to subagent via SubagentExecutor"
            );

            let req = crate::agents::subagent::DispatchRequest {
                agent_name: agent_name.to_string(),
                task: task.to_string(),
                mode_override,
                cancel,
                parent_agent: parent_agent.clone(),
                parent_context: None, // Context inheritance handled by executor based on definition
                delegate_depth: 0,
            };

            match executor.dispatch(req).await {
                Ok(result) => {
                    info!(target_agent = %agent_name, "Subagent completed successfully");
                    debug!(target_agent = %agent_name, output = %result.output, "Subagent result");
                    Ok(ToolResult::success(result.output))
                }
                Err(e) => {
                    warn!(target_agent = %agent_name, error = %e, "Subagent execution failed");
                    Ok(ToolResult::error(format!(
                        "SubAgent '{}' execution failed: {}",
                        agent_name, e
                    )))
                }
            }
        })
    }
}
