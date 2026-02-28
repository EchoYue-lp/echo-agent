//! 任务管理器

use crate::tasks::task::{Task, TaskStatus};
use std::collections::HashMap;

/// DAG 任务集合管理器，负责任务的增删改查和依赖调度
pub struct TaskManager {
    pub(crate) tasks: HashMap<String, Task>,
}

impl TaskManager {
    pub(crate) fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub(crate) fn add_task(&mut self, task: Task) {
        self.tasks.insert(task.id.clone(), task);
    }

    pub(crate) fn update_task(&mut self, id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.status = status;
        }
    }

    pub(crate) fn get_task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    pub(crate) fn delete_task(&mut self, id: &str) {
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
        ready.sort_by(|a, b| b.priority.cmp(&a.priority));
        ready.first().copied()
    }

    /// 检查是否所有任务都已终结（完成、取消或失败均视为终结）
    pub fn is_all_completed(&self) -> bool {
        self.tasks.values().all(|t| {
            matches!(
                t.status,
                TaskStatus::Completed | TaskStatus::Cancelled | TaskStatus::Failed(_)
            )
        })
    }

    /// 生成适合注入 LLM 上下文的任务进度摘要
    pub fn get_summary(&self) -> String {
        let (completed, total) = self.get_progress();
        let pending = self.get_pending_tasks().len();
        let in_progress = self.get_in_progress_tasks().len();

        format!(
            "任务进度: {}/{} 完成 | {} 待处理 | {} 进行中",
            completed, total, pending, in_progress
        )
    }

    /// 深度优先搜索检测循环
    pub(crate) fn dfs_detect_cycle(
        &self,
        task_id: &str,
        visited: &mut HashMap<String, VisitState>,
        path: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(task_id.to_string(), VisitState::Visiting);
        path.push(task_id.to_string());

        if let Some(task) = self.tasks.get(task_id) {
            for dep_id in &task.dependencies {
                if let Some(_dep_task) = self.tasks.get(dep_id) {
                    match visited.get(dep_id).copied() {
                        Some(VisitState::Visiting) => {
                            let cycle_start = path.iter().position(|id| id == dep_id).unwrap();
                            cycles.push(path[cycle_start..].to_vec());
                        }
                        Some(VisitState::Visited) => {}
                        None => {
                            self.dfs_detect_cycle(dep_id, visited, path, cycles);
                        }
                    }
                }
            }
        }

        path.pop();
        visited.insert(task_id.to_string(), VisitState::Visited);
    }

    /// 检查是否存在循环依赖
    pub fn has_circular_dependencies(&self) -> bool {
        !self.detect_circular_dependencies().is_empty()
    }

    pub(crate) fn get_dependency_chain_recursive(
        &self,
        task_id: &str,
        current_chain: &mut Vec<String>,
        chains: &mut Vec<Vec<String>>,
    ) {
        current_chain.push(task_id.to_string());

        if let Some(task) = self.tasks.get(task_id) {
            if task.dependencies.is_empty() {
                chains.push(current_chain.clone());
            } else {
                for dep_id in &task.dependencies {
                    self.get_dependency_chain_recursive(dep_id, current_chain, chains);
                }
            }
        }

        current_chain.pop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum VisitState {
    Visiting,
    Visited,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
