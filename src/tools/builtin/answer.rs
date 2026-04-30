use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};

pub struct FinalAnswerTool;

impl Tool for FinalAnswerTool {
    fn name(&self) -> &str {
        "final_answer"
    }

    fn description(&self) -> &str {
        "Call this tool to return the final answer when you have gathered enough information to respond to the user"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "answer": {
                    "type": "string",
                    "description": "The final answer"
                }
            },
            "required": ["answer"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let answer = parameters
                .get("answer")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidParameter {
                    name: "answer".to_string(),
                    message: "answer is required".to_string(),
                })?;

            Ok(ToolResult::success(answer.to_string()))
        })
    }
}
