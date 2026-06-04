use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use crate::error::ToolError;
use crate::human_loop::{HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use crate::tools::{Tool, ToolParameters, ToolResult};

/// LLM-triggered human-in-the-loop tool.
///
/// Called when the LLM is uncertain about user intent, needs additional
/// information, or requires user confirmation. Uses the injected
/// [`HumanLoopProvider`] to asynchronously request user input, supporting
/// multiple channels such as CLI, HTTP Webhook, WebSocket, etc.
pub struct HumanInLoop {
    provider: Arc<dyn HumanLoopProvider>,
}

impl HumanInLoop {
    pub fn new(provider: Arc<dyn HumanLoopProvider>) -> Self {
        Self { provider }
    }
}

impl Tool for HumanInLoop {
    fn name(&self) -> &str {
        "human_in_loop"
    }

    fn description(&self) -> &str {
        "Use this tool when you are uncertain about user intent, need additional information, or require user confirmation."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reasoning": {
                    "type": "string",
                    "description": "Trigger reason: why is human intervention needed? Provided by LLM when intent is unclear; user confirmation when tool has risks."
                },
                "tool": {
                    "type": "string",
                    "description": "Name of the tool that triggered this (optional)"
                },
                "approval_type": {
                    "type": "string",
                    "description": "Trigger type: 'LLM' when the LLM triggers it, 'tool' when a tool triggers it"
                }
            },
            "required": ["reasoning", "approval_type"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let approval_type = parameters
                .get("approval_type")
                .and_then(|t| t.as_str())
                .ok_or_else(|| ToolError::MissingParameter("approval_type".to_string()))?;

            let reasoning = parameters
                .get("reasoning")
                .and_then(|t| t.as_str())
                .ok_or_else(|| ToolError::MissingParameter("reasoning".to_string()))?;

            let tool = parameters
                .get("tool")
                .and_then(|t| t.as_str())
                .unwrap_or("None");

            let prompt = format!(
                "I need your help.\nTrigger type: {approval_type}\nTrigger reason: {reasoning}\nTrigger tool: {tool}\n\nPlease reply directly with your opinion or confirmation:"
            );

            let req = HumanLoopRequest::input(prompt);
            let result_text = match self.provider.request(req).await? {
                HumanLoopResponse::Text(text) => text,
                HumanLoopResponse::Approved => "User confirmed".to_string(),
                HumanLoopResponse::ApprovedWithScope { scope } => {
                    format!("User confirmed (scope: {:?})", scope)
                }
                HumanLoopResponse::ModifiedArgs { args, scope } => {
                    format!(
                        "User confirmed after modifying arguments (args: {}, scope: {:?})",
                        args, scope
                    )
                }
                HumanLoopResponse::Rejected { reason } => {
                    format!(
                        "User rejected{}",
                        reason.map(|r| format!(", reason: {r}")).unwrap_or_default()
                    )
                }
                HumanLoopResponse::Timeout => "Timed out waiting for user input".to_string(),
                HumanLoopResponse::Deferred => "User deferred the decision".to_string(),
                HumanLoopResponse::Selection {
                    selection,
                    instructions,
                } => {
                    format!(
                        "User selected: {}{}",
                        selection,
                        instructions
                            .map(|i| format!(" (instructions: {})", i))
                            .unwrap_or_default()
                    )
                }
            };

            Ok(ToolResult::success(format!(
                "User response (trigger reason: {reasoning}, tool: {tool}): {result_text}"
            )))
        })
    }
}
