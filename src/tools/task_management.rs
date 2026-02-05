use crate::error::{Result, ToolError};
use crate::tasks::{Task, TaskManager, TaskStatus};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

// 1. 创建任务工具
pub struct CreateTaskTool {
    task_manager: Arc<RwLock<TaskManager>>,
}

impl CreateTaskTool {
    pub fn new(task_manager: Arc<RwLock<TaskManager>>) -> Self {
        Self { task_manager }
    }
}

impl Tool for CreateTaskTool {
    fn name(&self) -> &str {
        "create_task"
    }

    fn description(&self) -> &str {
        "将复杂问题拆解为子任务。创建一个新的待执行任务。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "任务唯一标识符，如 task_1, task_2"
                },
                "description": {
                    "type": "string",
                    "description": "任务的详细描述，说明要做什么"
                },
                "reasoning": {
                    "type": "string",
                    "description": "为什么需要这个任务，它如何帮助解决主问题"
                },
                "dependencies": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "依赖的任务ID列表（必须先完成这些任务）"
                },
                "priority": {
                    "type": "number",
                    "description": "优先级 0-10，默认5"
                }
            },
            "required": ["task_id", "description", "reasoning"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
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

        let priority = parameters
            .get("priority")
            .and_then(|v| v.as_f64())
            .unwrap_or(5.0) as u8;

        let task = Task {
            id: task_id.to_string(),
            description: description.to_string(),
            status: TaskStatus::Pending,
            dependencies,
            priority: priority.min(10),
            result: None,
            reasoning: Some(reasoning.to_string()),
            parent_id: None,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        let mut manager = self.task_manager.write().unwrap();
        manager.add_task(task);

        Ok(ToolResult::success(format!(
            "✓ 已创建任务 [{}]: {}\n推理: {}",
            task_id, description, reasoning
        )))
    }
}

// 2. 查看任务列表工具
pub struct ListTasksTool {
    task_manager: Arc<RwLock<TaskManager>>,
}

impl ListTasksTool {
    pub fn new(task_manager: Arc<RwLock<TaskManager>>) -> Self {
        Self { task_manager }
    }
}

impl Tool for ListTasksTool {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "查看当前所有任务的状态和进度"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "enum": ["all", "pending", "in_progress", "completed", "ready"],
                    "description": "筛选条件：all-所有, pending-待处理, ready-可立即执行"
                }
            }
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        let filter = parameters
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("all");

        let manager = self.task_manager.read().unwrap();

        let tasks = match filter {
            "pending" => manager.get_pending_tasks(),
            "in_progress" => manager.get_in_progress_tasks(),
            "completed" => manager.get_completed_tasks(),
            "ready" => manager.get_ready_tasks(),
            _ => manager.get_all_tasks(),
        };

        let summary = manager.get_summary();

        let task_list = tasks
            .iter()
            .map(|t| {
                format!(
                    "[{}] {:?} - {} (优先级: {}, 依赖: {:?})",
                    t.id, t.status, t.description, t.priority, t.dependencies
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult::success(format!(
            "{}\n\n任务列表:\n{}",
            summary,
            if task_list.is_empty() {
                "无任务"
            } else {
                &task_list
            }
        )))
    }
}

// 3. 更新任务状态工具
pub struct UpdateTaskTool {
    task_manager: Arc<RwLock<TaskManager>>,
}

impl UpdateTaskTool {
    pub fn new(task_manager: Arc<RwLock<TaskManager>>) -> Self {
        Self { task_manager }
    }
}

impl Tool for UpdateTaskTool {
    fn name(&self) -> &str {
        "update_task"
    }

    fn description(&self) -> &str {
        "更新任务的状态（开始执行、标记完成、记录失败等）"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "要更新的任务ID"
                },
                "status": {
                    "type": "string",
                    "enum": ["in_progress", "completed", "cancelled", "failed"],
                    "description": "新状态"
                },
                "result": {
                    "type": "string",
                    "description": "任务执行结果（完成时填写）"
                },
                "reason": {
                    "type": "string",
                    "description": "失败或取消的原因"
                }
            },
            "required": ["task_id", "status"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
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
            "failed" => TaskStatus::Failed(reason.unwrap_or_default()),
            _ => {
                return Err(ToolError::InvalidParameter {
                    name: "status".to_string(),
                    message: format!("无效的状态: {}", status_str),
                }
                .into());
            }
        };

        let mut manager = self.task_manager.write().unwrap();
        manager.update_task(task_id, new_status.clone());

        // 更新结果
        if let Some(task) = manager.get_task_mut(task_id) {
            task.result = result;
            task.updated_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
        }

        Ok(ToolResult::success(format!(
            "✓ 任务 [{}] 状态已更新为: {:?}",
            task_id, new_status
        )))
    }
}

// 4. 制定计划工具（高级）
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

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        let analysis = parameters
            .get("analysis")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("analysis".to_string()))?;

        let strategy = parameters
            .get("strategy")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("strategy".to_string()))?;

        Ok(ToolResult::success(format!(
            "📋 计划已制定\n\n分析:\n{}\n\n策略:\n{}\n\n请使用 create_task 创建具体的子任务",
            analysis, strategy
        )))
    }
}
