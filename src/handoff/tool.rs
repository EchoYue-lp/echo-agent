//! Handoff built-in tool — allows LLM to actively trigger inter-Agent Handoff during the ReAct loop

use crate::handoff::{HandoffContext, HandoffManager, HandoffTarget};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Handoff tool: after registering in the Agent's tool list, LLM can trigger a Handoff by calling this tool
pub struct HandoffTool {
    manager: Arc<Mutex<HandoffManager>>,
    /// Current Agent name (used as source)
    source_agent: String,
}

impl HandoffTool {
    /// Create a new HandoffTool instance.
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
        "Transfer control of the current task to another Agent. Use this tool when you determine a task is better suited for a specialized Agent."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target_agent": {
                    "type": "string",
                    "description": "Name of the target Agent"
                },
                "message": {
                    "type": "string",
                    "description": "Task description to pass to the target Agent"
                },
                "transfer_history": {
                    "type": "boolean",
                    "description": "Whether to transfer conversation history to the target Agent",
                    "default": false
                },
                "metadata": {
                    "type": "object",
                    "description": "Additional metadata to pass (key-value pairs)",
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

            // Build HandoffTarget
            let mut target = HandoffTarget::new(&target_agent).with_message(&message);
            if transfer_history {
                target = target.with_history();
            }

            // Build HandoffContext
            let mut context = HandoffContext::new().with_source(&self.source_agent);

            // Add additional metadata
            if let Some(meta) = params.get("metadata")
                && let Some(obj) = meta.as_object()
            {
                for (k, v) in obj {
                    if let Some(val) = v.as_str() {
                        context = context.with_metadata(k, val);
                    }
                }
            }

            let manager = self.manager.lock().await;
            match manager.handoff(target, context).await {
                Ok(result) => Ok(ToolResult::success(format!(
                    "[Handoff complete] Agent '{}' returned:\n{}",
                    result.target_agent, result.output
                ))),
                Err(e) => Ok(ToolResult::error(format!("Handoff failed: {}", e))),
            }
        })
    }
}
