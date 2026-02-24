use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;
use tracing::info;

pub struct ThinkTool;

#[async_trait::async_trait]
impl Tool for ThinkTool {
    fn name(&self) -> &str {
        "think"
    }

    fn description(&self) -> &str {
        "在采取行动前，先使用此工具记录你的思考和推理过程。
        参数：reasoning - 你对问题的分析和计划"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reasoning": {
                    "type": "string",
                    "description": "你的思考过程：分析问题、制定计划、推理步骤"
                }
            },
            "required": ["reasoning"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let reasoning = parameters
            .get("reasoning")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("reasoning".to_string()))?;

        info!("Thinking: {}", reasoning);

        // 思考工具总是成功，只是记录
        Ok(ToolResult::success(format!("✓ 已记录思考过程")))
    }
}
