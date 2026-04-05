//! 任务管理器

use crate::tasks::events::TaskEventBus;
use crate::tasks::hooks::TaskHookContext;
use crate::tasks::hooks::TaskHookRegistry;
use crate::tasks::task::{Task, TaskStatus};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

/// DAG 任务集合管理器，负责任务的增删改查和依赖调度。
/// 使用 `DashMap` 实现并发安全，可配合 `Arc<TaskManager>` 在异步任务间共享。
pub struct TaskManager {
    pub(crate) tasks: DashMap<String, Task>,
    hooks: TaskHookRegistry,
    event_bus: Option<TaskEventBus>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::new(),
            event_bus: None,
        }
    }

    /// 创建带日志 hooks 的管理器
    pub fn with_logging() -> Self {
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::with_logging(),
            event_bus: None,
        }
    }

    /// 创建带事件总线的管理器
    pub fn with_event_bus() -> Self {
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::new(),
            event_bus: Some(TaskEventBus::new()),
        }
    }

    /// 创建带日志 hooks 和事件总线的管理器
    pub fn with_logging_and_events() -> Self {
        let mut bus = TaskEventBus::new();
        bus.register(Box::new(crate::tasks::events::LoggingListener));
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::new(),
            event_bus: Some(bus),
        }
    }

    /// 获取事件总线引用（如果已配置）
    pub fn event_bus(&self) -> Option<&TaskEventBus> {
        self.event_bus.as_ref()
    }

    /// 获取事件总线的 Arc 引用（用于跨任务共享）
    pub fn event_bus_arc(&self) -> Option<Arc<TaskEventBus>> {
        self.event_bus.as_ref().map(|b| Arc::new(b.clone()))
    }

    // ── CRUD ──────────────────────────────────────────────────────────────

    pub fn add_task(&self, task: Task) {
        if let Some(ref bus) = self.event_bus {
            bus.emit(crate::tasks::events::TaskEvent::Created { task: task.clone() });
        }
        self.tasks.insert(task.id.clone(), task);
    }

    /// 获取任务（克隆）
    pub fn get_task(&self, id: &str) -> Option<Task> {
        self.tasks.get(id).map(|r| r.value().clone())
    }

    /// 更新任务状态（带状态机校验）
    ///
    /// 如果当前状态不允许转换到目标状态，返回 `Err`。
    /// 同时通过事件总线广播状态变更。
    pub fn update_task(&self, id: &str, status: TaskStatus) -> Result<(), String> {
        if let Some(mut task) = self.tasks.get_mut(id) {
            let old_status = task.status.clone();
            let new_status = task.status.transition_to(status)?;
            task.status = new_status.clone();
            task.updated_at = crate::utils::time::now_secs();

            // 发送状态变更事件
            if let Some(ref bus) = self.event_bus {
                bus.emit(crate::tasks::events::TaskEvent::Updated {
                    task_id: id.to_string(),
                    old_status,
                    new_status: new_status.clone(),
                });

                // 如果进入终态，发送对应的具体事件
                match &new_status {
                    TaskStatus::Completed => {
                        let result = task.result.clone().unwrap_or_default();
                        bus.emit(crate::tasks::events::TaskEvent::Completed {
                            task_id: id.to_string(),
                            result,
                        });
                    }
                    TaskStatus::Failed(err) => {
                        bus.emit(crate::tasks::events::TaskEvent::Failed {
                            task_id: id.to_string(),
                            error: err.clone(),
                            attempt: task.retry_count,
                        });
                    }
                    _ => {}
                }
            }

            Ok(())
        } else {
            Err(format!("Task not found: {}", id))
        }
    }

    /// `update_task` 的别名，executor 使用
    pub fn update_task_status(&self, id: &str, status: TaskStatus) -> Result<(), String> {
        self.update_task(id, status)
    }

    /// 设置任务结果
    pub fn set_task_result(&self, id: &str, result: String) {
        if let Some(mut task) = self.tasks.get_mut(id) {
            task.result = Some(result);
            task.updated_at = crate::utils::time::now_secs();
        }
    }

    /// 记录任务执行
    pub fn record_task_execution(
        &self,
        id: &str,
        attempt: u32,
        error: Option<String>,
        duration_secs: Option<u64>,
        result: Option<String>,
    ) {
        if let Some(mut task) = self.tasks.get_mut(id) {
            task.record_execution(attempt, error, duration_secs, result);
        }
    }

    pub fn delete_task(&self, id: &str) {
        self.tasks.remove(id);
        if let Some(ref bus) = self.event_bus {
            bus.emit(crate::tasks::events::TaskEvent::Deleted {
                task_id: id.to_string(),
            });
        }
    }

    /// 清空所有任务
    pub fn clear(&self) {
        self.tasks.clear();
    }

    /// 取消指定任务
    pub fn cancel_task(&self, id: &str) -> bool {
        match self.update_task(id, TaskStatus::Cancelled) {
            Ok(()) => true,
            Err(_) => false,
        }
    }

    /// 取消所有任务
    pub fn cancel_all(&self) {
        let task_ids: Vec<String> = self.tasks.iter().map(|r| r.key().clone()).collect();
        for id in task_ids {
            let _ = self.update_task(&id, TaskStatus::Cancelled);
        }
    }

    // ── 查询 ──────────────────────────────────────────────────────────────

    /// 获取所有任务（克隆）
    pub fn get_all_tasks(&self) -> Vec<Task> {
        self.tasks.iter().map(|r| r.value().clone()).collect()
    }

    pub fn get_pending_tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|r| r.value().status == TaskStatus::Pending)
            .map(|r| r.value().clone())
            .collect()
    }

    pub fn get_in_progress_tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|r| r.value().status == TaskStatus::InProgress)
            .map(|r| r.value().clone())
            .collect()
    }

    pub fn get_completed_tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|r| r.value().status == TaskStatus::Completed)
            .map(|r| r.value().clone())
            .collect()
    }

    /// 获取所有可执行的任务（依赖已满足），返回克隆
    pub fn get_ready_tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|entry| {
                let task = entry.value();
                task.status == TaskStatus::Pending
                    && task.dependencies.iter().all(|dep_id| {
                        self.tasks
                            .get(dep_id)
                            .map(|dep| dep.value().status == TaskStatus::Completed)
                            .unwrap_or(false)
                    })
            })
            .map(|r| r.value().clone())
            .collect()
    }

    /// 获取进度统计
    pub fn get_progress(&self) -> (usize, usize) {
        let completed = self
            .tasks
            .iter()
            .filter(|r| r.value().status == TaskStatus::Completed)
            .count();
        let total = self.tasks.len();
        (completed, total)
    }

    /// 获取下一个应该执行的任务
    pub fn get_next_task(&self) -> Option<Task> {
        let mut ready = self.get_ready_tasks();
        ready.sort_by(|a, b| b.priority.cmp(&a.priority));
        ready.into_iter().next()
    }

    /// 检查是否所有任务都已终结
    pub fn is_all_completed(&self) -> bool {
        self.tasks.iter().all(|r| r.value().status.is_terminal())
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

    /// 获取依赖指定任务的所有下游任务 ID
    ///
    /// 当 task_id 完成时，可以调用此方法查找哪些任务可能变为就绪。
    pub fn get_dependent_tasks(&self, task_id: &str) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|entry| entry.value().dependencies.contains(&task_id.to_string()))
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// 当指定任务完成后，唤醒依赖它的任务
    ///
    /// 返回新变为就绪的任务 ID 列表。
    pub fn wake_dependents(&self, completed_task_id: &str) -> Vec<String> {
        let dependents = self.get_dependent_tasks(completed_task_id);
        let mut newly_ready = Vec::new();

        for dep_id in &dependents {
            if let Some(task) = self.tasks.get(dep_id) {
                if task.status == TaskStatus::Pending {
                    // 检查该任务的所有依赖是否都已完成
                    let all_deps_done = task.dependencies.iter().all(|dep_id| {
                        self.tasks
                            .get(dep_id)
                            .map(|dep| dep.value().status == TaskStatus::Completed)
                            .unwrap_or(false)
                    });
                    if all_deps_done {
                        newly_ready.push(dep_id.clone());
                    }
                }
            }
        }

        newly_ready
    }

    // ── Hooks ─────────────────────────────────────────────────────────────

    /// 获取 hooks 注册表的引用
    pub fn hooks(&self) -> &TaskHookRegistry {
        &self.hooks
    }

    /// 创建 hook 上下文
    pub fn create_hook_context(
        &self,
        task_id: &str,
        attempt: u32,
        executor: Option<String>,
    ) -> Option<TaskHookContext> {
        self.tasks.get(task_id).map(|r| TaskHookContext {
            task: r.value().clone(),
            attempt,
            executor,
        })
    }

    // ── DAG 分析（内部） ──────────────────────────────────────────────────

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
                if self.tasks.contains_key(dep_id) {
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

    // ── 持久化 ──────────────────────────────────────────────────────────────

    /// 从 TaskStore 加载所有任务到内存
    pub async fn load_from_store(
        &self,
        store: &dyn crate::tasks::store::TaskStore,
    ) -> crate::error::Result<()> {
        let tasks = store.load_all().await?;
        for task in tasks {
            self.tasks.insert(task.id.clone(), task);
        }
        Ok(())
    }

    /// 将所有任务持久化到 TaskStore
    pub async fn save_to_store(
        &self,
        store: &dyn crate::tasks::store::TaskStore,
    ) -> crate::error::Result<()> {
        let tasks = self.get_all_tasks();
        store.save_all(&tasks).await
    }

    /// 从检查点恢复任务状态
    pub async fn restore_from_checkpoint(
        &self,
        checkpoint: &crate::tasks::store::ExecutionCheckpoint,
    ) {
        for task in &checkpoint.tasks {
            self.tasks.insert(task.id.clone(), task.clone());
        }
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
