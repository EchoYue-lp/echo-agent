//! Structural validation for the canonical revisioned task graph.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::tasks::{Task, TaskSpec};

/// Product-neutral bounds enforced for every committed task graph.
#[derive(Debug, Clone)]
pub struct PlanValidator {
    pub max_tasks: usize,
    pub max_depth: usize,
    pub max_retries: u32,
}

impl Default for PlanValidator {
    fn default() -> Self {
        Self {
            max_tasks: 100,
            max_depth: 10,
            max_retries: 10,
        }
    }
}

impl PlanValidator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate one coherent revisioned runtime snapshot.
    pub fn validate_task_snapshot(&self, tasks: &[Task]) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for task in tasks {
            if task.spec.id != task.execution.task_id {
                errors.push(format!(
                    "task spec id '{}' does not match execution id '{}'",
                    task.spec.id, task.execution.task_id
                ));
            }
        }
        if let Err(spec_errors) = self.validate_task_specs(
            &tasks
                .iter()
                .map(|task| task.spec.clone())
                .collect::<Vec<_>>(),
        ) {
            errors.extend(spec_errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate immutable task identities, dependency topology, and retry bounds.
    pub fn validate_task_specs(&self, tasks: &[TaskSpec]) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if tasks.is_empty() {
            errors.push("plan must contain at least one task".to_string());
        }
        if tasks.len() > self.max_tasks {
            errors.push(format!(
                "plan contains {} tasks, maximum allowed is {}",
                tasks.len(),
                self.max_tasks
            ));
        }

        let mut ids = HashSet::new();
        for task in tasks {
            let id = task.id.trim();
            if id.is_empty() {
                errors.push("task id must not be empty".to_string());
            } else if !ids.insert(id.to_string()) {
                errors.push(format!("duplicate task id '{id}'"));
            }
            if task.title.trim().is_empty() {
                errors.push(format!("task '{}' title must not be empty", task.id));
            }
            if task.description.trim().is_empty() {
                errors.push(format!("task '{}' description must not be empty", task.id));
            }
            if task.max_retries > self.max_retries {
                errors.push(format!(
                    "task '{}' max_retries {} exceeds the runtime limit {}",
                    task.id, task.max_retries, self.max_retries
                ));
            }
            if task
                .depends_on
                .iter()
                .any(|dependency| dependency == &task.id)
            {
                errors.push(format!("task '{}' cannot depend on itself", task.id));
            }
        }

        let known_ids: HashSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
        for task in tasks {
            for dependency in &task.depends_on {
                if !known_ids.contains(dependency.as_str()) {
                    errors.push(format!(
                        "task '{}' depends on '{}' which does not exist",
                        task.id, dependency
                    ));
                }
            }
        }

        let (visited, depths) = task_topology(tasks, &known_ids);
        if visited != known_ids.len() {
            errors.push("dependency graph contains a cycle".to_string());
        }
        for (task_id, depth) in depths {
            if depth > self.max_depth {
                errors.push(format!(
                    "dependency depth {depth} exceeds maximum {} for task '{task_id}'",
                    self.max_depth
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn task_topology(tasks: &[TaskSpec], known_ids: &HashSet<&str>) -> (usize, HashMap<String, usize>) {
    let mut indegree: HashMap<String, usize> =
        tasks.iter().map(|task| (task.id.clone(), 0)).collect();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for task in tasks {
        for dependency in &task.depends_on {
            if dependency != &task.id && known_ids.contains(dependency.as_str()) {
                if let Some(count) = indegree.get_mut(&task.id) {
                    *count = count.saturating_add(1);
                }
                dependents
                    .entry(dependency.as_str())
                    .or_default()
                    .push(task.id.as_str());
            }
        }
    }

    let mut queue: BinaryHeap<Reverse<String>> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(task_id, _)| Reverse(task_id.clone()))
        .collect();
    let mut depths: HashMap<String, usize> =
        queue.iter().map(|task_id| (task_id.0.clone(), 1)).collect();
    let mut visited = 0usize;
    while let Some(Reverse(task_id)) = queue.pop() {
        visited = visited.saturating_add(1);
        let current_depth = depths.get(&task_id).copied().unwrap_or(1);
        if let Some(children) = dependents.get(task_id.as_str()) {
            for child in children {
                let next_depth = current_depth.saturating_add(1);
                depths
                    .entry((*child).to_string())
                    .and_modify(|depth| *depth = (*depth).max(next_depth))
                    .or_insert(next_depth);
                if let Some(count) = indegree.get_mut(*child) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        queue.push(Reverse((*child).to_string()));
                    }
                }
            }
        }
    }
    (visited, depths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{TaskExecution, TaskStatus};

    fn spec(id: &str, depends_on: &[&str]) -> TaskSpec {
        TaskSpec {
            id: id.to_string(),
            title: format!("Task {id}"),
            description: format!("Do {id}"),
            depends_on: depends_on
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            max_retries: 3,
            extension: serde_json::json!({ "product": "test" }),
        }
    }

    #[test]
    fn accepts_product_neutral_dag() {
        let specs = vec![spec("a", &[]), spec("b", &["a"])];
        assert!(PlanValidator::default().validate_task_specs(&specs).is_ok());
    }

    #[test]
    fn rejects_cycles_and_unknown_dependencies() {
        let cyclic = vec![spec("a", &["b"]), spec("b", &["a"])];
        let errors = PlanValidator::default()
            .validate_task_specs(&cyclic)
            .err()
            .unwrap_or_default();
        assert!(errors.iter().any(|error| error.contains("cycle")));

        let unknown = vec![spec("a", &["missing"])];
        let errors = PlanValidator::default()
            .validate_task_specs(&unknown)
            .err()
            .unwrap_or_default();
        assert!(errors.iter().any(|error| error.contains("does not exist")));
    }

    #[test]
    fn rejects_spec_execution_identity_mismatch() {
        let task = Task {
            spec: spec("spec-id", &[]),
            execution: TaskExecution {
                task_id: "execution-id".to_string(),
                status: TaskStatus::Pending,
                retry_count: 0,
                failure_fingerprint: None,
                claim: None,
            },
        };
        assert!(
            PlanValidator::default()
                .validate_task_snapshot(&[task])
                .is_err()
        );
    }
}
