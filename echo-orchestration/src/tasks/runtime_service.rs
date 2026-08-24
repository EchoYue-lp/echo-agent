//! Unified task-runtime service and claim state transitions.
//!
//! [`RuntimeTaskService`] is the framework entry point around the canonical
//! private DAG executor. Persistence adapters retain their own atomic
//! transaction and event formats, but call the pure mutation functions in this
//! module while holding that transaction. This keeps claim/CAS/retry semantics
//! identical across in-memory and product file stores.

use std::sync::Arc;

use echo_core::error::Result;
use tokio_util::sync::CancellationToken;

use super::runtime_executor::RuntimeDagExecutor;
use super::{
    RuntimeDagController, RuntimeDagOutcome, RuntimeInterruptionDisposition, RuntimePlanSnapshot,
    RuntimeTaskClaimOutcome, RuntimeTaskServiceConfig, Task, TaskClaim, TaskId, TaskStatus,
};
use super::{RuntimeTaskResolution, RuntimeTaskResolutionRequest};
use crate::planning::PlanValidator;

/// Rejected mutation of the framework-owned runtime task state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeTaskMutationError {
    #[error("task specification cannot be claimed: {message}")]
    InvalidTaskSpec { message: String },
    #[error("invalid runtime task transition: {message}")]
    InvalidTransition { message: String },
    #[error("claim cannot be settled to active state {status:?}")]
    SettlementRequiresInactiveStatus { status: TaskStatus },
    #[error("task '{task_id}' cannot be explicitly retried while it owns a runtime claim")]
    ExplicitRetryRequiresUnclaimedTask { task_id: String },
    #[error("task '{task_id}' in state {status:?} cannot be explicitly retried")]
    ExplicitRetryRequiresRestartableStatus { task_id: String, status: TaskStatus },
    #[error("task '{task_id}' in state {status:?} cannot be resumed; expected Paused")]
    ResumeRequiresPausedStatus { task_id: String, status: TaskStatus },
    #[error("task '{task_id}' retry counter overflowed")]
    RetryCounterOverflow { task_id: String },
    #[error("task '{task_id}' attempt counter overflowed")]
    AttemptCounterOverflow { task_id: String },
    #[error("task '{task_id}' retry_count {retry_count} exceeds max_retries {max_retries}")]
    RetryBudgetInvariant {
        task_id: String,
        retry_count: u32,
        max_retries: u32,
    },
    #[error("task '{task_id}' has an invalid runtime claim: {message}")]
    InvalidClaim { task_id: String, message: String },
}

/// Typed compare-and-set result for one exact physical claim settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTaskSettlementOutcome {
    /// The exact claim was current and its state transition was committed.
    Settled,
    /// The claim identity, attempt, specification, or lifecycle was replaced.
    Superseded,
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
/// out, or explicitly blocked task after an operator or product policy asks for
/// another attempt. Paused work uses [`RuntimeTaskResumeOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTaskRetryOutcome {
    /// The task returned to Pending and consumed one retry.
    Retried { retry_count: u32 },
    /// No retry budget remains; the existing state was preserved.
    Exhausted { retry_count: u32, max_retries: u32 },
    /// The revision or expected task changed, so no state changed.
    Superseded,
}

/// Result of resuming one exact paused task without consuming retry budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTaskResumeOutcome {
    /// The exact paused task returned to Pending without a retry increment.
    Resumed,
    /// The graph revision or expected task changed, so no state changed.
    Superseded,
}

/// Durable task effects of settling a runtime interruption at a safe point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInterruptionReceipt {
    /// The exact cancellation or pause policy that was applied.
    pub disposition: RuntimeInterruptionDisposition,
    /// Unfinished tasks whose lifecycle changed during settlement.
    pub interrupted_task_ids: Vec<TaskId>,
    /// Terminal siblings retained without replay or overwrite.
    pub retained_terminal_task_ids: Vec<TaskId>,
    /// Unstarted tasks retained by a resumable pause.
    pub pending_task_ids: Vec<TaskId>,
}

/// Optimistic-concurrency result of a run-level interruption settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeInterruptionSettlementOutcome {
    /// The exact revision was settled and produced a typed receipt.
    Settled(RuntimeInterruptionReceipt),
    /// The graph revision changed before settlement; reload and retry.
    ReloadSnapshot,
}

/// Public framework entry point for a revisioned runtime task graph.
///
/// The service is the only construction boundary for the canonical DAG
/// executor. Framework-owned mechanisms are dependency projection, exact claim
/// identity, bounded waves, retry bookkeeping, interruption settlement, and
/// revision safe points. Applications inject persistence and dispatch plus the
/// product decision that maps a stop request to [`RuntimeInterruptionDisposition`].
/// UI projection, review policy, worktrees, and product run status stay outside
/// this service.
pub struct RuntimeTaskService<C: RuntimeDagController> {
    executor: RuntimeDagExecutor<C>,
}

impl<C: RuntimeDagController> RuntimeTaskService<C> {
    pub fn new(controller: Arc<C>, config: RuntimeTaskServiceConfig) -> Self {
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
    if task.execution.retry_count > task.spec.max_retries {
        return Err(RuntimeTaskMutationError::RetryBudgetInvariant {
            task_id: task.spec.id.clone(),
            retry_count: task.execution.retry_count,
            max_retries: task.spec.max_retries,
        });
    }
    if task != expected_task
        || task.execution.task_id != task.spec.id
        || task.execution.status != TaskStatus::Pending
        || task.execution.claim.is_some()
    {
        return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
    }
    let attempt = task.execution.retry_count.checked_add(1).ok_or_else(|| {
        RuntimeTaskMutationError::AttemptCounterOverflow {
            task_id: task.spec.id.clone(),
        }
    })?;
    let claim = TaskClaim::new(
        expected_revision,
        attempt,
        task.spec
            .stable_hash()
            .map_err(|message| RuntimeTaskMutationError::InvalidTaskSpec { message })?,
    );
    task.execution.status = TaskStatus::Running;
    task.execution.claim = Some(claim.clone());
    Ok(RuntimeTaskClaimOutcome::Claimed(claim))
}

/// Settle one exact physical claim and clear claim ownership.
pub fn settle_runtime_claim(
    snapshot: &mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
    status: TaskStatus,
) -> std::result::Result<RuntimeTaskSettlementOutcome, RuntimeTaskMutationError> {
    if matches!(
        status,
        TaskStatus::Pending | TaskStatus::Running | TaskStatus::Retrying { .. }
    ) {
        return Err(RuntimeTaskMutationError::SettlementRequiresInactiveStatus { status });
    }
    let Some(task) = claimed_task_mut(snapshot, task_id, claim)? else {
        return Ok(RuntimeTaskSettlementOutcome::Superseded);
    };
    task.execution.status = task
        .execution
        .status
        .transition_to(status)
        .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
    task.execution.claim = None;
    Ok(RuntimeTaskSettlementOutcome::Settled)
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
    let Some(task) = claimed_task_mut(snapshot, task_id, claim)? else {
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
    task.execution.retry_count = task.execution.retry_count.checked_add(1).ok_or_else(|| {
        RuntimeTaskMutationError::RetryCounterOverflow {
            task_id: task.spec.id.clone(),
        }
    })?;
    task.execution.failure_fingerprint = failure_fingerprint;
    task.execution.claim = None;
    Ok(RuntimeTaskRequeueOutcome::Requeued)
}

/// Apply one uncommitted dispatch assessment to the exact current claim.
///
/// Persistence adapters call this function while holding their atomic
/// transaction, then publish any product side effect only when the typed result
/// is not [`RuntimeTaskResolution::Superseded`].
pub fn settle_runtime_resolution(
    snapshot: &mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
    request: RuntimeTaskResolutionRequest,
) -> std::result::Result<RuntimeTaskResolution, RuntimeTaskMutationError> {
    match request {
        RuntimeTaskResolutionRequest::Completed => settle_requested_status(
            snapshot,
            task_id,
            claim,
            TaskStatus::Completed,
            RuntimeTaskResolution::Completed,
        ),
        RuntimeTaskResolutionRequest::Requeue {
            failure_fingerprint,
            error,
        } => match requeue_runtime_claim(
            snapshot,
            task_id,
            claim,
            failure_fingerprint,
            error.clone(),
        )? {
            RuntimeTaskRequeueOutcome::Requeued => Ok(RuntimeTaskResolution::Pending),
            RuntimeTaskRequeueOutcome::Exhausted => Ok(RuntimeTaskResolution::Failed { error }),
            RuntimeTaskRequeueOutcome::Superseded => Ok(RuntimeTaskResolution::Superseded),
        },
        RuntimeTaskResolutionRequest::Skipped => settle_requested_status(
            snapshot,
            task_id,
            claim,
            TaskStatus::Skipped,
            RuntimeTaskResolution::Skipped,
        ),
        RuntimeTaskResolutionRequest::Failed { error } => settle_requested_status(
            snapshot,
            task_id,
            claim,
            TaskStatus::Failed(error.clone()),
            RuntimeTaskResolution::Failed { error },
        ),
        RuntimeTaskResolutionRequest::Blocked { error, disposition } => settle_requested_status(
            snapshot,
            task_id,
            claim,
            TaskStatus::Blocked(error.clone()),
            RuntimeTaskResolution::Blocked { error, disposition },
        ),
        RuntimeTaskResolutionRequest::Cancelled => settle_requested_status(
            snapshot,
            task_id,
            claim,
            TaskStatus::Cancelled,
            RuntimeTaskResolution::Cancelled,
        ),
    }
}

fn settle_requested_status(
    snapshot: &mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
    status: TaskStatus,
    resolution: RuntimeTaskResolution,
) -> std::result::Result<RuntimeTaskResolution, RuntimeTaskMutationError> {
    match settle_runtime_claim(snapshot, task_id, claim, status)? {
        RuntimeTaskSettlementOutcome::Settled => Ok(resolution),
        RuntimeTaskSettlementOutcome::Superseded => Ok(RuntimeTaskResolution::Superseded),
    }
}

/// Explicitly retry one exact unclaimed task from a loaded relation revision.
///
/// The whole expected task participates in optimistic concurrency, including
/// its failure fingerprint and retry counter. Products keep run restart,
/// review, and other policy outside this mutation. Derived dependency blockers
/// disappear from [`super::DagExecutionState`] when this new snapshot is read.
/// A paused task uses [`resume_runtime_task`] without consuming retry budget.
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
        TaskStatus::Failed(_) | TaskStatus::TimedOut { .. } | TaskStatus::Blocked(_)
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

/// Resume one exact paused task without increasing its retry counter.
pub fn resume_runtime_task(
    snapshot: &mut RuntimePlanSnapshot,
    expected_task: &Task,
    expected_revision: u64,
) -> std::result::Result<RuntimeTaskResumeOutcome, RuntimeTaskMutationError> {
    if snapshot.revision != expected_revision {
        return Ok(RuntimeTaskResumeOutcome::Superseded);
    }
    let Some(task) = snapshot
        .tasks
        .iter_mut()
        .find(|task| task.spec.id == expected_task.spec.id)
    else {
        return Ok(RuntimeTaskResumeOutcome::Superseded);
    };
    if task != expected_task {
        return Ok(RuntimeTaskResumeOutcome::Superseded);
    }
    if task.execution.claim.is_some() {
        return Err(RuntimeTaskMutationError::InvalidClaim {
            task_id: task.spec.id.clone(),
            message: "paused task still owns a claim".to_string(),
        });
    }
    if !matches!(task.execution.status, TaskStatus::Paused(_)) {
        return Err(RuntimeTaskMutationError::ResumeRequiresPausedStatus {
            task_id: task.spec.id.clone(),
            status: task.execution.status.clone(),
        });
    }
    task.execution.status = task
        .execution
        .status
        .transition_to(TaskStatus::Pending)
        .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
    Ok(RuntimeTaskResumeOutcome::Resumed)
}

/// Settle a run-level pause or cancellation against one exact graph revision.
///
/// Cancellation marks every unfinished task Cancelled. Pause converts only
/// active claimed tasks to Paused and leaves unstarted tasks Pending. Every
/// claim is validated before mutation, so a stale revision, attempt, spec hash,
/// or physical claim identity fails closed without a partial settlement.
pub fn settle_runtime_interruption(
    snapshot: &mut RuntimePlanSnapshot,
    expected_revision: u64,
    disposition: RuntimeInterruptionDisposition,
) -> std::result::Result<RuntimeInterruptionSettlementOutcome, RuntimeTaskMutationError> {
    if snapshot.revision != expected_revision {
        return Ok(RuntimeInterruptionSettlementOutcome::ReloadSnapshot);
    }
    validate_runtime_snapshot_claims(snapshot)?;

    let mut interrupted_task_ids = Vec::new();
    let mut retained_terminal_task_ids = Vec::new();
    let mut pending_task_ids = Vec::new();
    for task in &mut snapshot.tasks {
        if task.execution.status.is_terminal() {
            retained_terminal_task_ids.push(task.spec.id.clone());
            continue;
        }
        match &disposition {
            RuntimeInterruptionDisposition::Cancelled => {
                task.execution.status = task
                    .execution
                    .status
                    .transition_to(TaskStatus::Cancelled)
                    .map_err(|message| RuntimeTaskMutationError::InvalidTransition { message })?;
                task.execution.claim = None;
                interrupted_task_ids.push(task.spec.id.clone());
            }
            RuntimeInterruptionDisposition::Paused { reason } => {
                if matches!(
                    task.execution.status,
                    TaskStatus::Running | TaskStatus::Retrying { .. }
                ) {
                    task.execution.status = task
                        .execution
                        .status
                        .transition_to(TaskStatus::Paused(reason.clone()))
                        .map_err(|message| RuntimeTaskMutationError::InvalidTransition {
                            message,
                        })?;
                    task.execution.claim = None;
                    interrupted_task_ids.push(task.spec.id.clone());
                } else if task.execution.status == TaskStatus::Pending {
                    pending_task_ids.push(task.spec.id.clone());
                }
            }
        }
    }
    interrupted_task_ids.sort();
    retained_terminal_task_ids.sort();
    pending_task_ids.sort();
    Ok(RuntimeInterruptionSettlementOutcome::Settled(
        RuntimeInterruptionReceipt {
            disposition,
            interrupted_task_ids,
            retained_terminal_task_ids,
            pending_task_ids,
        },
    ))
}

/// Cancel every unfinished task in one exact graph revision.
pub fn cancel_unfinished_runtime_tasks(
    snapshot: &mut RuntimePlanSnapshot,
    expected_revision: u64,
) -> std::result::Result<RuntimeInterruptionSettlementOutcome, RuntimeTaskMutationError> {
    settle_runtime_interruption(
        snapshot,
        expected_revision,
        RuntimeInterruptionDisposition::Cancelled,
    )
}

pub fn runtime_claim_is_current(
    snapshot: &RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
) -> std::result::Result<bool, RuntimeTaskMutationError> {
    let Some(task) = snapshot.tasks.iter().find(|task| task.spec.id == task_id) else {
        return Ok(false);
    };
    claim_matches_task(snapshot.revision, task, claim)
}

fn claimed_task_mut<'a>(
    snapshot: &'a mut RuntimePlanSnapshot,
    task_id: &str,
    claim: &TaskClaim,
) -> std::result::Result<Option<&'a mut Task>, RuntimeTaskMutationError> {
    let Some(position) = snapshot
        .tasks
        .iter()
        .position(|task| task.spec.id == task_id)
    else {
        return Ok(None);
    };
    let Some(task) = snapshot.tasks.get(position) else {
        return Ok(None);
    };
    if !claim_matches_task(snapshot.revision, task, claim)? {
        return Ok(None);
    }
    Ok(snapshot.tasks.get_mut(position))
}

fn claim_matches_task(
    snapshot_revision: u64,
    task: &Task,
    claim: &TaskClaim,
) -> std::result::Result<bool, RuntimeTaskMutationError> {
    if claim.revision > snapshot_revision
        || !task.execution.status.is_running()
        || task.execution.claim.as_ref() != Some(claim)
    {
        return Ok(false);
    }
    let expected_attempt = task.execution.retry_count.checked_add(1).ok_or_else(|| {
        RuntimeTaskMutationError::AttemptCounterOverflow {
            task_id: task.spec.id.clone(),
        }
    })?;
    if claim.attempt != expected_attempt {
        return Ok(false);
    }
    let spec_hash = task
        .spec
        .stable_hash()
        .map_err(|message| RuntimeTaskMutationError::InvalidTaskSpec { message })?;
    Ok(claim.spec_hash == spec_hash)
}

/// Validate claim/status/attempt/spec invariants for every task in a snapshot.
///
/// The executor calls this after structural DAG validation on every load so a
/// malformed terminal claim fails closed instead of reporting false success.
/// A Running task without a claim remains valid externally managed in-flight
/// work; run-level interruption can still settle its lifecycle.
pub fn validate_runtime_snapshot_claims(
    snapshot: &RuntimePlanSnapshot,
) -> std::result::Result<(), RuntimeTaskMutationError> {
    for task in &snapshot.tasks {
        if let Some(claim) = task.execution.claim.as_ref()
            && !claim_matches_task(snapshot.revision, task, claim)?
        {
            return Err(RuntimeTaskMutationError::InvalidClaim {
                task_id: task.spec.id.clone(),
                message: "claim revision, attempt, specification, or status is stale".to_string(),
            });
        }
        if task.execution.claim.is_none()
            && matches!(task.execution.status, TaskStatus::Retrying { .. })
        {
            return Err(RuntimeTaskMutationError::InvalidClaim {
                task_id: task.spec.id.clone(),
                message: "Retrying task has no physical claim".to_string(),
            });
        }
    }
    Ok(())
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
        let current = snapshot.tasks.first_mut().ok_or_else(|| {
            RuntimeTaskMutationError::InvalidTransition {
                message: "claim ABA fixture is missing".to_string(),
            }
        })?;
        current.execution.retry_count = 1;
        current.execution.failure_fingerprint = Some("newer-attempt".to_string());
        assert_eq!(
            claim_runtime_task(&mut snapshot, &expected, 3)?,
            RuntimeTaskClaimOutcome::ReloadSnapshot
        );
        snapshot.tasks = vec![expected.clone()];
        let claim = match claim_runtime_task(&mut snapshot, &expected, 3)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "claim unexpectedly reloaded".to_string(),
                });
            }
        };
        assert!(runtime_claim_is_current(&snapshot, "a", &claim)?);
        let stale = TaskClaim::new(3, claim.attempt, claim.spec_hash.clone());
        assert_eq!(
            settle_runtime_claim(&mut snapshot, "a", &stale, TaskStatus::Completed)?,
            RuntimeTaskSettlementOutcome::Superseded
        );
        assert_eq!(
            settle_runtime_claim(&mut snapshot, "a", &claim, TaskStatus::Completed)?,
            RuntimeTaskSettlementOutcome::Settled
        );
        let settled =
            snapshot
                .tasks
                .first()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "settled task is missing".to_string(),
                })?;
        assert_eq!(settled.execution.status, TaskStatus::Completed);
        assert!(settled.execution.claim.is_none());
        Ok(())
    }

    #[test]
    fn claim_rejects_consumed_retry_count_above_current_budget()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut invalid = task("retry-budget");
        invalid.spec.max_retries = 1;
        invalid.execution.retry_count = 2;
        let mut snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![invalid.clone()],
        };

        assert_eq!(
            claim_runtime_task(&mut snapshot, &invalid, 1),
            Err(RuntimeTaskMutationError::RetryBudgetInvariant {
                task_id: "retry-budget".to_string(),
                retry_count: 2,
                max_retries: 1,
            })
        );
        assert_eq!(snapshot.tasks, vec![invalid]);
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

        assert_eq!(
            error,
            RuntimeTaskMutationError::SettlementRequiresInactiveStatus {
                status: TaskStatus::Retrying {
                    attempt: claim.attempt,
                    last_error: "transient".to_string(),
                },
            }
        );
        assert_eq!(snapshot.revision, before_revision);
        assert_eq!(snapshot.tasks, before_tasks);
        assert!(runtime_claim_is_current(&snapshot, "a", &claim)?);
        Ok(())
    }

    #[test]
    fn requeue_mutation_preserves_claim_boundaries()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let expected = task("a");
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
        Ok(())
    }

    #[test]
    fn typed_resolution_request_uses_canonical_claim_settlement()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let completed = task("completed");
        let retrying = task("retrying");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 3,
            tasks: vec![completed.clone(), retrying.clone()],
        };
        let completed_claim = match claim_runtime_task(&mut snapshot, &completed, 3)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "completed request claim unexpectedly reloaded".to_string(),
                });
            }
        };
        let retry_claim = match claim_runtime_task(&mut snapshot, &retrying, 3)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "retry request claim unexpectedly reloaded".to_string(),
                });
            }
        };

        assert_eq!(
            settle_runtime_resolution(
                &mut snapshot,
                "completed",
                &completed_claim,
                RuntimeTaskResolutionRequest::Completed,
            )?,
            RuntimeTaskResolution::Completed
        );
        assert_eq!(
            settle_runtime_resolution(
                &mut snapshot,
                "retrying",
                &retry_claim,
                RuntimeTaskResolutionRequest::Requeue {
                    failure_fingerprint: Some("transient".to_string()),
                    error: "retry later".to_string(),
                },
            )?,
            RuntimeTaskResolution::Pending
        );
        assert_eq!(
            settle_runtime_resolution(
                &mut snapshot,
                "completed",
                &completed_claim,
                RuntimeTaskResolutionRequest::Failed {
                    error: "late".to_string(),
                },
            )?,
            RuntimeTaskResolution::Superseded
        );
        let completed = snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "completed")
            .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                message: "completed request task is missing".to_string(),
            })?;
        let retrying = snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "retrying")
            .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                message: "retry request task is missing".to_string(),
            })?;
        assert_eq!(completed.execution.status, TaskStatus::Completed);
        assert!(completed.execution.claim.is_none());
        assert_eq!(retrying.execution.status, TaskStatus::Pending);
        assert_eq!(retrying.execution.retry_count, 1);
        assert!(retrying.execution.claim.is_none());
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
            expected = retried.clone();
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
    fn paused_resume_does_not_consume_retry_budget()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut paused = task("paused");
        paused.execution.status = TaskStatus::Paused("user requested pause".to_string());
        paused.execution.retry_count = 1;
        paused.execution.failure_fingerprint = Some("prior-failure".to_string());
        let mut snapshot = RuntimePlanSnapshot {
            revision: 9,
            tasks: vec![paused.clone()],
        };

        assert_eq!(
            resume_runtime_task(&mut snapshot, &paused, 9)?,
            RuntimeTaskResumeOutcome::Resumed
        );
        let resumed =
            snapshot
                .tasks
                .first()
                .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                    message: "resumed task is missing".to_string(),
                })?;
        assert_eq!(resumed.execution.status, TaskStatus::Pending);
        assert_eq!(resumed.execution.retry_count, 1);
        assert_eq!(
            resumed.execution.failure_fingerprint.as_deref(),
            Some("prior-failure")
        );
        assert!(resumed.execution.claim.is_none());
        Ok(())
    }

    #[test]
    fn pause_settles_claim_and_keeps_unstarted_tasks_resumable()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let active = task("active");
        let pending = task("pending");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 4,
            tasks: vec![active.clone(), pending],
        };
        let claim = match claim_runtime_task(&mut snapshot, &active, 4)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "pause fixture claim unexpectedly reloaded".to_string(),
                });
            }
        };

        let receipt = match settle_runtime_interruption(
            &mut snapshot,
            4,
            RuntimeInterruptionDisposition::Paused {
                reason: "operator pause".to_string(),
            },
        )? {
            RuntimeInterruptionSettlementOutcome::Settled(receipt) => receipt,
            RuntimeInterruptionSettlementOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "pause settlement unexpectedly reloaded".to_string(),
                });
            }
        };
        assert_eq!(receipt.interrupted_task_ids, vec!["active".to_string()]);
        assert_eq!(receipt.pending_task_ids, vec!["pending".to_string()]);
        assert!(!runtime_claim_is_current(&snapshot, "active", &claim)?);
        let paused = snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "active")
            .cloned()
            .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                message: "paused task is missing".to_string(),
            })?;
        assert_eq!(paused.execution.retry_count, 0);
        assert!(paused.execution.claim.is_none());
        assert_eq!(
            resume_runtime_task(&mut snapshot, &paused, 4)?,
            RuntimeTaskResumeOutcome::Resumed
        );
        let resumed = snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "active")
            .ok_or_else(|| RuntimeTaskMutationError::InvalidTransition {
                message: "resumed active task is missing".to_string(),
            })?;
        assert_eq!(resumed.execution.status, TaskStatus::Pending);
        assert_eq!(resumed.execution.retry_count, 0);
        Ok(())
    }

    #[test]
    fn cancellation_retains_terminal_sibling_and_cancels_all_unfinished()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let mut completed = task("completed");
        completed.execution.status = TaskStatus::Completed;
        let active = task("active");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 6,
            tasks: vec![completed, active.clone(), task("pending")],
        };
        let _claim = match claim_runtime_task(&mut snapshot, &active, 6)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "cancel fixture claim unexpectedly reloaded".to_string(),
                });
            }
        };
        let before = snapshot.tasks.clone();
        assert_eq!(
            cancel_unfinished_runtime_tasks(&mut snapshot, 5)?,
            RuntimeInterruptionSettlementOutcome::ReloadSnapshot
        );
        assert_eq!(snapshot.tasks, before);

        let receipt = match cancel_unfinished_runtime_tasks(&mut snapshot, 6)? {
            RuntimeInterruptionSettlementOutcome::Settled(receipt) => receipt,
            RuntimeInterruptionSettlementOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "cancel settlement unexpectedly reloaded".to_string(),
                });
            }
        };
        assert_eq!(
            receipt.interrupted_task_ids,
            vec!["active".to_string(), "pending".to_string()]
        );
        assert_eq!(
            receipt.retained_terminal_task_ids,
            vec!["completed".to_string()]
        );
        for task in &snapshot.tasks {
            if task.spec.id == "completed" {
                assert_eq!(task.execution.status, TaskStatus::Completed);
            } else {
                assert_eq!(task.execution.status, TaskStatus::Cancelled);
            }
            assert!(task.execution.claim.is_none());
        }
        Ok(())
    }

    #[test]
    fn claims_survive_newer_graph_revision_but_corrupt_identity_fails_closed()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        let expected = task("claimed");
        let mut snapshot = RuntimePlanSnapshot {
            revision: 2,
            tasks: vec![expected.clone()],
        };
        let claim = match claim_runtime_task(&mut snapshot, &expected, 2)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "stale fixture claim unexpectedly reloaded".to_string(),
                });
            }
        };
        snapshot.revision = 3;
        assert!(runtime_claim_is_current(&snapshot, "claimed", &claim)?);
        assert_eq!(
            settle_runtime_claim(&mut snapshot, "claimed", &claim, TaskStatus::Completed)?,
            RuntimeTaskSettlementOutcome::Settled
        );

        let expected = task("corrupt");
        let mut corrupt_snapshot = RuntimePlanSnapshot {
            revision: 2,
            tasks: vec![expected.clone()],
        };
        let corrupt_claim = match claim_runtime_task(&mut corrupt_snapshot, &expected, 2)? {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "corrupt fixture claim unexpectedly reloaded".to_string(),
                });
            }
        };
        let claimed = corrupt_snapshot.tasks.first_mut().ok_or_else(|| {
            RuntimeTaskMutationError::InvalidTransition {
                message: "claimed fixture is missing".to_string(),
            }
        })?;
        claimed.spec.title = "changed after claim".to_string();
        assert!(!runtime_claim_is_current(
            &corrupt_snapshot,
            "corrupt",
            &corrupt_claim
        )?);
        let before = corrupt_snapshot.tasks.clone();
        assert!(matches!(
            settle_runtime_interruption(
                &mut corrupt_snapshot,
                2,
                RuntimeInterruptionDisposition::Cancelled,
            ),
            Err(RuntimeTaskMutationError::InvalidClaim { .. })
        ));
        assert_eq!(corrupt_snapshot.tasks, before);
        Ok(())
    }

    #[test]
    fn snapshot_claim_validation_rejects_inactive_claims_and_allows_external_running()
    -> std::result::Result<(), RuntimeTaskMutationError> {
        for status in [TaskStatus::Pending, TaskStatus::Completed] {
            let mut invalid = task("invalid-claim");
            invalid.execution.status = status;
            invalid.execution.claim = Some(TaskClaim::new(
                1,
                1,
                invalid
                    .spec
                    .stable_hash()
                    .map_err(|message| RuntimeTaskMutationError::InvalidTaskSpec { message })?,
            ));
            let snapshot = RuntimePlanSnapshot {
                revision: 1,
                tasks: vec![invalid],
            };
            assert!(matches!(
                validate_runtime_snapshot_claims(&snapshot),
                Err(RuntimeTaskMutationError::InvalidClaim { .. })
            ));
        }

        let mut retrying_without_claim = task("retrying-without-claim");
        retrying_without_claim.execution.status = TaskStatus::Retrying {
            attempt: 1,
            last_error: "transient".to_string(),
        };
        let invalid_retrying = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![retrying_without_claim],
        };
        assert!(matches!(
            validate_runtime_snapshot_claims(&invalid_retrying),
            Err(RuntimeTaskMutationError::InvalidClaim { .. })
        ));

        let mut external = task("external");
        external.execution.status = TaskStatus::Running;
        let mut snapshot = RuntimePlanSnapshot {
            revision: 2,
            tasks: vec![external],
        };
        validate_runtime_snapshot_claims(&snapshot)?;
        let receipt = match settle_runtime_interruption(
            &mut snapshot,
            2,
            RuntimeInterruptionDisposition::Paused {
                reason: "pause external work".to_string(),
            },
        )? {
            RuntimeInterruptionSettlementOutcome::Settled(receipt) => receipt,
            RuntimeInterruptionSettlementOutcome::ReloadSnapshot => {
                return Err(RuntimeTaskMutationError::InvalidTransition {
                    message: "external pause unexpectedly reloaded".to_string(),
                });
            }
        };
        assert_eq!(receipt.interrupted_task_ids, vec!["external".to_string()]);
        assert_eq!(
            snapshot.tasks.first().map(|task| &task.execution.status),
            Some(&TaskStatus::Paused("pause external work".to_string()))
        );
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
            TaskStatus::Paused("paused".to_string()),
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
