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
    #[error("task '{task_id}' cannot be explicitly retried while it owns a runtime claim")]
    ExplicitRetryRequiresUnclaimedTask { task_id: String },
    #[error("task '{task_id}' in state {status:?} cannot be explicitly retried")]
    ExplicitRetryRequiresRestartableStatus { task_id: String, status: TaskStatus },
    #[error("task '{task_id}' retry counter overflowed")]
    RetryCounterOverflow { task_id: String },
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

/// Result of explicitly restarting one unclaimed task.
///
/// This differs from [`RuntimeTaskRequeueOutcome`]: requeue resolves a live
/// claimed attempt, while explicit retry restarts a persisted failed, timed
/// out, blocked, or paused task after an operator or product policy asks for
/// another attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTaskRetryOutcome {
    /// The task returned to Pending and consumed one retry.
    Retried { retry_count: u32 },
    /// No retry budget remains; the existing state was preserved.
    Exhausted { retry_count: u32, max_retries: u32 },
    /// The revision or expected task changed, so no state changed.
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

/// Explicitly retry one exact unclaimed task from a loaded relation revision.
///
/// The whole expected task participates in optimistic concurrency, including
/// its failure fingerprint and retry counter. Products keep run restart,
/// descendant unblocking, review, and other policy outside this mutation.
pub fn retry_runtime_task(
    snapshot: &mut RuntimePlanSnapshot,
    expected_task: &Task,
    expected_revision: u64,
) -> std::result::Result<RuntimeTaskRetryOutcome, RuntimeTaskMutationError> {
    if snapshot.revision != expected_revision {
        return Ok(RuntimeTaskRetryOutcome::Superseded);
    }
    let Some(task) = snapshot
        .tasks
        .iter_mut()
        .find(|task| task.spec.id == expected_task.spec.id)
    else {
        return Ok(RuntimeTaskRetryOutcome::Superseded);
    };
    if task != expected_task {
        return Ok(RuntimeTaskRetryOutcome::Superseded);
    }
    if task.execution.claim.is_some() {
        return Err(
            RuntimeTaskMutationError::ExplicitRetryRequiresUnclaimedTask {
                task_id: task.spec.id.clone(),
            },
        );
    }
    if !matches!(
        task.execution.status,
        TaskStatus::Failed(_)
            | TaskStatus::TimedOut { .. }
            | TaskStatus::Blocked(_)
            | TaskStatus::Paused(_)
    ) {
        return Err(
            RuntimeTaskMutationError::ExplicitRetryRequiresRestartableStatus {
                task_id: task.spec.id.clone(),
                status: task.execution.status.clone(),
            },
        );
    }
    if task.execution.retry_count >= task.spec.max_retries {
        return Ok(RuntimeTaskRetryOutcome::Exhausted {
            retry_count: task.execution.retry_count,
            max_retries: task.spec.max_retries,
        });
    }
    let retry_count = task.execution.retry_count.checked_add(1).ok_or_else(|| {
        RuntimeTaskMutationError::RetryCounterOverflow {
            task_id: task.spec.id.clone(),
        }
    })?;
    task.execution.status = TaskStatus::Pending;
    task.execution.retry_count = retry_count;
    Ok(RuntimeTaskRetryOutcome::Retried { retry_count })
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

    #[test]
    fn explicit_retry_supports_each_restartable_state_and_preserves_failure_fingerprint()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let restartable = vec![
            TaskStatus::Failed("failed".to_string()),
            TaskStatus::TimedOut {
                error: "timed out".to_string(),
            },
            TaskStatus::Blocked("blocked".to_string()),
            TaskStatus::Paused("paused".to_string()),
        ];
        for status in restartable {
            let mut expected = task("restartable");
            expected.execution.status = status;
            expected.execution.retry_count = 1;
            expected.execution.failure_fingerprint = Some("stable-fingerprint".to_string());
            let mut snapshot = RuntimePlanSnapshot {
                revision: 8,
                tasks: vec![expected.clone()],
            };

            assert_eq!(
                retry_runtime_task(&mut snapshot, &expected, 8)?,
                RuntimeTaskRetryOutcome::Retried { retry_count: 2 }
            );
            let retried = snapshot.tasks.first().ok_or_else(|| {
                RuntimeTaskMutationError::InvalidTransition {
                    message: "explicitly retried task is missing".to_string(),
                }
            })?;
            assert_eq!(retried.execution.status, TaskStatus::Pending);
            assert_eq!(retried.execution.retry_count, 2);
            assert_eq!(
                retried.execution.failure_fingerprint.as_deref(),
                Some("stable-fingerprint")
            );
            assert!(retried.execution.claim.is_none());
        }
        Ok(())
    }

    #[test]
    fn explicit_retry_budget_zero_and_n_boundary_are_exact()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut zero = task("zero");
        zero.spec.max_retries = 0;
        zero.execution.status = TaskStatus::Failed("failed".to_string());
        let mut zero_snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![zero.clone()],
        };
        assert_eq!(
            retry_runtime_task(&mut zero_snapshot, &zero, 1)?,
            RuntimeTaskRetryOutcome::Exhausted {
                retry_count: 0,
                max_retries: 0,
            }
        );
        assert_eq!(zero_snapshot.tasks, vec![zero]);

        let mut expected = task("bounded");
        expected.spec.max_retries = 2;
        expected.execution.status = TaskStatus::Failed("attempt one".to_string());
        let mut snapshot = RuntimePlanSnapshot {
            revision: 5,
            tasks: vec![expected.clone()],
        };
        assert_eq!(
            retry_runtime_task(&mut snapshot, &expected, 5)?,
            RuntimeTaskRetryOutcome::Retried { retry_count: 1 }
        );
        let current = snapshot.tasks.first_mut().ok_or_else(|| {
            RuntimeTaskMutationError::InvalidTransition {
                message: "first retry lost task".to_string(),
            }
        })?;
        current.execution.status = TaskStatus::TimedOut {
            error: "attempt two".to_string(),
        };
        current.execution.failure_fingerprint = Some("attempt-two".to_string());
        let second_expected = current.clone();
        assert_eq!(
            retry_runtime_task(&mut snapshot, &second_expected, 5)?,
            RuntimeTaskRetryOutcome::Retried { retry_count: 2 }
        );
        let current = snapshot.tasks.first_mut().ok_or_else(|| {
            RuntimeTaskMutationError::InvalidTransition {
                message: "second retry lost task".to_string(),
            }
        })?;
        current.execution.status = TaskStatus::Blocked("attempt three".to_string());
        let exhausted_expected = current.clone();
        let before_exhaustion = snapshot.tasks.clone();
        assert_eq!(
            retry_runtime_task(&mut snapshot, &exhausted_expected, 5)?,
            RuntimeTaskRetryOutcome::Exhausted {
                retry_count: 2,
                max_retries: 2,
            }
        );
        assert_eq!(snapshot.tasks, before_exhaustion);
        Ok(())
    }

    #[test]
    fn explicit_retry_rejects_stale_invalid_and_claimed_tasks_without_mutation()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut expected = task("retry");
        expected.execution.status = TaskStatus::Failed("failed".to_string());
        expected.execution.failure_fingerprint = Some("current".to_string());
        let mut snapshot = RuntimePlanSnapshot {
            revision: 11,
            tasks: vec![expected.clone()],
        };
        let before = snapshot.tasks.clone();
        assert_eq!(
            retry_runtime_task(&mut snapshot, &expected, 10)?,
            RuntimeTaskRetryOutcome::Superseded
        );
        let mut stale_fingerprint = expected.clone();
        stale_fingerprint.execution.failure_fingerprint = Some("stale".to_string());
        assert_eq!(
            retry_runtime_task(&mut snapshot, &stale_fingerprint, 11)?,
            RuntimeTaskRetryOutcome::Superseded
        );
        assert_eq!(snapshot.tasks, before);

        let invalid_statuses = vec![
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Skipped,
            TaskStatus::Cancelled,
            TaskStatus::Retrying {
                attempt: 1,
                last_error: "retrying".to_string(),
            },
        ];
        for status in invalid_statuses {
            let mut invalid = task("invalid");
            invalid.execution.status = status.clone();
            let mut invalid_snapshot = RuntimePlanSnapshot {
                revision: 12,
                tasks: vec![invalid.clone()],
            };
            let error = retry_runtime_task(&mut invalid_snapshot, &invalid, 12)
                .err()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "invalid explicit retry unexpectedly succeeded".to_string(),
                })?;
            assert_eq!(
                error,
                RuntimeTaskMutationError::ExplicitRetryRequiresRestartableStatus {
                    task_id: "invalid".to_string(),
                    status,
                }
            );
            assert_eq!(invalid_snapshot.tasks, vec![invalid]);
        }

        let mut claimed = task("claimed");
        claimed.execution.status = TaskStatus::Blocked("blocked".to_string());
        claimed.execution.claim = Some(TaskClaim::new(12, 1, "spec".to_string()));
        let mut claimed_snapshot = RuntimePlanSnapshot {
            revision: 12,
            tasks: vec![claimed.clone()],
        };
        let error = retry_runtime_task(&mut claimed_snapshot, &claimed, 12)
            .err()
            .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                message: "claimed explicit retry unexpectedly succeeded".to_string(),
            })?;
        assert_eq!(
            error,
            RuntimeTaskMutationError::ExplicitRetryRequiresUnclaimedTask {
                task_id: "claimed".to_string(),
            }
        );
        assert_eq!(claimed_snapshot.tasks, vec![claimed]);
        Ok(())
    }
}
