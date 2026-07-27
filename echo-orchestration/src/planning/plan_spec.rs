//! Plan specification for structured task planning
//!
//! This module defines the `PlanSpec` structure that allows LLMs to output
//! complete plans, which are then validated by the framework before creating
//! the task DAG.

use crate::tasks::{
    CheckpointPolicy, ContextScope, OutputType, RiskLevel, RuntimeTask, RuntimeTaskExecution,
    RuntimeTaskKind, RuntimeTaskSpec, Task, TaskInput, TaskOutput, TaskType, VerificationSpec,
    VerificationType,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const DEFAULT_AGENT_ROLE: &str = "default";

/// Complete plan specification output by LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSpec {
    /// Overall goal of the plan
    pub goal: String,

    /// Assumptions made during planning
    #[serde(default)]
    pub assumptions: Vec<String>,

    /// List of tasks in the plan
    pub tasks: Vec<TaskSpec>,

    /// Dependency edges (from_task_id -> to_task_id)
    #[serde(default)]
    pub edges: Vec<Dependency>,

    /// Milestones for tracking progress
    #[serde(default)]
    pub milestones: Vec<Milestone>,

    /// Overall verification strategy
    #[serde(default)]
    pub verification_strategy: PlanVerificationStrategy,

    /// Estimated complexity level
    #[serde(default)]
    pub estimated_complexity: Complexity,

    /// Context budget for the plan
    #[serde(default = "default_context_budget")]
    pub context_budget: usize,

    /// Fallback strategy if plan fails
    #[serde(default)]
    pub fallback_strategy: PlanFallbackStrategy,
}

fn default_context_budget() -> usize {
    100_000
}

/// Task specification within a plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Unique task identifier
    pub id: String,

    /// Task type classification
    #[serde(default)]
    pub task_type: TaskType,

    /// Detailed task description
    pub description: String,

    /// Acceptance criteria - conditions that must be met
    pub acceptance_criteria: Vec<String>,

    /// Input specifications
    #[serde(default)]
    pub inputs: Vec<TaskInput>,

    /// Expected output specifications
    #[serde(default)]
    pub expected_outputs: Vec<TaskOutput>,

    /// Allowed tools for this task
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,

    /// Context scope for task execution
    #[serde(default)]
    pub context_scope: ContextScope,

    /// Risk level classification
    #[serde(default)]
    pub risk_level: RiskLevel,

    /// Whether this task can be parallelized
    #[serde(default = "default_true")]
    pub can_parallelize: bool,

    /// Whether this task requires write access
    #[serde(default)]
    pub requires_write_access: bool,

    /// Verification specification
    #[serde(default)]
    pub verification: VerificationSpec,

    /// Checkpoint policy
    #[serde(default)]
    pub checkpoint_policy: CheckpointPolicy,

    /// Task priority (0-10)
    #[serde(default = "default_priority")]
    pub priority: u8,

    /// Assigned agent (optional)
    #[serde(default)]
    pub assigned_agent: Option<String>,

    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Estimated duration in seconds
    #[serde(default)]
    pub estimated_duration_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

fn default_priority() -> u8 {
    5
}

impl TaskSpec {
    /// Convert TaskSpec to Task
    pub fn to_task(&self) -> Task {
        let mut task = Task::new(&self.id, &self.description)
            .with_task_type(self.task_type.clone())
            .with_acceptance_criteria(self.acceptance_criteria.clone())
            .with_inputs(self.inputs.clone())
            .with_expected_outputs(self.expected_outputs.clone())
            .with_context_scope(self.context_scope.clone())
            .with_risk_level(self.risk_level.clone())
            .with_can_parallelize(self.can_parallelize)
            .with_requires_write_access(self.requires_write_access)
            .with_verification(self.verification.clone())
            .with_checkpoint_policy(self.checkpoint_policy.clone())
            .with_priority(self.priority);

        if let Some(ref tools) = self.allowed_tools {
            task = task.with_allowed_tools(tools.clone());
        }

        if let Some(ref agent) = self.assigned_agent {
            task = task.with_assigned_agent(agent.clone());
        }

        if !self.tags.is_empty() {
            task = task.with_tags(self.tags.clone());
        }

        task
    }

    /// Compile this authoring specification into the canonical runtime shape.
    ///
    /// Rich authoring-only policy stays available in `metadata`; the runtime
    /// fields contain only semantics used by the generic DAG executor.
    pub fn to_runtime_spec(&self, dependencies: Vec<String>) -> Result<RuntimeTaskSpec, String> {
        let authoring_metadata = serde_json::to_value(self)
            .map_err(|error| format!("failed to serialize task '{}': {error}", self.id))?;
        let required_artifacts = self
            .expected_outputs
            .iter()
            .filter(|output| matches!(output.output_type, OutputType::File | OutputType::Artifact))
            .map(|output| {
                if output.target.trim().is_empty() {
                    output.name.clone()
                } else {
                    output.target.clone()
                }
            })
            .collect();
        let execution_checks = if matches!(
            self.verification.verification_type,
            VerificationType::Command | VerificationType::Test
        ) {
            self.verification.command.iter().cloned().collect()
        } else {
            Vec::new()
        };

        Ok(RuntimeTaskSpec {
            id: self.id.clone(),
            title: self.description.clone(),
            description: self.description.clone(),
            kind: runtime_kind_for_authoring_task(self),
            agent_role: self
                .assigned_agent
                .clone()
                .unwrap_or_else(|| DEFAULT_AGENT_ROLE.to_string()),
            depends_on: dependencies,
            files: Vec::new(),
            allowed_tools: self.allowed_tools.clone().unwrap_or_default(),
            required_artifacts,
            execution_checks,
            acceptance_criteria: self.acceptance_criteria.clone(),
            max_retries: 0,
            metadata: serde_json::json!({ "authoring_task_spec": authoring_metadata }),
        })
    }
}

fn runtime_kind_for_authoring_task(task: &TaskSpec) -> RuntimeTaskKind {
    match task.task_type {
        TaskType::Discovery => RuntimeTaskKind::Investigation,
        TaskType::Implementation => RuntimeTaskKind::Implementation,
        TaskType::Verification => RuntimeTaskKind::Verification,
        TaskType::Background | TaskType::Delegation if task.requires_write_access => {
            RuntimeTaskKind::Implementation
        }
        TaskType::Background | TaskType::Delegation => RuntimeTaskKind::Investigation,
    }
}

/// Dependency edge between tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Source task ID (depends on)
    pub from: String,

    /// Target task ID (depended upon)
    pub to: String,

    /// Dependency type
    pub dependency_type: DependencyType,
}

/// Type of dependency relationship
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyType {
    /// Hard dependency: must complete before starting
    Required,
    /// Soft dependency: prefer to complete before starting
    Preferred,
    /// Optional dependency: use result if available
    Optional,
}

/// Milestone for tracking plan progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    /// Unique milestone identifier
    pub id: String,

    /// Milestone name
    pub name: String,

    /// Milestone description
    pub description: String,

    /// Success criteria for this milestone
    pub success_criteria: Vec<String>,

    /// Task IDs associated with this milestone
    pub task_ids: Vec<String>,

    /// Checkpoint policy for this milestone
    pub checkpoint_policy: CheckpointPolicy,
}

/// Plan-level verification strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PlanVerificationStrategy {
    /// Verify each task independently
    #[default]
    PerTask,
    /// Verify at each milestone
    PerMilestone,
    /// Only verify at the end
    FinalOnly,
    /// Continuous verification throughout
    Continuous,
}

/// Estimated complexity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Complexity {
    Low,
    #[default]
    Medium,
    High,
}

/// Fallback strategy if plan fails
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PlanFallbackStrategy {
    /// Replan with new strategy
    #[default]
    Replan,
    /// Ask user for guidance
    AskUser,
    /// Abort execution
    Abort,
}

impl PlanSpec {
    /// Create a new empty plan
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            assumptions: Vec::new(),
            tasks: Vec::new(),
            edges: Vec::new(),
            milestones: Vec::new(),
            verification_strategy: PlanVerificationStrategy::default(),
            estimated_complexity: Complexity::default(),
            context_budget: 100_000, // Default 100k tokens
            fallback_strategy: PlanFallbackStrategy::default(),
        }
    }

    /// Add a task to the plan
    pub fn add_task(&mut self, task: TaskSpec) {
        self.tasks.push(task);
    }

    /// Add a dependency edge
    pub fn add_dependency(
        &mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        dep_type: DependencyType,
    ) {
        self.edges.push(Dependency {
            from: from.into(),
            to: to.into(),
            dependency_type: dep_type,
        });
    }

    /// Add a milestone
    pub fn add_milestone(&mut self, milestone: Milestone) {
        self.milestones.push(milestone);
    }

    /// Compile immutable runtime task specifications.
    ///
    /// Only required edges block execution. Preferred and optional edges stay
    /// authoring policy and never create a second hidden ready-frontier rule.
    pub fn to_runtime_specs(&self) -> Result<Vec<RuntimeTaskSpec>, String> {
        let task_ids: HashSet<&str> = self.tasks.iter().map(|task| task.id.as_str()).collect();
        let mut dependencies: HashMap<&str, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            if !task_ids.contains(edge.from.as_str()) {
                return Err(format!(
                    "Dependency references non-existent task: {}",
                    edge.from
                ));
            }
            if !task_ids.contains(edge.to.as_str()) {
                return Err(format!(
                    "Dependency references non-existent task: {}",
                    edge.to
                ));
            }
            if matches!(edge.dependency_type, DependencyType::Required) {
                dependencies
                    .entry(edge.from.as_str())
                    .or_default()
                    .push(edge.to.clone());
            }
        }

        self.tasks
            .iter()
            .map(|task| {
                let mut task_dependencies =
                    dependencies.remove(task.id.as_str()).unwrap_or_default();
                task_dependencies.sort();
                task_dependencies.dedup();
                task.to_runtime_spec(task_dependencies)
            })
            .collect()
    }

    /// Compile a pending runtime snapshot for the generic DAG executor.
    pub fn to_runtime_tasks(&self) -> Result<Vec<RuntimeTask>, String> {
        self.to_runtime_specs().map(|specs| {
            specs
                .into_iter()
                .map(|spec| RuntimeTask {
                    execution: RuntimeTaskExecution::pending(spec.id.clone()),
                    spec,
                })
                .collect()
        })
    }

    /// Convert PlanSpec to rich task records with canonical dependencies set.
    pub fn to_tasks(&self) -> Result<Vec<Task>, String> {
        let runtime_specs = self.to_runtime_specs()?;
        let dependencies: HashMap<&str, &[String]> = runtime_specs
            .iter()
            .map(|spec| (spec.id.as_str(), spec.depends_on.as_slice()))
            .collect();
        Ok(self
            .tasks
            .iter()
            .map(|spec| {
                let task_dependencies = dependencies
                    .get(spec.id.as_str())
                    .map(|items| items.to_vec())
                    .unwrap_or_default();
                spec.to_task().with_dependencies(task_dependencies)
            })
            .collect())
    }

    /// Get task IDs in topological order
    pub fn topological_order(&self) -> Result<Vec<String>, String> {
        let specs = self.to_runtime_specs()?;
        super::validator::runtime_topological_order(&specs)
    }
}

/// Validation report for plan validation
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn add_error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    pub fn add_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

impl Default for ValidationReport {
    fn default() -> Self {
        Self::new()
    }
}
