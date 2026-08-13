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
    pub fn get_dependency_chain(&self, task_id: &str) -> Result<Vec<Vec<String>>, String> {
        if self.get_task(task_id).is_none() {
            return Err(format!("task '{task_id}' does not exist"));
        }
        let cycles = self.detect_circular_dependencies();
        if !cycles.is_empty() {
            return Err(format!("circular dependencies detected: {cycles:?}"));
        }
        let mut chains = Vec::new();
        let mut stack = vec![(task_id.to_string(), vec![task_id.to_string()])];
        while let Some((current, chain)) = stack.pop() {
            let task = self
                .get_task(&current)
                .ok_or_else(|| format!("dependency task '{current}' does not exist"))?;
            if task.dependencies.is_empty() {
                chains.push(chain);
                continue;
            }
            for dependency in task.dependencies.iter().rev() {
                let mut dependency_chain = chain.clone();
                dependency_chain.push(dependency.clone());
                stack.push((dependency.clone(), dependency_chain));
            }
        }
        Ok(chains)
    }
}
