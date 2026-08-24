//! Generic task-runtime primitives.
//!
//! These types are deliberately product-neutral. Application layers own their
//! concrete persistence, approval gates, UI projections, and event protocols.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use echo_core::error::Result;
pub use echo_core::tools::NestedDelegationPolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tokio_util::sync::CancellationToken;

/// Stable task identifier used by runtime DAG primitives.
pub type TaskId = String;

/// Product-neutral disposition for a requested runtime interruption.
///
/// The framework owns how claims and unfinished tasks are settled. An
/// application only decides whether its current stop request is a terminal
/// cancellation or a resumable pause, and supplies the pause reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RuntimeInterruptionDisposition {
    /// Stop the run permanently and cancel every unfinished task.
    #[default]
    Cancelled,
    /// Stop dispatching while retaining unstarted tasks for an explicit resume.
    Paused { reason: String },
}

/// Generic lifecycle state shared by task specifications and managed records.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Pending,
    Running,
    Blocked(String),
    Completed,
    Failed(String),
    Skipped,
    Cancelled,
    TimedOut {
        error: String,
    },
    Retrying {
        attempt: u32,
        last_error: String,
    },
    Paused(String),
}

impl TaskStatus {
    /// Whether this state has no further automatic work.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed(_)
                | Self::TimedOut { .. }
                | Self::Skipped
                | Self::Cancelled
        )
    }

    /// Whether this task is currently occupying execution capacity.
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running | Self::Retrying { .. })
    }

    /// Whether transitioning from this state to `target` is valid.
    pub fn can_transition_to(&self, target: &Self) -> bool {
        match self {
            Self::Pending => matches!(
                target,
                Self::Running
                    | Self::Cancelled
                    | Self::Blocked(_)
                    | Self::Skipped
                    | Self::Paused(_)
            ),
            Self::Running => matches!(
                target,
                Self::Completed
                    | Self::Cancelled
                    | Self::Failed(_)
                    | Self::TimedOut { .. }
                    | Self::Retrying { .. }
                    | Self::Blocked(_)
                    | Self::Skipped
                    | Self::Paused(_)
            ),
            Self::Retrying { .. } => matches!(
                target,
                Self::Pending
                    | Self::Completed
                    | Self::Cancelled
                    | Self::Failed(_)
                    | Self::TimedOut { .. }
                    | Self::Retrying { .. }
                    | Self::Paused(_)
            ),
            Self::Blocked(_) => matches!(target, Self::Pending | Self::Cancelled),
            Self::Paused(_) => matches!(target, Self::Pending | Self::Cancelled),
            Self::Completed
            | Self::Failed(_)
            | Self::TimedOut { .. }
            | Self::Skipped
            | Self::Cancelled => false,
        }
    }

    /// Validate and return a requested state transition.
    pub fn transition_to(&self, target: Self) -> std::result::Result<Self, String> {
        if !self.can_transition_to(&target) {
            return Err(format!(
                "Invalid task state transition: {self:?} -> {target:?}"
            ));
        }
        Ok(target)
    }
}

/// Immutable, product-neutral task specification for runtime DAG scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub depends_on: Vec<TaskId>,
    pub max_retries: u32,
    /// Product-owned data that does not participate in framework scheduling.
    #[serde(default)]
    pub extension: serde_json::Value,
}

impl TaskSpec {
    /// Stable SHA-256 identity for one immutable task specification.
    pub fn stable_hash(&self) -> std::result::Result<String, String> {
        let bytes = serde_json::to_vec(self).map_err(|error| {
            format!(
                "failed to serialize task '{}' for hashing: {error}",
                self.id
            )
        })?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// Durable lease for one concrete task dispatch attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClaim {
    /// Unique identity for this physical lease, including reclaim of the same
    /// logical revision and retry attempt after a crashed executor.
    pub claim_id: String,
    pub revision: u64,
    pub attempt: u32,
    pub spec_hash: String,
}

impl TaskClaim {
    pub fn new(revision: u64, attempt: u32, spec_hash: String) -> Self {
        Self {
            claim_id: uuid::Uuid::new_v4().to_string(),
            revision,
            attempt,
            spec_hash,
        }
    }

    /// Globally unique execution identity. The run namespace prevents the same
    /// plan task/revision/attempt in separate TaskRuns from sharing lifecycle.
    pub fn execution_id(&self, run_id: &str, task_id: &str) -> String {
        format!(
            "{run_id}:{task_id}:{}:{}:{}",
            self.revision, self.attempt, self.claim_id
        )
    }
}

/// Mutable execution state for one runtime task specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskExecution {
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub retry_count: u32,
    pub failure_fingerprint: Option<String>,
    #[serde(default)]
    pub claim: Option<TaskClaim>,
}

impl TaskExecution {
    pub fn pending(task_id: impl Into<TaskId>) -> Self {
        Self {
            task_id: task_id.into(),
            status: TaskStatus::Pending,
            retry_count: 0,
            failure_fingerprint: None,
            claim: None,
        }
    }
}

/// One runtime DAG node: immutable specification plus mutable execution state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub spec: TaskSpec,
    pub execution: TaskExecution,
}

/// Product-neutral context passed to a task Subagent.
#[derive(Debug, Clone)]
pub struct TaskSubagentContext {
    pub run_id: String,
    pub cancel: CancellationToken,
    pub delegation_policy: NestedDelegationPolicy,
    /// Dependencies explicitly waived by a Skipped lifecycle rather than a
    /// reusable successful output.
    pub waived_dependency_ids: Vec<TaskId>,
}

impl TaskSubagentContext {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            cancel: CancellationToken::new(),
            delegation_policy: NestedDelegationPolicy::default(),
            waived_dependency_ids: Vec::new(),
        }
    }

    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_delegation_policy(mut self, policy: NestedDelegationPolicy) -> Self {
        self.delegation_policy = policy;
        self
    }

    pub fn with_waived_dependencies(mut self, dependency_ids: Vec<TaskId>) -> Self {
        self.waived_dependency_ids = dependency_ids;
        self
    }

    pub fn child_delegation_context(&self) -> Option<Self> {
        self.delegation_policy
            .child_policy()
            .map(|delegation_policy| Self {
                run_id: self.run_id.clone(),
                cancel: self.cancel.child_token(),
                delegation_policy,
                waived_dependency_ids: self.waived_dependency_ids.clone(),
            })
    }
}

/// A bounded follow-up task proposed by a Subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedTask {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub dependencies: Vec<TaskId>,
    pub why_needed: String,
    pub risk: String,
    #[serde(default)]
    pub extension: serde_json::Value,
}

/// Compact per-task execution summary produced at task boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskExecutionSummary {
    pub run_id: String,
    pub task_id: TaskId,
    pub subagent_name: String,
    pub completed_work: Vec<String>,
    pub decisions: Vec<String>,
    pub failures: Vec<String>,
    pub verification: Vec<String>,
    pub next_implications: Vec<String>,
    #[serde(default)]
    pub suggested_tasks: Vec<SuggestedTask>,
    /// Product-owned evidence, artifacts, touched resources, or UI metadata.
    #[serde(default)]
    pub extension: serde_json::Value,
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
        task: Task,
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
    pub paused: HashSet<TaskId>,
}

/// Derived dependency state for one task in a committed graph snapshot.
///
/// This is a projection, never a persisted task lifecycle. In particular,
/// [`BlockedByFailure`](Self::BlockedByFailure) disappears automatically when
/// its failed ancestor is retried in a newer snapshot. Applications may still
/// persist [`TaskStatus::Blocked`] for product-owned review or input policy,
/// but dependency traversal must not encode this projection as a status string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagDependencyState {
    /// Every dependency completed successfully or was explicitly waived.
    Satisfied {
        /// Dependencies explicitly waived because they were Skipped.
        waived_dependency_ids: Vec<TaskId>,
    },
    /// No dependency failed, but at least one has not completed yet.
    Waiting {
        unresolved_dependency_ids: Vec<TaskId>,
    },
    /// One or more transitive ancestors failed or timed out.
    BlockedByFailure { failed_ancestor_ids: Vec<TaskId> },
}

impl DagExecutionState {
    /// Build state from a snapshot. Already-completed tasks are treated as
    /// resolved; already-running tasks are treated as externally in-flight.
    pub fn from_tasks(tasks: &[Task]) -> Self {
        let completed = tasks
            .iter()
            .filter(|task| task.execution.status == TaskStatus::Completed)
            .map(|task| task.spec.id.clone())
            .collect();
        let in_flight = tasks
            .iter()
            .filter(|task| task.execution.status.is_running())
            .map(|task| task.spec.id.clone())
            .collect();
        let failed = tasks
            .iter()
            .filter(|task| {
                matches!(
                    task.execution.status,
                    TaskStatus::Failed(_) | TaskStatus::TimedOut { .. }
                )
            })
            .map(|task| task.spec.id.clone())
            .collect();
        let skipped = tasks
            .iter()
            .filter(|task| task.execution.status == TaskStatus::Skipped)
            .map(|task| task.spec.id.clone())
            .collect();
        let cancelled = tasks
            .iter()
            .filter(|task| task.execution.status == TaskStatus::Cancelled)
            .map(|task| task.spec.id.clone())
            .collect();
        let paused = tasks
            .iter()
            .filter(|task| matches!(task.execution.status, TaskStatus::Paused(_)))
            .map(|task| task.spec.id.clone())
            .collect();

        Self {
            completed,
            in_flight,
            failed,
            skipped,
            cancelled,
            paused,
        }
    }

    /// Refresh externally in-flight tasks from a newer task snapshot.
    pub fn refresh_in_flight(&mut self, tasks: &[Task]) -> DagRefresh {
        let mut refresh = DagRefresh::default();
        if self.in_flight.is_empty() {
            return refresh;
        }

        for task_id in self.in_flight.clone() {
            let Some(task) = tasks.iter().find(|task| task.spec.id == task_id) else {
                continue;
            };

            match &task.execution.status {
                TaskStatus::Completed => {
                    self.in_flight.remove(&task_id);
                    self.completed.insert(task_id.clone());
                    refresh.completed.push(task_id);
                }
                TaskStatus::Failed(_) | TaskStatus::TimedOut { .. } => {
                    self.in_flight.remove(&task_id);
                    self.failed.insert(task_id.clone());
                    refresh.failed.push(task_id);
                }
                TaskStatus::Skipped | TaskStatus::Cancelled => {
                    self.in_flight.remove(&task_id);
                    if task.execution.status == TaskStatus::Skipped {
                        self.skipped.insert(task_id.clone());
                    } else {
                        self.cancelled.insert(task_id.clone());
                    }
                    refresh.terminal_non_success.push(task_id);
                }
                TaskStatus::Pending
                | TaskStatus::Running
                | TaskStatus::Retrying { .. }
                | TaskStatus::Blocked(_)
                | TaskStatus::Paused(_) => {}
            }
        }

        refresh
    }

    /// Task ids that are ready to dispatch in the next wave.
    pub fn ready_task_ids(&self, tasks: &[Task]) -> Vec<TaskId> {
        let dependencies = self.dependency_states(tasks);
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
                task.execution.status == TaskStatus::Pending
                    && matches!(
                        dependencies.get(&task.spec.id),
                        Some(DagDependencyState::Satisfied { .. })
                    )
            })
            .map(|task| task.spec.id.clone())
            .collect()
    }

    /// Derive dependency state for every task without mutating the snapshot.
    pub fn dependency_states(&self, tasks: &[Task]) -> HashMap<TaskId, DagDependencyState> {
        let mut failed_ancestors: HashMap<TaskId, HashSet<TaskId>> = self
            .failed
            .iter()
            .map(|task_id| (task_id.clone(), HashSet::from([task_id.clone()])))
            .collect();
        loop {
            let mut changed = false;
            for task in tasks {
                if self.completed.contains(&task.spec.id)
                    || self.skipped.contains(&task.spec.id)
                    || self.cancelled.contains(&task.spec.id)
                    || self.failed.contains(&task.spec.id)
                {
                    continue;
                }
                let mut inherited = failed_ancestors
                    .get(&task.spec.id)
                    .cloned()
                    .unwrap_or_default();
                for dependency in &task.spec.depends_on {
                    if let Some(ancestors) = failed_ancestors.get(dependency) {
                        inherited.extend(ancestors.iter().cloned());
                    }
                }
                if inherited.is_empty() {
                    continue;
                }
                let entry = failed_ancestors.entry(task.spec.id.clone()).or_default();
                let before = entry.len();
                entry.extend(inherited);
                if entry.len() != before {
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        tasks
            .iter()
            .map(|task| {
                let state = if !self.failed.contains(&task.spec.id) {
                    match failed_ancestors.get(&task.spec.id) {
                        Some(ancestors) if !ancestors.is_empty() => {
                            let mut failed_ancestor_ids: Vec<_> =
                                ancestors.iter().cloned().collect();
                            failed_ancestor_ids.sort();
                            DagDependencyState::BlockedByFailure {
                                failed_ancestor_ids,
                            }
                        }
                        _ => {
                            let mut unresolved_dependency_ids: Vec<_> = task
                                .spec
                                .depends_on
                                .iter()
                                .filter(|dependency| {
                                    !self.completed.contains(*dependency)
                                        && !self.skipped.contains(*dependency)
                                })
                                .cloned()
                                .collect();
                            unresolved_dependency_ids.sort();
                            if unresolved_dependency_ids.is_empty() {
                                let mut waived_dependency_ids: Vec<_> = task
                                    .spec
                                    .depends_on
                                    .iter()
                                    .filter(|dependency| self.skipped.contains(*dependency))
                                    .cloned()
                                    .collect();
                                waived_dependency_ids.sort();
                                DagDependencyState::Satisfied {
                                    waived_dependency_ids,
                                }
                            } else {
                                DagDependencyState::Waiting {
                                    unresolved_dependency_ids,
                                }
                            }
                        }
                    }
                } else {
                    let mut unresolved_dependency_ids: Vec<_> = task
                        .spec
                        .depends_on
                        .iter()
                        .filter(|dependency| {
                            !self.completed.contains(*dependency)
                                && !self.skipped.contains(*dependency)
                        })
                        .cloned()
                        .collect();
                    unresolved_dependency_ids.sort();
                    if unresolved_dependency_ids.is_empty() {
                        let mut waived_dependency_ids: Vec<_> = task
                            .spec
                            .depends_on
                            .iter()
                            .filter(|dependency| self.skipped.contains(*dependency))
                            .cloned()
                            .collect();
                        waived_dependency_ids.sort();
                        DagDependencyState::Satisfied {
                            waived_dependency_ids,
                        }
                    } else {
                        DagDependencyState::Waiting {
                            unresolved_dependency_ids,
                        }
                    }
                };
                (task.spec.id.clone(), state)
            })
            .collect()
    }

    /// Whether every task has completed or was deliberately skipped.
    pub fn all_completed(&self, tasks: &[Task]) -> bool {
        self.completed.len().saturating_add(self.skipped.len()) == tasks.len()
    }

    /// Whether every unfinished task is either failed or blocked by a failed
    /// dependency.
    pub fn all_unfinished_failed_or_blocked(&self, tasks: &[Task]) -> bool {
        let dependencies = self.dependency_states(tasks);
        tasks.iter().all(|task| {
            self.completed.contains(&task.spec.id)
                || self.skipped.contains(&task.spec.id)
                || self.cancelled.contains(&task.spec.id)
                || self.failed.contains(&task.spec.id)
                || matches!(
                    dependencies.get(&task.spec.id),
                    Some(DagDependencyState::BlockedByFailure { .. })
                )
                || matches!(task.execution.status, TaskStatus::Blocked(_))
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
        DagDependencyState, NestedDelegationPolicy, TaskExecution, TaskExecutionSummary, TaskSpec,
        TaskStatus, TaskSubagentContext,
    };
    use crate::tasks::runtime::{DagExecutionState, Task};

    #[test]
    fn terminal_statuses_are_classified() {
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed("boom".to_string()).is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Running.is_running());
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
        let context =
            TaskSubagentContext::new("run-1").with_delegation_policy(NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 0,
                max_delegate_depth: 1,
            });

        let child = context.child_delegation_context();

        assert!(child.is_some());
        let child = child.unwrap_or_else(|| TaskSubagentContext::new("missing"));
        assert_eq!(child.run_id, "run-1");
        assert_eq!(child.delegation_policy.delegate_depth, 1);
        assert!(!child.delegation_policy.can_delegate());
    }

    #[test]
    fn task_execution_summary_extension_round_trips_product_evidence() -> Result<(), String> {
        let summary = TaskExecutionSummary {
            run_id: "run-1".to_string(),
            task_id: "task-1".to_string(),
            subagent_name: "researcher".to_string(),
            completed_work: vec!["inspected runtime".to_string()],
            decisions: Vec::new(),
            failures: Vec::new(),
            verification: vec!["tests passed".to_string()],
            next_implications: Vec::new(),
            suggested_tasks: Vec::new(),
            extension: serde_json::json!({
                "touched_files": {
                    "read": ["src/lib.rs"],
                    "written": ["src/tasks.rs"]
                }
            }),
            created_at: chrono::Utc::now(),
        };

        let encoded = serde_json::to_value(&summary).map_err(|error| error.to_string())?;
        let decoded: TaskExecutionSummary =
            serde_json::from_value(encoded).map_err(|error| error.to_string())?;

        assert_eq!(decoded, summary);
        Ok(())
    }

    fn runtime_task(id: &str, status: TaskStatus, deps: &[&str]) -> Task {
        Task {
            spec: TaskSpec {
                id: id.to_string(),
                title: id.to_string(),
                description: format!("execute {id}"),
                depends_on: deps.iter().map(|dep| dep.to_string()).collect(),
                max_retries: 3,
                extension: serde_json::json!({ "role": "explorer" }),
            },
            execution: TaskExecution {
                task_id: id.to_string(),
                status,
                retry_count: 0,
                failure_fingerprint: None,
                claim: None,
            },
        }
    }

    #[test]
    fn task_spec_hash_changes_with_dispatch_contract() -> Result<(), String> {
        let original = runtime_task("task", TaskStatus::Pending, &[]).spec;
        let mut changed = original.clone();
        changed.description = "changed contract".to_string();

        assert_eq!(original.stable_hash()?, original.stable_hash()?);
        assert_ne!(original.stable_hash()?, changed.stable_hash()?);
        Ok(())
    }

    #[test]
    fn dag_state_initializes_from_snapshot() {
        let tasks = vec![
            runtime_task("done", TaskStatus::Completed, &[]),
            runtime_task("running", TaskStatus::Running, &[]),
            runtime_task("failed", TaskStatus::Failed("boom".to_string()), &[]),
        ];

        let state = DagExecutionState::from_tasks(&tasks);

        assert!(state.completed.contains("done"));
        assert!(state.in_flight.contains("running"));
        assert!(state.failed.contains("failed"));
    }

    #[test]
    fn dag_ready_frontier_requires_completed_dependencies() {
        let tasks = vec![
            runtime_task("a", TaskStatus::Completed, &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
            runtime_task("c", TaskStatus::Pending, &["b"]),
        ];
        let state = DagExecutionState::from_tasks(&tasks);

        assert_eq!(state.ready_task_ids(&tasks), vec!["b".to_string()]);
        assert_eq!(
            state.dependency_states(&tasks).get("b"),
            Some(&DagDependencyState::Satisfied {
                waived_dependency_ids: Vec::new(),
            })
        );
    }

    #[test]
    fn skipped_dependency_is_resolved_for_ready_frontier() {
        let tasks = vec![
            runtime_task("a", TaskStatus::Skipped, &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
        ];
        let state = DagExecutionState::from_tasks(&tasks);

        assert_eq!(state.ready_task_ids(&tasks), vec!["b".to_string()]);
        assert_eq!(
            state.dependency_states(&tasks).get("b"),
            Some(&DagDependencyState::Satisfied {
                waived_dependency_ids: vec!["a".to_string()],
            })
        );
    }

    #[test]
    fn dag_refresh_observes_external_in_flight_completion() {
        let original = vec![runtime_task("a", TaskStatus::Running, &[])];
        let updated = vec![runtime_task("a", TaskStatus::Completed, &[])];
        let mut state = DagExecutionState::from_tasks(&original);

        let refresh = state.refresh_in_flight(&updated);

        assert_eq!(refresh.completed, vec!["a".to_string()]);
        assert!(state.completed.contains("a"));
        assert!(!state.in_flight.contains("a"));
    }

    #[test]
    fn dag_detects_failed_downstream_blockers() {
        let tasks = vec![
            runtime_task("a", TaskStatus::Failed("boom".to_string()), &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
        ];
        let state = DagExecutionState::from_tasks(&tasks);

        assert_eq!(
            state.dependency_states(&tasks).get("b"),
            Some(&DagDependencyState::BlockedByFailure {
                failed_ancestor_ids: vec!["a".to_string()],
            })
        );
        assert!(state.all_unfinished_failed_or_blocked(&tasks));
    }

    #[test]
    fn dag_failure_blocking_is_transitive() -> Result<(), String> {
        let mut tasks = vec![
            runtime_task("a", TaskStatus::Failed("boom".to_string()), &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
            runtime_task("c", TaskStatus::Pending, &["b"]),
            runtime_task("d", TaskStatus::Pending, &["c"]),
        ];
        let state = DagExecutionState::from_tasks(&tasks);

        let dependencies = state.dependency_states(&tasks);
        for task_id in ["b", "c", "d"] {
            assert_eq!(
                dependencies.get(task_id),
                Some(&DagDependencyState::BlockedByFailure {
                    failed_ancestor_ids: vec!["a".to_string()],
                })
            );
        }
        assert!(state.all_unfinished_failed_or_blocked(&tasks));

        let upstream = tasks
            .iter_mut()
            .find(|task| task.spec.id == "a")
            .ok_or_else(|| "upstream fixture is missing".to_string())?;
        upstream.execution.status = TaskStatus::Pending;
        let retried_state = DagExecutionState::from_tasks(&tasks);
        assert!(
            retried_state
                .dependency_states(&tasks)
                .values()
                .all(|state| !matches!(state, DagDependencyState::BlockedByFailure { .. }))
        );
        Ok(())
    }
}
