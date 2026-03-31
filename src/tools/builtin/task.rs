use futures::future::BoxFuture;

use crate::error::{ReactError, ToolError};
use crate::tasks::{Task, TaskManager, TaskStatus};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

/// 获取当前时间戳（秒），不会 panic
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 安全读取 RwLock，将 poisoned lock 转为 ReactError
fn read_lock<T>(lock: &RwLock<T>) -> crate::error::Result<std::sync::RwLockReadGuard<'_, T>> {
    lock.read()
        .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))
}

/// 安全写入 RwLock，将 poisoned lock 转为 ReactError
fn write_lock<T>(lock: &RwLock<T>) -> crate::error::Result<std::sync::RwLockWriteGuard<'_, T>> {
    lock.write()
        .map_err(|e| ReactError::Other(format!("Lock poisoned: {}", e)))
}

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

            let now = now_secs();
            let task = Task {
                id: task_id.to_string(),
                description: description.to_string(),
                status: TaskStatus::Pending,
                dependencies,
                priority,
                result: None,
                reasoning: Some(reasoning.to_string()),
                parent_id: None,
                created_at: now,
                updated_at: now,
            };

            let mut manager = write_lock(&self.task_manager)?;

            // 先添加任务
            manager.add_task(task.clone());

            // 再检测是否因添加此任务而产生循环依赖
            let has_circular_deps = manager.has_circular_dependencies();

            if has_circular_deps {
                // 检测循环依赖
                let cycles = manager.detect_circular_dependencies();
                let cycle_paths: Vec<String> = cycles
                    .iter()
                    .map(|cycle| format!("[{}]", cycle.join(" → ")))
                    .collect();

                // 回滚：移除刚添加的有问题的任务
                manager.delete_task(task_id);

                let error_msg = format!(
                    "❌ 任务 [{}] 创建失败：此任务与现有任务形成循环依赖！\n\n循环路径: {}\n\n请检查依赖关系并重新规划。",
                    task_id,
                    cycle_paths.join(" | ")
                );

                return Ok(ToolResult::error(error_msg));
            }

            info!(
                "Task [{}] created successfully, no circular dependencies.",
                task_id
            );

            let deps_str = if task.dependencies.is_empty() {
                "无".to_string()
            } else {
                task.dependencies.join(", ")
            };

            let create = format!(
                "✅ 已创建任务 [{}]\n📝 描述: {}\n💭 推理: {}\n⭐ 优先级: {}\n🔗 依赖: {}",
                task_id, description, reasoning, priority, deps_str
            );

            debug!("Task create parameters: {:?}", parameters);
            info!("Task create: {}", create);

            Ok(ToolResult::success(create))
        })
    }
}

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
                "failed" => TaskStatus::Failed(reason.unwrap_or_default()),
                _ => {
                    return Err(ToolError::InvalidParameter {
                        name: "status".to_string(),
                        message: format!("无效的状态: {}", status_str),
                    }
                    .into());
                }
            };

            let mut manager = write_lock(&self.task_manager)?;
            manager.update_task(task_id, new_status.clone());

            // 更新结果
            if let Some(task) = manager.get_task_mut(task_id) {
                task.result = result;
                task.updated_at = now_secs();
            }

            let update = format!("✓ 任务 [{}] 状态已更新为: {:?}", task_id, new_status);
            debug!("Task update parameters:{:?} ", parameters);
            info!("Task update:{}", update);

            Ok(ToolResult::success(update))
        })
    }
}

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

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let filter = parameters
                .get("filter")
                .and_then(|v| v.as_str())
                .unwrap_or("all");

            let manager = read_lock(&self.task_manager)?;

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
                        "taskid:[{}] ,task status:{:?}  ,task description: {} (任务优先级: {}, 任务依赖: {:?})",
                        t.id, t.status, t.description, t.priority, t.dependencies
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            let list = format!(
                "{}\n\n任务列表:\n{}",
                summary,
                if task_list.is_empty() {
                    "无任务"
                } else {
                    &task_list
                }
            );

            debug!("Task list parameters:{:?} ", parameters);
            info!("Task list:{}", list);

            Ok(ToolResult::success(list))
        })
    }
}

pub struct VisualizeDependenciesTool {
    task_manager: Arc<RwLock<TaskManager>>,
}

impl VisualizeDependenciesTool {
    pub fn new(task_manager: Arc<RwLock<TaskManager>>) -> Self {
        Self { task_manager }
    }
}

impl Tool for VisualizeDependenciesTool {
    fn name(&self) -> &str {
        "visualize_dependencies"
    }

    fn description(&self) -> &str {
        "生成任务依赖关系的可视化图表（Mermaid 格式）"
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
            let manager = read_lock(&self.task_manager)?;
            let mermaid = manager.visualize_dependencies();
            Ok(ToolResult::success(mermaid))
        })
    }
}

pub struct GetExecutionOrderTool {
    task_manager: Arc<RwLock<TaskManager>>,
}

impl GetExecutionOrderTool {
    pub fn new(task_manager: Arc<RwLock<TaskManager>>) -> Self {
        Self { task_manager }
    }
}

impl Tool for GetExecutionOrderTool {
    fn name(&self) -> &str {
        "get_execution_order"
    }

    fn description(&self) -> &str {
        "获取任务的推荐执行顺序（基于依赖关系）"
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
            let manager = read_lock(&self.task_manager)?;
            match manager.get_topological_order() {
                Ok(order) => {
                    let output = order
                        .iter()
                        .enumerate()
                        .map(|(i, id)| format!("{}. {}", i + 1, id))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(ToolResult::success(output))
                }
                Err(e) => Ok(ToolResult::error(e)),
            }
        })
    }
}
