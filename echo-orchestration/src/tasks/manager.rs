//! Task manager

use super::events::TaskEventBus;
use super::hooks::TaskHookContext;
use super::hooks::TaskHookRegistry;
use super::task::{Task, TaskStatus};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

/// DAG task collection manager, responsible for task CRUD and dependency scheduling.
/// Uses `DashMap` for concurrency safety, can be shared across async tasks with `Arc<TaskManager>`.
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

    /// Create a manager with logging hooks
    pub fn with_logging() -> Self {
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::with_logging(),
            event_bus: None,
        }
    }

    /// Create a manager with an event bus
    pub fn with_event_bus() -> Self {
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::new(),
            event_bus: Some(TaskEventBus::new()),
        }
    }

    /// Create a manager with logging hooks and an event bus
    pub fn with_logging_and_events() -> Self {
        let mut bus = TaskEventBus::new();
        bus.register(Arc::new(super::events::LoggingListener));
        Self {
            tasks: DashMap::new(),
            hooks: TaskHookRegistry::new(),
            event_bus: Some(bus),
        }
    }

    /// Get the event bus reference (if configured)
    pub fn event_bus(&self) -> Option<&TaskEventBus> {
        self.event_bus.as_ref()
    }

    /// Get the Arc reference of the event bus (for cross-task sharing)
    pub fn event_bus_arc(&self) -> Option<Arc<TaskEventBus>> {
        self.event_bus.as_ref().map(|b| Arc::new(b.clone()))
    }

    // ── CRUD ──────────────────────────────────────────────────────────────

    pub fn add_task(&self, task: Task) {
        if let Some(ref bus) = self.event_bus {
            bus.emit(super::events::TaskEvent::Created { task: task.clone() });
        }
        self.tasks.insert(task.id.clone(), task);
    }

    /// Get a task (clone)
    pub fn get_task(&self, id: &str) -> Option<Task> {
        self.tasks.get(id).map(|r| r.value().clone())
    }

    /// Update task status (with state machine validation)
    ///
    /// If current state does not allow transition to target state, return `Err`.
    /// Also broadcasts the state change via the event bus.
    /// Sends an event only when the state actually changes (old_status != new_status).
    pub fn update_task(&self, id: &str, status: TaskStatus) -> Result<(), String> {
        if let Some(mut task) = self.tasks.get_mut(id) {
            let old_status = task.status.clone();
            let new_status = task.status.transition_to(status)?;
            task.status = new_status.clone();
            task.updated_at = super::time::now_secs();

            // Only send events when the state actually changes
            if old_status != new_status
                && let Some(ref bus) = self.event_bus
            {
                bus.emit(super::events::TaskEvent::Updated {
                    task_id: id.to_string(),
                    old_status: old_status.clone(),
                    new_status: new_status.clone(),
                });

                // If entering a terminal state, send the corresponding specific event
                match &new_status {
                    TaskStatus::Completed => {
                        let result = task.result.clone().unwrap_or_default();
                        bus.emit(super::events::TaskEvent::Completed {
                            task_id: id.to_string(),
                            result,
                        });
                    }
                    TaskStatus::Failed(err) => {
                        bus.emit(super::events::TaskEvent::Failed {
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

    /// Alias for `update_task`, used by executor
    pub fn update_task_status(&self, id: &str, status: TaskStatus) -> Result<(), String> {
        self.update_task(id, status)
    }

    /// Set task result
    pub fn set_task_result(&self, id: &str, result: String) {
        if let Some(mut task) = self.tasks.get_mut(id) {
            task.result = Some(result);
            task.updated_at = super::time::now_secs();
        }
    }

    /// Record task execution
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
            bus.emit(super::events::TaskEvent::Deleted {
                task_id: id.to_string(),
            });
        }
    }

    /// Clear all tasks
    pub fn clear(&self) {
        self.tasks.clear();
    }

    /// Cancel a specific task
    pub fn cancel_task(&self, id: &str) -> bool {
        match self.update_task(id, TaskStatus::Cancelled) {
            Ok(()) => true,
            Err(_) => false,
        }
    }

    /// Cancel all tasks
    pub fn cancel_all(&self) {
        let task_ids: Vec<String> = self.tasks.iter().map(|r| r.key().clone()).collect();
        for id in task_ids {
            let _ = self.update_task(&id, TaskStatus::Cancelled);
        }
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    /// Get all tasks (cloned)
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

    /// Get all executable tasks (dependencies satisfied), returns clones
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

    /// Get progress statistics
    pub fn get_progress(&self) -> (usize, usize) {
        let completed = self
            .tasks
            .iter()
            .filter(|r| r.value().status == TaskStatus::Completed)
            .count();
        let total = self.tasks.len();
        (completed, total)
    }

    /// Get the next task to execute
    pub fn get_next_task(&self) -> Option<Task> {
        let mut ready = self.get_ready_tasks();
        ready.sort_by_key(|task| std::cmp::Reverse(task.priority));
        ready.into_iter().next()
    }

    /// Check if all tasks have reached a terminal state
    pub fn is_all_completed(&self) -> bool {
        self.tasks.iter().all(|r| r.value().status.is_terminal())
    }

    /// Check if there are pending tasks whose dependencies include a
    /// terminal-failure state (Failed / TimedOut / Cancelled).
    ///
    /// Such tasks can never become ready because their upstream will never
    /// complete successfully. Used for deadlock diagnosis in the executor.
    pub fn has_unresolvable_pending(&self) -> bool {
        self.tasks.iter().any(|r| {
            let task = r.value();
            if task.status != TaskStatus::Pending {
                return false;
            }
            task.dependencies.iter().any(|dep_id| {
                self.tasks
                    .get(dep_id)
                    .map(|dep| {
                        matches!(
                            dep.value().status,
                            TaskStatus::Failed(_)
                                | TaskStatus::TimedOut { .. }
                                | TaskStatus::Cancelled
                        )
                    })
                    .unwrap_or(true) // missing dependency = unresolvable
            })
        })
    }

    /// Generate a task progress summary suitable for injection into LLM context
    pub fn get_summary(&self) -> String {
        let (completed, total) = self.get_progress();
        let pending = self.get_pending_tasks().len();
        let in_progress = self.get_in_progress_tasks().len();

        format!(
            "Task progress: {}/{} completed | {} pending | {} in progress",
            completed, total, pending, in_progress
        )
    }

    /// Get all downstream task IDs that depend on the specified task
    ///
    /// When a task_id completes, call this method to find which tasks may become ready.
    pub fn get_dependent_tasks(&self, task_id: &str) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|entry| entry.value().dependencies.contains(&task_id.to_string()))
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// When the specified task completes, wake up tasks that depend on it
    ///
    /// Returns a list of task IDs that newly became ready.
    pub fn wake_dependents(&self, completed_task_id: &str) -> Vec<String> {
        let dependents = self.get_dependent_tasks(completed_task_id);
        let mut newly_ready = Vec::new();

        for dep_id in &dependents {
            if let Some(task) = self.tasks.get(dep_id)
                && task.status == TaskStatus::Pending
            {
                // Check if all dependencies of this task have completed
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

        newly_ready
    }

    // ── Hooks ─────────────────────────────────────────────────────────────

    /// Get the hooks registry reference
    pub fn hooks(&self) -> &TaskHookRegistry {
        &self.hooks
    }

    /// Create hook context
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

    // ── DAG Analysis (internal) ──────────────────────────────────────────────

    /// Depth-first search to detect cycles
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
                            if let Some(cycle_start) = path.iter().position(|id| id == dep_id) {
                                cycles.push(path[cycle_start..].to_vec());
                            }
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

    /// Check if cyclic dependencies exist
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

    // ── Run-Level Operations ──────────────────────────────────────────────────

    /// Get all tasks belonging to a specific run.
    /// Tasks without a run_id are excluded.
    pub fn get_tasks_by_run(&self, run_id: &str) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|r| r.value().run_id.as_deref() == Some(run_id))
            .map(|r| r.value().clone())
            .collect()
    }

    /// Get pending (non-terminal) tasks for a run.
    pub fn get_run_pending(&self, run_id: &str) -> Vec<Task> {
        self.tasks
            .iter()
            .filter(|r| {
                r.value().run_id.as_deref() == Some(run_id) && !r.value().status.is_terminal()
            })
            .map(|r| r.value().clone())
            .collect()
    }

    /// Generate a progress summary for a specific run.
    pub fn get_run_summary(&self, run_id: &str) -> String {
        let tasks = self.get_tasks_by_run(run_id);
        let total = tasks.len();
        let completed = tasks.iter().filter(|t| t.status.is_terminal()).count();
        format!("Run {run_id}: {completed}/{total} tasks completed")
    }

    /// Check if all tasks in a run have reached terminal state.
    pub fn is_run_complete(&self, run_id: &str) -> bool {
        self.tasks
            .iter()
            .filter(|r| r.value().run_id.as_deref() == Some(run_id))
            .all(|r| r.value().status.is_terminal())
    }

    /// Pause all non-terminal tasks in a run.
    pub fn pause_run(&self, run_id: &str, reason: &str) {
        let paused = TaskStatus::Paused(reason.to_string());
        for mut entry in self.tasks.iter_mut() {
            if entry.value().run_id.as_deref() == Some(run_id)
                && !entry.value().status.is_terminal()
            {
                let _ = entry.value_mut().status.transition_to(paused.clone());
            }
        }
    }

    /// Resume all Paused tasks in a run back to Pending.
    pub fn resume_run(&self, run_id: &str) {
        for mut entry in self.tasks.iter_mut() {
            if entry.value().run_id.as_deref() == Some(run_id)
                && matches!(entry.value().status, TaskStatus::Paused(_))
            {
                entry.value_mut().status = TaskStatus::Pending;
            }
        }
    }

    // ── Persistence ──────────────────────────────────────────────────────────

    /// Load all tasks from TaskStore into memory
    pub async fn load_from_store(
        &self,
        store: &dyn super::store::TaskStore,
    ) -> echo_core::error::Result<()> {
        let tasks = store.load_all().await?;
        for task in tasks {
            self.tasks.insert(task.id.clone(), task);
        }
        Ok(())
    }

    /// Persist all tasks to TaskStore
    pub async fn save_to_store(
        &self,
        store: &dyn super::store::TaskStore,
    ) -> echo_core::error::Result<()> {
        let tasks = self.get_all_tasks();
        store.save_all(&tasks).await
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
