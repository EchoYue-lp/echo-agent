//! Plan validation logic
//!
//! This module provides validation logic for `PlanSpec` to ensure
//! plans are well-formed before creating the task DAG.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::planning::plan_spec::{PlanSpec, ValidationReport};
use crate::tasks::{RuntimeTask, RuntimeTaskSpec};

/// Plan validator configuration
#[derive(Debug, Clone)]
pub struct PlanValidator {
    /// Maximum number of tasks allowed in a plan
    pub max_tasks: usize,

    /// Maximum dependency depth allowed
    pub max_depth: usize,

    /// Whether to require acceptance criteria for all tasks
    pub require_acceptance_criteria: bool,

    /// Whether to require verification for all tasks
    pub require_verification: bool,

    /// Maximum automatic retries declared by one runtime task.
    pub max_retries: u32,
}

impl Default for PlanValidator {
    fn default() -> Self {
        Self {
            max_tasks: 100,
            max_depth: 10,
            require_acceptance_criteria: true,
            require_verification: true,
            max_retries: 10,
        }
    }
}

impl PlanValidator {
    /// Create a new plan validator
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate a plan specification
    pub fn validate(&self, plan: &PlanSpec) -> ValidationReport {
        let mut report = ValidationReport::new();

        // 1. Check task count
        if plan.tasks.len() > self.max_tasks {
            report.add_error(format!(
                "Plan contains {} tasks, maximum allowed is {}",
                plan.tasks.len(),
                self.max_tasks
            ));
        }

        if plan.tasks.is_empty() {
            report.add_error("Plan contains no tasks");
        }

        // 2. Check for duplicate task IDs
        let mut task_ids = std::collections::HashSet::new();
        for task in &plan.tasks {
            if !task_ids.insert(&task.id) {
                report.add_error(format!("Duplicate task ID: {}", task.id));
            }
        }

        // 3. Check all tasks have acceptance criteria (if required)
        if self.require_acceptance_criteria {
            for task in &plan.tasks {
                if task.acceptance_criteria.is_empty() {
                    report.add_error(format!("Task '{}' missing acceptance criteria", task.id));
                }
            }
        }

        // 4. Check all tasks have verification (if required)
        if self.require_verification {
            for task in &plan.tasks {
                if matches!(
                    task.verification.verification_type,
                    crate::tasks::VerificationType::None
                ) {
                    report.add_warning(format!("Task '{}' has no verification specified", task.id));
                }
            }
        }

        // 5. Validate dependencies exist
        for edge in &plan.edges {
            if !task_ids.contains(&edge.from) {
                report.add_error(format!(
                    "Dependency references non-existent task: {}",
                    edge.from
                ));
            }
            if !task_ids.contains(&edge.to) {
                report.add_error(format!(
                    "Dependency references non-existent task: {}",
                    edge.to
                ));
            }
        }

        // 6. Check for circular dependencies
        if let Err(msg) = plan.topological_order() {
            report.add_error(msg);
        }

        // 7. Check for parallel write conflicts
        let write_tasks: Vec<_> = plan
            .tasks
            .iter()
            .filter(|t| t.requires_write_access && t.can_parallelize)
            .collect();

        if write_tasks.len() > 1 {
            // Check if any write tasks are independent (no dependencies between them)
            let mut has_conflict = false;
            for i in 0..write_tasks.len() {
                for j in (i + 1)..write_tasks.len() {
                    let task_i = write_tasks[i];
                    let task_j = write_tasks[j];

                    // Check if they're independent (no dependency between them)
                    let has_dependency = plan.edges.iter().any(|e| {
                        (e.from == task_i.id && e.to == task_j.id)
                            || (e.from == task_j.id && e.to == task_i.id)
                    });

                    if !has_dependency {
                        has_conflict = true;
                        report.add_warning(format!(
                            "Parallel write tasks detected: '{}' and '{}' (consider isolating with worktree)",
                            task_i.id, task_j.id
                        ));
                    }
                }
            }

            if has_conflict {
                report.add_warning(
                    "Multiple independent write tasks can run in parallel. \
                     Consider setting can_parallelize=false or using worktree isolation.",
                );
            }
        }

        // 8. Validate milestone task IDs
        for milestone in &plan.milestones {
            for task_id in &milestone.task_ids {
                if !task_ids.contains(task_id) {
                    report.add_error(format!(
                        "Milestone '{}' references non-existent task: {}",
                        milestone.id, task_id
                    ));
                }
            }

            if milestone.task_ids.is_empty() {
                report.add_warning(format!(
                    "Milestone '{}' has no associated tasks",
                    milestone.id
                ));
            }
        }

        // 9. Check dependency depth
        if let Ok(order) = plan.topological_order() {
            let task_map: std::collections::HashMap<String, &crate::planning::TaskSpec> =
                plan.tasks.iter().map(|t| (t.id.clone(), t)).collect();

            // Build dependency map from edges
            let mut dep_map: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for edge in &plan.edges {
                dep_map
                    .entry(edge.from.clone())
                    .or_default()
                    .push(edge.to.clone());
            }

            let mut depths: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            for task_id in &order {
                if let Some(_task) = task_map.get(task_id) {
                    let deps = dep_map.get(task_id).cloned().unwrap_or_default();
                    let max_dep_depth = deps
                        .iter()
                        .filter_map(|dep_id| depths.get(dep_id))
                        .copied()
                        .max()
                        .unwrap_or(0);

                    let depth = max_dep_depth + 1;
                    depths.insert(task_id.clone(), depth);

                    if depth > self.max_depth {
                        report.add_error(format!(
                            "Dependency depth {} exceeds maximum {} for task '{}'",
                            depth, self.max_depth, task_id
                        ));
                    }
                }
            }
        }

        // 10. Validate task priorities
        for task in &plan.tasks {
            if task.priority > 10 {
                report.add_warning(format!(
                    "Task '{}' has priority {} > 10, clamping to 10",
                    task.id, task.priority
                ));
            }
        }

        // 11. Check goal is not empty
        if plan.goal.trim().is_empty() {
            report.add_error("Plan goal is empty");
        }

        // 12. Validate context budget
        if plan.context_budget == 0 {
            report.add_warning("Plan context budget is 0, using default");
        } else if plan.context_budget > 1_000_000 {
            report.add_warning(format!(
                "Plan context budget {} is very large (>1M tokens)",
                plan.context_budget
            ));
        }

        report
    }

    /// Validate one coherent revisioned runtime snapshot.
    ///
    /// Runtime validation deliberately does not apply the `PlanSpec`-specific
    /// acceptance/verification policy flags: applications may make those
    /// checks optional or enforce them through review policy. Structural DAG
    /// integrity remains framework-owned.
    pub fn validate_runtime_snapshot(
        &self,
        tasks: &[RuntimeTask],
    ) -> std::result::Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for task in tasks {
            if task.spec.id != task.execution.task_id {
                errors.push(format!(
                    "task spec id '{}' does not match execution id '{}'",
                    task.spec.id, task.execution.task_id
                ));
            }
        }
        if let Err(spec_errors) = self.validate_runtime_specs(
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

    /// Validate immutable runtime task specifications and their dependencies.
    pub fn validate_runtime_specs(
        &self,
        tasks: &[RuntimeTaskSpec],
    ) -> std::result::Result<(), Vec<String>> {
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
            if task.agent_role.trim().is_empty() {
                errors.push(format!(
                    "task '{}' Subagent role must not be empty",
                    task.id
                ));
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
            for tool in &task.allowed_tools {
                if tool.trim().is_empty() {
                    errors.push(format!("task '{}' contains an empty tool name", task.id));
                }
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

        let (processed, depths) = runtime_topological_depths(tasks, &known_ids);
        if processed < known_ids.len() {
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

fn runtime_topological_depths(
    tasks: &[RuntimeTaskSpec],
    known_ids: &HashSet<&str>,
) -> (usize, HashMap<String, usize>) {
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

    let mut queue: VecDeque<String> = indegree
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(task_id, _)| task_id.clone())
        .collect();
    let mut depths: HashMap<String, usize> =
        queue.iter().map(|task_id| (task_id.clone(), 1)).collect();
    let mut processed = 0usize;
    while let Some(task_id) = queue.pop_front() {
        processed = processed.saturating_add(1);
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
                        queue.push_back((*child).to_string());
                    }
                }
            }
        }
    }
    (processed, depths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan_spec::*;
    use crate::tasks::*;

    #[test]
    fn test_valid_plan() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(TaskSpec {
            id: "task1".to_string(),
            task_type: TaskType::Discovery,
            description: "First task".to_string(),
            acceptance_criteria: vec!["Criterion 1".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Low,
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        plan.add_task(TaskSpec {
            id: "task2".to_string(),
            task_type: TaskType::Implementation,
            description: "Second task".to_string(),
            acceptance_criteria: vec!["Criterion 2".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Medium,
            can_parallelize: false,
            requires_write_access: true,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        plan.add_dependency("task2", "task1", DependencyType::Required);

        let validator = PlanValidator::new();
        let report = validator.validate(&plan);

        assert!(
            report.is_valid(),
            "Plan should be valid: {:?}",
            report.errors
        );
    }

    #[test]
    fn test_empty_plan() {
        let plan = PlanSpec::new("Test goal");
        let validator = PlanValidator::new();
        let report = validator.validate(&plan);

        assert!(!report.is_valid());
        assert!(report.errors.iter().any(|e| e.contains("no tasks")));
    }

    #[test]
    fn test_duplicate_task_ids() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(TaskSpec {
            id: "task1".to_string(),
            task_type: TaskType::Discovery,
            description: "First task".to_string(),
            acceptance_criteria: vec!["Criterion 1".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Low,
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        plan.add_task(TaskSpec {
            id: "task1".to_string(), // Duplicate ID
            task_type: TaskType::Discovery,
            description: "Duplicate task".to_string(),
            acceptance_criteria: vec!["Criterion 2".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Low,
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        let validator = PlanValidator::new();
        let report = validator.validate(&plan);

        assert!(!report.is_valid());
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Duplicate task ID"))
        );
    }

    #[test]
    fn test_missing_acceptance_criteria() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(TaskSpec {
            id: "task1".to_string(),
            task_type: TaskType::Discovery,
            description: "First task".to_string(),
            acceptance_criteria: vec![], // Empty!
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Low,
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        let validator = PlanValidator::new();
        let report = validator.validate(&plan);

        assert!(!report.is_valid());
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("acceptance criteria"))
        );
    }

    #[test]
    fn test_circular_dependencies() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(TaskSpec {
            id: "task1".to_string(),
            task_type: TaskType::Discovery,
            description: "First task".to_string(),
            acceptance_criteria: vec!["Criterion 1".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Low,
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        plan.add_task(TaskSpec {
            id: "task2".to_string(),
            task_type: TaskType::Discovery,
            description: "Second task".to_string(),
            acceptance_criteria: vec!["Criterion 2".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Low,
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        // Create circular dependency
        plan.add_dependency("task1", "task2", DependencyType::Required);
        plan.add_dependency("task2", "task1", DependencyType::Required);

        let validator = PlanValidator::new();
        let report = validator.validate(&plan);

        assert!(!report.is_valid());
        assert!(report.errors.iter().any(|e| e.contains("circular")));
    }

    #[test]
    fn test_parallel_write_conflict() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(TaskSpec {
            id: "task1".to_string(),
            task_type: TaskType::Implementation,
            description: "Write task 1".to_string(),
            acceptance_criteria: vec!["Criterion 1".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::High,
            can_parallelize: true,       // Can parallelize
            requires_write_access: true, // Requires write
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        plan.add_task(TaskSpec {
            id: "task2".to_string(),
            task_type: TaskType::Implementation,
            description: "Write task 2".to_string(),
            acceptance_criteria: vec!["Criterion 2".to_string()],
            inputs: vec![],
            expected_outputs: vec![],
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::High,
            can_parallelize: true,       // Can parallelize
            requires_write_access: true, // Requires write
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: vec![],
            estimated_duration_secs: None,
        });

        // No dependency between them - they can run in parallel

        let validator = PlanValidator::new();
        let report = validator.validate(&plan);

        assert!(report.is_valid()); // Still valid, just warnings
        assert!(report.has_warnings());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("Parallel write tasks"))
        );
    }

    fn runtime_spec(id: &str, dependencies: &[&str]) -> RuntimeTaskSpec {
        RuntimeTaskSpec {
            id: id.to_string(),
            title: id.to_string(),
            description: format!("execute {id}"),
            kind: RuntimeTaskKind::Investigation,
            agent_role: "explorer".to_string(),
            depends_on: dependencies
                .iter()
                .map(|dependency| dependency.to_string())
                .collect(),
            files: Vec::new(),
            allowed_tools: Vec::new(),
            required_artifacts: Vec::new(),
            execution_checks: Vec::new(),
            acceptance_criteria: Vec::new(),
            max_retries: 3,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn runtime_validation_accepts_acyclic_specs() {
        let tasks = vec![runtime_spec("a", &[]), runtime_spec("b", &["a"])];
        assert!(
            PlanValidator::default()
                .validate_runtime_specs(&tasks)
                .is_ok()
        );
    }

    #[test]
    fn runtime_validation_rejects_dangling_dependencies_and_cycles() -> Result<(), String> {
        let dangling = PlanValidator::default()
            .validate_runtime_specs(&[runtime_spec("a", &["missing"])])
            .err()
            .ok_or_else(|| "dangling dependency unexpectedly passed validation".to_string())?;
        assert!(
            dangling
                .iter()
                .any(|error| error.contains("does not exist"))
        );

        let cycle = PlanValidator::default()
            .validate_runtime_specs(&[runtime_spec("a", &["b"]), runtime_spec("b", &["a"])])
            .err()
            .ok_or_else(|| "dependency cycle unexpectedly passed validation".to_string())?;
        assert!(cycle.iter().any(|error| error.contains("cycle")));
        Ok(())
    }

    #[test]
    fn runtime_validation_rejects_mismatched_execution_identity() -> Result<(), String> {
        let task = RuntimeTask {
            spec: runtime_spec("spec-id", &[]),
            execution: RuntimeTaskExecution {
                task_id: "execution-id".to_string(),
                status: RuntimeTaskStatus::Pending,
                retry_count: 0,
                failure_fingerprint: None,
            },
        };
        let errors = PlanValidator::default()
            .validate_runtime_snapshot(&[task])
            .err()
            .ok_or_else(|| "mismatched task identity unexpectedly passed validation".to_string())?;
        assert!(errors.iter().any(|error| error.contains("does not match")));
        Ok(())
    }
}
