//! Unified task-runtime service and claim state transitions.
//!
//! [`RuntimeTaskService`] is the framework entry point around the canonical
//! [`super::RuntimeDagExecutor`]. Persistence adapters retain their own atomic
//! transaction and event formats, but call the pure mutation functions in this
//! module while holding that transaction. This keeps claim/CAS/retry semantics
//! identical across in-memory and product file stores.

use std::sync::Arc;

use echo_core::error::Result;
use tokio_util::sync::CancellationToken;

use super::{
    RuntimeDagController, RuntimeDagExecutor, RuntimeDagExecutorConfig, RuntimeDagOutcome,
    RuntimePlanSnapshot, RuntimeTaskClaimOutcome, Task, TaskClaim, TaskStatus,
};
use crate::planning::PlanValidator;

/// Rejected mutation of the framework-owned runtime task state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeTaskMutationError {
    #[error("task specification cannot be claimed: {message}")]
    InvalidTaskSpec { message: String },
    #[error("invalid runtime task transition: {message}")]
    InvalidTransition { message: String },
    #[error(
        "Retrying cannot be settled directly; use requeue_runtime_claim to preserve claim atomicity"
    )]
    RetryingRequiresRequeue,
}

/// Result of atomically resolving one requested retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTaskRequeueOutcome {
    /// The exact claim was returned to Pending and consumed one retry.
    Requeued,
    /// The retry budget was already exhausted; the exact claim became Failed.
    Exhausted,
    /// The physical claim is no longer current, so no state changed.
    Superseded,
}

/// One service that drives a revisioned task graph through the framework DAG
/// executor. Applications inject persistence/dispatch policy through `C`.
pub struct RuntimeTaskService<C: RuntimeDagController> {
    executor: RuntimeDagExecutor<C>,
}

impl<C: RuntimeDagController> RuntimeTaskService<C> {
    pub fn new(controller: Arc<C>, config: RuntimeDagExecutorConfig) -> Self {
        Self {
            executor: RuntimeDagExecutor::new(controller, config),
        }
    }

    pub fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.executor = self.executor.with_validator(validator);
        self
    }

    pub async fn execute(
        &self,
        run_id: &str,
        cancel: CancellationToken,
    ) -> Result<RuntimeDagOutcome> {
        self.executor.execute(run_id, cancel).await
    }
}

/// Claim one exact Pending task from a loaded relation revision.
pub fn claim_runtime_task(
    snapshot: &mut RuntimePlanSnapshot,
    expected_task: &Task,
    expected_revision: u64,
) -> std::result::Result<RuntimeTaskClaimOutcome, RuntimeTaskMutationError> {
    if snapshot.revision != expected_revision {
        return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
    }
    let Some(task) = snapshot
        .tasks
        .iter_mut()
        .find(|task| task.spec.id == expected_task.spec.id)
    else {
        return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
    };
    if task.spec != expected_task.spec
        || task.execution.status != TaskStatus::Pending
        || task.execution.claim.is_some()
    {
        return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
    }
    let claim = TaskClaim::new(
        expected_revision,
        task.execution.retry_count.saturating_add(1),
        task.spec
            .stable_hash()
            .map_err(|message| RuntimeTaskMutationError::InvalidTaskSpec { message })?,
    );
    task.execution.status = TaskStatus::Running;
    task.execution.claim = Some(claim.clone());
    Ok(RuntimeTaskClaimOutcome::Claimed(claim))
}

/// Settle an exact physical claim. `false` means it was superseded.
pub fn settle_runtime_claim(
    snapshot: &mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
    status: TaskStatus,
) -> std::result::Result<bool, RuntimeTaskMutationError> {
    if matches!(status, TaskStatus::Retrying { .. }) {
        return Err(RuntimeTaskMutationError::RetryingRequiresRequeue);
    }
    let Some(task) = claimed_task_mut(snapshot, task_id, claim) else {
        return Ok(false);
    };
    task.execution.status = task
        .execution
        .status
        .transition_to(status)
        .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
    task.execution.claim = None;
    Ok(true)
}

/// Requeue an exact failed attempt without exposing an unclaimed intermediate
/// state. `retry_count` counts completed retry decisions, while claim attempt
/// identity remains `retry_count + 1`.
pub fn requeue_runtime_claim(
    snapshot: &mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
    failure_fingerprint: Option<String>,
    error: String,
) -> std::result::Result<RuntimeTaskRequeueOutcome, RuntimeTaskMutationError> {
    let Some(task) = claimed_task_mut(snapshot, task_id, claim) else {
        return Ok(RuntimeTaskRequeueOutcome::Superseded);
    };
    if task.execution.retry_count >= task.spec.max_retries {
        task.execution.status = task
            .execution
            .status
            .transition_to(TaskStatus::Failed(error))
            .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
        task.execution.failure_fingerprint = failure_fingerprint;
        task.execution.claim = None;
        return Ok(RuntimeTaskRequeueOutcome::Exhausted);
    }
    let retrying = task
        .execution
        .status
        .transition_to(TaskStatus::Retrying {
            attempt: claim.attempt,
            last_error: error,
        })
        .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
    task.execution.status = retrying
        .transition_to(TaskStatus::Pending)
        .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
    task.execution.retry_count = task.execution.retry_count.saturating_add(1);
    task.execution.failure_fingerprint = failure_fingerprint;
    task.execution.claim = None;
    Ok(RuntimeTaskRequeueOutcome::Requeued)
}

/// Block an unclaimed Pending task. Already-resolved or claimed tasks are left
/// untouched so dependency propagation cannot overwrite active work.
pub fn block_runtime_task(snapshot: &mut RuntimePlanSnapshot, task_id: &str, reason: &str) -> bool {
    let Some(task) = snapshot
        .tasks
        .iter_mut()
        .find(|task| task.spec.id == task_id)
    else {
        return false;
    };
    if task.execution.status != TaskStatus::Pending || task.execution.claim.is_some() {
        return false;
    }
    task.execution.status = TaskStatus::Blocked(reason.to_string());
    true
}

pub fn runtime_claim_is_current(
    snapshot: &RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
) -> bool {
    snapshot.tasks.iter().any(|task| {
        task.spec.id == task_id
            && task.execution.status == TaskStatus::Running
            && task.execution.claim.as_ref() == Some(claim)
    })
}

fn claimed_task_mut<'a>(
    snapshot: &'a mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
) -> Option<&'a mut Task> {
    snapshot.tasks.iter_mut().find(|task| {
        task.spec.id == task_id
            && task.execution.status == TaskStatus::Running
            && task.execution.claim.as_ref() == Some(claim)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{TaskExecution, TaskSpec};

    fn task(id: &str) -> Task {
        Task {
            spec: TaskSpec {
                id: id.to_string(),
                title: id.to_string(),
                description: id.to_string(),
                depends_on: Vec::new(),
                max_retries: 2,
                extension: serde_json::Value::Null,
            },
            execution: TaskExecution::pending(id),
        }
    }

    #[test]
    fn claim_settle_and_stale_cas_share_one_transition_authority()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let expected = task("a");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 3,
            tasks: vec![expected.clone()],
        };
        assert_eq!(
            claim_runtime_task(&mut snapshot, &expected, 2)?,
            RuntimeTaskClaimOutcome::ReloadSnapshot
        );
        let claim = match claim_runtime_task(&mut snapshot, &expected, 3)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "claim unexpectedly reloaded".to_string(),
                });
            }
        };
        assert!(runtime_claim_is_current(&snapshot, "a", &claim));
        let stale = TaskClaim::new(3, claim.attempt, claim.spec_hash.clone());
        assert!(!settle_runtime_claim(
            &mut snapshot,
            "a",
            &stale,
            TaskStatus::Completed,
        )?);
        assert!(settle_runtime_claim(
            &mut snapshot,
            "a",
            &claim,
            TaskStatus::Completed,
        )?);
        let settled =
            snapshot
                .tasks
                .first()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "settled task is missing".to_string(),
                })?;
        assert_eq!(settled.execution.status, TaskStatus::Completed);
        Ok(())
    }

    #[test]
    fn settle_rejects_retrying_without_mutating_snapshot()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let expected = task("a");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 7,
            tasks: vec![expected.clone()],
        };
        let claim = match claim_runtime_task(&mut snapshot, &expected, 7)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "claim unexpectedly reloaded".to_string(),
                });
            }
        };
        let before_revision = snapshot.revision;
        let before_tasks = snapshot.tasks.clone();

        let error = settle_runtime_claim(
            &mut snapshot,
            "a",
            &claim,
            TaskStatus::Retrying {
                attempt: claim.attempt,
                last_error: "transient".to_string(),
            },
        )
        .err()
        .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
            message: "direct Retrying settlement unexpectedly succeeded".to_string(),
        })?;

        assert_eq!(error, RuntimeTaskMutationError::RetryingRequiresRequeue);
        assert_eq!(snapshot.revision, before_revision);
        assert_eq!(snapshot.tasks, before_tasks);
        assert!(runtime_claim_is_current(&snapshot, "a", &claim));
        Ok(())
    }

    #[test]
    fn retry_and_block_mutations_preserve_claim_boundaries()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let expected = task("a");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![expected.clone(), task("b")],
        };
        let claim = match claim_runtime_task(&mut snapshot, &expected, 1)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "claim unexpectedly reloaded".to_string(),
                });
            }
        };
        assert!(!block_runtime_task(
            &mut snapshot,
            "a",
            "must not overwrite"
        ));
        assert_eq!(
            requeue_runtime_claim(
                &mut snapshot,
                "a",
                &claim,
                Some("fingerprint".to_string()),
                "transient".to_string(),
            )?,
            RuntimeTaskRequeueOutcome::Requeued
        );
        let retried =
            snapshot
                .tasks
                .first()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "retried task is missing".to_string(),
                })?;
        assert_eq!(retried.execution.status, TaskStatus::Pending);
        assert_eq!(retried.execution.retry_count, 1);
        assert!(block_runtime_task(&mut snapshot, "b", "upstream failed"));
        let blocked =
            snapshot
                .tasks
                .get(1)
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "blocked task is missing".to_string(),
                })?;
        assert_eq!(
            blocked.execution.status,
            TaskStatus::Blocked("upstream failed".to_string())
        );
        Ok(())
    }

    #[test]
    fn retry_budget_zero_exhausts_the_first_failed_claim()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut expected = task("a");
        expected.spec.max_retries = 0;
        let mut snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![expected.clone()],
        };
        let claim = match claim_runtime_task(&mut snapshot, &expected, 1)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "claim unexpectedly reloaded".to_string(),
                });
            }
        };

        assert_eq!(
            requeue_runtime_claim(
                &mut snapshot,
                "a",
                &claim,
                Some("fp-zero".to_string()),
                "no retry".to_string(),
            )?,
            RuntimeTaskRequeueOutcome::Exhausted
        );
        let exhausted =
            snapshot
                .tasks
                .first()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "exhausted task is missing".to_string(),
                })?;
        assert_eq!(
            exhausted.execution.status,
            TaskStatus::Failed("no retry".to_string())
        );
        assert_eq!(exhausted.execution.retry_count, 0);
        assert_eq!(
            exhausted.execution.failure_fingerprint.as_deref(),
            Some("fp-zero")
        );
        assert!(exhausted.execution.claim.is_none());
        Ok(())
    }

    #[test]
    fn retry_budget_requeues_until_boundary_then_exhausts()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut expected = task("a");
        expected.spec.max_retries = 2;
        let mut snapshot = RuntimePlanSnapshot {
            revision: 4,
            tasks: vec![expected.clone()],
        };

        for retry_count in 1..=2 {
            let claim = match claim_runtime_task(&mut snapshot, &expected, 4)? {
                RuntimeTaskClaimOutcome::Claimed(claim) => claim,
                RuntimeTaskClaimOutcome::ReloadSnapshot => {
                    return Err(RuntimeTaskMutationError::InvalidTransition {
                        message: "claim unexpectedly reloaded".to_string(),
                    });
                }
            };
            assert_eq!(
                requeue_runtime_claim(
                    &mut snapshot,
                    "a",
                    &claim,
                    None,
                    format!("retry {retry_count}"),
                )?,
                RuntimeTaskRequeueOutcome::Requeued
            );
            let retried = snapshot.tasks.first().ok_or_else(|| {
                RuntimeTaskMutationError::InvalidTransition {
                    message: "retried task is missing".to_string(),
                }
            })?;
            assert_eq!(retried.execution.retry_count, retry_count);
            assert_eq!(retried.execution.status, TaskStatus::Pending);
        }

        let final_claim = match claim_runtime_task(&mut snapshot, &expected, 4)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "boundary claim unexpectedly reloaded".to_string(),
                });
            }
        };
        assert_eq!(final_claim.attempt, 3);
        assert_eq!(
            requeue_runtime_claim(
                &mut snapshot,
                "a",
                &final_claim,
                Some("fp-final".to_string()),
                "exhausted".to_string(),
            )?,
            RuntimeTaskRequeueOutcome::Exhausted
        );
        assert_eq!(
            requeue_runtime_claim(&mut snapshot, "a", &final_claim, None, "late".to_string(),)?,
            RuntimeTaskRequeueOutcome::Superseded
        );
        let exhausted =
            snapshot
                .tasks
                .first()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "boundary task is missing".to_string(),
                })?;
        assert_eq!(
            exhausted.execution.status,
            TaskStatus::Failed("exhausted".to_string())
        );
        assert_eq!(exhausted.execution.retry_count, 2);
        assert!(exhausted.execution.claim.is_none());
        Ok(())
    }
}
