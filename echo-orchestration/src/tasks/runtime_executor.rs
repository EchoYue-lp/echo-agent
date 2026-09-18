//! Store- and policy-driven DAG execution for dynamic Agent plans.
//!
//! The framework owns dependency traversal, revision safe points, bounded
//! Subagent waves, cancellation, failure propagation, and stall detection.
//! Applications provide persistence, dispatch, review, and product policy
//! through [`RuntimeDagController`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use echo_core::agent::ExecutionAdmission;
use echo_core::error::{ReactError, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, watch};
use tokio::task::{AbortHandle, Id, JoinSet};
use tokio_util::sync::CancellationToken;

use super::runtime::{
    DagDependencyState, DagExecutionState, NestedDelegationPolicy, RuntimeInterruptionDisposition,
    Task, TaskClaim, TaskId, TaskStatus, TaskSubagentContext,
};
use super::runtime_service::{
    RuntimeInterruptionSettlementOutcome, RuntimeTaskAttemptInterruptDisposition,
    RuntimeTaskAttemptInterruptProjectionError, RuntimeTaskSettlementOutcome,
    validate_runtime_snapshot_claims,
};
use crate::planning::PlanValidator;

/// One coherent plan revision loaded from the runtime authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimePlanSnapshot {
    pub revision: u64,
    pub tasks: Vec<Task>,
}

/// Product decision when a task cannot proceed automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStopDisposition {
    Fail,
    Pause,
}

/// Uncommitted application assessment of one completed dispatch attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskResolutionRequest {
    Completed,
    Requeue {
        failure_fingerprint: Option<String>,
        error: String,
        exhaustion: RuntimeRetryExhaustion,
    },
    Skipped,
    Failed {
        error: String,
    },
    TimedOut {
        error: String,
    },
    Blocked {
        error: String,
        disposition: RuntimeStopDisposition,
    },
    Cancelled,
}

/// Committed result of applying a typed resolution request to one exact claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskResolution {
    Completed,
    Pending,
    Skipped,
    Failed {
        error: String,
    },
    TimedOut {
        error: String,
    },
    Blocked {
        error: String,
        disposition: RuntimeStopDisposition,
    },
    Cancelled,
    Superseded,
}

/// Receipt from the process-local control projection after durable claim
/// settlement. The durable task state is already authoritative when this is
/// returned; a retryable cleanup failure is reported separately and must not
/// trigger a second CAS or reverse the committed terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAttemptControlCleanupReceipt {
    Retired,
    NotFound,
    AlreadyConsumed,
    RetryableFailure { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAttemptControlObservation {
    pub run_id: String,
    pub task_id: TaskId,
    pub claim: TaskClaim,
    pub receipt: RuntimeAttemptControlCleanupReceipt,
}

pub type RuntimeAttemptControlObserver =
    Arc<dyn Fn(&RuntimeAttemptControlObservation) + Send + Sync>;

/// Terminal state used when a requeued claim consumes its final retry.
///
/// The retry decision remains separate from the terminal classification so an
/// application can preserve a typed timeout without mutating canonical task
/// state after framework settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRetryExhaustion {
    Failed,
    TimedOut,
}

/// Result of atomically claiming a task from one loaded plan revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskClaimOutcome {
    Claimed(TaskClaim),
    /// The revision, status, or specification changed. Reload the snapshot;
    /// this is optimistic-concurrency control, not a task failure.
    ReloadSnapshot,
}

/// Terminal settlement for a claim that cannot reach normal resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeClaimAbandonment {
    /// The run stopped and this still-owned claim follows the typed stop policy.
    Interrupted {
        disposition: RuntimeInterruptionDisposition,
    },
    /// Dispatch or settlement infrastructure failed before normal resolution.
    Failed { error: String },
}

/// Terminal result of driving one dynamic plan snapshot sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDagOutcome {
    Completed,
    Failed {
        failed_task_id: TaskId,
        error: String,
    },
    Paused {
        task_id: Option<TaskId>,
        reason: String,
    },
    Stalled {
        reason: String,
    },
    Cancelled,
}

/// Persistence, dispatch, and product-policy adapter for [`super::RuntimeTaskService`].
///
/// `resolve_dispatch` owns application-specific review but returns an
/// uncommitted request. `settle_resolution` provides the adapter transaction
/// boundary and must apply [`super::settle_runtime_resolution`] rather than
/// reimplementing claim, retry, or terminal transitions.
#[async_trait]
pub trait RuntimeDagController: Send + Sync + 'static {
    type DispatchOutput: Send + 'static;

    async fn load_snapshot(&self, run_id: &str) -> Result<RuntimePlanSnapshot>;

    /// Atomically transition one Pending task from `expected_revision` into a
    /// claimed Running attempt.
    async fn claim_task(
        &self,
        run_id: &str,
        task: &Task,
        expected_revision: u64,
    ) -> Result<RuntimeTaskClaimOutcome>;

    /// Verify whether one exact physical claim still owns its task.
    async fn claim_is_current(
        &self,
        run_id: &str,
        task_id: &str,
        claim: &TaskClaim,
    ) -> Result<bool>;

    /// Select a conflict-free subset of the ready frontier.
    ///
    /// The default dispatches the whole frontier. Applications may defer tasks
    /// for product-specific resource or file-ownership policy, but must return
    /// at least one id when `ready_task_ids` is non-empty.
    fn select_ready_wave(&self, _tasks: &[Task], ready_task_ids: Vec<TaskId>) -> Vec<TaskId> {
        ready_task_ids
    }

    async fn dispatch_task(
        &self,
        context: TaskSubagentContext,
        task: Task,
    ) -> Result<Self::DispatchOutput>;

    /// Register the exact claim-derived context before shared admission or a
    /// semaphore can delay dispatch. Implementations may bind the context to a
    /// process-local live-control registry; the durable claim remains the
    /// authority and is validated by the caller before this hook runs.
    async fn reserve_attempt_control(&self, _context: &TaskSubagentContext) -> Result<()> {
        Ok(())
    }

    /// Project a durable exact interrupt into the live reservation/attempt
    /// registry. Returning `true` means the controller accepted the request;
    /// the runtime still re-checks the durable claim before returning.
    async fn request_live_interrupt(
        &self,
        _run_id: &str,
        _task_id: &str,
        _claim: &TaskClaim,
    ) -> std::result::Result<
        Option<RuntimeTaskAttemptInterruptDisposition>,
        RuntimeTaskAttemptInterruptProjectionError,
    > {
        Ok(None)
    }

    /// Retire the process-local control projection after the durable claim has
    /// reached a terminal or superseded outcome. Cleanup is diagnostic and must
    /// never mutate the already committed task state.
    async fn retire_attempt_control(
        &self,
        _run_id: &str,
        _task: &Task,
        _claim: &TaskClaim,
    ) -> RuntimeAttemptControlCleanupReceipt {
        RuntimeAttemptControlCleanupReceipt::Retired
    }

    /// Reconcile process-local attempt projections with the execution IDs
    /// still owned by the durable snapshot for this run.
    async fn reconcile_attempt_control(
        &self,
        _run_id: &str,
        _current_execution_ids: &HashSet<String>,
    ) -> Result<()> {
        Ok(())
    }

    async fn resolve_dispatch(
        &self,
        run_id: &str,
        claim: TaskClaim,
        task: Task,
        dispatch: Result<Self::DispatchOutput>,
    ) -> Result<RuntimeTaskResolutionRequest>;

    /// Atomically apply one framework-owned resolution request and return its
    /// typed committed receipt.
    async fn settle_resolution(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        request: RuntimeTaskResolutionRequest,
    ) -> Result<RuntimeTaskResolution>;

    /// Settle a claim whose dispatch cannot reach normal resolution. The
    /// implementation must use compare-and-set semantics so a superseded
    /// claim cannot overwrite newer work.
    async fn abandon_claim(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        abandonment: RuntimeClaimAbandonment,
    ) -> Result<RuntimeTaskSettlementOutcome>;

    async fn failed_task_disposition(
        &self,
        _run_id: &str,
        _task: &Task,
        all_unfinished_failed_or_blocked: bool,
    ) -> Result<RuntimeStopDisposition> {
        Ok(if all_unfinished_failed_or_blocked {
            RuntimeStopDisposition::Fail
        } else {
            RuntimeStopDisposition::Pause
        })
    }

    /// Map a cancellation-token stop request to product-neutral cancellation or
    /// resumable pause. Framework task settlement remains authoritative.
    async fn interruption_disposition(
        &self,
        _run_id: &str,
    ) -> Result<RuntimeInterruptionDisposition> {
        Ok(RuntimeInterruptionDisposition::Cancelled)
    }

    /// Atomically settle unfinished tasks at an interruption safe point using
    /// [`super::settle_runtime_interruption`]. A revision mismatch must return
    /// `ReloadSnapshot`; adapters must not recreate claim or retry semantics.
    async fn settle_interruption(
        &self,
        run_id: &str,
        expected_revision: u64,
        disposition: RuntimeInterruptionDisposition,
    ) -> Result<RuntimeInterruptionSettlementOutcome>;

    async fn note_stalled(&self, _run_id: &str, _reason: &str) -> Result<()> {
        Ok(())
    }
}

/// Execution configuration accepted by [`super::RuntimeTaskService`].
#[derive(Clone)]
pub struct RuntimeTaskServiceConfig {
    pub max_concurrent_subagents: usize,
    pub external_progress_poll_interval: Duration,
    /// Maximum time to let cancellation-aware Subagents finish their durable
    /// terminal writes before remaining non-cooperative dispatches are aborted.
    pub cancellation_grace_period: Duration,
    pub delegation_policy: NestedDelegationPolicy,
    /// Optional process-wide admission shared with subagent dispatchers.
    pub shared_admission: Option<Arc<ExecutionAdmission>>,
    /// Optional diagnostic observer for post-CAS live-control cleanup.
    pub attempt_control_observer: Option<RuntimeAttemptControlObserver>,
}

impl std::fmt::Debug for RuntimeTaskServiceConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeTaskServiceConfig")
            .field("max_concurrent_subagents", &self.max_concurrent_subagents)
            .field(
                "external_progress_poll_interval",
                &self.external_progress_poll_interval,
            )
            .field("cancellation_grace_period", &self.cancellation_grace_period)
            .field("delegation_policy", &self.delegation_policy)
            .field("shared_admission", &self.shared_admission)
            .field(
                "has_attempt_control_observer",
                &self.attempt_control_observer.is_some(),
            )
            .finish()
    }
}

impl Default for RuntimeTaskServiceConfig {
    fn default() -> Self {
        Self {
            max_concurrent_subagents: 4,
            external_progress_poll_interval: Duration::from_millis(250),
            cancellation_grace_period: Duration::from_secs(5),
            delegation_policy: NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 0,
                max_delegate_depth: 2,
            },
            shared_admission: None,
            attempt_control_observer: None,
        }
    }
}

/// The framework's executor for revisioned dynamic Agent plans.
pub(crate) struct RuntimeDagExecutor<C: RuntimeDagController> {
    pub(crate) controller: Arc<C>,
    pub(crate) config: RuntimeTaskServiceConfig,
    validator: PlanValidator,
    pub(crate) attempt_cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pub(crate) attempt_runs: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) attempt_abort_handles: Arc<Mutex<HashMap<String, AbortHandle>>>,
    attempt_task_ids: Arc<Mutex<HashMap<Id, String>>>,
    pub(crate) pending_attempt_interrupts: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) scheduled_attempt_aborts: Arc<Mutex<HashMap<String, u64>>>,
    attempt_settlements: Arc<Mutex<HashMap<String, AttemptSettlementProjection>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptSettlementPhase {
    Active,
    JoinedAwaitingDurableSettlement,
}

struct AttemptSettlementProjection {
    phase: AttemptSettlementPhase,
    settled_tx: watch::Sender<Option<AttemptSettlementObservation>>,
    cancel: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttemptSettlementObservation {
    Settled { status: String },
    AuthorityUnknown { error: String },
}

pub(crate) enum RuntimeAttemptObservation {
    Active {
        cancel: CancellationToken,
        settled_rx: watch::Receiver<Option<AttemptSettlementObservation>>,
    },
    JoinedAwaitingDurableSettlement {
        settled_rx: watch::Receiver<Option<AttemptSettlementObservation>>,
    },
}

enum InterruptionBoundary {
    Outcome(RuntimeDagOutcome),
    ReloadSnapshot,
    NoUnfinishedWork,
}

impl<C: RuntimeDagController> RuntimeDagExecutor<C> {
    pub(crate) fn new(controller: Arc<C>, config: RuntimeTaskServiceConfig) -> Self {
        Self {
            controller,
            config,
            validator: PlanValidator::default(),
            attempt_cancellations: Arc::new(Mutex::new(HashMap::new())),
            attempt_runs: Arc::new(Mutex::new(HashMap::new())),
            attempt_abort_handles: Arc::new(Mutex::new(HashMap::new())),
            attempt_task_ids: Arc::new(Mutex::new(HashMap::new())),
            pending_attempt_interrupts: Arc::new(Mutex::new(HashMap::new())),
            scheduled_attempt_aborts: Arc::new(Mutex::new(HashMap::new())),
            attempt_settlements: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Override structural limits while retaining the canonical validator.
    pub(crate) fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
    }

    pub(crate) fn schedule_attempt_abort(&self, execution_id: String, rearm: bool) {
        let generation = {
            let Ok(mut scheduled) = self.scheduled_attempt_aborts.lock() else {
                return;
            };
            if scheduled.contains_key(&execution_id) && !rearm {
                return;
            }
            let generation = scheduled
                .get(&execution_id)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            scheduled.insert(execution_id.clone(), generation);
            generation
        };
        let abort_handles = Arc::clone(&self.attempt_abort_handles);
        let scheduled_attempt_aborts = Arc::clone(&self.scheduled_attempt_aborts);
        let grace_period = self.config.cancellation_grace_period;
        tokio::spawn(async move {
            tokio::time::sleep(grace_period).await;
            let is_current = scheduled_attempt_aborts
                .lock()
                .ok()
                .and_then(|scheduled| scheduled.get(&execution_id).copied())
                == Some(generation);
            if !is_current {
                return;
            }
            let handle = abort_handles
                .lock()
                .ok()
                .and_then(|handles| handles.get(&execution_id).cloned());
            if let Some(handle) = handle.filter(|handle| !handle.is_finished()) {
                handle.abort();
            }
            if let Ok(mut scheduled) = scheduled_attempt_aborts.lock()
                && scheduled.get(&execution_id).copied() == Some(generation)
            {
                scheduled.remove(&execution_id);
            }
        });
    }

    pub(crate) fn observe_attempt(&self, execution_id: &str) -> Option<RuntimeAttemptObservation> {
        let settlements = self
            .attempt_settlements
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let projection = settlements.get(execution_id)?;
        let settled_rx = projection.settled_tx.subscribe();
        Some(match projection.phase {
            AttemptSettlementPhase::Active => RuntimeAttemptObservation::Active {
                cancel: projection.cancel.clone(),
                settled_rx,
            },
            AttemptSettlementPhase::JoinedAwaitingDurableSettlement => {
                RuntimeAttemptObservation::JoinedAwaitingDurableSettlement { settled_rx }
            }
        })
    }

    fn mark_attempt_joined(&self, execution_id: &str) {
        if let Ok(mut settlements) = self.attempt_settlements.lock()
            && let Some(projection) = settlements.get_mut(execution_id)
        {
            projection.phase = AttemptSettlementPhase::JoinedAwaitingDurableSettlement;
        }
    }

    pub(crate) fn settle_attempt_projection(&self, execution_id: &str, status: impl Into<String>) {
        if let Ok(settlements) = self.attempt_settlements.lock()
            && let Some(projection) = settlements.get(execution_id)
        {
            projection
                .settled_tx
                .send_replace(Some(AttemptSettlementObservation::Settled {
                    status: status.into(),
                }));
        }
    }

    pub(crate) fn reconcile_local_attempt_control(
        &self,
        run_id: &str,
        current_execution_ids: &HashSet<String>,
    ) {
        let stale_execution_ids = self
            .attempt_runs
            .lock()
            .map(|runs| {
                runs.iter()
                    .filter_map(|(execution_id, attempt_run_id)| {
                        (attempt_run_id == run_id && !current_execution_ids.contains(execution_id))
                            .then_some(execution_id.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for execution_id in stale_execution_ids {
            let phase = self
                .attempt_settlements
                .lock()
                .ok()
                .and_then(|settlements| {
                    settlements
                        .get(&execution_id)
                        .map(|projection| projection.phase)
                });
            self.settle_attempt_projection(&execution_id, "superseded");
            if let Some(cancel) = self
                .attempt_cancellations
                .lock()
                .ok()
                .and_then(|controls| controls.get(&execution_id).cloned())
            {
                cancel.cancel();
            }
            if phase == Some(AttemptSettlementPhase::JoinedAwaitingDurableSettlement) {
                self.retire_local_attempt_control(&execution_id);
            } else {
                self.schedule_attempt_abort(execution_id, false);
            }
        }
        if let Ok(mut pending) = self.pending_attempt_interrupts.lock() {
            pending.retain(|execution_id, pending_run_id| {
                pending_run_id != run_id || current_execution_ids.contains(execution_id)
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn has_attempt_supervisor_state(&self, execution_id: &str) -> bool {
        self.attempt_cancellations
            .lock()
            .is_ok_and(|controls| controls.contains_key(execution_id))
            || self
                .attempt_runs
                .lock()
                .is_ok_and(|runs| runs.contains_key(execution_id))
            || self
                .attempt_abort_handles
                .lock()
                .is_ok_and(|handles| handles.contains_key(execution_id))
            || self
                .pending_attempt_interrupts
                .lock()
                .is_ok_and(|pending| pending.contains_key(execution_id))
            || self
                .scheduled_attempt_aborts
                .lock()
                .is_ok_and(|scheduled| scheduled.contains_key(execution_id))
            || self
                .attempt_settlements
                .lock()
                .is_ok_and(|settlements| settlements.contains_key(execution_id))
    }

    fn mark_attempt_authority_unknown(&self, execution_id: &str, error: impl Into<String>) {
        if let Ok(settlements) = self.attempt_settlements.lock()
            && let Some(projection) = settlements.get(execution_id)
        {
            projection.settled_tx.send_replace(Some(
                AttemptSettlementObservation::AuthorityUnknown {
                    error: error.into(),
                },
            ));
        }
    }

    fn retire_local_attempt_control(&self, execution_id: &str) {
        if let Ok(mut controls) = self.attempt_cancellations.lock() {
            controls.remove(execution_id);
        }
        if let Ok(mut runs) = self.attempt_runs.lock() {
            runs.remove(execution_id);
        }
        if let Ok(mut handles) = self.attempt_abort_handles.lock() {
            handles.remove(execution_id);
        }
        if let Ok(mut pending) = self.pending_attempt_interrupts.lock() {
            pending.remove(execution_id);
        }
        if let Ok(mut scheduled) = self.scheduled_attempt_aborts.lock() {
            scheduled.remove(execution_id);
        }
        if let Ok(mut settlements) = self.attempt_settlements.lock() {
            settlements.remove(execution_id);
        }
    }

    pub(crate) fn observe_attempt_control_cleanup(
        &self,
        run_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        receipt: RuntimeAttemptControlCleanupReceipt,
    ) {
        if let Some(observer) = &self.config.attempt_control_observer {
            observer(&RuntimeAttemptControlObservation {
                run_id: run_id.to_string(),
                task_id: task_id.to_string(),
                claim: claim.clone(),
                receipt,
            });
        }
    }

    pub(crate) async fn execute(
        &self,
        run_id: &str,
        cancel: CancellationToken,
    ) -> Result<RuntimeDagOutcome> {
        let subagent_semaphore =
            Arc::new(Semaphore::new(self.config.max_concurrent_subagents.max(1)));
        let mut active_revision: Option<u64> = None;
        let mut failure_errors: HashMap<TaskId, String> = HashMap::new();

        loop {
            // Every loop boundary is a safe point: all locally-dispatched
            // handles from the previous wave have been joined and resolved.
            let snapshot = self.controller.load_snapshot(run_id).await?;
            if let Err(errors) = self.validator.validate_task_snapshot(&snapshot.tasks) {
                return Err(ReactError::Other(format!(
                    "invalid runtime plan snapshot: {}",
                    errors.join("; ")
                )));
            }
            validate_runtime_snapshot_claims(&snapshot).map_err(|error| {
                ReactError::Other(format!("invalid runtime claim snapshot: {error}"))
            })?;
            if active_revision != Some(snapshot.revision) {
                if let Some(previous) = active_revision {
                    tracing::info!(
                        run_id,
                        from_revision = previous,
                        to_revision = snapshot.revision,
                        "runtime DAG executor applied a plan revision at a safe point"
                    );
                }
                active_revision = Some(snapshot.revision);
            }

            let tasks = snapshot.tasks;
            let state = DagExecutionState::from_tasks(&tasks);

            if !state.cancelled.is_empty() {
                match self
                    .settle_interruption_boundary(
                        run_id,
                        snapshot.revision,
                        RuntimeInterruptionDisposition::Cancelled,
                        false,
                    )
                    .await?
                {
                    InterruptionBoundary::Outcome(outcome) => return Ok(outcome),
                    InterruptionBoundary::ReloadSnapshot => continue,
                    InterruptionBoundary::NoUnfinishedWork => {}
                }
            }

            if state.all_completed(&tasks) {
                return Ok(RuntimeDagOutcome::Completed);
            }

            let durable_pause = tasks
                .iter()
                .find(|task| matches!(task.execution.status, TaskStatus::Paused(_)))
                .map(|paused_task| RuntimeInterruptionDisposition::Paused {
                    reason: persisted_status_error(&paused_task.execution.status)
                        .unwrap_or_else(|| "runtime task paused".to_string()),
                });
            let requested_interruption = if cancel.is_cancelled() {
                Some(self.controller.interruption_disposition(run_id).await?)
            } else {
                None
            };
            let interruption =
                prioritize_interruption(durable_pause.clone(), requested_interruption);
            if let Some(disposition) = interruption {
                match self
                    .settle_interruption_boundary(
                        run_id,
                        snapshot.revision,
                        disposition,
                        durable_pause.is_none(),
                    )
                    .await?
                {
                    InterruptionBoundary::Outcome(outcome) => return Ok(outcome),
                    InterruptionBoundary::ReloadSnapshot => continue,
                    InterruptionBoundary::NoUnfinishedWork => {}
                }
            }

            if let Some(failed_task) = tasks
                .iter()
                .find(|task| state.failed.contains(&task.spec.id))
            {
                let dependency_states = state.dependency_states(&tasks);
                let derived_blocked: Vec<_> = dependency_states
                    .iter()
                    .filter_map(|(task_id, dependency_state)| {
                        matches!(
                            dependency_state,
                            DagDependencyState::BlockedByFailure { .. }
                        )
                        .then_some(task_id)
                    })
                    .collect();
                tracing::debug!(
                    run_id,
                    failed_task_id = %failed_task.spec.id,
                    derived_blocked_task_ids = ?derived_blocked,
                    "runtime DAG derived dependency blockers from the current snapshot"
                );

                let error = failure_errors
                    .remove(&failed_task.spec.id)
                    .or_else(|| persisted_status_error(&failed_task.execution.status))
                    .unwrap_or_else(|| format!("task '{}' failed", failed_task.spec.title));
                let disposition = self
                    .controller
                    .failed_task_disposition(
                        run_id,
                        failed_task,
                        state.all_unfinished_failed_or_blocked(&tasks),
                    )
                    .await?;
                return Ok(stop_outcome(
                    disposition,
                    failed_task.spec.id.clone(),
                    error,
                ));
            }

            let ready_task_ids = state.ready_task_ids(&tasks);
            if ready_task_ids.is_empty() {
                if !state.in_flight.is_empty() {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            let disposition = self.controller.interruption_disposition(run_id).await?;
                            match self
                                .settle_interruption_boundary(
                                    run_id,
                                    snapshot.revision,
                                    disposition,
                                    true,
                                )
                                .await?
                            {
                                InterruptionBoundary::Outcome(outcome) => return Ok(outcome),
                                InterruptionBoundary::ReloadSnapshot
                                | InterruptionBoundary::NoUnfinishedWork => {}
                            }
                        }
                        _ = tokio::time::sleep(self.config.external_progress_poll_interval) => {}
                    }
                    continue;
                }

                if let Some(blocked_task) = tasks
                    .iter()
                    .find(|task| matches!(task.execution.status, TaskStatus::Blocked(_)))
                {
                    let error = persisted_status_error(&blocked_task.execution.status)
                        .unwrap_or_else(|| format!("task '{}' blocked", blocked_task.spec.title));
                    let disposition = self
                        .controller
                        .failed_task_disposition(
                            run_id,
                            blocked_task,
                            state.all_unfinished_failed_or_blocked(&tasks),
                        )
                        .await?;
                    return Ok(stop_outcome(
                        disposition,
                        blocked_task.spec.id.clone(),
                        error,
                    ));
                }

                let reason = "DAG stalled with unfinished tasks (cycle or blocked)";
                self.controller.note_stalled(run_id, reason).await?;
                return Ok(RuntimeDagOutcome::Stalled {
                    reason: reason.to_string(),
                });
            }

            let selected_ids = self
                .controller
                .select_ready_wave(&tasks, ready_task_ids.clone());
            let selected_ids = validate_selected_wave(&ready_task_ids, selected_ids)?;
            let selected_set: HashSet<&str> = selected_ids.iter().map(String::as_str).collect();
            let dependency_states = state.dependency_states(&tasks);
            let selected_tasks: Vec<Task> = tasks
                .iter()
                .filter(|task| selected_set.contains(task.spec.id.as_str()))
                .cloned()
                .collect();

            tracing::info!(
                run_id,
                revision = snapshot.revision,
                ready_count = ready_task_ids.len(),
                selected_count = selected_tasks.len(),
                selected_tasks = ?selected_ids,
                completed_count = state.completed.len(),
                skipped_count = state.skipped.len(),
                total_count = tasks.len(),
                "runtime DAG executor dispatching wave"
            );

            let mut join_set = JoinSet::new();
            let mut outstanding_claims: HashMap<String, (Task, TaskClaim)> = HashMap::new();
            let mut wave_errors = Vec::new();
            for task in selected_tasks {
                if cancel.is_cancelled() {
                    break;
                }
                let controller = self.controller.clone();
                let semaphore = subagent_semaphore.clone();
                let shared_admission = self.config.shared_admission.clone();
                let claim = match self
                    .controller
                    .claim_task(run_id, &task, snapshot.revision)
                    .await
                {
                    Ok(RuntimeTaskClaimOutcome::Claimed(claim)) => claim,
                    Ok(RuntimeTaskClaimOutcome::ReloadSnapshot) => continue,
                    Err(error) => {
                        wave_errors.push(error.to_string());
                        break;
                    }
                };
                let claim_id = claim.claim_id.clone();
                // Each claim owns a child token: cancelling one exact attempt
                // must not cancel sibling tasks in the same ready wave. The
                // token is published before waiting for shared admission.
                let task_cancel = cancel.child_token();
                let execution_id = claim.execution_id(run_id, &task.spec.id);
                let pending_interrupt = self
                    .pending_attempt_interrupts
                    .lock()
                    .ok()
                    .is_some_and(|mut pending| pending.remove(&execution_id).is_some());
                if let Ok(mut controls) = self.attempt_cancellations.lock() {
                    controls.insert(execution_id.clone(), task_cancel.clone());
                }
                if let Ok(mut runs) = self.attempt_runs.lock() {
                    runs.insert(execution_id.clone(), run_id.to_string());
                }
                if let Ok(mut settlements) = self.attempt_settlements.lock() {
                    let (settled_tx, _) = watch::channel(None);
                    settlements.insert(
                        execution_id.clone(),
                        AttemptSettlementProjection {
                            phase: AttemptSettlementPhase::Active,
                            settled_tx,
                            cancel: task_cancel.clone(),
                        },
                    );
                }
                if pending_interrupt {
                    task_cancel.cancel();
                }
                let dispatch_run_id = run_id.to_string();
                let delegation_policy = self.config.delegation_policy;
                let waived_dependency_ids = match dependency_states.get(&task.spec.id) {
                    Some(DagDependencyState::Satisfied {
                        waived_dependency_ids,
                    }) => waived_dependency_ids.clone(),
                    _ => Vec::new(),
                };
                let context = match TaskSubagentContext::from_claim(
                    dispatch_run_id.clone(),
                    task.spec.id.clone(),
                    claim.clone(),
                    task_cancel.clone(),
                ) {
                    Ok(context) => context
                        .with_delegation_policy(delegation_policy)
                        .with_waived_dependencies(waived_dependency_ids),
                    Err(error) => {
                        let message = format!("invalid exact task context: {error}");
                        match self
                            .controller
                            .abandon_claim(
                                run_id,
                                &claim,
                                &task,
                                RuntimeClaimAbandonment::Failed {
                                    error: message.clone(),
                                },
                            )
                            .await
                        {
                            Ok(_) => {
                                self.cleanup_attempt_control(run_id, &task, &claim).await;
                            }
                            Err(settlement_error) => wave_errors.push(format!(
                                "invalid context abandonment outcome is unknown: {settlement_error}"
                            )),
                        }
                        wave_errors.push(message);
                        continue;
                    }
                };
                if let Err(error) = self.controller.reserve_attempt_control(&context).await {
                    let message = format!("exact attempt control reservation failed: {error}");
                    match self
                        .controller
                        .abandon_claim(
                            run_id,
                            &claim,
                            &task,
                            RuntimeClaimAbandonment::Failed {
                                error: message.clone(),
                            },
                        )
                        .await
                    {
                        Ok(_) => {
                            self.cleanup_attempt_control(run_id, &task, &claim).await;
                        }
                        Err(settlement_error) => {
                            wave_errors.push(format!(
                                "reservation failure abandonment outcome is unknown: {settlement_error}"
                            ));
                        }
                    }
                    wave_errors.push(message);
                    continue;
                }
                outstanding_claims.insert(claim_id.clone(), (task.clone(), claim.clone()));
                let abort_execution_id = execution_id.clone();
                let supervisor_cancel = task_cancel.clone();
                let abort_handle = join_set.spawn(async move {
                    let dispatch = if let Some(admission) = shared_admission {
                        let lease = tokio::select! {
                            biased;
                            _ = task_cancel.cancelled() => None,
                            lease = admission.issue_wait(format!("runtime:{dispatch_run_id}:{claim_id}")) => Some(
                                lease.map_err(|error| ReactError::Other(format!("shared execution admission rejected task: {error}")))
                            ),
                        };
                        match lease {
                            Some(Ok(lease)) => {
                                let result = controller.dispatch_task(context, task).await;
                                drop(lease);
                                result
                            }
                            Some(Err(error)) => Err(error),
                            None => controller.dispatch_task(context, task).await,
                        }
                    } else {
                        let permit = tokio::select! {
                            biased;
                            _ = task_cancel.cancelled() => None,
                            permit = semaphore.acquire_owned() => Some(permit),
                        };
                        match permit {
                            Some(Ok(permit)) => {
                                let result = controller.dispatch_task(context, task).await;
                                drop(permit);
                                result
                            }
                            Some(Err(error)) => Err(ReactError::Other(format!(
                                "Subagent semaphore closed: {error}"
                            ))),
                            None => controller.dispatch_task(context, task).await,
                        }
                    };
                    (claim_id, dispatch)
                });
                if let Ok(mut task_ids) = self.attempt_task_ids.lock() {
                    task_ids.insert(abort_handle.id(), abort_execution_id.clone());
                }
                if let Ok(mut handles) = self.attempt_abort_handles.lock() {
                    handles.insert(abort_execution_id.clone(), abort_handle);
                }
                if supervisor_cancel.is_cancelled() {
                    self.schedule_attempt_abort(abort_execution_id, true);
                }
            }

            let mut wave_results = Vec::new();
            let mut cancellation_observed = false;
            let cancellation_grace = tokio::time::sleep(Duration::ZERO);
            tokio::pin!(cancellation_grace);
            while !join_set.is_empty() {
                tokio::select! {
                    biased;
                    joined = join_set.join_next_with_id() => {
                        match joined {
                            Some(Ok((task_id, result))) => {
                                if let Some(execution_id) = self
                                    .attempt_task_ids
                                    .lock()
                                    .ok()
                                    .and_then(|mut task_ids| task_ids.remove(&task_id))
                                {
                                    self.mark_attempt_joined(&execution_id);
                                }
                                wave_results.push((
                                    result,
                                    !cancellation_observed && !cancel.is_cancelled(),
                                ));
                            }
                            Some(Err(error)) => {
                                let execution_id = self
                                    .attempt_task_ids
                                    .lock()
                                    .ok()
                                    .and_then(|mut task_ids| task_ids.remove(&error.id()));
                                if let Some(execution_id) = execution_id {
                                    self.mark_attempt_joined(&execution_id);
                                    let cancelled = self
                                        .attempt_cancellations
                                        .lock()
                                        .ok()
                                        .and_then(|controls| controls.get(&execution_id).cloned())
                                        .is_some_and(|token| token.is_cancelled());
                                    let claim_id = cancelled.then(|| {
                                        outstanding_claims
                                            .iter()
                                            .find(|(_, (task, claim))| {
                                                claim.execution_id(run_id, &task.spec.id)
                                                    == execution_id
                                            })
                                            .map(|(claim_id, _)| claim_id.clone())
                                    }).flatten();
                                    if error.is_cancelled() && let Some(claim_id) = claim_id {
                                        wave_results.push((
                                            (
                                                claim_id,
                                                Err(ReactError::Agent(Box::new(
                                                    echo_core::error::AgentError::Cancelled(
                                                        "exact attempt aborted after cancellation grace period".to_string(),
                                                    ),
                                                ))),
                                            ),
                                            false,
                                        ));
                                    } else {
                                        wave_errors.push(format!(
                                            "Subagent dispatch task '{execution_id}' failed to join: {error}"
                                        ));
                                    }
                                } else {
                                    wave_errors.push(format!(
                                        "Subagent dispatch task failed to join: {error}"
                                    ));
                                }
                            }
                            None => {}
                        }
                    }
                    _ = cancel.cancelled(), if !cancellation_observed => {
                        cancellation_observed = true;
                        cancellation_grace.as_mut().reset(
                            tokio::time::Instant::now() + self.config.cancellation_grace_period,
                        );
                    }
                    _ = &mut cancellation_grace, if cancellation_observed => {
                        join_set.abort_all();
                        while let Some(joined) = join_set.join_next_with_id().await {
                            match joined {
                                Ok((task_id, result)) => {
                                    if let Some(execution_id) = self
                                        .attempt_task_ids
                                        .lock()
                                        .ok()
                                        .and_then(|mut task_ids| task_ids.remove(&task_id))
                                    {
                                        self.mark_attempt_joined(&execution_id);
                                    }
                                    wave_results.push((result, false));
                                }
                                Err(error) if error.is_cancelled() => {
                                    if let Some(execution_id) = self
                                        .attempt_task_ids
                                        .lock()
                                        .ok()
                                        .and_then(|mut task_ids| task_ids.remove(&error.id()))
                                    {
                                        self.mark_attempt_joined(&execution_id);
                                    }
                                }
                                Err(error) => {
                                    if let Some(execution_id) = self
                                        .attempt_task_ids
                                        .lock()
                                        .ok()
                                        .and_then(|mut task_ids| task_ids.remove(&error.id()))
                                    {
                                        self.mark_attempt_joined(&execution_id);
                                    }
                                    wave_errors.push(format!(
                                        "Subagent dispatch task failed to join: {error}"
                                    ));
                                }
                            }
                        }
                        break;
                    }
                }
            }

            // Joined attempts remain addressable until their durable claim CAS
            // is proven settled or superseded. Cleanup happens after that point.

            let mut interruption_policy_failed = false;
            let mut interruption_is_durable = false;
            let mut interruption = if cancellation_observed || cancel.is_cancelled() {
                match self.controller.interruption_disposition(run_id).await {
                    Ok(disposition) => Some(disposition),
                    Err(error) => {
                        interruption_policy_failed = true;
                        wave_errors.push(format!(
                            "failed to resolve runtime interruption disposition: {error}"
                        ));
                        Some(RuntimeInterruptionDisposition::Paused {
                            reason: format!(
                                "interruption policy unavailable; preserved for resume: {error}"
                            ),
                        })
                    }
                }
            } else {
                None
            };
            let mut pending_outcome = None;
            for ((claim_id, dispatch), completed_before_interruption) in wave_results {
                let Some((task, claim)) = outstanding_claims.remove(&claim_id) else {
                    wave_errors.push(format!("dispatch returned unknown claim '{claim_id}'"));
                    continue;
                };
                if interruption.is_some() && !completed_before_interruption && dispatch.is_err() {
                    let disposition = interruption
                        .clone()
                        .ok_or_else(|| ReactError::Other("interruption disappeared".to_string()))?;
                    match self
                        .settle_abandonment(
                            run_id,
                            &claim,
                            &task,
                            RuntimeClaimAbandonment::Interrupted { disposition },
                        )
                        .await
                    {
                        Ok(_) => {
                            self.cleanup_attempt_control(run_id, &task, &claim).await;
                        }
                        Err(error) => {
                            self.mark_attempt_authority_unknown(
                                &claim.execution_id(run_id, &task.spec.id),
                                error.to_string(),
                            );
                            wave_errors.push(error.to_string());
                        }
                    }
                    continue;
                }
                let request = match self
                    .controller
                    .resolve_dispatch(run_id, claim.clone(), task.clone(), dispatch)
                    .await
                {
                    Ok(resolution) => resolution,
                    Err(error) => {
                        let message = error.to_string();
                        match self
                            .controller
                            .claim_is_current(run_id, &task.spec.id, &claim)
                            .await
                        {
                            Ok(false) => {
                                self.settle_attempt_projection(
                                    &claim.execution_id(run_id, &task.spec.id),
                                    "superseded",
                                );
                                self.cleanup_attempt_control(run_id, &task, &claim).await;
                                continue;
                            }
                            Ok(true) => match self
                                .settle_abandonment(
                                    run_id,
                                    &claim,
                                    &task,
                                    RuntimeClaimAbandonment::Failed {
                                        error: message.clone(),
                                    },
                                )
                                .await
                            {
                                Ok(_) => {
                                    self.cleanup_attempt_control(run_id, &task, &claim).await;
                                }
                                Err(abandon_error) => {
                                    self.mark_attempt_authority_unknown(
                                        &claim.execution_id(run_id, &task.spec.id),
                                        abandon_error.to_string(),
                                    );
                                    wave_errors.push(abandon_error.to_string());
                                }
                            },
                            Err(lookup_error) => {
                                let lookup_message = format!(
                                    "runtime settlement outcome is unknown and claim lookup failed: {lookup_error}"
                                );
                                self.mark_attempt_authority_unknown(
                                    &claim.execution_id(run_id, &task.spec.id),
                                    lookup_message.clone(),
                                );
                                wave_errors.push(lookup_message);
                            }
                        }
                        wave_errors.push(message);
                        continue;
                    }
                };
                let resolution = match self
                    .settle_dispatch_request(run_id, &claim, &task, request)
                    .await
                {
                    Ok(resolution) => resolution,
                    Err(error) => {
                        let message = error.to_string();
                        match self
                            .controller
                            .claim_is_current(run_id, &task.spec.id, &claim)
                            .await
                        {
                            Ok(false) => {
                                self.settle_attempt_projection(
                                    &claim.execution_id(run_id, &task.spec.id),
                                    "superseded",
                                );
                                self.cleanup_attempt_control(run_id, &task, &claim).await;
                                continue;
                            }
                            Ok(true) => match self
                                .settle_abandonment(
                                    run_id,
                                    &claim,
                                    &task,
                                    RuntimeClaimAbandonment::Failed {
                                        error: message.clone(),
                                    },
                                )
                                .await
                            {
                                Ok(_) => {
                                    self.cleanup_attempt_control(run_id, &task, &claim).await;
                                }
                                Err(abandon_error) => {
                                    self.mark_attempt_authority_unknown(
                                        &claim.execution_id(run_id, &task.spec.id),
                                        abandon_error.to_string(),
                                    );
                                    wave_errors.push(abandon_error.to_string());
                                }
                            },
                            Err(lookup_error) => {
                                let lookup_message = format!(
                                    "runtime settlement outcome is unknown and claim lookup failed: {lookup_error}"
                                );
                                self.mark_attempt_authority_unknown(
                                    &claim.execution_id(run_id, &task.spec.id),
                                    lookup_message.clone(),
                                );
                                wave_errors.push(lookup_message);
                            }
                        }
                        wave_errors.push(message);
                        continue;
                    }
                };
                self.cleanup_attempt_control(run_id, &task, &claim).await;
                match resolution {
                    RuntimeTaskResolution::Completed
                    | RuntimeTaskResolution::Pending
                    | RuntimeTaskResolution::Skipped
                    | RuntimeTaskResolution::Superseded => {}
                    RuntimeTaskResolution::Failed { error }
                    | RuntimeTaskResolution::TimedOut { error } => {
                        failure_errors.entry(task.spec.id).or_insert(error);
                    }
                    RuntimeTaskResolution::Blocked { error, disposition } => {
                        if pending_outcome.is_none() {
                            pending_outcome = Some(stop_outcome(disposition, task.spec.id, error));
                        }
                    }
                    RuntimeTaskResolution::Cancelled => {
                        interruption = prioritize_interruption(
                            interruption,
                            Some(RuntimeInterruptionDisposition::Cancelled),
                        );
                        interruption_is_durable = true;
                    }
                }
            }

            if cancel.is_cancelled() && !interruption_policy_failed {
                match self.controller.interruption_disposition(run_id).await {
                    Ok(disposition) => {
                        interruption = prioritize_interruption(interruption, Some(disposition));
                    }
                    Err(error) => {
                        interruption_policy_failed = true;
                        wave_errors.push(format!(
                            "failed to resolve runtime interruption disposition at wave boundary: {error}"
                        ));
                        interruption = prioritize_interruption(
                            interruption,
                            Some(RuntimeInterruptionDisposition::Paused {
                                reason: format!(
                                    "interruption policy unavailable; preserved for resume: {error}"
                                ),
                            }),
                        );
                    }
                }
            }

            for (_claim_id, (task, claim)) in outstanding_claims {
                let abandonment = if let Some(disposition) = interruption.clone() {
                    RuntimeClaimAbandonment::Interrupted { disposition }
                } else {
                    RuntimeClaimAbandonment::Failed {
                        error: wave_errors
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "dispatch ended without a result".to_string()),
                    }
                };
                match self
                    .settle_abandonment(run_id, &claim, &task, abandonment)
                    .await
                {
                    Ok(_) => {
                        self.cleanup_attempt_control(run_id, &task, &claim).await;
                    }
                    Err(error) => {
                        self.mark_attempt_authority_unknown(
                            &claim.execution_id(run_id, &task.spec.id),
                            error.to_string(),
                        );
                        wave_errors.push(error.to_string());
                    }
                }
            }

            if interruption_policy_failed {
                let cleanup_disposition = interruption.clone().unwrap_or_else(|| {
                    RuntimeInterruptionDisposition::Paused {
                        reason: "interruption policy unavailable; preserved for resume".to_string(),
                    }
                });
                match self
                    .controller
                    .settle_interruption(run_id, snapshot.revision, cleanup_disposition)
                    .await
                {
                    Ok(RuntimeInterruptionSettlementOutcome::Settled(_)) => {}
                    Ok(RuntimeInterruptionSettlementOutcome::ReloadSnapshot) => wave_errors.push(
                        "runtime interruption cleanup lost its expected revision".to_string(),
                    ),
                    Err(error) => wave_errors.push(format!(
                        "runtime interruption cleanup failed after policy error: {error}"
                    )),
                }
            }

            if !wave_errors.is_empty() {
                return Err(ReactError::Other(wave_errors.join("; ")));
            }

            // Resolve the whole wave before honoring a stop request so completed
            // siblings are never replayed after resume.
            if let Some(disposition) = interruption {
                match self
                    .settle_interruption_boundary(
                        run_id,
                        snapshot.revision,
                        disposition,
                        !interruption_is_durable,
                    )
                    .await?
                {
                    InterruptionBoundary::Outcome(outcome) => return Ok(outcome),
                    InterruptionBoundary::ReloadSnapshot
                    | InterruptionBoundary::NoUnfinishedWork => {}
                }
            }
            if let Some(outcome) = pending_outcome {
                return Ok(outcome);
            }
        }
    }

    async fn settle_interruption_boundary(
        &self,
        run_id: &str,
        expected_revision: u64,
        disposition: RuntimeInterruptionDisposition,
        suppress_noop_outcome: bool,
    ) -> Result<InterruptionBoundary> {
        match self
            .controller
            .settle_interruption(run_id, expected_revision, disposition.clone())
            .await?
        {
            RuntimeInterruptionSettlementOutcome::ReloadSnapshot => {
                Ok(InterruptionBoundary::ReloadSnapshot)
            }
            RuntimeInterruptionSettlementOutcome::Settled(receipt) => {
                if receipt.disposition != disposition {
                    return Err(ReactError::Other(
                        "runtime interruption receipt changed the requested disposition"
                            .to_string(),
                    ));
                }
                if suppress_noop_outcome
                    && receipt.interrupted_task_ids.is_empty()
                    && receipt.pending_task_ids.is_empty()
                {
                    return Ok(InterruptionBoundary::NoUnfinishedWork);
                }
                Ok(InterruptionBoundary::Outcome(match disposition {
                    RuntimeInterruptionDisposition::Cancelled => RuntimeDagOutcome::Cancelled,
                    RuntimeInterruptionDisposition::Paused { reason } => {
                        RuntimeDagOutcome::Paused {
                            task_id: None,
                            reason,
                        }
                    }
                }))
            }
        }
    }

    async fn settle_dispatch_request(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        request: RuntimeTaskResolutionRequest,
    ) -> Result<RuntimeTaskResolution> {
        let resolution = self
            .controller
            .settle_resolution(run_id, claim, task, request.clone())
            .await?;
        if !resolution_matches_request(&request, &resolution) {
            return Err(ReactError::Other(format!(
                "runtime settlement receipt {resolution:?} does not match request {request:?}"
            )));
        }
        if self
            .controller
            .claim_is_current(run_id, &task.spec.id, claim)
            .await?
        {
            return Err(ReactError::Other(format!(
                "runtime settlement receipt {resolution:?} left claim '{}' active",
                claim.claim_id
            )));
        }
        self.settle_attempt_projection(
            &claim.execution_id(run_id, &task.spec.id),
            runtime_resolution_status(&resolution),
        );
        Ok(resolution)
    }

    async fn settle_abandonment(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        abandonment: RuntimeClaimAbandonment,
    ) -> Result<RuntimeTaskSettlementOutcome> {
        let intended_status = abandonment_status(&abandonment);
        let settlement = self
            .controller
            .abandon_claim(run_id, claim, task, abandonment)
            .await?;
        if self
            .controller
            .claim_is_current(run_id, &task.spec.id, claim)
            .await?
        {
            return Err(ReactError::Other(format!(
                "runtime claim abandonment {settlement:?} left claim '{}' active",
                claim.claim_id
            )));
        }
        let status = match settlement {
            RuntimeTaskSettlementOutcome::Settled => intended_status,
            RuntimeTaskSettlementOutcome::Superseded => "superseded",
        };
        self.settle_attempt_projection(&claim.execution_id(run_id, &task.spec.id), status);
        Ok(settlement)
    }

    async fn cleanup_attempt_control(&self, run_id: &str, task: &Task, claim: &TaskClaim) {
        let execution_id = claim.execution_id(run_id, &task.spec.id);
        let receipt = self
            .controller
            .retire_attempt_control(run_id, task, claim)
            .await;
        self.observe_attempt_control_cleanup(run_id, &task.spec.id, claim, receipt.clone());
        self.retire_local_attempt_control(&execution_id);
        match receipt {
            RuntimeAttemptControlCleanupReceipt::RetryableFailure { ref error } => {
                tracing::warn!(
                    run_id,
                    task_id = %task.spec.id,
                    claim_id = %claim.claim_id,
                    error = %error,
                    "exact attempt control cleanup requires retry after durable settlement"
                );
            }
            RuntimeAttemptControlCleanupReceipt::Retired
            | RuntimeAttemptControlCleanupReceipt::NotFound
            | RuntimeAttemptControlCleanupReceipt::AlreadyConsumed => {}
        }
    }
}

fn runtime_resolution_status(resolution: &RuntimeTaskResolution) -> &'static str {
    match resolution {
        RuntimeTaskResolution::Completed => "completed",
        RuntimeTaskResolution::Pending => "pending",
        RuntimeTaskResolution::Skipped => "skipped",
        RuntimeTaskResolution::Failed { .. } => "failed",
        RuntimeTaskResolution::TimedOut { .. } => "timed_out",
        RuntimeTaskResolution::Blocked { .. } => "blocked",
        RuntimeTaskResolution::Cancelled => "cancelled",
        RuntimeTaskResolution::Superseded => "superseded",
    }
}

fn abandonment_status(abandonment: &RuntimeClaimAbandonment) -> &'static str {
    match abandonment {
        RuntimeClaimAbandonment::Interrupted {
            disposition: RuntimeInterruptionDisposition::Cancelled,
        } => "cancelled",
        RuntimeClaimAbandonment::Interrupted {
            disposition: RuntimeInterruptionDisposition::Paused { .. },
        } => "paused",
        RuntimeClaimAbandonment::Failed { .. } => "failed",
    }
}

fn resolution_matches_request(
    request: &RuntimeTaskResolutionRequest,
    resolution: &RuntimeTaskResolution,
) -> bool {
    if resolution == &RuntimeTaskResolution::Superseded {
        return true;
    }
    match (request, resolution) {
        (RuntimeTaskResolutionRequest::Completed, RuntimeTaskResolution::Completed)
        | (RuntimeTaskResolutionRequest::Skipped, RuntimeTaskResolution::Skipped)
        | (RuntimeTaskResolutionRequest::Cancelled, RuntimeTaskResolution::Cancelled) => true,
        (RuntimeTaskResolutionRequest::Requeue { .. }, RuntimeTaskResolution::Pending) => true,
        (
            RuntimeTaskResolutionRequest::Requeue {
                error: requested,
                exhaustion: RuntimeRetryExhaustion::Failed,
                ..
            },
            RuntimeTaskResolution::Failed { error: settled },
        )
        | (
            RuntimeTaskResolutionRequest::Requeue {
                error: requested,
                exhaustion: RuntimeRetryExhaustion::TimedOut,
                ..
            },
            RuntimeTaskResolution::TimedOut { error: settled },
        )
        | (
            RuntimeTaskResolutionRequest::Failed { error: requested },
            RuntimeTaskResolution::Failed { error: settled },
        )
        | (
            RuntimeTaskResolutionRequest::TimedOut { error: requested },
            RuntimeTaskResolution::TimedOut { error: settled },
        ) => requested == settled,
        (
            RuntimeTaskResolutionRequest::Blocked {
                error: requested_error,
                disposition: requested_disposition,
            },
            RuntimeTaskResolution::Blocked {
                error: settled_error,
                disposition: settled_disposition,
            },
        ) => requested_error == settled_error && requested_disposition == settled_disposition,
        _ => false,
    }
}

/// Merge durable and requested interruption signals with one explicit order:
/// Cancelled is stronger than Paused, and the earlier Paused reason is stable.
fn prioritize_interruption(
    current: Option<RuntimeInterruptionDisposition>,
    incoming: Option<RuntimeInterruptionDisposition>,
) -> Option<RuntimeInterruptionDisposition> {
    match (current, incoming) {
        (Some(RuntimeInterruptionDisposition::Cancelled), _)
        | (_, Some(RuntimeInterruptionDisposition::Cancelled)) => {
            Some(RuntimeInterruptionDisposition::Cancelled)
        }
        (Some(paused @ RuntimeInterruptionDisposition::Paused { .. }), _) => Some(paused),
        (None, disposition) => disposition,
    }
}

fn persisted_status_error(status: &TaskStatus) -> Option<String> {
    match status {
        TaskStatus::Failed(error) | TaskStatus::Blocked(error) | TaskStatus::Paused(error) => {
            Some(error.clone())
        }
        TaskStatus::TimedOut { error } => Some(error.clone()),
        TaskStatus::Retrying { last_error, .. } => Some(last_error.clone()),
        TaskStatus::Pending
        | TaskStatus::Running
        | TaskStatus::Completed
        | TaskStatus::Skipped
        | TaskStatus::Cancelled => None,
    }
}

fn validate_selected_wave(
    ready_task_ids: &[TaskId],
    selected_task_ids: Vec<TaskId>,
) -> Result<Vec<TaskId>> {
    if selected_task_ids.is_empty() {
        return Err(ReactError::Other(
            "runtime DAG controller selected an empty wave from a non-empty frontier".to_string(),
        ));
    }

    let ready: HashSet<&str> = ready_task_ids.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    for selected in &selected_task_ids {
        if !ready.contains(selected.as_str()) {
            return Err(ReactError::Other(format!(
                "runtime DAG controller selected non-ready task '{selected}'"
            )));
        }
        if !seen.insert(selected.as_str()) {
            return Err(ReactError::Other(format!(
                "runtime DAG controller selected task '{selected}' more than once"
            )));
        }
    }
    Ok(selected_task_ids)
}

fn stop_outcome(
    disposition: RuntimeStopDisposition,
    failed_task_id: TaskId,
    error: String,
) -> RuntimeDagOutcome {
    match disposition {
        RuntimeStopDisposition::Fail => RuntimeDagOutcome::Failed {
            failed_task_id,
            error,
        },
        RuntimeStopDisposition::Pause => RuntimeDagOutcome::Paused {
            task_id: Some(failed_task_id),
            reason: error,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::tasks::{RuntimeTaskAttemptInterruptError, TaskStatus};

    #[derive(Default)]
    struct ScriptedController {
        snapshot: Mutex<Option<RuntimePlanSnapshot>>,
        order: Mutex<Vec<TaskId>>,
        fail: Mutex<HashMap<TaskId, String>>,
        blocked: Mutex<HashMap<TaskId, (String, RuntimeStopDisposition)>>,
        wait_for_cancel: Mutex<HashSet<TaskId>>,
        ignore_cancel: Mutex<HashSet<TaskId>>,
        insert_after: Mutex<Option<TaskId>>,
        reload_claim_once: Mutex<bool>,
        interruption: Mutex<RuntimeInterruptionDisposition>,
        interruption_error: Mutex<Option<String>>,
        dispatch_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
        cancel_after_dispatch: Mutex<HashMap<TaskId, CancellationToken>>,
        claim_ready: Mutex<Option<Arc<tokio::sync::Notify>>>,
        reserved: Mutex<Vec<String>>,
        retired: Mutex<Vec<String>>,
        current_check_count: Mutex<usize>,
        current_check_error_at: Mutex<Option<usize>>,
        stale_after_current_check: Mutex<Option<usize>>,
        supersede_on_stale_check: Mutex<bool>,
        settlement_error_once: Mutex<Option<String>>,
        settle_response_error_once: Mutex<bool>,
        reservation_error: Mutex<Option<String>>,
        reservation_delay: Mutex<Option<Duration>>,
        settlement_entered: Mutex<Option<Arc<tokio::sync::Notify>>>,
        settlement_release: Mutex<Option<Arc<tokio::sync::Notify>>>,
        reconciliation_entered: Mutex<Option<Arc<tokio::sync::Notify>>>,
        reconciliation_release: Mutex<Option<Arc<tokio::sync::Notify>>>,
        reconciliation_error: Mutex<Option<String>>,
        abandonment_error_once: Mutex<bool>,
        cleanup_receipt: Mutex<Option<RuntimeAttemptControlCleanupReceipt>>,
    }

    impl ScriptedController {
        fn with_tasks(tasks: Vec<Task>) -> Self {
            Self {
                snapshot: Mutex::new(Some(RuntimePlanSnapshot { revision: 1, tasks })),
                ..Self::default()
            }
        }

        fn statuses(&self) -> HashMap<TaskId, TaskStatus> {
            self.snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .tasks
                        .iter()
                        .map(|task| (task.spec.id.clone(), task.execution.status.clone()))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl RuntimeDagController for ScriptedController {
        type DispatchOutput = TaskId;

        async fn load_snapshot(&self, _run_id: &str) -> Result<RuntimePlanSnapshot> {
            self.snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))
        }

        async fn claim_task(
            &self,
            _run_id: &str,
            task: &Task,
            expected_revision: u64,
        ) -> Result<RuntimeTaskClaimOutcome> {
            let mut reload_claim_once = self
                .reload_claim_once
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *reload_claim_once {
                *reload_claim_once = false;
                return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
            }
            drop(reload_claim_once);
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshot
                .as_mut()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            let result = super::super::runtime_service::claim_runtime_task(
                snapshot,
                task,
                expected_revision,
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
            if let Some(notify) = self
                .claim_ready
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
            {
                notify.notify_one();
            }
            Ok(result)
        }

        async fn claim_is_current(
            &self,
            _run_id: &str,
            task_id: &str,
            claim: &TaskClaim,
        ) -> Result<bool> {
            let check_count = {
                let mut count = self
                    .current_check_count
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *count = count.saturating_add(1);
                *count
            };
            let current_check_error = {
                let mut error_at = self
                    .current_check_error_at
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if error_at.is_some_and(|error_at| check_count >= error_at) {
                    error_at.take()
                } else {
                    None
                }
            };
            if current_check_error.is_some() {
                return Err(ReactError::Other(
                    "scripted current-claim lookup unavailable".to_string(),
                ));
            }
            let stale = self
                .stale_after_current_check
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some_and(|limit| check_count >= limit);
            if stale {
                if *self
                    .supersede_on_stale_check
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                {
                    let mut snapshot = self
                        .snapshot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(task) = snapshot.as_mut().and_then(|snapshot| {
                        snapshot
                            .tasks
                            .iter_mut()
                            .find(|task| task.spec.id == task_id)
                    }) {
                        task.execution.status = TaskStatus::Completed;
                        task.execution.claim = None;
                    }
                }
                return Ok(false);
            }
            let snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshot
                .as_ref()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            super::super::runtime_service::runtime_claim_is_current(snapshot, task_id, claim)
                .map_err(|error| ReactError::Other(error.to_string()))
        }

        async fn reserve_attempt_control(&self, context: &TaskSubagentContext) -> Result<()> {
            let delay = *self
                .reservation_delay
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(error) = self
                .reservation_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                return Err(ReactError::Other(error));
            }
            let execution_id = context
                .execution_id()
                .ok_or_else(|| ReactError::Other("missing exact execution identity".to_string()))?;
            self.reserved
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(execution_id);
            Ok(())
        }

        async fn reconcile_attempt_control(
            &self,
            _run_id: &str,
            _current_execution_ids: &HashSet<String>,
        ) -> Result<()> {
            let entered = self
                .reconciliation_entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let release = self
                .reconciliation_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(entered) = entered {
                entered.notify_one();
            }
            if let Some(release) = release {
                release.notified().await;
            }
            if let Some(error) = self
                .reconciliation_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                return Err(ReactError::Other(error));
            }
            Ok(())
        }

        async fn retire_attempt_control(
            &self,
            run_id: &str,
            task: &Task,
            claim: &TaskClaim,
        ) -> RuntimeAttemptControlCleanupReceipt {
            self.retired
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(claim.execution_id(run_id, &task.spec.id));
            self.cleanup_receipt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .unwrap_or(RuntimeAttemptControlCleanupReceipt::Retired)
        }

        async fn dispatch_task(
            &self,
            context: TaskSubagentContext,
            task: Task,
        ) -> Result<Self::DispatchOutput> {
            self.order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(task.spec.id.clone());
            let barrier = self
                .dispatch_barrier
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(barrier) = barrier {
                barrier.wait().await;
            }
            let wait_for_cancel = self
                .wait_for_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&task.spec.id);
            if wait_for_cancel {
                context.cancellation_token().cancelled().await;
                return Err(ReactError::Agent(Box::new(
                    echo_core::error::AgentError::Cancelled("cancelled by test".to_string()),
                )));
            }
            let ignore_cancel = self
                .ignore_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&task.spec.id);
            if ignore_cancel {
                std::future::pending::<()>().await;
            }
            let failure = self
                .fail
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            let task_id = task.spec.id.clone();
            let result = match failure {
                Some(error) => Err(ReactError::Other(error)),
                None => Ok(task_id.clone()),
            };
            if let Some(cancel) = self
                .cancel_after_dispatch
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&task_id)
            {
                cancel.cancel();
            }
            result
        }

        async fn resolve_dispatch(
            &self,
            _run_id: &str,
            _claim: TaskClaim,
            task: Task,
            dispatch: Result<Self::DispatchOutput>,
        ) -> Result<RuntimeTaskResolutionRequest> {
            let blocked = self
                .blocked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            Ok(match dispatch {
                Ok(_) if blocked.is_some() => {
                    let (error, disposition) = blocked.ok_or_else(|| {
                        ReactError::Other("scripted blocker disappeared".to_string())
                    })?;
                    RuntimeTaskResolutionRequest::Blocked { error, disposition }
                }
                Ok(_) => RuntimeTaskResolutionRequest::Completed,
                Err(ReactError::Agent(error))
                    if matches!(
                        error.as_ref(),
                        echo_core::error::AgentError::Cancelled(_)
                            | echo_core::error::AgentError::Interrupted
                    ) =>
                {
                    RuntimeTaskResolutionRequest::Cancelled
                }
                Err(error) => RuntimeTaskResolutionRequest::Failed {
                    error: error.to_string(),
                },
            })
        }

        async fn settle_resolution(
            &self,
            _run_id: &str,
            claim: &TaskClaim,
            task: &Task,
            request: RuntimeTaskResolutionRequest,
        ) -> Result<RuntimeTaskResolution> {
            let settlement_entered = self
                .settlement_entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let settlement_release = self
                .settlement_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(entered) = settlement_entered {
                entered.notify_one();
            }
            if let Some(release) = settlement_release {
                release.notified().await;
            }
            if let Some(error) = self
                .settlement_error_once
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                return Err(ReactError::Other(error));
            }
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshot
                .as_mut()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            let resolution = super::super::runtime_service::settle_runtime_resolution(
                snapshot,
                &task.spec.id,
                claim,
                request,
            )
            .map_err(|error| ReactError::Other(error.to_string()))?;
            let insert_after = self
                .insert_after
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if resolution != RuntimeTaskResolution::Superseded
                && insert_after.as_deref() == Some(task.spec.id.as_str())
            {
                snapshot.revision = snapshot.revision.checked_add(1).ok_or_else(|| {
                    ReactError::Other("scripted snapshot revision overflowed".to_string())
                })?;
                snapshot.tasks.push(runtime_task(
                    "inserted",
                    TaskStatus::Pending,
                    &[task.spec.id.as_str()],
                ));
            }
            let mut response_error = self
                .settle_response_error_once
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *response_error {
                *response_error = false;
                return Err(ReactError::Other(
                    "scripted lost settlement response".to_string(),
                ));
            }
            Ok(resolution)
        }

        async fn abandon_claim(
            &self,
            _run_id: &str,
            claim: &TaskClaim,
            task: &Task,
            abandonment: RuntimeClaimAbandonment,
        ) -> Result<RuntimeTaskSettlementOutcome> {
            let mut abandonment_error = self
                .abandonment_error_once
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *abandonment_error {
                *abandonment_error = false;
                return Err(ReactError::Other(
                    "scripted abandonment outcome unknown".to_string(),
                ));
            }
            drop(abandonment_error);
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshot
                .as_mut()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            let status = match abandonment {
                RuntimeClaimAbandonment::Interrupted { disposition } => match disposition {
                    RuntimeInterruptionDisposition::Cancelled => TaskStatus::Cancelled,
                    RuntimeInterruptionDisposition::Paused { reason } => TaskStatus::Paused(reason),
                },
                RuntimeClaimAbandonment::Failed { error } => TaskStatus::Failed(error),
            };
            super::super::runtime_service::settle_runtime_claim(
                snapshot,
                &task.spec.id,
                claim,
                status,
            )
            .map_err(|error| ReactError::Other(error.to_string()))
        }

        async fn interruption_disposition(
            &self,
            _run_id: &str,
        ) -> Result<RuntimeInterruptionDisposition> {
            if let Some(error) = self
                .interruption_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                return Err(ReactError::Other(error));
            }
            Ok(self
                .interruption
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone())
        }

        async fn settle_interruption(
            &self,
            _run_id: &str,
            expected_revision: u64,
            disposition: RuntimeInterruptionDisposition,
        ) -> Result<RuntimeInterruptionSettlementOutcome> {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshot
                .as_mut()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            super::super::runtime_service::settle_runtime_interruption(
                snapshot,
                expected_revision,
                disposition,
            )
            .map_err(|error| ReactError::Other(error.to_string()))
        }
    }

    fn runtime_task(id: &str, status: TaskStatus, dependencies: &[&str]) -> Task {
        Task {
            spec: crate::tasks::TaskSpec {
                id: id.to_string(),
                title: id.to_string(),
                description: format!("execute {id}"),
                depends_on: dependencies
                    .iter()
                    .map(|dependency| dependency.to_string())
                    .collect(),
                max_retries: 1,
                extension: serde_json::Value::Null,
            },
            execution: crate::tasks::TaskExecution {
                task_id: id.to_string(),
                status,
                retry_count: 0,
                failure_fingerprint: None,
                claim: None,
            },
        }
    }

    #[tokio::test]
    async fn executor_follows_dependencies_and_safe_point_revision() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Pending, &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
        ]));
        *controller
            .insert_after
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some("a".to_string());
        let executor =
            RuntimeDagExecutor::new(controller.clone(), RuntimeTaskServiceConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Completed);
        let order = controller
            .order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let a_position = order
            .iter()
            .position(|id| id == "a")
            .ok_or_else(|| ReactError::Other("task 'a' was not dispatched".to_string()))?;
        let b_position = order
            .iter()
            .position(|id| id == "b")
            .ok_or_else(|| ReactError::Other("task 'b' was not dispatched".to_string()))?;
        let inserted_position = order
            .iter()
            .position(|id| id == "inserted")
            .ok_or_else(|| ReactError::Other("revised task was not dispatched".to_string()))?;
        assert!(a_position < b_position);
        assert!(a_position < inserted_position);
        Ok(())
    }

    #[tokio::test]
    async fn exact_control_is_reserved_before_dispatch_and_retired_after_settlement() -> Result<()>
    {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "exact",
            TaskStatus::Pending,
            &[],
        )]));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );

        assert_eq!(
            service
                .execute("exact-run", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Completed
        );
        let reserved = controller
            .reserved
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let retired = controller
            .retired
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(reserved.len(), 1);
        assert_eq!(retired, reserved);
        assert!(
            reserved
                .first()
                .is_some_and(|execution_id| execution_id.starts_with("exact-run:exact:1:1:"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn lost_settlement_response_reloads_committed_authority_without_abandonment() -> Result<()>
    {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "committed",
            TaskStatus::Pending,
            &[],
        )]));
        *controller
            .settle_response_error_once
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );

        assert_eq!(
            service
                .execute("lost-settlement", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Completed
        );
        assert_eq!(
            controller.statuses().get("committed"),
            Some(&TaskStatus::Completed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn joined_attempt_remains_addressable_until_durable_settlement() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "joined",
            TaskStatus::Pending,
            &[],
        )]));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *controller
            .settlement_entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(entered.clone());
        *controller
            .settlement_release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(release.clone());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let execution = tokio::spawn({
            let service = Arc::clone(&service);
            async move {
                service
                    .execute("joined-settlement", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("settlement barrier was not reached".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("joined claim was not current".to_string()))?;
        let interrupt = tokio::spawn({
            let service = Arc::clone(&service);
            let claim = claim.clone();
            async move {
                service
                    .request_attempt_interrupt("joined-settlement", "joined", &claim)
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!interrupt.is_finished());
        release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), execution)
                .await
                .map_err(|_| ReactError::Other("joined settlement did not finish".to_string()))?
                .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??,
            RuntimeDagOutcome::Completed
        );
        let receipt = tokio::time::timeout(Duration::from_secs(2), interrupt)
            .await
            .map_err(|_| ReactError::Other("joined interrupt receipt did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("interrupt task panicked: {error}")))?
            .map_err(ReactError::from)?;
        assert!(!receipt.requested);
        assert_eq!(
            receipt.disposition,
            RuntimeTaskAttemptInterruptDisposition::AlreadySettled {
                status: "completed".to_string(),
            }
        );
        assert_eq!(service.pending_attempt_interrupt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn joined_attempt_reports_unknown_authority_and_recovery_settles_observation()
    -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "joined",
            TaskStatus::Pending,
            &[],
        )]));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *controller
            .settlement_entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(entered.clone());
        *controller
            .settlement_release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(release.clone());
        *controller
            .settlement_error_once
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("scripted durable settlement unavailable".to_string());
        *controller
            .current_check_error_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(3);
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let execution = tokio::spawn({
            let service = Arc::clone(&service);
            async move {
                service
                    .execute("joined-unknown", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("settlement barrier was not reached".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("joined claim was not current".to_string()))?;
        let execution_id = claim.execution_id("joined-unknown", "joined");
        let interrupt = tokio::spawn({
            let service = Arc::clone(&service);
            let claim = claim.clone();
            async move {
                service
                    .request_attempt_interrupt("joined-unknown", "joined", &claim)
                    .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!interrupt.is_finished());
        release.notify_one();

        let execution_error = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("unknown settlement did not finish".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))?
            .err()
            .ok_or_else(|| {
                ReactError::Other("unknown settlement unexpectedly succeeded".to_string())
            })?;
        assert!(execution_error.to_string().contains("claim lookup failed"));
        let interrupt_error = tokio::time::timeout(Duration::from_secs(2), interrupt)
            .await
            .map_err(|_| ReactError::Other("unknown interrupt remained pending".to_string()))?
            .map_err(|error| ReactError::Other(format!("interrupt task panicked: {error}")))?
            .err()
            .ok_or_else(|| {
                ReactError::Other("unknown interrupt unexpectedly succeeded".to_string())
            })?;
        assert!(matches!(
            interrupt_error,
            RuntimeTaskAttemptInterruptError::DurableAuthority { .. }
        ));
        assert!(matches!(
            service.attempt_settlement_observation(&execution_id),
            Some(AttemptSettlementObservation::AuthorityUnknown { .. })
        ));
        let mut recovered_settlement = service
            .attempt_settlement_receiver(&execution_id)
            .ok_or_else(|| ReactError::Other("recovery settlement watch missing".to_string()))?;
        let _ = recovered_settlement.borrow_and_update();
        let recovery_waiter = tokio::spawn(async move {
            recovered_settlement
                .changed()
                .await
                .map_err(|_| ReactError::Other("recovery settlement watch closed".to_string()))?;
            Ok::<_, ReactError>(recovered_settlement.borrow_and_update().clone())
        });

        let reconcile_entered = Arc::new(tokio::sync::Notify::new());
        let reconcile_release = Arc::new(tokio::sync::Notify::new());
        *controller
            .reconciliation_entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(reconcile_entered.clone());
        *controller
            .reconciliation_release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(reconcile_release.clone());
        {
            let mut snapshot = controller
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let task = snapshot
                .as_mut()
                .and_then(|snapshot| snapshot.tasks.first_mut())
                .ok_or_else(|| ReactError::Other("joined task disappeared".to_string()))?;
            task.execution.status = TaskStatus::Completed;
            task.execution.claim = None;
        }
        let reconciliation = tokio::spawn({
            let service = Arc::clone(&service);
            async move { service.reconcile_attempt_control("joined-unknown").await }
        });
        tokio::time::timeout(Duration::from_secs(2), reconcile_entered.notified())
            .await
            .map_err(|_| ReactError::Other("reconciliation hook was not reached".to_string()))?;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), recovery_waiter)
                .await
                .map_err(|_| {
                    ReactError::Other(
                        "durable recovery did not release the settlement waiter".to_string(),
                    )
                })?
                .map_err(|error| {
                    ReactError::Other(format!("recovery waiter task panicked: {error}"))
                })??,
            Some(AttemptSettlementObservation::Settled {
                status: "superseded".to_string(),
            })
        );
        assert!(!service.has_attempt_supervisor_state(&execution_id));
        service.reconcile_local_attempt_control("joined-unknown", &HashSet::new());
        assert!(!service.has_attempt_supervisor_state(&execution_id));
        reconcile_release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), reconciliation)
            .await
            .map_err(|_| ReactError::Other("reconciliation remained blocked".to_string()))?
            .map_err(|error| {
                ReactError::Other(format!("reconciliation task panicked: {error}"))
            })??;
        Ok(())
    }

    #[tokio::test]
    async fn unknown_reservation_abandonment_preserves_control_projection() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "reserved",
            TaskStatus::Pending,
            &[],
        )]));
        *controller
            .reservation_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("reservation failed".to_string());
        *controller
            .abandonment_error_once
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );

        let error = service
            .execute("unknown-reservation", CancellationToken::new())
            .await
            .err()
            .ok_or_else(|| ReactError::Other("reservation failure unexpectedly ran".to_string()))?;
        assert!(error.to_string().contains("outcome is unknown"));
        assert!(
            controller
                .retired
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert_eq!(
            controller.statuses().get("reserved"),
            Some(&TaskStatus::Running)
        );
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_failure_is_observed_without_reversing_committed_terminal() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "completed",
            TaskStatus::Pending,
            &[],
        )]));
        *controller
            .cleanup_receipt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(RuntimeAttemptControlCleanupReceipt::RetryableFailure {
                error: "cleanup unavailable".to_string(),
            });
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_receipts = Arc::clone(&observed);
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                attempt_control_observer: Some(Arc::new(move |observation| {
                    observed_receipts
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push((observation.task_id.clone(), observation.receipt.clone()));
                })),
                ..RuntimeTaskServiceConfig::default()
            },
        );

        assert_eq!(
            service
                .execute("cleanup-observer", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Completed
        );
        assert_eq!(
            controller.statuses().get("completed"),
            Some(&TaskStatus::Completed)
        );
        assert!(
            observed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|(task_id, receipt)| {
                    task_id == "completed"
                        && matches!(
                            receipt,
                            RuntimeAttemptControlCleanupReceipt::RetryableFailure { .. }
                        )
                })
        );
        Ok(())
    }

    #[tokio::test]
    async fn executor_treats_skipped_tasks_as_resolved() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "skipped",
            TaskStatus::Skipped,
            &[],
        )]));
        let executor = RuntimeDagExecutor::new(controller, RuntimeTaskServiceConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn executor_rejects_claims_attached_to_inactive_statuses() -> Result<()> {
        for status in [TaskStatus::Pending, TaskStatus::Completed] {
            let mut invalid = runtime_task("invalid", status, &[]);
            invalid.execution.claim = Some(TaskClaim::new(
                1,
                1,
                invalid.spec.stable_hash().map_err(ReactError::Other)?,
            ));
            let controller = Arc::new(ScriptedController::with_tasks(vec![invalid]));
            let service = super::super::RuntimeTaskService::new(
                controller,
                RuntimeTaskServiceConfig::default(),
            );

            let error = service
                .execute("invalid-claim", CancellationToken::new())
                .await
                .err()
                .ok_or_else(|| {
                    ReactError::Other("inactive claim snapshot unexpectedly executed".to_string())
                })?;
            assert!(error.to_string().contains("invalid runtime claim snapshot"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn executor_treats_cancelled_snapshot_as_interrupted() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "cancelled",
            TaskStatus::Cancelled,
            &[],
        )]));
        let executor = RuntimeDagExecutor::new(controller, RuntimeTaskServiceConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        Ok(())
    }

    #[tokio::test]
    async fn recovered_interruption_takes_precedence_over_retained_failure() -> Result<()> {
        let cancelled = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("failed", TaskStatus::Failed("old failure".to_string()), &[]),
            runtime_task("cancelled", TaskStatus::Cancelled, &[]),
            runtime_task("pending", TaskStatus::Pending, &[]),
        ]));
        let cancelled_service = super::super::RuntimeTaskService::new(
            cancelled.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        *cancelled
            .interruption
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            RuntimeInterruptionDisposition::Paused {
                reason: "weaker requested pause".to_string(),
            };
        let requested_pause = CancellationToken::new();
        requested_pause.cancel();
        assert_eq!(
            cancelled_service
                .execute("cancelled-run", requested_pause)
                .await?,
            RuntimeDagOutcome::Cancelled
        );
        let cancelled_statuses = cancelled.statuses();
        assert_eq!(
            cancelled_statuses.get("failed"),
            Some(&TaskStatus::Failed("old failure".to_string()))
        );
        assert_eq!(
            cancelled_statuses.get("pending"),
            Some(&TaskStatus::Cancelled)
        );

        let paused = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("failed", TaskStatus::Failed("old failure".to_string()), &[]),
            runtime_task(
                "paused",
                TaskStatus::Paused("resume later".to_string()),
                &[],
            ),
            runtime_task("pending", TaskStatus::Pending, &[]),
        ]));
        let paused_service = super::super::RuntimeTaskService::new(
            paused.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        assert_eq!(
            paused_service
                .execute("paused-run", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Paused {
                task_id: None,
                reason: "resume later".to_string(),
            }
        );
        assert_eq!(paused.statuses().get("pending"), Some(&TaskStatus::Pending));
        Ok(())
    }

    #[tokio::test]
    async fn completed_snapshot_wins_over_late_cancellation_request() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("completed", TaskStatus::Completed, &[]),
            runtime_task("skipped", TaskStatus::Skipped, &[]),
        ]));
        let service =
            super::super::RuntimeTaskService::new(controller, RuntimeTaskServiceConfig::default());
        let cancel = CancellationToken::new();
        cancel.cancel();

        assert_eq!(
            service.execute("completed-run", cancel).await?,
            RuntimeDagOutcome::Completed
        );
        Ok(())
    }

    #[tokio::test]
    async fn last_success_settlement_wins_over_same_boundary_cancellation() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "last",
            TaskStatus::Pending,
            &[],
        )]));
        let cancel = CancellationToken::new();
        controller
            .cancel_after_dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("last".to_string(), cancel.clone());
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );

        assert_eq!(
            service.execute("last-success", cancel).await?,
            RuntimeDagOutcome::Completed
        );
        assert_eq!(
            controller.statuses().get("last"),
            Some(&TaskStatus::Completed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn late_cancellation_overrides_blocked_resolution_at_boundary() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "blocked",
            TaskStatus::Pending,
            &[],
        )]));
        controller
            .blocked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                "blocked".to_string(),
                ("await review".to_string(), RuntimeStopDisposition::Pause),
            );
        let cancel = CancellationToken::new();
        controller
            .cancel_after_dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("blocked".to_string(), cancel.clone());
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );

        assert_eq!(
            service.execute("blocked-cancel", cancel).await?,
            RuntimeDagOutcome::Cancelled
        );
        assert_eq!(
            controller.statuses().get("blocked"),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn service_settles_pre_wave_cancellation_without_dispatch() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Pending, &[]),
            runtime_task("b", TaskStatus::Pending, &[]),
        ]));
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let cancel = CancellationToken::new();
        cancel.cancel();

        let outcome = runtime_tasks.execute("run", cancel).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert!(
            controller
                .statuses()
                .values()
                .all(|status| status == &TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn service_settles_pre_wave_pause_without_retry_or_dispatch() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "pending",
            TaskStatus::Pending,
            &[],
        )]));
        *controller
            .interruption
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            RuntimeInterruptionDisposition::Paused {
                reason: "deterministic pause".to_string(),
            };
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let cancel = CancellationToken::new();
        cancel.cancel();

        let outcome = runtime_tasks.execute("run", cancel).await?;

        assert_eq!(
            outcome,
            RuntimeDagOutcome::Paused {
                task_id: None,
                reason: "deterministic pause".to_string(),
            }
        );
        assert_eq!(
            controller.statuses().get("pending"),
            Some(&TaskStatus::Pending)
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_drains_wave_and_preserves_completed_siblings() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("fast", TaskStatus::Pending, &[]),
            runtime_task("slow", TaskStatus::Pending, &[]),
        ]));
        controller
            .ignore_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("slow".to_string());
        *controller
            .dispatch_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Arc::new(tokio::sync::Barrier::new(2)));
        let cancel = CancellationToken::new();
        controller
            .cancel_after_dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("fast".to_string(), cancel.clone());
        let runtime_tasks = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(200),
                ..RuntimeTaskServiceConfig::default()
            },
        );

        let outcome = runtime_tasks.execute("run", cancel).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        let statuses = controller.statuses();
        assert_eq!(statuses.get("fast"), Some(&TaskStatus::Completed));
        assert_eq!(statuses.get("slow"), Some(&TaskStatus::Cancelled));
        Ok(())
    }

    #[tokio::test]
    async fn exact_runtime_interrupt_cancels_only_claimed_task_child() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "slow",
            TaskStatus::Pending,
            &[],
        )]));
        let claim_ready = Arc::new(tokio::sync::Notify::new());
        *controller
            .claim_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(claim_ready.clone());
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("slow".to_string());
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let run_cancel = CancellationToken::new();
        let execution = tokio::spawn({
            let runtime_tasks = Arc::clone(&runtime_tasks);
            let run_cancel = run_cancel.clone();
            async move { runtime_tasks.execute("exact-interrupt", run_cancel).await }
        });
        tokio::time::timeout(Duration::from_secs(2), claim_ready.notified())
            .await
            .map_err(|_| ReactError::Other("claim was not admitted".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("claimed task was not persisted".to_string()))?;
        let receipt = runtime_tasks
            .request_attempt_interrupt("exact-interrupt", "slow", &claim)
            .await?;
        assert!(receipt.requested);
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("exact interrupt did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert_eq!(
            controller.statuses().get("slow"),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_interrupt_before_shared_admission_runs_pre_cancelled_dispatch() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "queued",
            TaskStatus::Pending,
            &[],
        )]));
        let claim_ready = Arc::new(tokio::sync::Notify::new());
        *controller
            .claim_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(claim_ready.clone());
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("queued".to_string());
        let admission = Arc::new(ExecutionAdmission::with_capacity(1));
        let held = admission
            .issue("held")
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                shared_admission: Some(admission),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let execution = tokio::spawn({
            let runtime_tasks = Arc::clone(&runtime_tasks);
            async move {
                runtime_tasks
                    .execute("pre-admission-interrupt", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), claim_ready.notified())
            .await
            .map_err(|_| ReactError::Other("claim was not admitted".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("claimed task was not persisted".to_string()))?;
        assert!(
            runtime_tasks
                .request_attempt_interrupt("pre-admission-interrupt", "queued", &claim)
                .await?
                .requested
        );
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("pre-admission interrupt did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["queued"]
        );
        drop(held);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interrupt_before_runtime_reservation_returns_typed_queued_receipt() -> Result<()>
    {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "queued",
            TaskStatus::Pending,
            &[],
        )]));
        let expected = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .cloned()
            .ok_or_else(|| ReactError::Other("queued task missing".to_string()))?;
        let claim = match controller
            .claim_task("queued-receipt", &expected, 1)
            .await?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err(ReactError::Other("queued task was not claimed".to_string()));
            }
        };
        let service =
            super::super::RuntimeTaskService::new(controller, RuntimeTaskServiceConfig::default());

        let receipt = service
            .request_attempt_interrupt("queued-receipt", "queued", &claim)
            .await?;
        assert_eq!(
            receipt.disposition,
            RuntimeTaskAttemptInterruptDisposition::QueuedBeforeReservation
        );
        assert!(receipt.requested);
        Ok(())
    }

    #[tokio::test]
    async fn delayed_reservation_rearms_abort_for_pre_handle_interrupt() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "delayed",
            TaskStatus::Pending,
            &[],
        )]));
        let claim_ready = Arc::new(tokio::sync::Notify::new());
        *controller
            .claim_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(claim_ready.clone());
        *controller
            .reservation_delay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Duration::from_millis(80));
        controller
            .ignore_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("delayed".to_string());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let execution = tokio::spawn({
            let service = Arc::clone(&service);
            async move {
                service
                    .execute("delayed-reservation", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), claim_ready.notified())
            .await
            .map_err(|_| ReactError::Other("delayed claim was not admitted".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("delayed claim was not persisted".to_string()))?;
        let receipt = service
            .request_attempt_interrupt("delayed-reservation", "delayed", &claim)
            .await?;
        assert_eq!(
            receipt.disposition,
            RuntimeTaskAttemptInterruptDisposition::ActiveRequested
        );
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("delayed reservation escaped exact abort".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert_eq!(
            controller.statuses().get("delayed"),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_interrupt_retires_projection_when_claim_supersedes_during_request() -> Result<()>
    {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "slow",
            TaskStatus::Pending,
            &[],
        )]));
        let claim_ready = Arc::new(tokio::sync::Notify::new());
        *controller
            .claim_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(claim_ready.clone());
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("slow".to_string());
        *controller
            .stale_after_current_check
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(2);
        *controller
            .supersede_on_stale_check
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let execution = tokio::spawn({
            let runtime_tasks = Arc::clone(&runtime_tasks);
            async move {
                runtime_tasks
                    .execute("stale-interrupt", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), claim_ready.notified())
            .await
            .map_err(|_| ReactError::Other("claim was not admitted".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("claimed task was not persisted".to_string()))?;
        let error = runtime_tasks
            .request_attempt_interrupt("stale-interrupt", "slow", &claim)
            .await
            .err()
            .ok_or_else(|| {
                ReactError::Other("stale interrupt unexpectedly succeeded".to_string())
            })?;
        assert!(error.to_string().contains("stale or settled"));
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("stale interrupt did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Completed);
        assert_eq!(
            controller.statuses().get("slow"),
            Some(&TaskStatus::Completed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_claim_outcome_survives_reconciliation_failure() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "settled",
            TaskStatus::Completed,
            &[],
        )]));
        *controller
            .reconciliation_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("live cleanup unavailable".to_string());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_cleanup = Arc::clone(&observed);
        let service = super::super::RuntimeTaskService::new(
            controller,
            RuntimeTaskServiceConfig {
                attempt_control_observer: Some(Arc::new(move |observation| {
                    observed_cleanup
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(observation.clone());
                })),
                ..RuntimeTaskServiceConfig::default()
            },
        );
        let stale = TaskClaim::new(1, 1, "stale".to_string());

        assert!(matches!(
            service
                .request_attempt_interrupt("stale-cleanup", "settled", &stale)
                .await,
            Err(super::super::RuntimeTaskAttemptInterruptError::StaleOrSettledClaim { .. })
        ));
        assert!(
            observed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|observation| {
                    observation.run_id == "stale-cleanup"
                        && observation.task_id == "settled"
                        && matches!(
                            &observation.receipt,
                            RuntimeAttemptControlCleanupReceipt::RetryableFailure { .. }
                        )
                })
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_interrupt_aborts_non_cooperative_attempt_after_grace() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "stuck",
            TaskStatus::Pending,
            &[],
        )]));
        let claim_ready = Arc::new(tokio::sync::Notify::new());
        *controller
            .claim_ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(claim_ready.clone());
        controller
            .ignore_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("stuck".to_string());
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let execution = tokio::spawn({
            let runtime_tasks = Arc::clone(&runtime_tasks);
            async move {
                runtime_tasks
                    .execute("forced-interrupt", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), claim_ready.notified())
            .await
            .map_err(|_| ReactError::Other("claim was not admitted".to_string()))?;
        let claim = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|snapshot| snapshot.tasks.first())
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("claimed task was not persisted".to_string()))?;
        let mut receipt = None;
        for _ in 0..100 {
            let candidate = runtime_tasks
                .request_attempt_interrupt("forced-interrupt", "stuck", &claim)
                .await?;
            if candidate.requested {
                receipt = Some(candidate);
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(receipt.is_some());
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("non-cooperative interrupt did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert_eq!(
            controller.statuses().get("stuck"),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_exact_aborts_settle_their_own_claims() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("left", TaskStatus::Pending, &[]),
            runtime_task("right", TaskStatus::Pending, &[]),
        ]));
        controller
            .ignore_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend(["left".to_string(), "right".to_string()]);
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                max_concurrent_subagents: 2,
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let execution = tokio::spawn({
            let runtime_tasks = Arc::clone(&runtime_tasks);
            async move {
                runtime_tasks
                    .execute("concurrent-aborts", CancellationToken::new())
                    .await
            }
        });
        let claims = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let claims = controller
                    .snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .map(|snapshot| {
                        snapshot
                            .tasks
                            .iter()
                            .filter_map(|task| {
                                task.execution
                                    .claim
                                    .clone()
                                    .map(|claim| (task.spec.id.clone(), claim))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if claims.len() == 2 {
                    break claims;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("both exact claims were not admitted".to_string()))?;
        for (task_id, claim) in claims {
            assert!(
                runtime_tasks
                    .request_attempt_interrupt("concurrent-aborts", &task_id, &claim)
                    .await?
                    .requested
            );
        }
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("concurrent exact aborts did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        let statuses = controller.statuses();
        assert_eq!(statuses.get("left"), Some(&TaskStatus::Cancelled));
        assert_eq!(statuses.get("right"), Some(&TaskStatus::Cancelled));
        Ok(())
    }

    #[tokio::test]
    async fn root_and_exact_abort_converge_without_cross_attempt_attribution() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("exact", TaskStatus::Pending, &[]),
            runtime_task("root", TaskStatus::Pending, &[]),
        ]));
        controller
            .ignore_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend(["exact".to_string(), "root".to_string()]);
        let runtime_tasks = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                max_concurrent_subagents: 2,
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let run_cancel = CancellationToken::new();
        let execution = tokio::spawn({
            let runtime_tasks = Arc::clone(&runtime_tasks);
            let run_cancel = run_cancel.clone();
            async move { runtime_tasks.execute("root-exact", run_cancel).await }
        });
        let exact_claim = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let claim = controller
                    .snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .and_then(|snapshot| {
                        let all_claimed = snapshot
                            .tasks
                            .iter()
                            .all(|task| task.execution.claim.is_some());
                        all_claimed.then(|| {
                            snapshot
                                .tasks
                                .iter()
                                .find(|task| task.spec.id == "exact")
                                .and_then(|task| task.execution.claim.clone())
                        })
                    })
                    .flatten();
                if let Some(claim) = claim {
                    break claim;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("root/exact claims were not admitted".to_string()))?;
        assert!(
            runtime_tasks
                .request_attempt_interrupt("root-exact", "exact", &exact_claim)
                .await?
                .requested
        );
        run_cancel.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("root/exact abort did not settle".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime task panicked: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        let statuses = controller.statuses();
        assert_eq!(statuses.get("exact"), Some(&TaskStatus::Cancelled));
        assert_eq!(statuses.get("root"), Some(&TaskStatus::Cancelled));
        Ok(())
    }

    #[tokio::test]
    async fn mid_wave_pause_preserves_completed_sibling_and_pauses_interrupted_claim() -> Result<()>
    {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("fast", TaskStatus::Pending, &[]),
            runtime_task("slow", TaskStatus::Pending, &[]),
        ]));
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("slow".to_string());
        *controller
            .dispatch_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Arc::new(tokio::sync::Barrier::new(2)));
        *controller
            .interruption
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            RuntimeInterruptionDisposition::Paused {
                reason: "pause at wave barrier".to_string(),
            };
        let cancel = CancellationToken::new();
        controller
            .cancel_after_dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("fast".to_string(), cancel.clone());
        let runtime_tasks = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(200),
                ..RuntimeTaskServiceConfig::default()
            },
        );

        let outcome = runtime_tasks.execute("run", cancel).await?;

        assert_eq!(
            outcome,
            RuntimeDagOutcome::Paused {
                task_id: None,
                reason: "pause at wave barrier".to_string(),
            }
        );
        let snapshot = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| ReactError::Other("missing paused snapshot".to_string()))?;
        let fast = snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "fast")
            .ok_or_else(|| ReactError::Other("fast task is missing".to_string()))?;
        let slow = snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "slow")
            .ok_or_else(|| ReactError::Other("slow task is missing".to_string()))?;
        assert_eq!(fast.execution.status, TaskStatus::Completed);
        assert_eq!(
            slow.execution.status,
            TaskStatus::Paused("pause at wave barrier".to_string())
        );
        assert!(slow.execution.claim.is_none());
        assert_eq!(slow.execution.retry_count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn interruption_policy_error_cleans_every_claim_before_returning_error() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("fast", TaskStatus::Pending, &[]),
            runtime_task("slow", TaskStatus::Pending, &[]),
        ]));
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("slow".to_string());
        *controller
            .dispatch_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(Arc::new(tokio::sync::Barrier::new(2)));
        *controller
            .interruption_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some("interruption authority unavailable".to_string());
        let cancel = CancellationToken::new();
        controller
            .cancel_after_dispatch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("fast".to_string(), cancel.clone());
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(200),
                ..RuntimeTaskServiceConfig::default()
            },
        );

        let error = service
            .execute("policy-error", cancel)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("policy error unexpectedly succeeded".to_string()))?;
        assert!(
            error
                .to_string()
                .contains("interruption authority unavailable")
        );
        let snapshot = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| ReactError::Other("policy-error snapshot missing".to_string()))?;
        assert!(
            snapshot
                .tasks
                .iter()
                .all(|task| task.execution.claim.is_none())
        );
        assert!(snapshot.tasks.iter().all(|task| {
            matches!(
                task.execution.status,
                TaskStatus::Completed | TaskStatus::Paused(_) | TaskStatus::Pending
            )
        }));
        assert!(
            snapshot
                .tasks
                .iter()
                .any(|task| matches!(task.execution.status, TaskStatus::Paused(_)))
        );
        assert!(
            snapshot
                .tasks
                .iter()
                .all(|task| task.execution.status != TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn reclaimed_physical_attempt_has_a_distinct_execution_id() {
        let first = TaskClaim::new(7, 2, "same-spec".to_string());
        let second = TaskClaim::new(7, 2, "same-spec".to_string());

        assert_ne!(first.claim_id, second.claim_id);
        assert_ne!(
            first.execution_id("run", "task"),
            second.execution_id("run", "task")
        );
    }

    #[tokio::test]
    async fn cancellation_abandons_a_non_cooperative_dispatch_claim() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "stuck",
            TaskStatus::Pending,
            &[],
        )]));
        controller
            .ignore_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("stuck".to_string());
        let executor = RuntimeDagExecutor::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(10),
                ..RuntimeTaskServiceConfig::default()
            },
        );
        let cancel = CancellationToken::new();
        let run_cancel = cancel.clone();
        let run = tokio::spawn(async move { executor.execute("run", run_cancel).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            while controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("task was not dispatched".to_string()))?;
        cancel.cancel();

        let outcome = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .map_err(|_| ReactError::Other("executor cancellation timed out".to_string()))?
            .map_err(|error| ReactError::Other(format!("executor failed to join: {error}")))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert_eq!(
            controller.statuses().get("stuck"),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn claim_conflict_reloads_without_failing_or_dispatching_stale_work() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "claimable",
            TaskStatus::Pending,
            &[],
        )]));
        *controller
            .reload_claim_once
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        let executor =
            RuntimeDagExecutor::new(controller.clone(), RuntimeTaskServiceConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Completed);
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["claimable"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn executor_preserves_persisted_terminal_error_details() -> Result<()> {
        let terminal_statuses = [
            TaskStatus::Failed("persisted failure".to_string()),
            TaskStatus::TimedOut {
                error: "persisted timeout".to_string(),
            },
            TaskStatus::Blocked("persisted blocker".to_string()),
        ];

        for status in terminal_statuses {
            let expected_error = persisted_status_error(&status)
                .ok_or_else(|| ReactError::Other("terminal status lost its detail".to_string()))?;
            let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
                "terminal",
                status,
                &[],
            )]));
            let executor = RuntimeDagExecutor::new(controller, RuntimeTaskServiceConfig::default());

            let outcome = executor.execute("run", CancellationToken::new()).await?;

            assert_eq!(
                outcome,
                RuntimeDagOutcome::Failed {
                    failed_task_id: "terminal".to_string(),
                    error: expected_error,
                }
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn executor_derives_downstream_block_without_persisting_it() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Pending, &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
        ]));
        controller
            .fail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("a".to_string(), "boom".to_string());
        let executor =
            RuntimeDagExecutor::new(controller.clone(), RuntimeTaskServiceConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(
            outcome,
            RuntimeDagOutcome::Failed {
                failed_task_id: "a".to_string(),
                error: "boom".to_string(),
            }
        );
        assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Pending));
        let tasks = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|snapshot| snapshot.tasks.clone())
            .ok_or_else(|| ReactError::Other("missing derived-block snapshot".to_string()))?;
        assert!(matches!(
            DagExecutionState::from_tasks(&tasks)
                .dependency_states(&tasks)
                .get("b"),
            Some(DagDependencyState::BlockedByFailure { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn executor_derives_transitive_downstream_chain_without_persistence() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Pending, &[]),
            runtime_task("b", TaskStatus::Pending, &["a"]),
            runtime_task("c", TaskStatus::Pending, &["b"]),
        ]));
        controller
            .fail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("a".to_string(), "boom".to_string());
        let executor =
            RuntimeDagExecutor::new(controller.clone(), RuntimeTaskServiceConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;
        assert!(matches!(outcome, RuntimeDagOutcome::Failed { .. }));
        let statuses = controller.statuses();
        for id in ["b", "c"] {
            assert_eq!(statuses.get(id), Some(&TaskStatus::Pending));
        }
        let tasks = controller
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|snapshot| snapshot.tasks.clone())
            .ok_or_else(|| ReactError::Other("missing transitive snapshot".to_string()))?;
        let dependencies = DagExecutionState::from_tasks(&tasks).dependency_states(&tasks);
        for id in ["b", "c"] {
            assert!(matches!(
                dependencies.get(id),
                Some(DagDependencyState::BlockedByFailure { .. })
            ));
        }
        Ok(())
    }
}
