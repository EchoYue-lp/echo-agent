//! Plan validation logic
//!
//! This module provides validation logic for `PlanSpec` to ensure
//! plans are well-formed before creating the task DAG.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::planning::plan_spec::{PlanSpec, ValidationReport};
use crate::tasks::{Task, TaskSpec};

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

        // Structural identity, dependency, cycle, depth, and retry checks use
        // the same immutable runtime specs consumed by the DAG executor.
        match plan.to_task_specs() {
            Ok(specs) => {
                if let Err(errors) = self.validate_task_specs(&specs) {
                    for error in errors {
                        report.add_error(error);
                    }
                }
            }
            Err(error) => report.add_error(error),
        }

        let task_ids: HashSet<&str> = plan.tasks.iter().map(|task| task.id.as_str()).collect();

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

        // Check for parallel write conflicts.
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
                    let has_dependency = plan.edges.iter().any(|edge| {
                        matches!(
                            &edge.dependency_type,
                            crate::planning::DependencyType::Required
                        ) && ((edge.from == task_i.id && edge.to == task_j.id)
                            || (edge.from == task_j.id && edge.to == task_i.id))
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

        // Validate milestone task IDs.
        for milestone in &plan.milestones {
            for task_id in &milestone.task_ids {
                if !task_ids.contains(task_id.as_str()) {
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

        // Validate task priorities.
        for task in &plan.tasks {
            if task.priority > 10 {
                report.add_warning(format!(
                    "Task '{}' has priority {} > 10, clamping to 10",
                    task.id, task.priority
                ));
            }
        }

        // Check goal is not empty.
        if plan.goal.trim().is_empty() {
            report.add_error("Plan goal is empty");
        }

        // Validate context budget.
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
    pub fn validate_task_snapshot(&self, tasks: &[Task]) -> std::result::Result<(), Vec<String>> {
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

    /// Validate immutable runtime task specifications and their dependencies.
    pub fn validate_task_specs(&self, tasks: &[TaskSpec]) -> std::result::Result<(), Vec<String>> {
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

        let (order, depths) = task_topology(tasks, &known_ids);
        if order.len() < known_ids.len() {
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

fn task_topology(
    tasks: &[TaskSpec],
    known_ids: &HashSet<&str>,
) -> (Vec<String>, HashMap<String, usize>) {
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
    let mut order = Vec::with_capacity(tasks.len());
    while let Some(Reverse(task_id)) = queue.pop() {
        order.push(task_id.clone());
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
    (order, depths)
}

pub(crate) fn task_topological_order(
    tasks: &[TaskSpec],
) -> std::result::Result<Vec<String>, String> {
    let known_ids: HashSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    if tasks.len() != known_ids.len() {
        return Err("Plan contains duplicate task IDs".to_string());
    }
    for task in tasks {
        for dependency in &task.depends_on {
            if dependency == &task.id {
                return Err(format!("Task '{}' cannot depend on itself", task.id));
            }
            if !known_ids.contains(dependency.as_str()) {
                return Err(format!(
                    "Task '{}' depends on '{}' which does not exist",
                    task.id, dependency
                ));
            }
        }
    }

    let (order, _) = task_topology(tasks, &known_ids);
    if order.len() != tasks.len() {
        Err("Plan contains circular dependencies".to_string())
    } else {
        Ok(order)
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

        plan.add_task(PlanTaskSpec {
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

        plan.add_task(PlanTaskSpec {
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
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("at least one task"))
        );
    }

    #[test]
    fn test_duplicate_task_ids() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(PlanTaskSpec {
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

        plan.add_task(PlanTaskSpec {
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
                .any(|error| error.contains("duplicate task id"))
        );
    }

    #[test]
    fn test_missing_acceptance_criteria() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(PlanTaskSpec {
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

        plan.add_task(PlanTaskSpec {
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

        plan.add_task(PlanTaskSpec {
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
        assert!(report.errors.iter().any(|error| error.contains("cycle")));
    }

    #[test]
    fn test_parallel_write_conflict() {
        let mut plan = PlanSpec::new("Test goal");

        plan.add_task(PlanTaskSpec {
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

        plan.add_task(PlanTaskSpec {
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

    fn runtime_spec(id: &str, dependencies: &[&str]) -> TaskSpec {
        TaskSpec {
            id: id.to_string(),
            title: id.to_string(),
            description: format!("execute {id}"),
            kind: TaskKind::Investigation,
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

    fn authoring_task(id: &str) -> PlanTaskSpec {
        PlanTaskSpec {
            id: id.to_string(),
            task_type: TaskType::Implementation,
            description: format!("execute {id}"),
            acceptance_criteria: vec![format!("{id} is complete")],
            inputs: Vec::new(),
            expected_outputs: Vec::new(),
            allowed_tools: None,
            context_scope: ContextScope::Relevant,
            risk_level: RiskLevel::Medium,
            can_parallelize: true,
            requires_write_access: true,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::OnFailure,
            priority: 5,
            assigned_agent: None,
            tags: Vec::new(),
            estimated_duration_secs: None,
        }
    }

    #[test]
    fn authoring_plan_compiles_to_canonical_runtime_specs() -> Result<(), String> {
        let mut first = authoring_task("first");
        first.expected_outputs.push(TaskOutput {
            name: "report".to_string(),
            output_type: OutputType::File,
            target: "reports/result.md".to_string(),
            validation: None,
        });
        first.verification = VerificationSpec {
            verification_type: VerificationType::Command,
            command: Some("cargo test".to_string()),
            ..VerificationSpec::default()
        };

        let mut plan = PlanSpec::new("canonical compilation");
        plan.add_task(first);
        plan.add_task(authoring_task("second"));
        plan.add_task(authoring_task("optional"));
        plan.add_dependency("second", "first", DependencyType::Required);
        plan.add_dependency("optional", "first", DependencyType::Optional);

        let specs = plan.to_task_specs()?;
        PlanValidator::default()
            .validate_task_specs(&specs)
            .map_err(|errors| errors.join("; "))?;
        let first = specs
            .iter()
            .find(|task| task.id == "first")
            .ok_or_else(|| "compiled first task is missing".to_string())?;
        let second = specs
            .iter()
            .find(|task| task.id == "second")
            .ok_or_else(|| "compiled second task is missing".to_string())?;
        let optional = specs
            .iter()
            .find(|task| task.id == "optional")
            .ok_or_else(|| "compiled optional task is missing".to_string())?;

        assert_eq!(second.depends_on, vec!["first"]);
        assert!(optional.depends_on.is_empty());
        assert_eq!(first.required_artifacts, vec!["reports/result.md"]);
        assert_eq!(first.execution_checks, vec!["cargo test"]);
        assert_eq!(first.acceptance_criteria, vec!["first is complete"]);
        assert!(first.metadata.get("authoring_task_spec").is_some());

        let order = plan.topological_order()?;
        let first_position = order
            .iter()
            .position(|task_id| task_id == "first")
            .ok_or_else(|| "first task missing from topological order".to_string())?;
        let second_position = order
            .iter()
            .position(|task_id| task_id == "second")
            .ok_or_else(|| "second task missing from topological order".to_string())?;
        assert!(first_position < second_position);
        Ok(())
    }

    #[test]
    fn authoring_plan_rejects_dangling_non_blocking_edges() {
        let mut plan = PlanSpec::new("dangling optional edge");
        plan.add_task(authoring_task("task"));
        plan.add_dependency("task", "missing", DependencyType::Optional);

        let report = PlanValidator::default().validate(&plan);

        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("non-existent task: missing"))
        );
    }

    #[test]
    fn runtime_validation_accepts_acyclic_specs() {
        let tasks = vec![runtime_spec("a", &[]), runtime_spec("b", &["a"])];
        assert!(PlanValidator::default().validate_task_specs(&tasks).is_ok());
    }

    #[test]
    fn runtime_validation_rejects_dangling_dependencies_and_cycles() -> Result<(), String> {
        let dangling = PlanValidator::default()
            .validate_task_specs(&[runtime_spec("a", &["missing"])])
            .err()
            .ok_or_else(|| "dangling dependency unexpectedly passed validation".to_string())?;
        assert!(
            dangling
                .iter()
                .any(|error| error.contains("does not exist"))
        );

        let cycle = PlanValidator::default()
            .validate_task_specs(&[runtime_spec("a", &["b"]), runtime_spec("b", &["a"])])
            .err()
            .ok_or_else(|| "dependency cycle unexpectedly passed validation".to_string())?;
        assert!(cycle.iter().any(|error| error.contains("cycle")));
        Ok(())
    }

    #[test]
    fn runtime_validation_rejects_mismatched_execution_identity() -> Result<(), String> {
        let task = Task {
            spec: runtime_spec("spec-id", &[]),
            execution: TaskExecution {
                task_id: "execution-id".to_string(),
                status: TaskStatus::Pending,
                retry_count: 0,
                failure_fingerprint: None,
                claim: None,
            },
        };
        let errors = PlanValidator::default()
            .validate_task_snapshot(&[task])
            .err()
            .ok_or_else(|| "mismatched task identity unexpectedly passed validation".to_string())?;
        assert!(errors.iter().any(|error| error.contains("does not match")));
        Ok(())
    }
}
