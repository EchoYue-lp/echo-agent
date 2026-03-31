use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use tracing::{debug, info};

pub struct PlanTool;

impl Tool for PlanTool {
    fn name(&self) -> &str {
        "plan"
    }

    fn description(&self) -> &str {
        "分析复杂问题并制定详细的执行计划。将大任务拆解为多个有序的子任务。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "analysis": {
                    "type": "string",
                    "description": "对问题的深入分析：难点、需要的信息、可能的方法"
                },
                "strategy": {
                    "type": "string",
                    "description": "解决策略：说明如何一步步解决这个问题"
                }
            },
            "required": ["analysis", "strategy"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let analysis = parameters
                .get("analysis")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("analysis".to_string()))?;

            let strategy = parameters
                .get("strategy")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("strategy".to_string()))?;

            let plan = format!(
                "📋 计划已制定\n\n分析:\n{}\n\n策略:\n{}\n\n请使用 create_task 创建具体的子任务",
                analysis, strategy
            );

            debug!("Task plan parameters:{:?} ", parameters);
            info!("Task plan:{}", plan);

            Ok(ToolResult::success(plan))
        })
    }
}
