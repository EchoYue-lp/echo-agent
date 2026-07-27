//! Generic task-runtime primitives.
//!
//! These types are deliberately product-neutral. Application layers own their
//! concrete persistence, approval gates, UI projections, and event protocols.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use echo_core::error::Result;
pub use echo_core::tools::NestedDelegationPolicy;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use tokio_util::sync::CancellationToken;

/// Stable task identifier used by runtime DAG primitives.
pub type TaskId = String;

/// Operation class for a runtime task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTaskKind {
    /// Read-only repository / data exploration or review.
    ReadOnlyReview,
    /// Read-only search, grep, file reads, hypothesis investigation.
    Investigation,
    /// Read-only verification plan.
    TestPlan,
    /// Scoped code or data change.
    Implementation,
    /// Focused root-cause investigation.
    Debugging,
    /// Spec / quality review of another task's output.
    Review,
    /// Final synthesis / report.
    Summary,
    /// Shell / build / test execution.
    Verification,
}

impl RuntimeTaskKind {
    /// `true` when the task kind does not mutate workspace state.
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::ReadOnlyReview
                | Self::Investigation
                | Self::TestPlan
                | Self::Review
                | Self::Summary
        )
    }

    /// Stable snake_case representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnlyReview => "read_only_review",
            Self::Investigation => "investigation",
            Self::TestPlan => "test_plan",
            Self::Implementation => "implementation",
            Self::Debugging => "debugging",
            Self::Review => "review",
            Self::Summary => "summary",
            Self::Verification => "verification",
        }
    }
}

impl FromStr for RuntimeTaskKind {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "read_only_review" => Ok(Self::ReadOnlyReview),
            "investigation" => Ok(Self::Investigation),
            "test_plan" => Ok(Self::TestPlan),
            "implementation" => Ok(Self::Implementation),
            "debugging" => Ok(Self::Debugging),
            "review" => Ok(Self::Review),
            "summary" => Ok(Self::Summary),
            "verification" => Ok(Self::Verification),
            other => Err(format!("unknown runtime task kind: {other}")),
        }
    }
}

/// Generic lifecycle state for a task node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTaskStatus {
    #[default]
    Pending,
    Running,
    Blocked,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl RuntimeTaskStatus {
    /// Whether this state has no further automatic work.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }

    /// Whether this task is currently occupying execution capacity.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

/// Immutable, product-neutral task specification for runtime DAG scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTaskSpec {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub kind: RuntimeTaskKind,
    pub agent_role: String,
    pub depends_on: Vec<TaskId>,
    pub files: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub required_artifacts: Vec<String>,
    pub execution_checks: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub max_retries: u32,
    /// Product metadata that does not participate in framework scheduling.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Mutable execution state for one runtime task specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTaskExecution {
    pub task_id: TaskId,
    pub status: RuntimeTaskStatus,
    pub retry_count: u32,
    pub failure_fingerprint: Option<String>,
}

impl RuntimeTaskExecution {
    pub fn pending(task_id: impl Into<TaskId>) -> Self {
        Self {
            task_id: task_id.into(),
            status: RuntimeTaskStatus::Pending,
            retry_count: 0,
            failure_fingerprint: None,
        }
    }
}

/// One runtime DAG node: immutable specification plus mutable execution state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTask {
    pub spec: RuntimeTaskSpec,
    pub execution: RuntimeTaskExecution,
}

/// Concurrency caps for a generic task runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcurrencyLimits {
    /// Max simultaneous Subagents.
    pub max_concurrent_subagents: usize,
    /// Max simultaneous mutating tasks.
    pub max_concurrent_writes: usize,
    /// Max simultaneous shell/verification tasks.
    pub max_concurrent_shells: usize,
    /// Max simultaneous LLM calls across Subagents.
    pub max_parallel_llm_calls: usize,
}

impl Default for ConcurrencyLimits {
    fn default() -> Self {
        Self {
            max_concurrent_subagents: 4,
            max_concurrent_writes: 4,
            max_concurrent_shells: 1,
            max_parallel_llm_calls: 4,
        }
    }
}

/// Product-neutral context passed to a task Subagent.
#[derive(Debug, Clone)]
pub struct TaskSubagentContext {
    pub run_id: String,
    pub cancel: CancellationToken,
    pub concurrency_limits: ConcurrencyLimits,
    pub delegation_policy: NestedDelegationPolicy,
}

impl TaskSubagentContext {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            cancel: CancellationToken::new(),
            concurrency_limits: ConcurrencyLimits::default(),
            delegation_policy: NestedDelegationPolicy::default(),
        }
    }

    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_concurrency_limits(mut self, limits: ConcurrencyLimits) -> Self {
        self.concurrency_limits = limits;
        self
    }

    pub fn with_delegation_policy(mut self, policy: NestedDelegationPolicy) -> Self {
        self.delegation_policy = policy;
        self
    }

    pub fn child_delegation_context(&self) -> Option<Self> {
        self.delegation_policy
            .child_policy()
            .map(|delegation_policy| Self {
                run_id: self.run_id.clone(),
                cancel: self.cancel.child_token(),
                concurrency_limits: self.concurrency_limits,
                delegation_policy,
            })
    }
}

/// A bounded follow-up task proposed by a Subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedTask {
    pub title: String,
    pub description: String,
    pub kind: RuntimeTaskKind,
    pub agent_role: String,
    #[serde(default)]
    pub dependencies: Vec<TaskId>,
    pub why_needed: String,
    pub risk: String,
}

/// Compact per-task execution summary produced at task boundaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionSummary {
    pub run_id: String,
    pub task_id: TaskId,
    pub subagent_name: String,
    pub completed_work: Vec<String>,
    pub files_read: Vec<String>,
    pub files_changed: Vec<String>,
    pub decisions: Vec<String>,
    pub failures: Vec<String>,
    pub verification: Vec<String>,
    pub next_implications: Vec<String>,
    #[serde(default)]
    pub suggested_tasks: Vec<SuggestedTask>,
    #[serde(with = "echo_core::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
}

/// Product-neutral Subagent contract.
///
/// The framework defines the contract; products decide which concrete Subagent
/// implementation, tools, storage, and UI event mapping to use.
#[async_trait]
pub trait TaskSubagent: Send + Sync {
    async fn execute(
        &self,
        context: TaskSubagentContext,
        task: RuntimeTask,
        dependency_summaries: Vec<TaskExecutionSummary>,
    ) -> Result<TaskExecutionSummary>;
}

/// Pure DAG bookkeeping for task-runtime executors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DagExecutionState {
    pub completed: HashSet<TaskId>,
    pub in_flight: HashSet<TaskId>,
    pub failed: HashSet<TaskId>,
    pub skipped: HashSet<TaskId>,
    pub cancelled: HashSet<TaskId>,
}

impl DagExecutionState {
    /// Build state from a snapshot. Already-completed tasks are treated as
    /// resolved; already-running tasks are treated as externally in-flight.
    pub fn from_tasks(tasks: &[RuntimeTask]) -> Self {
        let completed = tasks
            .iter()
            .filter(|task| task.execution.status == RuntimeTaskStatus::Completed)
            .map(|task| task.spec.id.clone())
            .collect();
        let in_flight = tasks
            .iter()
            .filter(|task| task.execution.status == RuntimeTaskStatus::Running)
            .map(|task| task.spec.id.clone())
            .collect();
        let failed = tasks
            .iter()
            .filter(|task| task.execution.status == RuntimeTaskStatus::Failed)
            .map(|task| task.spec.id.clone())
            .collect();
        let skipped = tasks
            .iter()
            .filter(|task| task.execution.status == RuntimeTaskStatus::Skipped)
            .map(|task| task.spec.id.clone())
            .collect();
        let cancelled = tasks
            .iter()
            .filter(|task| task.execution.status == RuntimeTaskStatus::Cancelled)
            .map(|task| task.spec.id.clone())
            .collect();

        Self {
            completed,
            in_flight,
            failed,
            skipped,
            cancelled,
        }
    }

    /// Refresh externally in-flight tasks from a newer task snapshot.
    pub fn refresh_in_flight(&mut self, tasks: &[RuntimeTask]) -> DagRefresh {
        let mut refresh = DagRefresh::default();
        if self.in_flight.is_empty() {
            return refresh;
        }

        for task_id in self.in_flight.clone() {
            let Some(task) = tasks.iter().find(|task| task.spec.id == task_id) else {
                continue;
            };

            match task.execution.status {
                RuntimeTaskStatus::Completed => {
                    self.in_flight.remove(&task_id);
                    self.completed.insert(task_id.clone());
                    refresh.completed.push(task_id);
                }
                RuntimeTaskStatus::Failed => {
                    self.in_flight.remove(&task_id);
                    self.failed.insert(task_id.clone());
                    refresh.failed.push(task_id);
                }
                RuntimeTaskStatus::Skipped | RuntimeTaskStatus::Cancelled => {
                    self.in_flight.remove(&task_id);
                    if task.execution.status == RuntimeTaskStatus::Skipped {
                        self.skipped.insert(task_id.clone());
                    } else {
                        self.cancelled.insert(task_id.clone());
                    }
                    refresh.terminal_non_success.push(task_id);
                }
                RuntimeTaskStatus::Pending
                | RuntimeTaskStatus::Running
                | RuntimeTaskStatus::Blocked => {}
            }
        }

        refresh
    }

    /// Task ids that are ready to dispatch in the next wave.
    pub fn ready_task_ids(&self, tasks: &[RuntimeTask]) -> Vec<TaskId> {
        tasks
            .iter()
            .filter(|task| {
                !self.completed.contains(&task.spec.id)
                    && !self.in_flight.contains(&task.spec.id)
                    && !self.failed.contains(&task.spec.id)
                    && !self.skipped.contains(&task.spec.id)
                    && !self.cancelled.contains(&task.spec.id)
            })
            .filter(|task| {
                task.execution.status == RuntimeTaskStatus::Pending
                    && task
                        .spec
                        .depends_on
                        .iter()
                        .all(|dep| self.completed.contains(dep))
            })
            .map(|task| task.spec.id.clone())
            .collect()
    }

    /// Downstream task ids blocked by failed dependencies.
    pub fn blocked_by_failures(&self, tasks: &[RuntimeTask]) -> Vec<TaskId> {
        tasks
            .iter()
            .filter(|task| {
                !self.completed.contains(&task.spec.id)
                    && !self.failed.contains(&task.spec.id)
                    && !self.skipped.contains(&task.spec.id)
                    && !self.cancelled.contains(&task.spec.id)
            })
            .filter(|task| {
                task.spec
                    .depends_on
                    .iter()
                    .any(|dep| self.failed.contains(dep))
            })
            .map(|task| task.spec.id.clone())
            .collect()
    }

    /// Whether every task has completed or was deliberately skipped.
    pub fn all_completed(&self, tasks: &[RuntimeTask]) -> bool {
        self.completed.len().saturating_add(self.skipped.len()) == tasks.len()
    }

    /// Whether every unfinished task is either failed or blocked by a failed
    /// dependency.
    pub fn all_unfinished_failed_or_blocked(&self, tasks: &[RuntimeTask]) -> bool {
        tasks.iter().all(|task| {
            self.completed.contains(&task.spec.id)
                || self.skipped.contains(&task.spec.id)
                || self.cancelled.contains(&task.spec.id)
                || self.failed.contains(&task.spec.id)
                || task
                    .spec
                    .depends_on
                    .iter()
                    .any(|dep| self.failed.contains(dep))
        })
    }
}

/// Result of refreshing in-flight tasks from a newer snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DagRefresh {
    pub completed: Vec<TaskId>,
    pub failed: Vec<TaskId>,
    pub terminal_non_success: Vec<TaskId>,
}

#[cfg(test)]
mod tests {
    use super::{
        ConcurrencyLimits, NestedDelegationPolicy, RuntimeTaskExecution, RuntimeTaskKind,
        RuntimeTaskSpec, RuntimeTaskStatus, TaskSubagentContext,
    };
    use crate::tasks::runtime::{DagExecutionState, RuntimeTask};
    use std::str::FromStr;

    #[test]
    fn runtime_task_kind_round_trips_snake_case() {
        let kind = RuntimeTaskKind::from_str("implementation").unwrap_or(RuntimeTaskKind::Summary);

        assert_eq!(kind, RuntimeTaskKind::Implementation);
        assert_eq!(kind.as_str(), "implementation");
        assert!(RuntimeTaskKind::from_str("not_real").is_err());
    }

    #[test]
    fn read_only_kinds_are_classified() {
        assert!(RuntimeTaskKind::Investigation.is_read_only());
        assert!(!RuntimeTaskKind::Implementation.is_read_only());
        assert!(!RuntimeTaskKind::Verification.is_read_only());
    }

    #[test]
    fn terminal_statuses_are_classified() {
        assert!(RuntimeTaskStatus::Completed.is_terminal());
        assert!(RuntimeTaskStatus::Failed.is_terminal());
        assert!(!RuntimeTaskStatus::Running.is_terminal());
        assert!(RuntimeTaskStatus::Running.is_running());
    }

    #[test]
    fn delegation_policy_advances_depth() {
        let policy = NestedDelegationPolicy {
            can_spawn_subagents: true,
            delegate_depth: 1,
            max_delegate_depth: 2,
        };

        let child = policy.child_policy();
        assert!(child.is_some());
        let child = child.unwrap_or_default();
        assert_eq!(child.delegate_depth, 2);
        assert!(!child.can_delegate());
    }

    #[test]
    fn task_subagent_context_builds_child_delegation_context() {
        let context = TaskSubagentContext::new("run-1")
            .with_concurrency_limits(ConcurrencyLimits {
                max_concurrent_subagents: 2,
                max_concurrent_writes: 1,
                max_concurrent_shells: 1,
                max_parallel_llm_calls: 2,
            })
            .with_delegation_policy(NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 0,
                max_delegate_depth: 1,
            });

        let child = context.child_delegation_context();

        assert!(child.is_some());
        let child = child.unwrap_or_else(|| TaskSubagentContext::new("missing"));
        assert_eq!(child.run_id, "run-1");
        assert_eq!(child.delegation_policy.delegate_depth, 1);
        assert_eq!(child.concurrency_limits.max_concurrent_subagents, 2);
        assert!(!child.delegation_policy.can_delegate());
    }

    fn runtime_task(id: &str, status: RuntimeTaskStatus, deps: &[&str]) -> RuntimeTask {
        RuntimeTask {
            spec: RuntimeTaskSpec {
                id: id.to_string(),
                title: id.to_string(),
                description: format!("execute {id}"),
                kind: RuntimeTaskKind::Investigation,
                agent_role: "explorer".to_string(),
                depends_on: deps.iter().map(|dep| dep.to_string()).collect(),
                files: Vec::new(),
                allowed_tools: Vec::new(),
                required_artifacts: Vec::new(),
                execution_checks: Vec::new(),
                acceptance_criteria: Vec::new(),
                max_retries: 3,
                metadata: serde_json::Value::Null,
            },
            execution: RuntimeTaskExecution {
                task_id: id.to_string(),
                status,
                retry_count: 0,
                failure_fingerprint: None,
            },
        }
    }

    #[test]
    fn dag_state_initializes_from_snapshot() {
        let tasks = vec![
            runtime_task("done", RuntimeTaskStatus::Completed, &[]),
            runtime_task("running", RuntimeTaskStatus::Running, &[]),
            runtime_task("failed", RuntimeTaskStatus::Failed, &[]),
        ];

        let state = DagExecutionState::from_tasks(&tasks);

        assert!(state.completed.contains("done"));
        assert!(state.in_flight.contains("running"));
        assert!(state.failed.contains("failed"));
    }

    #[test]
    fn dag_ready_frontier_requires_completed_dependencies() {
        let tasks = vec![
            runtime_task("a", RuntimeTaskStatus::Completed, &[]),
            runtime_task("b", RuntimeTaskStatus::Pending, &["a"]),
            runtime_task("c", RuntimeTaskStatus::Pending, &["b"]),
        ];
        let state = DagExecutionState::from_tasks(&tasks);

        assert_eq!(state.ready_task_ids(&tasks), vec!["b".to_string()]);
    }

    #[test]
    fn dag_refresh_observes_external_in_flight_completion() {
        let original = vec![runtime_task("a", RuntimeTaskStatus::Running, &[])];
        let updated = vec![runtime_task("a", RuntimeTaskStatus::Completed, &[])];
        let mut state = DagExecutionState::from_tasks(&original);

        let refresh = state.refresh_in_flight(&updated);

        assert_eq!(refresh.completed, vec!["a".to_string()]);
        assert!(state.completed.contains("a"));
        assert!(!state.in_flight.contains("a"));
    }

    #[test]
    fn dag_detects_failed_downstream_blockers() {
        let tasks = vec![
            runtime_task("a", RuntimeTaskStatus::Failed, &[]),
            runtime_task("b", RuntimeTaskStatus::Pending, &["a"]),
        ];
        let state = DagExecutionState::from_tasks(&tasks);

        assert_eq!(state.blocked_by_failures(&tasks), vec!["b".to_string()]);
        assert!(state.all_unfinished_failed_or_blocked(&tasks));
    }
}
