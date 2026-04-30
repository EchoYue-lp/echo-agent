/// Think tool (optional registration, not enabled by default).
///
/// # Background
///
/// The framework has switched to **(CoT via system prompt)**:
/// `ReactAgent` automatically appends CoT guidance to the system prompt when `enable_tool=true`,
/// letting the LLM output reasoning as text content, natively compatible with streaming (Token events).
///
/// This tool is **no longer registered by default**, only for manual opt-in in these scenarios:
/// - When structured tool-call records of each reasoning step are needed in the conversation history
/// - Non-streaming tasks where reasoning should be stored as tool_result in context
///
/// # Usage
///
/// ```rust,no_run
/// use echo_agent::tools::builtin::think::ThinkTool;
/// use echo_agent::prelude::{AgentConfig, ReactAgent};
///
/// let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant");
/// let mut agent = ReactAgent::new(config);
/// agent.add_tool(Box::new(ThinkTool));
/// ```
///
/// > **Note**: In the `execute_stream` streaming path, registering this tool causes model reasoning
/// > content to be written to tool call parameters instead of the `content` field, resulting in no
/// > `AgentEvent::Token` events during the reasoning phase.
/// > For streaming scenarios, rely on the CoT system prompt (default behavior); no need to register this tool.
use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;
use tracing::info;

/// Tool for internal reasoning and reflection.
pub struct ThinkTool;

impl Tool for ThinkTool {
    fn name(&self) -> &str {
        "think"
    }

    fn description(&self) -> &str {
        "Before taking action, use this tool to record your reasoning and analysis process. Parameter: reasoning - your analysis and plan for the problem."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reasoning": {
                    "type": "string",
                    "description": "Your reasoning process: analyze the problem, formulate a plan, reasoning steps"
                }
            },
            "required": ["reasoning"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let reasoning = parameters
                .get("reasoning")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("reasoning".to_string()))?;

            info!("Think: {}", reasoning);

            // Echo reasoning back to context so the next LLM call can see the full reasoning record
            Ok(ToolResult::success(reasoning.to_string()))
        })
    }
}
