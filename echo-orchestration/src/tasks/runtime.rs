//! Generic task-runtime primitives.
//!
//! These types are deliberately product-neutral. Application layers own their
//! concrete persistence, approval gates, UI projections, and event protocols.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use echo_core::error::Result;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

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

/// Product-neutral task node view for DAG scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeTask {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub kind: RuntimeTaskKind,
    pub agent_role: String,
    pub depends_on: Vec<TaskId>,
    pub files: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub verification: Vec<String>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub status: RuntimeTaskStatus,
}

/// Concurrency caps for a generic task runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcurrencyLimits {
    /// Max simultaneous worker agents.
    pub max_concurrent_workers: usize,
    /// Max simultaneous mutating tasks.
    pub max_concurrent_writes: usize,
    /// Max simultaneous shell/verification tasks.
    pub max_concurrent_shells: usize,
    /// Max simultaneous LLM calls across workers.
    pub max_parallel_llm_calls: usize,
}

impl Default for ConcurrencyLimits {
    fn default() -> Self {
        Self {
            max_concurrent_workers: 4,
            max_concurrent_writes: 4,
            max_concurrent_shells: 1,
            max_parallel_llm_calls: 4,
        }
    }
}

/// Nested delegation policy for workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NestedDelegationPolicy {
    /// Whether this worker role may spawn child subagents.
    pub can_spawn_subagents: bool,
    /// Current delegation depth for this worker.
    pub delegate_depth: u8,
    /// Maximum permitted delegation depth.
    pub max_delegate_depth: u8,
}

impl Default for NestedDelegationPolicy {
    fn default() -> Self {
        Self {
            can_spawn_subagents: false,
            delegate_depth: 0,
            max_delegate_depth: 2,
        }
    }
}

impl NestedDelegationPolicy {
    /// Whether a child subagent can be created under this policy.
    pub fn can_delegate(&self) -> bool {
        self.can_spawn_subagents && self.delegate_depth < self.max_delegate_depth
    }

    /// Policy to pass to a child worker, if delegation is allowed.
    pub fn child_policy(&self) -> Option<Self> {
        if !self.can_delegate() {
            return None;
        }

        Some(Self {
            can_spawn_subagents: self.can_spawn_subagents,
            delegate_depth: self.delegate_depth.saturating_add(1),
            max_delegate_depth: self.max_delegate_depth,
        })
    }
}

/// A bounded follow-up task proposed by a worker.
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
    pub worker_agent: String,
    pub completed_work: Vec<String>,
    pub files_read: Vec<String>,
    pub files_changed: Vec<String>,
    pub decisions: Vec<String>,
    pub failures: Vec<String>,
    pub verification: Vec<String>,
    pub next_implications: Vec<String>,
    #[serde(default)]
    pub suggested_tasks: Vec<SuggestedTask>,
    pub created_at: DateTime<Utc>,
}

/// Product-neutral worker contract.
///
/// The framework defines the contract; products decide which concrete worker
/// implementation, tools, storage, and UI event mapping to use.
#[async_trait]
pub trait TaskWorker: Send + Sync {
    async fn execute(
        &self,
        task: RuntimeTask,
        dependency_summaries: Vec<TaskExecutionSummary>,
        delegation_policy: NestedDelegationPolicy,
    ) -> Result<TaskExecutionSummary>;
}

#[cfg(test)]
mod tests {
    use super::{NestedDelegationPolicy, RuntimeTaskKind, RuntimeTaskStatus};
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
}
