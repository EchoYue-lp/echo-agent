//! Handoff 内置工具 — 让 LLM 在 ReAct 循环中主动触发 Agent 间 Handoff

use crate::handoff::{HandoffContext, HandoffManager, HandoffTarget};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Handoff 工具：注册到 Agent 的工具列表后，LLM 可通过调用此工具触发 Handoff
pub struct HandoffTool {
    manager: Arc<Mutex<HandoffManager>>,
    /// 当前 Agent 名称（作为 source）
    source_agent: String,
}

impl HandoffTool {
    pub fn new(manager: Arc<Mutex<HandoffManager>>, source_agent: impl Into<String>) -> Self {
        Self {
            manager,
            source_agent: source_agent.into(),
        }
    }
}

impl Tool for HandoffTool {
    fn name(&self) -> &str {
        "handoff"
    }

    fn description(&self) -> &str {
        "将当前任务的控制权转移给另一个 Agent。当你判断某个任务更适合由其他专业 Agent 处理时，使用此工具进行 Handoff。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target_agent": {
                    "type": "string",
                    "description": "目标 Agent 的名称"
                },
                "message": {
                    "type": "string",
                    "description": "要传递给目标 Agent 的任务描述"
                },
                "transfer_history": {
                    "type": "boolean",
                    "description": "是否传递对话历史给目标 Agent",
                    "default": false
                },
                "metadata": {
                    "type": "object",
                    "description": "要传递的额外元数据（键值对）",
                    "additionalProperties": { "type": "string" }
                }
            },
            "required": ["target_agent", "message"]
        })
    }

    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let target_agent = params
                .get("target_agent")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::error::ToolError::MissingParameter("target_agent".to_string())
                })?
                .to_string();

            let message = params
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::error::ToolError::MissingParameter("message".to_string()))?
                .to_string();

            let transfer_history = params
                .get("transfer_history")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // 构建 HandoffTarget
            let mut target = HandoffTarget::new(&target_agent).with_message(&message);
            if transfer_history {
                target = target.with_history();
            }

            // 构建 HandoffContext
            let mut context = HandoffContext::new().with_source(&self.source_agent);

            // 添加额外元数据
            if let Some(meta) = params.get("metadata") {
                if let Some(obj) = meta.as_object() {
                    for (k, v) in obj {
                        if let Some(val) = v.as_str() {
                            context = context.with_metadata(k, val);
                        }
                    }
                }
            }

            let manager = self.manager.lock().await;
            match manager.handoff(target, context).await {
                Ok(result) => Ok(ToolResult::success(format!(
                    "[Handoff 完成] Agent '{}' 返回:\n{}",
                    result.target_agent, result.output
                ))),
                Err(e) => Ok(ToolResult::error(format!("Handoff 失败: {}", e))),
            }
        })
    }
}
