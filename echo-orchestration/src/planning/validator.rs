//! Plan validation logic
//!
//! This module provides validation logic for `PlanSpec` to ensure
//! plans are well-formed before creating the task DAG.

use crate::planning::plan_spec::{PlanSpec, ValidationReport};

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
}

impl Default for PlanValidator {
    fn default() -> Self {
        Self {
            max_tasks: 100,
            max_depth: 10,
            require_acceptance_criteria: true,
            require_verification: true,
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
}
