//! DAG dependency analysis (cycle detection, topological sort, dependency chain query, Mermaid visualization)

use super::manager::TaskManager;
use std::collections::HashMap;

impl TaskManager {
    /// Detect cyclic dependencies, return all cycle paths
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited: HashMap<String, super::manager::VisitState> = HashMap::new();
        let mut path: Vec<String> = Vec::new();

        let task_ids: Vec<String> = self.tasks.iter().map(|r| r.key().clone()).collect();
        for task_id in task_ids {
            if visited.get(&task_id) != Some(&super::manager::VisitState::Visited) {
                self.dfs_detect_cycle(&task_id, &mut visited, &mut path, &mut cycles);
            }
        }

        cycles
    }

    /// Get topological sort (returns error if cyclic dependencies exist)
    pub fn get_topological_order(&self) -> Result<Vec<String>, String> {
        let specs = self
            .get_all_tasks()
            .iter()
            .map(super::Task::runtime_spec)
            .collect::<Vec<_>>();
        crate::planning::validator::runtime_topological_order(&specs)
    }

    /// Generate a visualization of the dependency graph (Mermaid format)
    pub fn visualize_dependencies(&self) -> String {
        let mut mermaid = String::from("graph TD\n");

        for entry in self.tasks.iter() {
            let task_id = entry.key();
            let task = entry.value();
            for dep_id in &task.dependencies {
                mermaid.push_str(&format!(
                    "  {}[{}] --> {}[{}]\n",
                    dep_id, dep_id, task_id, task_id
                ));
            }
        }

        mermaid
    }

    /// Get dependency chain (from the specified task to the root node)
    pub fn get_dependency_chain(&self, task_id: &str) -> Vec<Vec<String>> {
        let mut chains = Vec::new();
        let mut current_chain = Vec::new();
        self.get_dependency_chain_recursive(task_id, &mut current_chain, &mut chains);
        chains
    }
}
