use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 任务状态
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// 待处理
    Pending,
    /// 进行中
    InProgress,
    /// 已完成
    Completed,
    /// 已取消
    Cancelled,
    /// 失败
    Failed(String),
    /// 阻塞
    Blocked(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// 任务 ID
    pub id: String,
    /// 任务描述
    pub description: String,
    /// 任务状态
    pub status: TaskStatus,
    /// 依赖的任务 ID 列表
    pub dependencies: Vec<String>,
    /// 优先级 (0-10, 10 最高)
    pub priority: u8,
    /// 任务结果
    pub result: Option<String>,
    /// 任务理由理由
    pub reasoning: Option<String>,
    /// 父任务ID
    pub parent_id: Option<String>,
    /// 创建时间戳
    pub created_at: u64,
    /// 更新时间戳
    pub updated_at: u64,
}

impl Task {
    /// 创建新任务
    pub fn new(id: String, description: String) -> Self {
        Self {
            id,
            description,
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
            priority: 5,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// 设置依赖
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    /// 添加依赖
    pub fn add_dependency(&mut self, dep: String) {
        self.dependencies.push(dep);
    }

    /// 设置优先级
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(10);
        self
    }
}

pub struct TaskManager {
    tasks: HashMap<String, Task>,
}

impl TaskManager {
    fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub(crate) fn add_task(&mut self, task: Task) {
        self.tasks.insert(task.id.clone(), task);
    }

    fn get_task(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    pub(crate) fn update_task(&mut self, id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.status = status;
        }
    }

    pub(crate) fn get_task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    fn delete_task(&mut self, id: &str) {
        self.tasks.remove(id);
    }

    pub(crate) fn get_all_tasks(&self) -> Vec<&Task> {
        self.tasks.values().collect()
    }

    pub(crate) fn get_pending_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.status == TaskStatus::Pending)
            .collect()
    }

    pub(crate) fn get_in_progress_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.status == TaskStatus::InProgress)
            .collect()
    }

    pub(crate) fn get_completed_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.status == TaskStatus::Completed)
            .collect()
    }

    fn get_cancelled_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.status == TaskStatus::Cancelled)
            .collect()
    }

    fn get_failed_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| matches!(t.status, TaskStatus::Failed(_)))
            .collect()
    }

    /// 获取所有可执行的任务（依赖已满足）
    pub fn get_ready_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|task| {
                task.status == TaskStatus::Pending
                    && task.dependencies.iter().all(|dep_id| {
                        self.tasks
                            .get(dep_id)
                            .map(|dep| dep.status == TaskStatus::Completed)
                            .unwrap_or(false)
                    })
            })
            .collect()
    }

    /// 获取进度统计
    pub fn get_progress(&self) -> (usize, usize) {
        let completed = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        let total = self.tasks.len();
        (completed, total)
    }

    /// 获取下一个应该执行的任务
    pub fn get_next_task(&self) -> Option<&Task> {
        let mut ready = self.get_ready_tasks();
        ready.sort_by(|a, b| a.priority.cmp(&b.priority));
        ready.first().copied()
    }

    /// 新增：检查是否所有任务都完成
    pub fn is_all_completed(&self) -> bool {
        self.tasks
            .values()
            .all(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Cancelled))
    }

    /// 新增：生成任务摘要（给 LLM 看的）
    pub fn get_summary(&self) -> String {
        let (completed, total) = self.get_progress();
        let pending = self.get_pending_tasks().len();
        let in_progress = self.get_in_progress_tasks().len();

        format!(
            "任务进度: {}/{} 完成 | {} 待处理 | {} 进行中",
            completed, total, pending, in_progress
        )
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
