use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::tasks::{Task, TaskManager, TaskStatus};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{debug, info};

/// Get the current timestamp in seconds, will not panic
fn now_secs() -> u64 {
    crate::utils::time::now_secs()
}

pub struct CreateTaskTool {
    task_manager: Arc<TaskManager>,
}

impl CreateTaskTool {
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        Self { task_manager }
    }
}

impl Tool for CreateTaskTool {
    fn name(&self) -> &str {
        "create_task"
    }

    fn description(&self) -> &str {
        "Break down complex problems into sub-tasks. Create a new pending task."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Unique task identifier, e.g. task_1, task_2"
                },
                "description": {
                    "type": "string",
                    "description": "Detailed task description explaining what to do"
                },
                "reasoning": {
                    "type": "string",
                    "description": "Why this task is needed and how it helps solve the main problem"
                },
                "dependencies": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of dependent task IDs (these must be completed first)"
                },
                "priority": {
                    "type": "number",
                    "description": "Priority 0-10, default 5"
                },
                "assigned_agent": {
                    "type": "string",
                    "description": "Agent name assigned to execute this task (optional)"
                },
                "tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Task tags (optional, for categorization and filtering)"
                },
                // Enhanced fields (Step 1)
                "task_type": {
                    "type": "string",
                    "enum": ["discovery", "implementation", "verification", "background", "delegation"],
                    "description": "Task type classification (default: implementation)"
                },
                "acceptance_criteria": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Acceptance criteria - conditions that must be met for task completion"
                },
                "allowed_tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Allowed tools for this task (optional, None = all tools allowed)"
                },
                "context_scope": {
                    "type": "string",
                    "enum": ["minimal", "relevant", "full", "isolated"],
                    "description": "Context scope for task execution (default: relevant)"
                },
                "risk_level": {
                    "type": "string",
                    "enum": ["low", "medium", "high"],
                    "description": "Risk level classification (default: medium)"
                },
                "can_parallelize": {
                    "type": "boolean",
                    "description": "Whether this task can be parallelized with other tasks (default: true)"
                },
                "requires_write_access": {
                    "type": "boolean",
                    "description": "Whether this task requires write access (default: false)"
                }
            },
            "required": ["task_id", "description", "reasoning"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let task_id = parameters
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task_id".to_string()))?;

            let description = parameters
                .get("description")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("description".to_string()))?;

            let reasoning = parameters
                .get("reasoning")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("reasoning".to_string()))?;

            let dependencies = parameters
                .get("dependencies")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let priority = (parameters
                .get("priority")
                .and_then(|v| v.as_f64())
                .unwrap_or(5.0)
                .clamp(0.0, 10.0) as u8)
                .min(10);

            let assigned_agent = parameters
                .get("assigned_agent")
                .and_then(|v| v.as_str())
                .map(String::from);

            let tags: Vec<String> = parameters
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Parse enhanced fields (Step 1) with backward-compatible defaults
            use crate::tasks::{ContextScope, RiskLevel, TaskType};

            let task_type = parameters
                .get("task_type")
                .and_then(|v| v.as_str())
                .and_then(|s| match s {
                    "discovery" => Some(TaskType::Discovery),
                    "implementation" => Some(TaskType::Implementation),
                    "verification" => Some(TaskType::Verification),
                    "background" => Some(TaskType::Background),
                    "delegation" => Some(TaskType::Delegation),
                    _ => None,
                })
                .unwrap_or_default();

            let acceptance_criteria: Vec<String> = parameters
                .get("acceptance_criteria")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let allowed_tools = parameters
                .get("allowed_tools")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });

            let context_scope = parameters
                .get("context_scope")
                .and_then(|v| v.as_str())
                .and_then(|s| match s {
                    "minimal" => Some(ContextScope::Minimal),
                    "relevant" => Some(ContextScope::Relevant),
                    "full" => Some(ContextScope::Full),
                    "isolated" => Some(ContextScope::Isolated),
                    _ => None,
                })
                .unwrap_or_default();

            let risk_level = parameters
                .get("risk_level")
                .and_then(|v| v.as_str())
                .and_then(|s| match s {
                    "low" => Some(RiskLevel::Low),
                    "medium" => Some(RiskLevel::Medium),
                    "high" => Some(RiskLevel::High),
                    _ => None,
                })
                .unwrap_or_default();

            let can_parallelize = parameters
                .get("can_parallelize")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let requires_write_access = parameters
                .get("requires_write_access")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let now = now_secs();
            let task = Task::new(task_id, description)
                .with_subject(description)
                .with_dependencies(dependencies)
                .with_priority(priority)
                .with_task_type(task_type)
                .with_acceptance_criteria(acceptance_criteria)
                .with_context_scope(context_scope)
                .with_risk_level(risk_level)
                .with_can_parallelize(can_parallelize)
                .with_requires_write_access(requires_write_access);

            // Set optional fields
            let mut task = task;
            task.reasoning = Some(reasoning.to_string());
            task.assigned_agent = assigned_agent.clone();
            task.tags = tags.clone();
            task.created_at = now;
            task.updated_at = now;

            if let Some(tools) = allowed_tools {
                task = task.with_allowed_tools(tools);
            }

            // Add task first
            self.task_manager.add_task(task.clone());

            // Then check if adding this task created circular dependencies
            let has_circular_deps = self.task_manager.has_circular_dependencies();

            if has_circular_deps {
                let cycles = self.task_manager.detect_circular_dependencies();
                let cycle_paths: Vec<String> = cycles
                    .iter()
                    .map(|cycle| format!("[{}]", cycle.join(" → ")))
                    .collect();

                // Rollback: remove the problematic task that was just added
                self.task_manager.delete_task(task_id);

                let error_msg = format!(
                    "Task [{}] creation failed: this task forms a circular dependency with existing tasks. Cycle path: {}",
                    task_id,
                    cycle_paths.join(" → ")
                );

                return Ok(ToolResult::error(error_msg));
            }

            info!(
                "Task [{}] created successfully, no circular dependencies.",
                task_id
            );

            debug!("Task create parameters: {:?}", parameters);
            info!("Task [{}] created successfully", task_id);

            let result = serde_json::json!({
                "task_id": task_id,
                "description": description,
                "reasoning": reasoning,
                "priority": priority,
                "dependencies": task.dependencies,
                "assigned_agent": assigned_agent,
                "tags": tags,
                "status": "pending",
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

pub struct UpdateTaskTool {
    task_manager: Arc<TaskManager>,
}

impl UpdateTaskTool {
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        Self { task_manager }
    }
}

impl Tool for UpdateTaskTool {
    fn name(&self) -> &str {
        "update_task"
    }

    fn description(&self) -> &str {
        "Update task status (start execution, mark completed, record failure, etc.)"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task ID to update"
                },
                "status": {
                    "type": "string",
                    "enum": ["in_progress", "completed", "cancelled", "failed"],
                    "description": "New status"
                },
                "result": {
                    "type": "string",
                    "description": "Task execution result (filled when completed)"
                },
                "reason": {
                    "type": "string",
                    "description": "Reason for failure or cancellation"
                }
            },
            "required": ["task_id", "status"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let task_id = parameters
                .get("task_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task_id".to_string()))?;

            let status_str = parameters
                .get("status")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("status".to_string()))?;

            let result = parameters
                .get("result")
                .and_then(|v| v.as_str())
                .map(String::from);

            let reason = parameters
                .get("reason")
                .and_then(|v| v.as_str())
                .map(String::from);

            let new_status = match status_str {
                "in_progress" => TaskStatus::InProgress,
                "completed" => TaskStatus::Completed,
                "cancelled" => TaskStatus::Cancelled,
                "failed" => TaskStatus::Failed(reason.clone().unwrap_or_default()),
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "status".to_string(),
                        message: format!("Invalid status: {}", status_str),
                    }
                    .into());
                }
            };

            // Validate state transition
            if let Err(e) = self.task_manager.update_task(task_id, new_status.clone()) {
                return Ok(ToolResult::error(format!(
                    "Task [{}] status update failed: {}",
                    task_id, e
                )));
            }

            // Update result
            if let Some(ref r) = result {
                self.task_manager.set_task_result(task_id, r.clone());
            }

            debug!("Task update parameters:{:?} ", parameters);
            info!("Task [{}] status updated to: {:?}", task_id, new_status);

            let update_result = serde_json::json!({
                "task_id": task_id,
                "status": status_str,
                "result": result,
                "reason": reason,
            });
            Ok(ToolResult::success_json(update_result))
        })
    }
}

pub struct ListTasksTool {
    task_manager: Arc<TaskManager>,
}

impl ListTasksTool {
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        Self { task_manager }
    }
}

impl Tool for ListTasksTool {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "View the status and progress of all current tasks"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "enum": ["all", "pending", "in_progress", "completed", "ready"],
                    "description": "Filter: all - all tasks, pending - awaiting, ready - can execute now"
                }
            }
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let filter = parameters
                .get("filter")
                .and_then(|v| v.as_str())
                .unwrap_or("all");

            let tasks = match filter {
                "pending" => self.task_manager.get_pending_tasks(),
                "in_progress" => self.task_manager.get_in_progress_tasks(),
                "completed" => self.task_manager.get_completed_tasks(),
                "ready" => self.task_manager.get_ready_tasks(),
                _ => self.task_manager.get_all_tasks(),
            };

            let summary = self.task_manager.get_summary();

            let task_items: Vec<Value> = tasks
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.id,
                        "description": t.description,
                        "status": format!("{:?}", t.status),
                        "priority": t.priority,
                        "dependencies": t.dependencies,
                        "assigned_agent": t.assigned_agent,
                        "tags": t.tags,
                    })
                })
                .collect();

            debug!("Task list parameters:{:?} ", parameters);
            info!("Task list: {} tasks", task_items.len());

            let result = serde_json::json!({
                "summary": summary,
                "filter": filter,
                "tasks": task_items,
            });
            Ok(ToolResult::success_json(result))
        })
    }
}

pub struct VisualizeDependenciesTool {
    task_manager: Arc<TaskManager>,
}

impl VisualizeDependenciesTool {
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        Self { task_manager }
    }
}

impl Tool for VisualizeDependenciesTool {
    fn name(&self) -> &str {
        "visualize_dependencies"
    }

    fn description(&self) -> &str {
        "Generate a visualization chart of task dependencies (Mermaid format)"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn execute(
        &self,
        _parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let mermaid = self.task_manager.visualize_dependencies();
            Ok(ToolResult::success(mermaid))
        })
    }
}

pub struct GetExecutionOrderTool {
    task_manager: Arc<TaskManager>,
}

impl GetExecutionOrderTool {
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        Self { task_manager }
    }
}

impl Tool for GetExecutionOrderTool {
    fn name(&self) -> &str {
        "get_execution_order"
    }

    fn description(&self) -> &str {
        "Get the recommended task execution order (based on dependencies)"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn execute(
        &self,
        _parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            match self.task_manager.get_topological_order() {
                Ok(order) => {
                    let items: Vec<Value> = order
                        .iter()
                        .enumerate()
                        .map(|(i, id)| serde_json::json!({"order": i + 1, "task_id": id}))
                        .collect();
                    Ok(ToolResult::success_json(serde_json::json!({
                        "execution_order": items
                    })))
                }
                Err(e) => Ok(ToolResult::error(e)),
            }
        })
    }
}
