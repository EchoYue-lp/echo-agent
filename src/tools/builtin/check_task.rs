//! Check task status tool — poll the status of a previously spawned background task.

use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::tasks::TaskSpawner;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

/// Tool that checks the status of a background task by ID.
pub struct CheckTaskStatusTool {
    spawner: Arc<TaskSpawner>,
}

impl CheckTaskStatusTool {
    /// Create a new tool with the given task spawner.
    pub fn new(spawner: Arc<TaskSpawner>) -> Self {
        Self { spawner }
    }
}

impl Tool for CheckTaskStatusTool {
    fn name(&self) -> &str {
        "check_task_status"
    }

    fn description(&self) -> &str {
        "Check the status of a previously spawned background task. Returns the current status (running, completed, failed, cancelled) and any available result."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the background task to check (returned by spawn_background_task)"
                }
            },
            "required": ["task_id"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let spawner = self.spawner.clone();
        Box::pin(async move {
            let task_id = parameters
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task_id".to_string()))?
                .to_string();

            debug!(task_id = %task_id, "Checking background task status");

            // List all tasks and find the one with matching ID
            let tasks = spawner.list().await;
            let task = tasks.iter().find(|t| t.id == task_id);

            match task {
                Some(summary) => {
                    let status = summary.status.as_str();
                    let mut result =
                        format!("Task '{}': {}\nStatus: {}", task_id, summary.name, status);

                    if summary.status.is_terminal() {
                        result.push_str("\nTask has finished.");
                    }

                    Ok(ToolResult::success(result))
                }
                None => Ok(ToolResult::error(format!(
                    "Task '{}' not found. It may have been pruned or never existed.",
                    task_id
                ))),
            }
        })
    }
}

/// Tool that lists all active background tasks.
pub struct ListBackgroundTasksTool {
    spawner: Arc<TaskSpawner>,
}

impl ListBackgroundTasksTool {
    pub fn new(spawner: Arc<TaskSpawner>) -> Self {
        Self { spawner }
    }
}

impl Tool for ListBackgroundTasksTool {
    fn name(&self) -> &str {
        "list_background_tasks"
    }

    fn description(&self) -> &str {
        "List all active background tasks with their current status."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
        })
    }

    fn execute(
        &self,
        _parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let spawner = self.spawner.clone();
        Box::pin(async move {
            let tasks = spawner.list().await;

            if tasks.is_empty() {
                return Ok(ToolResult::success("No active background tasks."));
            }

            let mut lines = Vec::new();
            lines.push(format!("Active background tasks ({}):", tasks.len()));
            for t in &tasks {
                lines.push(format!(
                    "  - {} ({}) [{}]: {}",
                    t.name,
                    t.id,
                    t.status.as_str(),
                    if t.status.is_terminal() {
                        "finished"
                    } else {
                        "in progress"
                    }
                ));
            }

            Ok(ToolResult::success(lines.join("\n")))
        })
    }
}
