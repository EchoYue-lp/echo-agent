use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};

pub struct FinalAnswerTool;

impl Tool for FinalAnswerTool {
    fn name(&self) -> &str {
        "final_answer"
    }

    fn description(&self) -> &str {
        "当你已经收集到足够信息可以回答用户问题时，调用此工具返回最终答案"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "answer": {
                    "type": "string",
                    "description": "最终答案"
                }
            },
            "required": ["answer"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let answer = parameters
            .get("answer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParameter {
                name: "answer".to_string(),
                message: "answer is required".to_string(),
            })?;

        Ok(ToolResult::success(answer.to_string()))
    }
}
