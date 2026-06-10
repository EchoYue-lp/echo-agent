use futures::future::BoxFuture;
use std::sync::Arc;

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
        "Analyze complex problems and create a detailed execution plan. Break large tasks into ordered sub-tasks."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "analysis": {
                    "type": "string",
                    "description": "In-depth analysis of the problem: challenges, required info, possible approaches"
                },
                "strategy": {
                    "type": "string",
                    "description": "Solution strategy: describe how to solve the problem step by step"
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
                "📋 Plan created\n\nAnalysis:\n{}\n\nStrategy:\n{}\n\nUse create_task to create specific sub-tasks",
                analysis, strategy
            );

            debug!("Task plan parameters:{:?} ", parameters);
            info!("Task plan:{}", plan);

            Ok(ToolResult::success(plan))
        })
    }
}

/// A wrapper around PlanTool that captures the plan text into a shared state.
///
/// Registered by the builder instead of the plain PlanTool so that
/// `save_runtime_checkpoint` can capture the current plan for persistence.
pub struct PlanToolWithState {
    plan_state: Arc<tokio::sync::RwLock<Option<String>>>,
}

impl PlanToolWithState {
    pub fn new(plan_state: Arc<tokio::sync::RwLock<Option<String>>>) -> Self {
        Self { plan_state }
    }
}

impl Tool for PlanToolWithState {
    fn name(&self) -> &str {
        "plan"
    }

    fn description(&self) -> &str {
        PlanTool.description()
    }

    fn parameters(&self) -> Value {
        PlanTool.parameters()
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let plan_state = self.plan_state.clone();
        Box::pin(async move {
            let result = PlanTool.execute(parameters).await?;

            // Capture plan text into shared state for checkpointing
            let plan_text = result.output.clone();
            *plan_state.write().await = Some(plan_text);

            Ok(result)
        })
    }
}
