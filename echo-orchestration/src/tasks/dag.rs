//! DAG dependency analysis (cycle detection, topological sort, dependency chain query, Mermaid visualization)

use super::manager::TaskManager;

impl TaskManager {
    /// Detect cyclic dependencies through the canonical plan validator.
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        let specs = self
            .get_all_tasks()
            .iter()
            .map(super::ManagedTask::task_spec)
            .collect::<Vec<_>>();
        crate::planning::validator::task_dependency_cycles(&specs)
    }

    /// Get topological sort (returns error if cyclic dependencies exist)
    pub fn get_topological_order(&self) -> Result<Vec<String>, String> {
        let specs = self
            .get_all_tasks()
            .iter()
            .map(super::ManagedTask::task_spec)
            .collect::<Vec<_>>();
        crate::planning::validator::task_topological_order(&specs)
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
