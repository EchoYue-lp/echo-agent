use crate::tasks::TaskManager;
use std::collections::HashMap;

impl TaskManager {
    /// 检测循环依赖，返回所有循环路径
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited: HashMap<String, crate::tasks::manager::VisitState> = HashMap::new();
        let mut path: Vec<String> = Vec::new();

        for task_id in self.tasks.keys() {
            if visited.get(task_id) != Some(&crate::tasks::manager::VisitState::Visited) {
                self.dfs_detect_cycle(task_id, &mut visited, &mut path, &mut cycles);
            }
        }

        cycles
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
}
