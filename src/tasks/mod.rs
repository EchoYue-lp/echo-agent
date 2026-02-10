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
        ready.sort_by(|a, b| b.priority.cmp(&a.priority));
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

    /// 检测循环依赖，返回所有循环路径
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited: HashMap<String, VisitState> = HashMap::new();
        let mut path: Vec<String> = Vec::new();

        for task_id in self.tasks.keys() {
            if visited.get(task_id) != Some(&VisitState::Visited) {
                self.dfs_detect_cycle(task_id, &mut visited, &mut path, &mut cycles);
            }
        }

        cycles
    }

    /// 深度优先搜索检测循环
    fn dfs_detect_cycle(
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
                            // 找到循环，提取循环路径
                            let cycle_start = path.iter().position(|id| id == dep_id).unwrap();
                            let cycle: Vec<String> = path[cycle_start..].to_vec();
                            cycles.push(cycle);
                        }
                        Some(VisitState::Visited) => {
                            // 已经访问过，跳过
                        }
                        Some(VisitState::Unvisited) | None => {
                            // 未访问过，继续递归
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

    /// 获取拓扑排序（如果存在循环依赖则返回错误）
    pub fn get_topological_order(&self) -> Result<Vec<String>, String> {
        let cycles = self.detect_circular_dependencies();
        if !cycles.is_empty() {
            let cycle_strs: Vec<String> = cycles
                .iter()
                .map(|cycle| format!("[{}]", cycle.join(" -> ")))
                .collect();
            return Err(format!(
                "存在循环依赖，无法进行拓扑排序: {}",
                cycle_strs.join(", ")
            ));
        }

        // 使用 Kahn 算法进行拓扑排序
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut adj_list: HashMap<String, Vec<String>> = HashMap::new();

        // 初始化入度和邻接表
        for task_id in self.tasks.keys() {
            in_degree.insert(task_id.clone(), 0);
            adj_list.insert(task_id.clone(), Vec::new());
        }

        // 构建依赖图
        for (task_id, task) in &self.tasks {
            for dep_id in &task.dependencies {
                if let Some(adj) = adj_list.get_mut(dep_id) {
                    adj.push(task_id.clone());
                }
                if let Some(degree) = in_degree.get_mut(task_id) {
                    *degree += 1;
                }
            }
        }

        // 找到所有入度为 0 的节点
        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(id, _)| id.clone())
            .collect();
        queue.sort_by(|a, b| {
            self.tasks
                .get(a)
                .and_then(|t| self.tasks.get(b).map(|u| u.priority.cmp(&t.priority)))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut result = Vec::new();

        while let Some(task_id) = queue.pop() {
            result.push(task_id.clone());

            if let Some(adj) = adj_list.get(&task_id) {
                for neighbor in adj {
                    if let Some(degree) = in_degree.get_mut(neighbor) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push(neighbor.clone());
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// 生成依赖图的可视化（Mermaid 格式）
    pub fn visualize_dependencies(&self) -> String {
        let mut mermaid = String::from("graph TD\n");

        for (task_id, task) in &self.tasks {
            for dep_id in &task.dependencies {
                mermaid.push_str(&format!(
                    "  {}[{}] --> {}[{}]\n",
                    dep_id, dep_id, task_id, task_id
                ));
            }
        }

        mermaid
    }

    /// 获取依赖链（从指定任务到根节点）
    pub fn get_dependency_chain(&self, task_id: &str) -> Vec<Vec<String>> {
        let mut chains = Vec::new();
        let mut current_chain = Vec::new();
        self.get_dependency_chain_recursive(task_id, &mut current_chain, &mut chains);
        chains
    }

    fn get_dependency_chain_recursive(
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
enum VisitState {
    Unvisited,
    Visiting,
    Visited,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_task(id: &str, description: &str, dependencies: Vec<&str>) -> Task {
        Task {
            id: id.to_string(),
            description: description.to_string(),
            status: TaskStatus::Pending,
            dependencies: dependencies.into_iter().map(String::from).collect(),
            priority: 5,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn test_no_circular_dependencies() {
        let mut manager = TaskManager::new();

        // 创建无循环依赖的任务链: task1 -> task2 -> task3
        manager.add_task(create_task("task1", "First task", vec![]));
        manager.add_task(create_task("task2", "Second task", vec!["task1"]));
        manager.add_task(create_task("task3", "Third task", vec!["task2"]));

        let cycles = manager.detect_circular_dependencies();
        assert!(cycles.is_empty(), "应该没有循环依赖");
        assert!(!manager.has_circular_dependencies());
    }

    #[test]
    fn test_simple_circular_dependency() {
        let mut manager = TaskManager::new();

        // 创建简单循环: task1 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该检测到一个循环");
        assert!(manager.has_circular_dependencies());

        // 验证循环路径
        let cycle = &cycles[0];
        assert!(cycle.contains(&"task1".to_string()));
        assert!(cycle.contains(&"task2".to_string()));
    }

    #[test]
    fn test_complex_circular_dependency() {
        let mut manager = TaskManager::new();

        // 创建复杂循环: task1 -> task2 -> task3 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task3"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该检测到一个循环");
        assert_eq!(cycles[0].len(), 3, "循环应该包含3个任务");
    }

    #[test]
    fn test_multiple_circular_dependencies() {
        let mut manager = TaskManager::new();

        // 创建两个独立的循环
        // 循环1: task1 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        // 循环2: task3 -> task4 -> task3
        manager.add_task(create_task("task3", "Task 3", vec!["task4"]));
        manager.add_task(create_task("task4", "Task 4", vec!["task3"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 2, "应该检测到两个循环");
    }

    #[test]
    fn test_self_dependency() {
        let mut manager = TaskManager::new();

        // 创建自依赖: task1 -> task1
        manager.add_task(create_task("task1", "Task 1", vec!["task1"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该检测到自依赖循环");
        assert_eq!(cycles[0].len(), 1, "自依赖循环应该只包含一个任务");
    }

    #[test]
    fn test_mixed_dependencies() {
        let mut manager = TaskManager::new();

        // 混合情况：部分有循环，部分没有
        // 正常链: task1 -> task2 -> task3
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        // 循环: task4 -> task5 -> task4
        manager.add_task(create_task("task4", "Task 4", vec!["task5"]));
        manager.add_task(create_task("task5", "Task 5", vec!["task4"]));

        let cycles = manager.detect_circular_dependencies();
        assert_eq!(cycles.len(), 1, "应该只检测到一个循环");
        assert!(cycles[0].contains(&"task4".to_string()));
        assert!(cycles[0].contains(&"task5".to_string()));
    }

    #[test]
    fn test_topological_order_no_cycles() {
        let mut manager = TaskManager::new();

        // task3 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let order = manager.get_topological_order();
        assert!(order.is_ok(), "无循环依赖时应该成功");

        let order = order.unwrap();
        assert_eq!(order.len(), 3, "应该包含所有任务");

        // 验证顺序：task1 应该在 task2 之前，task2 应该在 task3 之前
        let pos1 = order.iter().position(|id| id == "task1").unwrap();
        let pos2 = order.iter().position(|id| id == "task2").unwrap();
        let pos3 = order.iter().position(|id| id == "task3").unwrap();

        assert!(pos1 < pos2, "task1 应该在 task2 之前");
        assert!(pos2 < pos3, "task2 应该在 task3 之前");
    }

    #[test]
    fn test_topological_order_with_cycles() {
        let mut manager = TaskManager::new();

        // 创建循环
        manager.add_task(create_task("task1", "Task 1", vec!["task2"]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));

        let order = manager.get_topological_order();
        assert!(order.is_err(), "有循环依赖时应该返回错误");

        let error_msg = order.unwrap_err();
        assert!(
            error_msg.contains("循环依赖"),
            "错误信息应该包含循环依赖提示"
        );
    }

    #[test]
    fn test_get_dependency_chain() {
        let mut manager = TaskManager::new();

        // task3 -> task2 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let chains = manager.get_dependency_chain("task3");
        assert_eq!(chains.len(), 1, "应该有一条依赖链");
        assert_eq!(
            chains[0],
            vec!["task3", "task2", "task1"],
            "依赖链顺序应该正确"
        );
    }

    #[test]
    fn test_get_dependency_chain_multiple() {
        let mut manager = TaskManager::new();

        // task4 依赖 task2 和 task3
        // task2 -> task1
        // task3 -> task1
        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task1"]));
        manager.add_task(create_task("task4", "Task 4", vec!["task2", "task3"]));

        let chains = manager.get_dependency_chain("task4");
        assert_eq!(chains.len(), 2, "应该有两条依赖链");

        // 验证两条链
        let chain1 = vec!["task4", "task2", "task1"];
        let chain2 = vec!["task4", "task3", "task1"];
        let chain1 = chain1.iter().map(|x| x.to_string()).collect();
        let chain2 = chain2.iter().map(|x| x.to_string()).collect();

        assert!(chains.contains(&chain1), "应该包含第一条链");
        assert!(chains.contains(&chain2), "应该包含第二条链");
    }

    #[test]
    fn test_visualize_dependencies() {
        let mut manager = TaskManager::new();

        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        let mermaid = manager.visualize_dependencies();
        assert!(mermaid.contains("graph TD"), "应该包含 Mermaid 图表类型");
        assert!(mermaid.contains("task1"), "应该包含 task1");
        assert!(mermaid.contains("task2"), "应该包含 task2");
        assert!(mermaid.contains("task3"), "应该包含 task3");
        assert!(mermaid.contains("-->"), "应该包含箭头");
    }

    #[test]
    fn test_ready_tasks_with_dependencies() {
        let mut manager = TaskManager::new();

        manager.add_task(create_task("task1", "Task 1", vec![]));
        manager.add_task(create_task("task2", "Task 2", vec!["task1"]));
        manager.add_task(create_task("task3", "Task 3", vec!["task2"]));

        // 初始状态只有 task1 是可执行的
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1, "应该只有一个可执行任务");
        assert_eq!(ready[0].id, "task1", "应该是 task1");

        // 标记 task1 完成
        manager.update_task("task1", TaskStatus::Completed);
        let ready = manager.get_ready_tasks();
        assert_eq!(ready.len(), 1, "应该只有一个可执行任务");
        assert_eq!(ready[0].id, "task2", "应该是 task2");
    }

    #[test]
    fn test_get_next_task_priority() {
        let mut manager = TaskManager::new();

        manager.add_task(Task {
            id: "task1".to_string(),
            description: "Low priority".to_string(),
            status: TaskStatus::Pending,
            dependencies: vec![],
            priority: 3,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        });

        manager.add_task(Task {
            id: "task2".to_string(),
            description: "High priority".to_string(),
            status: TaskStatus::Pending,
            dependencies: vec![],
            priority: 8,
            result: None,
            reasoning: None,
            parent_id: None,
            created_at: 0,
            updated_at: 0,
        });

        let next = manager.get_next_task();
        assert!(next.is_some(), "应该有下一个任务");
        assert_eq!(next.unwrap().id, "task2", "应该返回高优先级任务");
    }
}
