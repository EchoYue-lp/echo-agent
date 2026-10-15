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
use echo_core::agent::admission::UncertainExecutionFence;
use echo_core::agent::{ExecutionAdmission, KeyedExecutionLease};
use echo_core::error::{ReactError, Result};
use futures::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::{Id, JoinSet};
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
    /// Completion for a task with no application result contract.
    Completed,
    /// Completion and its recoverable typed application output are one fact.
    CompletedWithResult {
        result: serde_json::Value,
    },
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
    /// A previously committed exact-claim request wins over a late dispatch
    /// result. It is distinct from a run-level stop request.
    Interrupted {
        disposition: RuntimeInterruptionDisposition,
    },
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

    /// Persist one exact cancellation/pause request without settling its claim.
    /// True is same-claim durable admission; false is superseded/mismatched.
    /// Implementations delegate to the canonical revision service request CAS.
    async fn request_task_interruption(
        &self,
        run_id: &str,
        task: &Task,
        claim: &TaskClaim,
        disposition: RuntimeInterruptionDisposition,
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
    /// Maximum cancellation-observer wait. Expiry never aborts the original
    /// dispatch/cleanup owner or proves physical settlement.
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
    attempt_task_ids: Arc<Mutex<HashMap<Id, String>>>,
    pub(crate) pending_attempt_interrupts: Arc<Mutex<HashMap<String, String>>>,
    pub(crate) attempt_interruption_started: Arc<Mutex<HashMap<String, tokio::time::Instant>>>,
    attempt_settlements: Arc<Mutex<HashMap<String, AttemptSettlementProjection>>>,
}

struct JoinedRuntimeAttempt<O> {
    task: Task,
    claim: TaskClaim,
    dispatch: Result<O>,
    completed_before_interruption: bool,
}

struct JoinedAttemptSettlement {
    task_id: TaskId,
    resolution: Option<RuntimeTaskResolution>,
    errors: Vec<String>,
    interruption_policy_failed: bool,
}

type JoinedAttemptOwnerResult = (
    Task,
    TaskClaim,
    std::result::Result<JoinedAttemptSettlement, tokio::task::JoinError>,
);

impl<C: RuntimeDagController> Clone for RuntimeDagExecutor<C> {
    fn clone(&self) -> Self {
        Self {
            controller: self.controller.clone(),
            config: self.config.clone(),
            validator: self.validator.clone(),
            attempt_cancellations: self.attempt_cancellations.clone(),
            attempt_runs: self.attempt_runs.clone(),
            attempt_task_ids: self.attempt_task_ids.clone(),
            pending_attempt_interrupts: self.pending_attempt_interrupts.clone(),
            attempt_interruption_started: self.attempt_interruption_started.clone(),
            attempt_settlements: self.attempt_settlements.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptSettlementPhase {
    Active,
    JoinedAwaitingDurableSettlement,
    AuthorityUnknown,
    Settled,
}

enum AttemptAdmissionLease {
    Shared { _lease: KeyedExecutionLease },
    Local { _permit: OwnedSemaphorePermit },
    SharedUnknown { _fence: UncertainExecutionFence },
}

struct AttemptSettlementProjection {
    phase: AttemptSettlementPhase,
    settled_tx: watch::Sender<Option<AttemptSettlementObservation>>,
    cancel: CancellationToken,
    physical_closed: bool,
    admission: Option<AttemptAdmissionLease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttemptSettlementObservation {
    Settled { status: String },
    AuthorityUnknown { error: String },
}

/// Single publication boundary for attempt control facts. A genuine terminal
/// receipt is immutable; uncertainty is only replaced by a verified settlement
/// or reconciliation fact, never by another diagnostic.
fn publish_attempt_fact(
    sender: &watch::Sender<Option<AttemptSettlementObservation>>,
    next: AttemptSettlementObservation,
) -> Option<AttemptSettlementPhase> {
    sender.send_if_modified(|current| match (&*current, &next) {
        (Some(AttemptSettlementObservation::Settled { .. }), _) => false,
        (
            Some(AttemptSettlementObservation::AuthorityUnknown { .. }),
            AttemptSettlementObservation::AuthorityUnknown { .. },
        ) => false,
        _ => {
            *current = Some(next);
            true
        }
    });
    match sender.borrow().as_ref() {
        Some(AttemptSettlementObservation::Settled { .. }) => Some(AttemptSettlementPhase::Settled),
        Some(AttemptSettlementObservation::AuthorityUnknown { .. }) => {
            Some(AttemptSettlementPhase::AuthorityUnknown)
        }
        None => None,
    }
}

/// Independent of the calling execute future and its JoinHandle observer.
/// An external backend can unwind while a detached CAS owner is running; its
/// retained watch must resolve to unknown rather than remain pending forever.
struct SettlementOwnerExitGuard {
    settled_tx: watch::Sender<Option<AttemptSettlementObservation>>,
    execution_id: String,
    projections: Arc<Mutex<HashMap<String, AttemptSettlementProjection>>>,
}

impl Drop for SettlementOwnerExitGuard {
    fn drop(&mut self) {
        let phase = publish_attempt_fact(&self.settled_tx, AttemptSettlementObservation::AuthorityUnknown {
            error: "settlement owner exited before publishing a durable receipt; reconcile the exact claim before retry".to_string(),
        });
        if let Some(phase) = phase
            && let Some(projection) = self
                .projections
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get_mut(&self.execution_id)
        {
            projection.phase = phase;
        }
    }
}

/// Last-resort ownership for claims whose dispatch has not yet moved to its
/// independent settlement owner. On an unexpected driver unwind, retain the
/// actual JoinSet, observe every physical stop, then use the canonical backend
/// abandonment CAS. A failed CAS publishes uncertainty, never a false receipt.
struct UnjoinedWaveOwner<C: RuntimeDagController> {
    joins: JoinSet<(String, Result<C::DispatchOutput>)>,
    claims: HashMap<String, (Task, TaskClaim)>,
    executor: RuntimeDagExecutor<C>,
    run_id: String,
    armed: bool,
}
impl<C: RuntimeDagController> std::ops::Deref for UnjoinedWaveOwner<C> {
    type Target = JoinSet<(String, Result<C::DispatchOutput>)>;
    fn deref(&self) -> &Self::Target {
        &self.joins
    }
}
impl<C: RuntimeDagController> std::ops::DerefMut for UnjoinedWaveOwner<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.joins
    }
}
impl<C: RuntimeDagController> Drop for UnjoinedWaveOwner<C> {
    fn drop(&mut self) {
        if !self.armed || self.claims.is_empty() {
            return;
        }
        let mut joins = std::mem::replace(&mut self.joins, JoinSet::new());
        let claims = std::mem::take(&mut self.claims);
        let executor = self.executor.clone();
        let run_id = self.run_id.clone();
        for (task, claim) in claims.values() {
            if let Some(token) = executor
                .attempt_cancellations
                .lock()
                .ok()
                .and_then(|controls| {
                    controls
                        .get(&claim.execution_id(&run_id, &task.spec.id))
                        .cloned()
                })
            {
                token.cancel();
            }
        }
        let runtime = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime,
            Err(error) => {
                for (task, claim) in claims.values() {
                    executor.mark_attempt_authority_unknown(&claim.execution_id(&run_id, &task.spec.id),
                        format!("dispatch join owner unavailable: {error}; reconcile physical stop and exact claim"));
                }
                return;
            }
        };
        runtime.spawn(async move {
            let mut claims = claims;
            let cancelled = CancellationToken::new();
            cancelled.cancel();
            while let Some(joined) = joins.join_next_with_id().await {
                match joined {
                    Ok((id, (claim_id, dispatch))) => {
                        if let Some(execution_id) = executor
                            .attempt_task_ids
                            .lock()
                            .ok()
                            .and_then(|mut ids| ids.remove(&id))
                        {
                            if matches!(&dispatch, Err(ReactError::ExecutionNotSettled { .. })) {
                                executor.mark_attempt_authority_unknown(
                                    &execution_id,
                                    "unwind drain returned observation without physical receipt",
                                );
                            } else {
                                executor.mark_attempt_joined(&execution_id);
                            }
                        }
                        if let Some((task, claim)) = claims.remove(&claim_id) {
                            let settlement_owner = executor.clone();
                            let settlement_run = run_id.clone();
                            let cancellation = cancelled.clone();
                            // A slow/panicking CAS cannot make this drain owner
                            // drop unrelated physical joins during unwind.
                            tokio::spawn(async move {
                                let execution_id =
                                    claim.execution_id(&settlement_run, &task.spec.id);
                                let settled_tx = settlement_owner
                                    .attempt_settlements
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .get(&execution_id)
                                    .map(|projection| projection.settled_tx.clone());
                                let _guard =
                                    settled_tx.map(|settled_tx| SettlementOwnerExitGuard {
                                        settled_tx,
                                        execution_id,
                                        projections: settlement_owner.attempt_settlements.clone(),
                                    });
                                settlement_owner
                                    .settle_joined_attempt(
                                        &settlement_run,
                                        JoinedRuntimeAttempt {
                                            task,
                                            claim,
                                            dispatch,
                                            completed_before_interruption: false,
                                        },
                                        cancellation,
                                    )
                                    .await;
                            });
                        }
                    }
                    Err(error) => {
                        if let Some(execution_id) = executor
                            .attempt_task_ids
                            .lock()
                            .ok()
                            .and_then(|mut ids| ids.remove(&error.id()))
                        {
                            executor.mark_attempt_authority_unknown(
                                &execution_id,
                                format!(
                                    "physical dispatch owner lost without closure receipt: {error}"
                                ),
                            );
                        }
                    }
                }
            }
            // Missing joins cannot prove that a remote/detached producer died.
            for (_, (task, claim)) in claims {
                executor.mark_attempt_authority_unknown(
                    &claim.execution_id(&run_id, &task.spec.id),
                    "driver unwind lost physical closure knowledge; retain exact claim",
                );
            }
        });
    }
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
            attempt_task_ids: Arc::new(Mutex::new(HashMap::new())),
            pending_attempt_interrupts: Arc::new(Mutex::new(HashMap::new())),
            attempt_interruption_started: Arc::new(Mutex::new(HashMap::new())),
            attempt_settlements: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Override structural limits while retaining the canonical validator.
    pub(crate) fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
    }

    pub(crate) fn note_attempt_interruption(&self, execution_id: String) -> tokio::time::Instant {
        let mut started = self
            .attempt_interruption_started
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *started
            .entry(execution_id)
            .or_insert_with(tokio::time::Instant::now)
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
            AttemptSettlementPhase::JoinedAwaitingDurableSettlement
            | AttemptSettlementPhase::AuthorityUnknown
            | AttemptSettlementPhase::Settled => {
                RuntimeAttemptObservation::JoinedAwaitingDurableSettlement { settled_rx }
            }
        })
    }

    fn retain_attempt_admission(
        &self,
        execution_id: &str,
        cancel: CancellationToken,
        lease: AttemptAdmissionLease,
    ) {
        let mut projections = self
            .attempt_settlements
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let projection = projections
            .entry(execution_id.to_string())
            .or_insert_with(|| {
                let (settled_tx, _) = watch::channel(None);
                AttemptSettlementProjection {
                    phase: AttemptSettlementPhase::AuthorityUnknown,
                    settled_tx,
                    cancel,
                    physical_closed: false,
                    admission: None,
                }
            });
        projection.admission = Some(lease);
    }

    fn mark_attempt_joined(&self, execution_id: &str) {
        let mut settlements = self
            .attempt_settlements
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(projection) = settlements.get_mut(execution_id) {
            projection.physical_closed = true;
            if projection.phase == AttemptSettlementPhase::Active {
                projection.phase = AttemptSettlementPhase::JoinedAwaitingDurableSettlement;
            }
        }
    }

    fn attempt_authority_unknown(&self, execution_id: &str) -> bool {
        self.attempt_settlements
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(execution_id)
            .is_some_and(|projection| projection.phase == AttemptSettlementPhase::AuthorityUnknown)
    }

    fn publish_attempt_projection(&self, execution_id: &str, fact: AttemptSettlementObservation) {
        if let Ok(mut settlements) = self.attempt_settlements.lock()
            && let Some(projection) = settlements.get_mut(execution_id)
            && let Some(phase) = publish_attempt_fact(&projection.settled_tx, fact)
        {
            projection.phase = phase;
            if phase == AttemptSettlementPhase::AuthorityUnknown {
                projection.admission = projection.admission.take().map(|lease| match lease {
                    AttemptAdmissionLease::Shared { _lease } => {
                        AttemptAdmissionLease::SharedUnknown {
                            _fence: _lease.retain_unknown(),
                        }
                    }
                    other => other,
                });
            }
        }
    }

    pub(crate) fn settle_attempt_projection(&self, execution_id: &str, status: impl Into<String>) {
        self.publish_attempt_projection(
            execution_id,
            AttemptSettlementObservation::Settled {
                status: status.into(),
            },
        );
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
            let physical_closed = self
                .attempt_settlements
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&execution_id)
                .is_some_and(|projection| projection.physical_closed);
            if physical_closed {
                self.settle_attempt_projection(&execution_id, "superseded");
            } else {
                self.mark_attempt_authority_unknown(
                    &execution_id,
                    "durable ownership changed without original physical closure receipt",
                );
            }
            if let Some(cancel) = self
                .attempt_cancellations
                .lock()
                .ok()
                .and_then(|controls| controls.get(&execution_id).cloned())
            {
                cancel.cancel();
            }
            if physical_closed {
                self.retire_local_attempt_control(&execution_id);
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
                .pending_attempt_interrupts
                .lock()
                .is_ok_and(|pending| pending.contains_key(execution_id))
            || self
                .attempt_interruption_started
                .lock()
                .is_ok_and(|scheduled| scheduled.contains_key(execution_id))
            || self
                .attempt_settlements
                .lock()
                .is_ok_and(|settlements| settlements.contains_key(execution_id))
    }

    fn mark_attempt_authority_unknown(&self, execution_id: &str, error: impl Into<String>) {
        self.publish_attempt_projection(
            execution_id,
            AttemptSettlementObservation::AuthorityUnknown {
                error: error.into(),
            },
        );
    }

    fn retire_local_attempt_control(&self, execution_id: &str) {
        if let Ok(mut controls) = self.attempt_cancellations.lock() {
            controls.remove(execution_id);
        }
        if let Ok(mut runs) = self.attempt_runs.lock() {
            runs.remove(execution_id);
        }
        if let Ok(mut pending) = self.pending_attempt_interrupts.lock() {
            pending.remove(execution_id);
        }
        if let Ok(mut scheduled) = self.attempt_interruption_started.lock() {
            scheduled.remove(execution_id);
        }
        if let Ok(mut settlements) = self.attempt_settlements.lock()
            && let Some(projection) = settlements.remove(execution_id)
        {
            if projection.physical_closed
                && projection.phase == AttemptSettlementPhase::Settled
                && let Some(AttemptAdmissionLease::SharedUnknown { _fence }) = projection.admission
            {
                // This retirement path follows original physical join and a
                // canonical durable receipt, never a cancel/socket/missing key.
                if let Some(admission) = &self.config.shared_admission {
                    admission.keyed().confirm_uncertain_stopped(&_fence);
                }
            }
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

            if state.all_completed(&tasks) {
                return Ok(RuntimeDagOutcome::Completed);
            }

            // A task-local interruption is not a run stop request. Only the
            // explicit run token invokes run-wide settlement; paused/cancelled
            // nodes otherwise constrain dependencies while independent waves
            // continue from the current graph.
            if cancel.is_cancelled() && !state.in_flight.is_empty() {
                // These claims were loaded before this driver started a wave.
                // Re-entry/restart cannot infer another owner's physical stop.
                return Err(ReactError::ExecutionNotSettled {
                    run_id: run_id.to_string(),
                    execution_id: None,
                });
            }
            if cancel.is_cancelled() {
                let disposition = self.controller.interruption_disposition(run_id).await?;
                match self
                    .settle_interruption_boundary(run_id, snapshot.revision, disposition, true)
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
                            return Err(ReactError::ExecutionNotSettled { run_id: run_id.to_string(), execution_id: None });
                        }
                        _ = tokio::time::sleep(self.config.external_progress_poll_interval) => {}
                    }
                    continue;
                }

                // Aggregate interruption only after independent work is
                // exhausted. Do not mutate still-pending dependent branches.
                if !state.cancelled.is_empty() {
                    return Ok(RuntimeDagOutcome::Cancelled);
                }
                if let Some(paused_task) = tasks
                    .iter()
                    .find(|task| matches!(task.execution.status, TaskStatus::Paused(_)))
                {
                    return Ok(RuntimeDagOutcome::Paused {
                        task_id: Some(paused_task.spec.id.clone()),
                        reason: persisted_status_error(&paused_task.execution.status)
                            .unwrap_or_else(|| "runtime task paused".to_string()),
                    });
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

            let mut join_set = UnjoinedWaveOwner {
                joins: JoinSet::new(),
                claims: HashMap::new(),
                executor: self.clone(),
                run_id: run_id.to_string(),
                armed: true,
            };
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
                // Retain immediately after claim return, before any fallible
                // reservation or awaited admission boundary can unwind.
                join_set
                    .claims
                    .insert(claim_id.clone(), (task.clone(), claim.clone()));
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
                            physical_closed: false,
                            admission: None,
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
                            Err(settlement_error) => {
                                self.mark_attempt_authority_unknown(
                                    &execution_id,
                                    settlement_error.to_string(),
                                );
                                wave_errors.push(format!("invalid context abandonment outcome is unknown: {settlement_error}"));
                            }
                        }
                        wave_errors.push(message);
                        continue;
                    }
                };
                if let Err(error) = self.controller.reserve_attempt_control(&context).await {
                    let message = format!("exact attempt control reservation failed: {error}");
                    match self
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
                        Err(settlement_error) => {
                            self.mark_attempt_authority_unknown(
                                &execution_id,
                                settlement_error.to_string(),
                            );
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
                let admission_owner = self.clone();
                let admission_execution_id = execution_id.clone();
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
                                admission_owner.retain_attempt_admission(&admission_execution_id, task_cancel.clone(), AttemptAdmissionLease::Shared { _lease: lease });
                                controller.dispatch_task(context, task).await
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
                                admission_owner.retain_attempt_admission(&admission_execution_id, task_cancel.clone(), AttemptAdmissionLease::Local { _permit: permit });
                                controller.dispatch_task(context, task).await
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
                if supervisor_cancel.is_cancelled() {
                    if let Some((task, claim)) =
                        outstanding_claims.values().find(|(task, claim)| {
                            claim.execution_id(run_id, &task.spec.id) == abort_execution_id
                        })
                    {
                        let disposition = self.controller.interruption_disposition(run_id).await?;
                        if self
                            .controller
                            .request_task_interruption(run_id, task, claim, disposition)
                            .await?
                        {
                            self.note_attempt_interruption(abort_execution_id.clone());
                        }
                    }
                }
            }

            let mut wave_results = Vec::new();
            let mut settlements: FuturesUnordered<BoxFuture<'static, JoinedAttemptOwnerResult>> =
                FuturesUnordered::new();
            let mut interruption_policy_failed = false;
            let mut interruption = None;
            let mut pending_outcome = None;
            let mut cancellation_observed = false;
            let mut unsettled_execution: Option<String> = None;
            while !join_set.is_empty() || !settlements.is_empty() {
                tokio::select! {
                    biased;
                    joined = join_set.join_next_with_id(), if !join_set.is_empty() => {
                        match joined {
                            Some(Ok((task_id, result))) => {
                                if let Some(execution_id) = self
                                    .attempt_task_ids
                                    .lock()
                                    .ok()
                                    .and_then(|mut task_ids| task_ids.remove(&task_id))
                                {
                                    if matches!(&result.1, Err(ReactError::ExecutionNotSettled { .. })) {
                                        self.mark_attempt_authority_unknown(&execution_id, "dispatch returned an observation without closure receipt");
                                        unsettled_execution.get_or_insert(execution_id);
                                    } else {
                                        self.mark_attempt_joined(&execution_id);
                                    }
                                }
                                wave_results.push((
                                    result,
                                    !cancellation_observed && !cancel.is_cancelled(),
                                ));
                            }
                            Some(Err(error)) => {
                                if let Some(execution_id) = self.attempt_task_ids.lock().ok()
                                    .and_then(|mut ids| ids.remove(&error.id())) {
                                    self.mark_attempt_authority_unknown(&execution_id,
                                        format!("physical dispatch owner lost: {error}"));
                                    unsettled_execution.get_or_insert(execution_id);
                                }
                                wave_errors.push(format!("Subagent dispatch task failed to join: {error}"));
                            }
                            None => {}
                        }
                    }
                    settled = settlements.next(), if !settlements.is_empty() => {
                        if let Some((task, claim, result)) = settled {
                            match result {
                                Ok(settled) => {
                                    let execution_id = claim.execution_id(run_id, &task.spec.id);
                                    if self.attempt_authority_unknown(&execution_id) { unsettled_execution.get_or_insert(execution_id); }
                                    wave_errors.extend(settled.errors);
                                    interruption_policy_failed |= settled.interruption_policy_failed;
                                    if let Some(resolution) = settled.resolution {
                                        match resolution {
                                            RuntimeTaskResolution::Failed { error } | RuntimeTaskResolution::TimedOut { error } => {
                                                failure_errors.entry(settled.task_id).or_insert(error);
                                            }
                                            RuntimeTaskResolution::Blocked { error, disposition } => {
                                                if pending_outcome.is_none() { pending_outcome = Some(stop_outcome(disposition, settled.task_id, error)); }
                                            }
                                            RuntimeTaskResolution::Completed | RuntimeTaskResolution::Pending | RuntimeTaskResolution::Skipped | RuntimeTaskResolution::Superseded | RuntimeTaskResolution::Cancelled | RuntimeTaskResolution::Interrupted { .. } => {}
                                        }
                                    }
                                }
                                Err(error) => {
                                    let message = format!("runtime attempt settlement owner failed: {error}");
                                    let execution_id = claim.execution_id(run_id, &task.spec.id);
                                    self.mark_attempt_authority_unknown(&execution_id, message.clone());
                                    // A post-CAS cleanup panic cannot downgrade an
                                    // immutable real receipt to unknown execution.
                                    if self.attempt_authority_unknown(&execution_id) {
                                        unsettled_execution.get_or_insert(execution_id);
                                    }
                                    wave_errors.push(message);
                                }
                            }
                        }
                    }
                    _ = cancel.cancelled(), if !cancellation_observed => {
                        cancellation_observed = true;
                        match self.controller.interruption_disposition(run_id).await {
                            Ok(disposition) => {
                                for (task, claim) in outstanding_claims.values() {
                                    match self.controller.request_task_interruption(run_id, task, claim, disposition.clone()).await {
                                        Ok(true) => { self.note_attempt_interruption(claim.execution_id(run_id, &task.spec.id)); }
                                        Ok(false) => {}, // Supersession is not request admission.
                                        Err(error) => {
                                            self.mark_attempt_authority_unknown(&claim.execution_id(run_id, &task.spec.id), error.to_string());
                                            wave_errors.push(format!("durable interruption request failed: {error}"));
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                interruption_policy_failed = true;
                                for (task, claim) in outstanding_claims.values() {
                                    self.mark_attempt_authority_unknown(&claim.execution_id(run_id, &task.spec.id), error.to_string());
                                }
                                wave_errors.push(error.to_string());
                            }
                        }
                    }
                }
                // Resolution, CAS and post-CAS cleanup may await slow policy
                // or backends. They keep independent owners while this same
                // supervisor continues joining dispatches and enforcing root
                // cancellation grace. Dropping the caller's JoinHandle cannot
                // abort a CAS whose commit result is not yet known.
                for ((claim_id, dispatch), completed_before_interruption) in wave_results.drain(..)
                {
                    let Some((task, claim)) = outstanding_claims.remove(&claim_id) else {
                        wave_errors.push(format!("dispatch returned unknown claim '{claim_id}'"));
                        continue;
                    };
                    join_set.claims.remove(&claim_id);
                    let supervisor = self.clone();
                    let settlement_run_id = run_id.to_string();
                    let settlement_cancel = cancel.clone();
                    let observation_task = task.clone();
                    let observation_claim = claim.clone();
                    // Construct outside the async body, so even an owner
                    // dropped before its first poll resolves the exact watch.
                    let owner_exit_guard = self
                        .attempt_settlements
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(&claim.execution_id(run_id, &task.spec.id))
                        .map(|projection| SettlementOwnerExitGuard {
                            settled_tx: projection.settled_tx.clone(),
                            execution_id: claim.execution_id(run_id, &task.spec.id),
                            projections: self.attempt_settlements.clone(),
                        });
                    let owner = tokio::spawn(async move {
                        let _owner_exit_guard = owner_exit_guard;
                        supervisor
                            .settle_joined_attempt(
                                &settlement_run_id,
                                JoinedRuntimeAttempt {
                                    task,
                                    claim,
                                    dispatch,
                                    completed_before_interruption,
                                },
                                settlement_cancel,
                            )
                            .await
                    });
                    settlements.push(Box::pin(async move {
                        (observation_task, observation_claim, owner.await)
                    }));
                }
            }

            if let Some(execution_id) = unsettled_execution {
                join_set.armed = false;
                return Err(ReactError::ExecutionNotSettled {
                    run_id: run_id.to_string(),
                    execution_id: Some(execution_id),
                });
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

            let unknown_physical_owner = !outstanding_claims.is_empty();
            for (_claim_id, (task, claim)) in outstanding_claims {
                self.mark_attempt_authority_unknown(
                    &claim.execution_id(run_id, &task.spec.id),
                    "dispatch owner exited without an authoritative physical closure receipt",
                );
            }
            if unknown_physical_owner {
                join_set.armed = false;
                return Err(ReactError::ExecutionNotSettled {
                    run_id: run_id.to_string(),
                    execution_id: None,
                });
            }

            join_set.armed = false;

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
                    .settle_interruption_boundary(run_id, snapshot.revision, disposition, true)
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

    async fn settle_joined_attempt(
        &self,
        run_id: &str,
        attempt: JoinedRuntimeAttempt<C::DispatchOutput>,
        cancel: CancellationToken,
    ) -> JoinedAttemptSettlement {
        let JoinedRuntimeAttempt {
            task,
            claim,
            dispatch,
            completed_before_interruption,
        } = attempt;
        let mut wave_errors = Vec::new();
        let mut interruption_policy_failed = false;
        if let Err(ReactError::ExecutionNotSettled { .. }) = &dispatch {
            self.mark_attempt_authority_unknown(
                &claim.execution_id(run_id, &task.spec.id),
                "dispatch returned observation without physical closure receipt",
            );
            return JoinedAttemptSettlement {
                task_id: task.spec.id,
                resolution: None,
                errors: vec!["physical execution authority remains unknown".to_string()],
                interruption_policy_failed,
            };
        }
        let interruption =
            if cancel.is_cancelled() && !completed_before_interruption && dispatch.is_err() {
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
        if interruption.is_some() && !completed_before_interruption && dispatch.is_err() {
            let Some(disposition) = interruption.clone() else {
                return JoinedAttemptSettlement {
                    task_id: task.spec.id,
                    resolution: None,
                    errors: vec!["interruption disappeared".to_string()],
                    interruption_policy_failed,
                };
            };
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
            return JoinedAttemptSettlement {
                task_id: task.spec.id,
                resolution: None,
                errors: wave_errors,
                interruption_policy_failed,
            };
        }
        let request = match self
            .controller
            .resolve_dispatch(run_id, claim.clone(), task.clone(), dispatch)
            .await
        {
            Ok(resolution) => resolution,
            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                self.mark_attempt_authority_unknown(
                    &claim.execution_id(run_id, &task.spec.id),
                    error.to_string(),
                );
                return JoinedAttemptSettlement {
                    task_id: task.spec.id,
                    resolution: None,
                    errors: vec![error.to_string()],
                    interruption_policy_failed,
                };
            }
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
                        return JoinedAttemptSettlement {
                            task_id: task.spec.id,
                            resolution: None,
                            errors: wave_errors,
                            interruption_policy_failed,
                        };
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
                return JoinedAttemptSettlement {
                    task_id: task.spec.id,
                    resolution: None,
                    errors: wave_errors,
                    interruption_policy_failed,
                };
            }
        };
        let resolution = match self
            .settle_dispatch_request(run_id, &claim, &task, request)
            .await
        {
            Ok(resolution) => resolution,
            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                self.mark_attempt_authority_unknown(
                    &claim.execution_id(run_id, &task.spec.id),
                    error.to_string(),
                );
                return JoinedAttemptSettlement {
                    task_id: task.spec.id,
                    resolution: None,
                    errors: vec![error.to_string()],
                    interruption_policy_failed,
                };
            }
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
                        return JoinedAttemptSettlement {
                            task_id: task.spec.id,
                            resolution: None,
                            errors: wave_errors,
                            interruption_policy_failed,
                        };
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
                return JoinedAttemptSettlement {
                    task_id: task.spec.id,
                    resolution: None,
                    errors: wave_errors,
                    interruption_policy_failed,
                };
            }
        };
        self.cleanup_attempt_control(run_id, &task, &claim).await;

        JoinedAttemptSettlement {
            task_id: task.spec.id,
            resolution: Some(resolution),
            errors: wave_errors,
            interruption_policy_failed,
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
                    let current = self.controller.load_snapshot(run_id).await?;
                    if DagExecutionState::from_tasks(&current.tasks).all_completed(&current.tasks) {
                        return Ok(InterruptionBoundary::NoUnfinishedWork);
                    }
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
        let status = match &settlement {
            RuntimeTaskSettlementOutcome::Settled { status } => committed_task_status(status),
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
        RuntimeTaskResolution::Interrupted {
            disposition: RuntimeInterruptionDisposition::Cancelled,
        } => "cancelled",
        RuntimeTaskResolution::Interrupted {
            disposition: RuntimeInterruptionDisposition::Paused { .. },
        } => "paused",
        RuntimeTaskResolution::Pending => "pending",
        RuntimeTaskResolution::Skipped => "skipped",
        RuntimeTaskResolution::Failed { .. } => "failed",
        RuntimeTaskResolution::TimedOut { .. } => "timed_out",
        RuntimeTaskResolution::Blocked { .. } => "blocked",
        RuntimeTaskResolution::Cancelled => "cancelled",
        RuntimeTaskResolution::Superseded => "superseded",
    }
}

fn committed_task_status(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Retrying { .. } => "retrying",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed(_) => "failed",
        TaskStatus::TimedOut { .. } => "timed_out",
        TaskStatus::Blocked(_) => "blocked",
        TaskStatus::Skipped => "skipped",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Paused(_) => "paused",
    }
}

fn resolution_matches_request(
    request: &RuntimeTaskResolutionRequest,
    resolution: &RuntimeTaskResolution,
) -> bool {
    if resolution == &RuntimeTaskResolution::Superseded
        || matches!(resolution, RuntimeTaskResolution::Interrupted { .. })
    {
        return true;
    }
    match (request, resolution) {
        (
            RuntimeTaskResolutionRequest::Completed
            | RuntimeTaskResolutionRequest::CompletedWithResult { .. },
            RuntimeTaskResolution::Completed,
        )
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

    type TaskPhaseBarrier = (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>);

    #[derive(Default)]
    struct ScriptedController {
        snapshot: Mutex<Option<RuntimePlanSnapshot>>,
        order: Mutex<Vec<TaskId>>,
        fail: Mutex<HashMap<TaskId, String>>,
        dispatch_observation: Mutex<HashSet<TaskId>>,
        resolve_observation: Mutex<HashSet<TaskId>>,
        blocked: Mutex<HashMap<TaskId, (String, RuntimeStopDisposition)>>,
        wait_for_cancel: Mutex<HashSet<TaskId>>,
        insert_after: Mutex<Option<TaskId>>,
        reload_claim_once: Mutex<bool>,
        interruption: Mutex<RuntimeInterruptionDisposition>,
        interruption_error: Mutex<Option<String>>,
        dispatch_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
        claim_response_barrier: Mutex<Option<TaskPhaseBarrier>>,
        claim_owner_panic_after_commit: Mutex<bool>,
        reservation_barriers: Mutex<HashMap<TaskId, TaskPhaseBarrier>>,
        reservation_owner_panics: Mutex<HashSet<TaskId>>,
        dispatch_release: Mutex<HashMap<TaskId, Arc<tokio::sync::Notify>>>,
        dispatch_dropped: Mutex<HashMap<TaskId, Arc<std::sync::atomic::AtomicBool>>>,
        resolution_barriers: Mutex<HashMap<TaskId, TaskPhaseBarrier>>,
        settlement_barriers: Mutex<HashMap<TaskId, TaskPhaseBarrier>>,
        settlement_owner_panics: Mutex<HashSet<TaskId>>,
        cleanup_barriers: Mutex<HashMap<TaskId, TaskPhaseBarrier>>,
        cleanup_owner_panics: Mutex<HashSet<TaskId>>,
        cleanup_owner_exited: Mutex<HashMap<TaskId, Arc<tokio::sync::Notify>>>,
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
            let result = {
                let mut reload_claim_once = self
                    .reload_claim_once
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if *reload_claim_once {
                    *reload_claim_once = false;
                    return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
                }
                drop(reload_claim_once);
                let mut snapshot = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
                let snapshot = snapshot
                    .as_mut()
                    .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
                super::super::runtime_service::claim_runtime_task(snapshot, task, expected_revision)
                    .map_err(|error| ReactError::Other(error.to_string()))?
            };
            if let Some(notify) = self
                .claim_ready
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
            {
                notify.notify_one();
            }
            let barrier = self
                .claim_response_barrier
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some((entered, release)) = barrier {
                entered.notify_one();
                release.notified().await;
            }
            if *self
                .claim_owner_panic_after_commit
                .lock()
                .unwrap_or_else(|p| p.into_inner())
            {
                // Controlled external backend fault: a commit happened but no
                // exact claim receipt was returned to the framework owner.
                std::panic::resume_unwind(Box::new("claim committed then backend unwound"));
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

        async fn request_task_interruption(
            &self,
            _run_id: &str,
            task: &Task,
            claim: &TaskClaim,
            disposition: RuntimeInterruptionDisposition,
        ) -> Result<bool> {
            let mut guarded = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
            let snapshot = guarded
                .as_mut()
                .ok_or_else(|| ReactError::Other("snapshot missing".to_string()))?;
            super::super::request_runtime_task_interruption(
                snapshot,
                &task.spec.id,
                claim,
                disposition,
            )
            .map_err(|error| ReactError::Other(error.to_string()))
        }

        async fn reserve_attempt_control(&self, context: &TaskSubagentContext) -> Result<()> {
            let barrier = self
                .reservation_barriers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(context.task_id().unwrap_or_default())
                .cloned();
            if let Some((entered, release)) = barrier {
                entered.notify_one();
                release.notified().await;
            }
            if self
                .reservation_owner_panics
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(context.task_id().unwrap_or_default())
            {
                // External backend unwind fixture, not a production panic.
                std::panic::resume_unwind(Box::new("reservation backend unwind"));
            }
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
            let barrier = self
                .cleanup_barriers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            if let Some((entered, release)) = barrier {
                entered.notify_one();
                release.notified().await;
            }
            struct CleanupExitProbe(Option<Arc<tokio::sync::Notify>>);
            impl Drop for CleanupExitProbe {
                fn drop(&mut self) {
                    if let Some(exited) = &self.0 {
                        exited.notify_one();
                    }
                }
            }
            let _exit_probe = CleanupExitProbe(
                self.cleanup_owner_exited
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&task.spec.id)
                    .cloned(),
            );
            if self
                .cleanup_owner_panics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&task.spec.id)
            {
                // Test an external cleanup implementation unwinding after CAS.
                std::panic::resume_unwind(Box::new("injected external post-CAS cleanup panic"));
            }
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
            struct DispatchDropProbe(Option<Arc<std::sync::atomic::AtomicBool>>);
            impl Drop for DispatchDropProbe {
                fn drop(&mut self) {
                    if let Some(dropped) = &self.0 {
                        dropped.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
            let _drop_probe = DispatchDropProbe(
                self.dispatch_dropped
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&task.spec.id)
                    .cloned(),
            );
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
            let release = self
                .dispatch_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            if let Some(release) = release {
                release.notified().await;
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
            if self
                .dispatch_observation
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(&task.spec.id)
            {
                return Err(ReactError::ExecutionNotSettled {
                    run_id: context.run_id().to_string(),
                    execution_id: context.execution_id(),
                });
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
            run_id: &str,
            claim: TaskClaim,
            task: Task,
            dispatch: Result<Self::DispatchOutput>,
        ) -> Result<RuntimeTaskResolutionRequest> {
            if self
                .resolve_observation
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(&task.spec.id)
            {
                return Err(ReactError::ExecutionNotSettled {
                    run_id: run_id.to_string(),
                    execution_id: Some(claim.execution_id(run_id, &task.spec.id)),
                });
            }
            let barrier = self
                .resolution_barriers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            if let Some((entered, release)) = barrier {
                entered.notify_one();
                release.notified().await;
            }
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
            let barrier = self
                .settlement_barriers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            if let Some((entered, release)) = barrier {
                entered.notify_one();
                release.notified().await;
            }
            if self
                .settlement_owner_panics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(&task.spec.id)
            {
                // Simulates a panicking external backend, not framework policy.
                std::panic::resume_unwind(Box::new("injected external settlement owner panic"));
            }
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
                result: None,
                interruption_requested: None,
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
        assert!(
            matches!(&execution_error, ReactError::ExecutionNotSettled { run_id, execution_id: Some(actual) }
            if run_id == "joined-unknown" && actual == &execution_id)
        );
        assert!(
            matches!(service.attempt_settlement_observation(&execution_id),
            Some(AttemptSettlementObservation::AuthorityUnknown { error }) if error.contains("claim lookup failed"))
        );
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
        let claim = controller
            .load_snapshot("unknown-reservation")
            .await?
            .tasks
            .first()
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("unknown claim missing".to_string()))?;
        assert!(matches!(
            service.attempt_settlement_observation(
                &claim.execution_id("unknown-reservation", "reserved")
            ),
            Some(AttemptSettlementObservation::AuthorityUnknown { .. })
        ));
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

    #[test]
    fn attempt_publication_fact_preserves_terminal_receipts_and_requires_recovery_for_unknown() {
        let unknown = AttemptSettlementObservation::AuthorityUnknown {
            error: "commit response missing".to_string(),
        };
        let completed = AttemptSettlementObservation::Settled {
            status: "completed".to_string(),
        };
        let superseded = AttemptSettlementObservation::Settled {
            status: "superseded".to_string(),
        };
        let cases = [
            (
                None,
                unknown.clone(),
                unknown.clone(),
                AttemptSettlementPhase::AuthorityUnknown,
            ),
            (
                None,
                completed.clone(),
                completed.clone(),
                AttemptSettlementPhase::Settled,
            ),
            (
                Some(unknown.clone()),
                AttemptSettlementObservation::AuthorityUnknown {
                    error: "later cleanup failed".to_string(),
                },
                unknown.clone(),
                AttemptSettlementPhase::AuthorityUnknown,
            ),
            (
                Some(unknown),
                superseded.clone(),
                superseded.clone(),
                AttemptSettlementPhase::Settled,
            ),
            (
                Some(completed.clone()),
                superseded,
                completed.clone(),
                AttemptSettlementPhase::Settled,
            ),
            (
                Some(completed.clone()),
                AttemptSettlementObservation::AuthorityUnknown {
                    error: "late cleanup failed".to_string(),
                },
                completed,
                AttemptSettlementPhase::Settled,
            ),
        ];
        for (previous, next, expected, expected_phase) in cases {
            let (sender, receiver) = watch::channel(previous);
            assert_eq!(publish_attempt_fact(&sender, next), Some(expected_phase));
            assert_eq!(receiver.borrow().as_ref(), Some(&expected));
            drop(sender);
            assert_eq!(receiver.borrow().as_ref(), Some(&expected));
        }
    }

    #[tokio::test]
    async fn public_reconciliation_keeps_committed_receipt_during_held_cleanup_and_reexecution()
    -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "a",
            TaskStatus::Pending,
            &[],
        )]));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        controller
            .cleanup_barriers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("a".to_string(), (entered.clone(), release.clone()));
        controller
            .cleanup_owner_panics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("a".to_string());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let execution = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .execute("reconcile-committed", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("cleanup not entered".to_string()))?;
        let execution_id = controller
            .retired
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .first()
            .cloned()
            .ok_or_else(|| ReactError::Other("exact cleanup identity missing".to_string()))?;
        let receiver = service
            .attempt_settlement_receiver(&execution_id)
            .ok_or_else(|| ReactError::Other("settlement watch missing".to_string()))?;
        let completed = AttemptSettlementObservation::Settled {
            status: "completed".to_string(),
        };
        assert_eq!(receiver.borrow().as_ref(), Some(&completed));
        service
            .reconcile_attempt_control("reconcile-committed")
            .await?;
        assert_eq!(receiver.borrow().as_ref(), Some(&completed));
        assert_eq!(
            service
                .execute("reconcile-committed", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Completed
        );
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["a"]
        );
        release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("cleanup failure not reported".to_string()))?
            .map_err(|error| ReactError::Other(format!("executor join failed: {error}")))?;
        assert!(result.is_err_and(|error| error.to_string().contains("settlement owner failed")));
        assert_eq!(receiver.borrow().as_ref(), Some(&completed));
        assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Completed));
        Ok(())
    }

    #[tokio::test]
    async fn post_cas_cleanup_panic_cannot_downgrade_true_receipt_with_live_or_aborted_parent()
    -> Result<()> {
        for abort_parent in [false, true] {
            let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
                "a",
                TaskStatus::Pending,
                &[],
            )]));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let exited = Arc::new(tokio::sync::Notify::new());
            controller
                .cleanup_barriers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("a".to_string(), (entered.clone(), release.clone()));
            controller
                .cleanup_owner_panics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("a".to_string());
            controller
                .cleanup_owner_exited
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("a".to_string(), exited.clone());
            let service = Arc::new(super::super::RuntimeTaskService::new(
                controller.clone(),
                RuntimeTaskServiceConfig::default(),
            ));
            let execution = tokio::spawn({
                let service = service.clone();
                async move {
                    service
                        .execute("cleanup-after-cas", CancellationToken::new())
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(2), entered.notified())
                .await
                .map_err(|_| ReactError::Other("cleanup did not enter after CAS".to_string()))?;
            let execution_id = controller
                .retired
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .first()
                .cloned()
                .ok_or_else(|| ReactError::Other("cleanup exact identity missing".to_string()))?;
            let receiver = service
                .attempt_settlement_receiver(&execution_id)
                .ok_or_else(|| ReactError::Other("retained watch missing".to_string()))?;
            let completed = AttemptSettlementObservation::Settled {
                status: "completed".to_string(),
            };
            assert_eq!(receiver.borrow().as_ref(), Some(&completed));
            assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Completed));
            if abort_parent {
                execution.abort();
                assert!(execution.await.is_err_and(|error| error.is_cancelled()));
                release.notify_one();
                tokio::time::timeout(Duration::from_secs(2), exited.notified())
                    .await
                    .map_err(|_| {
                        ReactError::Other("detached cleanup owner did not exit".to_string())
                    })?;
            } else {
                release.notify_one();
                let result = tokio::time::timeout(Duration::from_secs(2), execution)
                    .await
                    .map_err(|_| {
                        ReactError::Other("live parent did not report cleanup failure".to_string())
                    })?
                    .map_err(|error| ReactError::Other(format!("executor join failed: {error}")))?;
                assert!(
                    result
                        .is_err_and(|error| error.to_string().contains("settlement owner failed"))
                );
            }
            assert_eq!(receiver.borrow().as_ref(), Some(&completed));
            assert_eq!(
                service.attempt_settlement_observation(&execution_id),
                Some(completed)
            );
            assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Completed));
            assert_eq!(
                controller
                    .load_snapshot("cleanup-after-cas")
                    .await?
                    .tasks
                    .first()
                    .and_then(|task| task.execution.claim.as_ref()),
                None
            );
        }
        Ok(())
    }

    #[test]
    fn execution_without_tokio_runtime_fails_before_backend_admission() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "a",
            TaskStatus::Pending,
            &[],
        )]));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        let result = futures::executor::block_on(
            service.execute("without-runtime", CancellationToken::new()),
        );
        assert!(result.is_err_and(|error| error.to_string().contains("active Tokio runtime")));
        assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Pending));
        assert!(
            controller
                .order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn claim_backend_commit_then_unwind_reports_unknown_without_guessing_ownership()
    -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "a",
            TaskStatus::Pending,
            &[],
        )]));
        *controller
            .claim_owner_panic_after_commit
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = true;
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        let result = service
            .execute("unreturned-claim", CancellationToken::new())
            .await;
        assert!(
            matches!(result, Err(ReactError::ExecutionNotSettled { ref run_id, execution_id: None }) if run_id == "unreturned-claim")
        );
        let before = controller.load_snapshot("unreturned-claim").await?;
        let claim = before
            .tasks
            .first()
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("committed unknown claim missing".to_string()))?;
        assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Running));
        assert!(
            service
                .attempt_settlement_receiver(&claim.execution_id("unreturned-claim", "a"))
                .is_none()
        );
        assert!(
            controller
                .order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        );
        // Reconciliation reads the authority but cannot prove an unreturned
        // backend claim belongs to this process. It must not mint a retry or
        // manufacture a settled receipt; backend fencing owns that decision.
        service
            .reconcile_attempt_control("unreturned-claim")
            .await?;
        let after = controller.load_snapshot("unreturned-claim").await?;
        assert_eq!(
            after.tasks.first().map(|task| task.execution.clone()),
            before.tasks.first().map(|task| task.execution.clone())
        );
        Ok(())
    }

    async fn prove_parent_abort_during_claim_or_reservation(claim_pending: bool) -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "pending",
            TaskStatus::Pending,
            &[],
        )]));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        if claim_pending {
            *controller
                .claim_response_barrier
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = Some((entered.clone(), release.clone()));
        } else {
            controller
                .reservation_barriers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert("pending".to_string(), (entered.clone(), release.clone()));
        }
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("pending".to_string());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        controller
            .dispatch_dropped
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("pending".to_string(), dropped.clone());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let parent_cancel = CancellationToken::new();
        let execution = tokio::spawn({
            let service = service.clone();
            let cancel = parent_cancel.clone();
            async move { service.execute("parent-drop-admission", cancel).await }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("claim/reservation never entered".to_string()))?;
        let claim = controller
            .load_snapshot("parent-drop-admission")
            .await?
            .tasks
            .first()
            .and_then(|t| t.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("claim commit missing".to_string()))?;
        execution.abort();
        assert!(execution.await.is_err_and(|e| e.is_cancelled()));
        assert!(!parent_cancel.is_cancelled());
        // The actual claim result/reservation must remain alive until released;
        // a cancelled observer cannot guess commit failure or issue a new claim.
        release.notify_one();
        tokio::time::timeout(Duration::from_millis(500),async {loop {
            if controller.statuses().get("pending")==Some(&TaskStatus::Cancelled) {break;}
            tokio::task::yield_now().await;
        }}).await.map_err(|_|ReactError::Other(format!("caller abort lost claim/reservation owner; claim_pending={claim_pending} status={:?}",controller.statuses().get("pending"))))?;
        let snapshot = controller.load_snapshot("parent-drop-admission").await?;
        assert!(
            snapshot
                .tasks
                .first()
                .is_some_and(|t| t.execution.claim.is_none())
        );
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .len(),
            1
        );
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
        let state = service
            .request_attempt_interrupt("parent-drop-admission", "pending", &claim)
            .await;
        assert!(matches!(
            state,
            Err(RuntimeTaskAttemptInterruptError::StaleOrSettledClaim { .. })
                | Ok(super::super::RuntimeTaskAttemptInterruptReceipt {
                    disposition: RuntimeTaskAttemptInterruptDisposition::AlreadySettled { .. },
                    ..
                })
        ));
        Ok(())
    }
    #[tokio::test]
    async fn parent_abort_during_claim_response_keeps_unknown_commit_owner() -> Result<()> {
        prove_parent_abort_during_claim_or_reservation(true).await
    }
    #[tokio::test]
    async fn parent_abort_during_reservation_keeps_exact_attempt_owner() -> Result<()> {
        prove_parent_abort_during_claim_or_reservation(false).await
    }

    #[tokio::test]
    async fn driver_unwind_retains_physical_joins_and_unknown_unstarted_claim() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Pending, &[]),
            runtime_task("b", TaskStatus::Pending, &[]),
        ]));
        let release_a = Arc::new(tokio::sync::Notify::new());
        controller
            .dispatch_release
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("a".to_string(), release_a.clone());
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("a".to_string());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        controller
            .dispatch_dropped
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("a".to_string(), dropped.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release_b = Arc::new(tokio::sync::Notify::new());
        controller
            .reservation_barriers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("b".to_string(), (entered.clone(), release_b.clone()));
        controller
            .reservation_owner_panics
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("b".to_string());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let execute = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .execute("driver-unwind", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("second reservation missing".to_string()))?;
        wait_runtime_dispatches(&controller, 1).await?;
        let snapshot = controller.load_snapshot("driver-unwind").await?;
        let b = snapshot
            .tasks
            .iter()
            .find(|t| t.spec.id == "b")
            .and_then(|t| t.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("b claim missing".to_string()))?;
        release_b.notify_one();
        assert!(
            execute
                .await
                .map_err(|e| ReactError::Other(e.to_string()))?
                .is_err()
        );
        assert!(!dropped.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Running));
        assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Running));
        release_a.notify_one();
        wait_runtime_status(&controller, "a", TaskStatus::Cancelled).await?;
        tokio::time::timeout(Duration::from_secs(2), async {
            while !matches!(
                service.attempt_settlement_observation(&b.execution_id("driver-unwind", "b")),
                Some(AttemptSettlementObservation::AuthorityUnknown { .. })
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("unknown owner not published".to_string()))?;
        assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Running));
        assert!(
            controller
                .load_snapshot("driver-unwind")
                .await?
                .tasks
                .iter()
                .find(|t| t.spec.id == "b")
                .is_some_and(|t| t.execution.claim.as_ref() == Some(&b))
        );
        Ok(())
    }

    #[tokio::test]
    async fn parent_abort_before_dispatch_join_keeps_owner_until_physical_stop_and_cas()
    -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "parked",
            TaskStatus::Pending,
            &[],
        )]));
        let entered = Arc::new(tokio::sync::Barrier::new(2));
        *controller
            .dispatch_barrier
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(entered.clone());
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("parked".to_string());
        let release = Arc::new(tokio::sync::Notify::new());
        controller
            .dispatch_release
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("parked".to_string(), release.clone());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        controller
            .dispatch_dropped
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("parked".to_string(), dropped.clone());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let parent_cancel = CancellationToken::new();
        let execution = tokio::spawn({
            let service = service.clone();
            let cancel = parent_cancel.clone();
            async move { service.execute("parent-drop-prejoin", cancel).await }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.wait())
            .await
            .map_err(|_| ReactError::Other("dispatch did not enter parked fixture".to_string()))?;
        let claim = controller
            .load_snapshot("parent-drop-prejoin")
            .await?
            .tasks
            .first()
            .and_then(|t| t.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("parked claim absent".to_string()))?;
        let execution_id = claim.execution_id("parent-drop-prejoin", "parked");
        let mut rx = service
            .attempt_settlement_receiver(&execution_id)
            .ok_or_else(|| ReactError::Other("attempt watch absent".to_string()))?;
        execution.abort();
        assert!(execution.await.is_err_and(|error| error.is_cancelled()));
        assert!(!parent_cancel.is_cancelled());
        assert!(
            tokio::time::timeout(Duration::from_millis(60), rx.changed())
                .await
                .is_err()
        );
        assert!(!dropped.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            controller.statuses().get("parked"),
            Some(&TaskStatus::Running)
        );
        release.notify_one();
        let observed = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if let Some(fact) = rx.borrow_and_update().clone() {
                    break Ok::<_, ReactError>(fact);
                }
                rx.changed().await.map_err(|_| {
                    ReactError::Other("watch closed without authoritative fact".to_string())
                })?;
            }
        })
        .await
        .map_err(|_| {
            ReactError::Other(format!(
                "caller abort left prejoin watch pending; physical_stopped={} durable_status={:?}",
                dropped.load(std::sync::atomic::Ordering::Acquire),
                controller.statuses().get("parked")
            ))
        })??;
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            observed,
            AttemptSettlementObservation::Settled {
                status: "cancelled".to_string()
            }
        );
        assert_eq!(
            controller.statuses().get("parked"),
            Some(&TaskStatus::Cancelled)
        );
        assert!(
            controller
                .load_snapshot("parent-drop-prejoin")
                .await?
                .tasks
                .first()
                .is_some_and(|task| task.execution.claim.is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn detached_settlement_owner_publishes_success_or_unknown_after_parent_abort()
    -> Result<()> {
        for outcome in ["success", "panic", "commit-response-unknown"] {
            let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
                "a",
                TaskStatus::Pending,
                &[],
            )]));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            controller
                .settlement_barriers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("a".to_string(), (entered.clone(), release.clone()));
            if outcome == "panic" {
                controller
                    .settlement_owner_panics
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert("a".to_string());
            }
            let service = Arc::new(super::super::RuntimeTaskService::new(
                controller.clone(),
                RuntimeTaskServiceConfig::default(),
            ));
            let execution = tokio::spawn({
                let service = service.clone();
                async move {
                    service
                        .execute("detached-owner", CancellationToken::new())
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(2), entered.notified())
                .await
                .map_err(|_| {
                    ReactError::Other("settlement owner did not enter backend".to_string())
                })?;
            let claim = controller
                .load_snapshot("detached-owner")
                .await?
                .tasks
                .first()
                .and_then(|task| task.execution.claim.clone())
                .ok_or_else(|| ReactError::Other("joined claim missing".to_string()))?;
            let execution_id = claim.execution_id("detached-owner", "a");
            let mut receiver = service
                .attempt_settlement_receiver(&execution_id)
                .ok_or_else(|| ReactError::Other("settlement watch missing".to_string()))?;
            execution.abort();
            let result = execution.await;
            assert!(result.is_err_and(|error| error.is_cancelled()));
            if outcome == "commit-response-unknown" {
                *controller
                    .settle_response_error_once
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                let count = *controller
                    .current_check_count
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *controller
                    .current_check_error_at
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(count.saturating_add(1));
            }
            release.notify_one();
            let observed = tokio::time::timeout(Duration::from_millis(500), async {
                loop {
                    if let Some(observed) = receiver.borrow_and_update().clone() {
                        break Ok::<_, ReactError>(observed);
                    }
                    receiver.changed().await.map_err(|_| {
                        ReactError::Other("settlement watch closed without result".to_string())
                    })?;
                }
            })
            .await
            .map_err(|_| {
                ReactError::Other(format!(
                    "detached owner left unresolved settlement watch: {outcome}"
                ))
            })??;
            if outcome == "success" {
                assert_eq!(
                    observed,
                    AttemptSettlementObservation::Settled {
                        status: "completed".to_string()
                    }
                );
                assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Completed));
            } else {
                assert!(matches!(
                    observed,
                    AttemptSettlementObservation::AuthorityUnknown { .. }
                ));
                assert!(matches!(
                    service.attempt_settlement_observation(&execution_id),
                    Some(AttemptSettlementObservation::AuthorityUnknown { .. })
                ));
                if outcome == "panic" {
                    assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Running));
                    let response = tokio::time::timeout(
                        Duration::from_millis(500),
                        service.request_attempt_interrupt("detached-owner", "a", &claim),
                    )
                    .await
                    .map_err(|_| {
                        ReactError::Other(
                            "exact control kept waiting after detached owner panic".to_string(),
                        )
                    })?;
                    assert!(matches!(
                        response,
                        Err(RuntimeTaskAttemptInterruptError::DurableAuthority { .. })
                    ));
                } else {
                    // The backend really committed before losing its response;
                    // a failed ownership lookup must still report unknown.
                    assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Completed));
                }
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn slow_resolution_or_cas_does_not_drop_original_dispatch_after_observer_grace()
    -> Result<()> {
        for hold_resolution in [true, false] {
            let controller = Arc::new(ScriptedController::with_tasks(vec![
                runtime_task("a", TaskStatus::Pending, &[]),
                runtime_task("b", TaskStatus::Pending, &[]),
            ]));
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            if hold_resolution {
                controller
                    .resolution_barriers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert("a".to_string(), (entered.clone(), release.clone()));
            } else {
                controller
                    .settlement_barriers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert("a".to_string(), (entered.clone(), release.clone()));
            }
            controller
                .wait_for_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("b".to_string());
            let release_b = Arc::new(tokio::sync::Notify::new());
            controller
                .dispatch_release
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert("b".to_string(), release_b.clone());
            let dropped_b = Arc::new(std::sync::atomic::AtomicBool::new(false));
            controller
                .dispatch_dropped
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("b".to_string(), dropped_b.clone());
            let service = Arc::new(super::super::RuntimeTaskService::new(
                controller.clone(),
                RuntimeTaskServiceConfig {
                    cancellation_grace_period: Duration::from_millis(20),
                    ..RuntimeTaskServiceConfig::default()
                },
            ));
            let cancel = CancellationToken::new();
            let execution = tokio::spawn({
                let service = service.clone();
                let cancel = cancel.clone();
                async move { service.execute("slow-settlement-root", cancel).await }
            });
            tokio::time::timeout(Duration::from_secs(2), entered.notified())
                .await
                .map_err(|_| ReactError::Other("slow settlement not entered".to_string()))?;
            cancel.cancel();
            let observed = tokio::time::timeout(Duration::from_secs(2), execution)
                .await
                .map_err(|_| ReactError::Other("root observation exceeded grace".to_string()))?
                .map_err(|error| ReactError::Other(error.to_string()))?;
            assert!(matches!(
                observed,
                Err(ReactError::ExecutionNotSettled { .. })
            ));
            assert!(!dropped_b.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Running));
            release_b.notify_one();
            release.notify_one();
            wait_runtime_status(&controller, "a", TaskStatus::Completed).await?;
            wait_runtime_status(&controller, "b", TaskStatus::Cancelled).await?;
            assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Completed));
            assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Cancelled));
        }
        Ok(())
    }

    #[tokio::test]
    async fn slow_sibling_settlement_does_not_delay_exact_attempt_cas() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Pending, &[]),
            runtime_task("b", TaskStatus::Pending, &[]),
            runtime_task("c", TaskStatus::Pending, &["b"]),
        ]));
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("a".to_string());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        controller
            .settlement_barriers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert("b".to_string(), (entered.clone(), release.clone()));
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        ));
        let execution = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .execute("slow-sibling-cas", CancellationToken::new())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .map_err(|_| ReactError::Other("sibling CAS not entered".to_string()))?;
        let claim = controller
            .load_snapshot("slow-sibling-cas")
            .await?
            .tasks
            .iter()
            .find(|task| task.spec.id == "a")
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("a claim missing".to_string()))?;
        {
            let mut guard = controller
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = guard
                .as_mut()
                .ok_or_else(|| ReactError::Other("snapshot missing".to_string()))?;
            assert!(
                super::super::request_runtime_task_interruption(
                    snapshot,
                    "a",
                    &claim,
                    RuntimeInterruptionDisposition::Cancelled
                )
                .map_err(|error| ReactError::Other(error.to_string()))?
            );
        }
        assert!(
            service
                .request_attempt_interrupt("slow-sibling-cas", "a", &claim)
                .await?
                .requested
        );
        let timely = tokio::time::timeout(Duration::from_millis(500), async {
            while controller.statuses().get("a") != Some(&TaskStatus::Cancelled) {
                tokio::task::yield_now().await;
            }
        })
        .await;
        if timely.is_ok() {
            assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Running));
            assert_eq!(controller.statuses().get("c"), Some(&TaskStatus::Pending));
        }
        release.notify_one();
        let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| ReactError::Other("sibling run did not finish".to_string()))?
            .map_err(|error| ReactError::Other(format!("runtime join failed: {error}")))??;
        if timely.is_err() {
            return Err(ReactError::Other(
                "slow sibling CAS blocked exact attempt settlement".to_string(),
            ));
        }
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Completed));
        assert_eq!(controller.statuses().get("c"), Some(&TaskStatus::Completed));
        Ok(())
    }

    #[tokio::test]
    async fn exact_attempt_settles_before_unrelated_sibling_joins() -> Result<()> {
        for disposition in [
            RuntimeInterruptionDisposition::Cancelled,
            RuntimeInterruptionDisposition::Paused {
                reason: "pause a only".to_string(),
            },
        ] {
            let controller = Arc::new(ScriptedController::with_tasks(vec![
                runtime_task("a", TaskStatus::Pending, &[]),
                runtime_task("b", TaskStatus::Pending, &[]),
                runtime_task("c", TaskStatus::Pending, &["b"]),
                runtime_task("d", TaskStatus::Pending, &["a"]),
            ]));
            controller
                .wait_for_cancel
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("a".to_string());
            let release_b = Arc::new(tokio::sync::Notify::new());
            controller
                .dispatch_release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert("b".to_string(), release_b.clone());
            let service = Arc::new(super::super::RuntimeTaskService::new(
                controller.clone(),
                RuntimeTaskServiceConfig::default(),
            ));
            let run_cancel = CancellationToken::new();
            let execution = tokio::spawn({
                let service = service.clone();
                let run_cancel = run_cancel.clone();
                async move { service.execute("exact-before-sibling", run_cancel).await }
            });
            let claim = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let snapshot = controller.load_snapshot("exact-before-sibling").await?;
                    if snapshot
                        .tasks
                        .iter()
                        .filter(|task| task.execution.status == TaskStatus::Running)
                        .count()
                        == 2
                        && let Some(claim) = snapshot
                            .tasks
                            .iter()
                            .find(|task| task.spec.id == "a")
                            .and_then(|task| task.execution.claim.clone())
                    {
                        break Ok::<_, ReactError>(claim);
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| ReactError::Other("parallel claims not admitted".to_string()))??;
            {
                let mut guarded = controller
                    .snapshot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let snapshot = guarded
                    .as_mut()
                    .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
                assert!(
                    super::super::request_runtime_task_interruption(
                        snapshot,
                        "a",
                        &claim,
                        disposition.clone()
                    )
                    .map_err(|error| ReactError::Other(error.to_string()))?
                );
            }
            assert!(
                service
                    .request_attempt_interrupt("exact-before-sibling", "a", &claim)
                    .await?
                    .requested
            );
            let expected_status = match &disposition {
                RuntimeInterruptionDisposition::Cancelled => TaskStatus::Cancelled,
                RuntimeInterruptionDisposition::Paused { reason } => {
                    TaskStatus::Paused(reason.clone())
                }
            };
            let settled = tokio::time::timeout(Duration::from_millis(500), async {
                loop {
                    if controller.statuses().get("a") == Some(&expected_status) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            if settled.is_err() {
                // Release only to clean the failing red fixture; success asserts
                // the durable receipt while the sibling is still held.
                run_cancel.cancel();
                release_b.notify_one();
                let _ = tokio::time::timeout(Duration::from_secs(2), execution).await;
                return Err(ReactError::Other(
                    "exact attempt waited for unrelated sibling before durable settlement"
                        .to_string(),
                ));
            }
            assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Running));
            assert_eq!(controller.statuses().get("c"), Some(&TaskStatus::Pending));
            assert!(!execution.is_finished());
            assert!(!run_cancel.is_cancelled());
            release_b.notify_one();
            let outcome = tokio::time::timeout(Duration::from_secs(2), execution)
                .await
                .map_err(|_| ReactError::Other("independent wave did not finish".to_string()))?
                .map_err(|error| ReactError::Other(format!("runtime join failed: {error}")))??;
            assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Completed));
            assert_eq!(controller.statuses().get("c"), Some(&TaskStatus::Completed));
            assert_eq!(controller.statuses().get("d"), Some(&TaskStatus::Pending));
            assert_eq!(
                outcome,
                match disposition {
                    RuntimeInterruptionDisposition::Cancelled => RuntimeDagOutcome::Cancelled,
                    RuntimeInterruptionDisposition::Paused { reason } =>
                        RuntimeDagOutcome::Paused {
                            task_id: Some("a".to_string()),
                            reason
                        },
                }
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn recovered_root_pause_retains_exact_cancel_and_resumes_independent_branch_only()
    -> Result<()> {
        let task_a = runtime_task("a", TaskStatus::Pending, &[]);
        let task_b = runtime_task("b", TaskStatus::Pending, &[]);
        let mut snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![
                task_a.clone(),
                task_b.clone(),
                runtime_task("c", TaskStatus::Pending, &["a"]),
                runtime_task("d", TaskStatus::Pending, &["b"]),
            ],
        };
        let RuntimeTaskClaimOutcome::Claimed(claim_a) =
            super::super::claim_runtime_task(&mut snapshot, &task_a, 1)
                .map_err(|error| ReactError::Other(error.to_string()))?
        else {
            return Err(ReactError::Other("a claim rejected".to_string()));
        };
        let RuntimeTaskClaimOutcome::Claimed(_) =
            super::super::claim_runtime_task(&mut snapshot, &task_b, 1)
                .map_err(|error| ReactError::Other(error.to_string()))?
        else {
            return Err(ReactError::Other("b claim rejected".to_string()));
        };
        assert!(
            super::super::request_runtime_task_interruption(
                &mut snapshot,
                "a",
                &claim_a,
                RuntimeInterruptionDisposition::Cancelled
            )
            .map_err(|error| ReactError::Other(error.to_string()))?
        );
        // The caller has proven both physical owners stopped. Reconstruct the
        // checkpoint before invoking the run-level recovery settlement.
        let encoded =
            serde_json::to_vec(&snapshot).map_err(|error| ReactError::Other(error.to_string()))?;
        let mut restored: RuntimePlanSnapshot = serde_json::from_slice(&encoded)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        super::super::settle_runtime_interruption(
            &mut restored,
            1,
            RuntimeInterruptionDisposition::Paused {
                reason: "restore run pause".to_string(),
            },
        )
        .map_err(|error| ReactError::Other(error.to_string()))?;
        let cancelled = restored
            .tasks
            .iter()
            .find(|task| task.spec.id == "a")
            .cloned()
            .ok_or_else(|| ReactError::Other("a missing".to_string()))?;
        assert_eq!(cancelled.execution.status, TaskStatus::Cancelled);
        assert_eq!(cancelled.execution.interruption_requested, None);
        assert!(super::super::resume_runtime_task(&mut restored, &cancelled, 1).is_err());
        let paused_b = restored
            .tasks
            .iter()
            .find(|task| task.spec.id == "b")
            .cloned()
            .ok_or_else(|| ReactError::Other("b missing".to_string()))?;
        assert_eq!(
            super::super::resume_runtime_task(&mut restored, &paused_b, 1)
                .map_err(|error| ReactError::Other(error.to_string()))?,
            super::super::RuntimeTaskResumeOutcome::Resumed
        );
        let encoded =
            serde_json::to_vec(&restored).map_err(|error| ReactError::Other(error.to_string()))?;
        let resumed: RuntimePlanSnapshot = serde_json::from_slice(&encoded)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let controller = Arc::new(ScriptedController::with_tasks(resumed.tasks));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        assert_eq!(
            service
                .execute("restore-exact-cancel", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Cancelled
        );
        assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Cancelled));
        assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Completed));
        assert_eq!(controller.statuses().get("c"), Some(&TaskStatus::Pending));
        assert_eq!(controller.statuses().get("d"), Some(&TaskStatus::Completed));
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["b", "d"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn review_abandonment_receipt_projects_actual_durable_pause_instead_of_requested_failure()
    -> Result<()> {
        let expected = runtime_task("paused", TaskStatus::Pending, &[]);
        let mut snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![expected.clone()],
        };
        let RuntimeTaskClaimOutcome::Claimed(claim) =
            super::super::claim_runtime_task(&mut snapshot, &expected, 1)
                .map_err(|error| ReactError::Other(error.to_string()))?
        else {
            return Err(ReactError::Other("claim rejected".to_string()));
        };
        super::super::request_runtime_task_interruption(
            &mut snapshot,
            "paused",
            &claim,
            RuntimeInterruptionDisposition::Paused {
                reason: "durable pause".to_string(),
            },
        )
        .map_err(|error| ReactError::Other(error.to_string()))?;
        let controller = Arc::new(ScriptedController::with_tasks(snapshot.tasks));
        let executor =
            RuntimeDagExecutor::new(controller.clone(), RuntimeTaskServiceConfig::default());
        let execution_id = claim.execution_id("actual-receipt", "paused");
        let (settled_tx, settled_rx) = watch::channel(None);
        executor
            .attempt_settlements
            .lock()
            .map_err(|_| ReactError::Other("projection lock".to_string()))?
            .insert(
                execution_id,
                AttemptSettlementProjection {
                    phase: AttemptSettlementPhase::JoinedAwaitingDurableSettlement,
                    settled_tx,
                    cancel: CancellationToken::new(),
                    physical_closed: true,
                    admission: None,
                },
            );
        executor
            .settle_abandonment(
                "actual-receipt",
                &claim,
                &expected,
                RuntimeClaimAbandonment::Failed {
                    error: "dispatch infrastructure failed".to_string(),
                },
            )
            .await?;
        assert_eq!(
            controller.statuses().get("paused"),
            Some(&TaskStatus::Paused("durable pause".to_string()))
        );
        assert_eq!(
            settled_rx.borrow().as_ref(),
            Some(&AttemptSettlementObservation::Settled {
                status: "paused".to_string()
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn task_local_cancel_preserves_independent_roots_and_next_wave() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Cancelled, &[]),
            runtime_task("b", TaskStatus::Pending, &[]),
            runtime_task("c", TaskStatus::Pending, &["b"]),
            runtime_task("d", TaskStatus::Pending, &["a"]),
        ]));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        assert_eq!(
            service
                .execute("local-cancel", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Cancelled
        );
        let statuses = controller.statuses();
        assert_eq!(statuses.get("a"), Some(&TaskStatus::Cancelled));
        assert_eq!(statuses.get("b"), Some(&TaskStatus::Completed));
        assert_eq!(statuses.get("c"), Some(&TaskStatus::Completed));
        assert_eq!(statuses.get("d"), Some(&TaskStatus::Pending));
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["b", "c"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn task_local_pause_runs_independent_waves_then_resumes_without_replaying_them()
    -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![
            runtime_task("a", TaskStatus::Paused("local pause".to_string()), &[]),
            runtime_task("b", TaskStatus::Pending, &[]),
            runtime_task("c", TaskStatus::Pending, &["b"]),
            runtime_task("d", TaskStatus::Pending, &["a"]),
        ]));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        assert_eq!(
            service
                .execute("local-pause", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Paused {
                task_id: Some("a".to_string()),
                reason: "local pause".to_string(),
            }
        );
        let statuses = controller.statuses();
        assert_eq!(statuses.get("b"), Some(&TaskStatus::Completed));
        assert_eq!(statuses.get("c"), Some(&TaskStatus::Completed));
        assert_eq!(statuses.get("d"), Some(&TaskStatus::Pending));
        {
            let mut guarded = controller
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = guarded
                .as_mut()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            let expected = snapshot
                .tasks
                .iter()
                .find(|task| task.spec.id == "a")
                .cloned()
                .ok_or_else(|| ReactError::Other("missing paused task".to_string()))?;
            assert_eq!(
                super::super::resume_runtime_task(snapshot, &expected, 1)
                    .map_err(|error| ReactError::Other(error.to_string()))?,
                super::super::RuntimeTaskResumeOutcome::Resumed
            );
            assert_eq!(
                snapshot
                    .tasks
                    .iter()
                    .find(|task| task.spec.id == "a")
                    .map(|task| task.execution.retry_count),
                Some(0)
            );
        }
        assert_eq!(
            service
                .execute("local-pause", CancellationToken::new())
                .await?,
            RuntimeDagOutcome::Completed
        );
        assert_eq!(
            controller
                .order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &["b", "c", "a", "d"]
        );
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
    async fn only_explicit_run_interruption_takes_precedence_over_retained_failure() -> Result<()> {
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
            RuntimeDagOutcome::Paused {
                task_id: None,
                reason: "weaker requested pause".to_string(),
            }
        );
        let cancelled_statuses = cancelled.statuses();
        assert_eq!(
            cancelled_statuses.get("failed"),
            Some(&TaskStatus::Failed("old failure".to_string()))
        );
        assert_eq!(
            cancelled_statuses.get("pending"),
            Some(&TaskStatus::Pending)
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
                task_id: Some("failed".to_string()),
                reason: "old failure".to_string(),
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
            .wait_for_cancel
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
    async fn delayed_reservation_preserves_first_exact_request_time() -> Result<()> {
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
            .wait_for_cancel
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
        let started =
            service.interruption_started_at(&claim.execution_id("delayed-reservation", "delayed"));
        assert!(started.is_some());
        let duplicate = service
            .request_attempt_interrupt("delayed-reservation", "delayed", &claim)
            .await?;
        assert!(!duplicate.requested);
        assert_eq!(
            started,
            service.interruption_started_at(&claim.execution_id("delayed-reservation", "delayed"))
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
        assert!(
            runtime_tasks
                .interruption_started_at(&claim.execution_id("stale-interrupt", "slow"))
                .is_none()
        );
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

    async fn held_runtime_fixture(
        names: &[&str],
    ) -> Result<(
        Arc<ScriptedController>,
        Arc<super::super::RuntimeTaskService<ScriptedController>>,
        HashMap<String, Arc<tokio::sync::Notify>>,
    )> {
        let controller = Arc::new(ScriptedController::with_tasks(
            names
                .iter()
                .map(|id| runtime_task(id, TaskStatus::Pending, &[]))
                .collect(),
        ));
        let mut releases = HashMap::new();
        for id in names {
            let release = Arc::new(tokio::sync::Notify::new());
            controller
                .dispatch_release
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert((*id).to_string(), release.clone());
            controller
                .wait_for_cancel
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert((*id).to_string());
            releases.insert((*id).to_string(), release);
        }
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                max_concurrent_subagents: names.len(),
                cancellation_grace_period: Duration::from_millis(20),
                ..Default::default()
            },
        ));
        Ok((controller, service, releases))
    }

    async fn wait_runtime_dispatches(controller: &ScriptedController, count: usize) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(2), async {
            while controller
                .order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .len()
                < count
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("physical dispatch did not start".to_string()))
    }

    async fn wait_runtime_status(
        controller: &ScriptedController,
        id: &str,
        status: TaskStatus,
    ) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(2), async {
            while controller.statuses().get(id) != Some(&status) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other(format!("original physical owner did not settle {id}")))
    }

    #[tokio::test]
    async fn exact_interrupt_keeps_original_claim_past_grace_until_cleanup() -> Result<()> {
        let (controller, service, releases) = held_runtime_fixture(&["stuck"]).await?;
        let mut execution = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .execute("exact-held", CancellationToken::new())
                    .await
            }
        });
        wait_runtime_dispatches(&controller, 1).await?;
        let claim = controller
            .load_snapshot("exact-held")
            .await?
            .tasks
            .first()
            .and_then(|t| t.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("claim missing".to_string()))?;
        let first = service
            .request_attempt_interrupt("exact-held", "stuck", &claim)
            .await?;
        let stamp = service.interruption_started_at(&claim.execution_id("exact-held", "stuck"));
        assert!(first.requested);
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut execution)
                .await
                .is_err(),
            "exact A cannot expire the root observer"
        );
        let duplicate = service
            .request_attempt_interrupt("exact-held", "stuck", &claim)
            .await?;
        assert!(!duplicate.requested);
        assert_eq!(
            service.interruption_started_at(&claim.execution_id("exact-held", "stuck")),
            stamp
        );
        assert_eq!(
            controller.statuses().get("stuck"),
            Some(&TaskStatus::Running)
        );
        releases
            .get("stuck")
            .ok_or_else(|| ReactError::Other("release missing".to_string()))?
            .notify_one();
        let outcome = execution
            .await
            .map_err(|e| ReactError::Other(e.to_string()))??;
        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
        assert!(
            controller
                .load_snapshot("exact-held")
                .await?
                .tasks
                .first()
                .is_some_and(|t| t.execution.claim.is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_exact_interrupts_retain_and_settle_their_own_claims() -> Result<()> {
        let (controller, service, releases) = held_runtime_fixture(&["left", "right"]).await?;
        let mut execution = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .execute("concurrent-held", CancellationToken::new())
                    .await
            }
        });
        wait_runtime_dispatches(&controller, 2).await?;
        let snapshot = controller.load_snapshot("concurrent-held").await?;
        for task in &snapshot.tasks {
            let claim = task
                .execution
                .claim
                .as_ref()
                .ok_or_else(|| ReactError::Other("claim missing".to_string()))?;
            assert!(
                service
                    .request_attempt_interrupt("concurrent-held", &task.spec.id, claim)
                    .await?
                    .requested
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut execution)
                .await
                .is_err()
        );
        releases
            .get("left")
            .ok_or_else(|| ReactError::Other("left release missing".to_string()))?
            .notify_one();
        wait_runtime_status(&controller, "left", TaskStatus::Cancelled).await?;
        assert_eq!(
            controller.statuses().get("right"),
            Some(&TaskStatus::Running)
        );
        assert!(!execution.is_finished());
        releases
            .get("right")
            .ok_or_else(|| ReactError::Other("right release missing".to_string()))?
            .notify_one();
        assert_eq!(
            execution
                .await
                .map_err(|e| ReactError::Other(e.to_string()))??,
            RuntimeDagOutcome::Cancelled
        );
        assert_eq!(
            controller.statuses().get("right"),
            Some(&TaskStatus::Cancelled)
        );
        Ok(())
    }

    #[tokio::test]
    async fn root_and_exact_observations_converge_only_after_original_cleanup() -> Result<()> {
        let (controller, service, releases) = held_runtime_fixture(&["exact", "root"]).await?;
        let cancel = CancellationToken::new();
        let execution = tokio::spawn({
            let service = service.clone();
            let cancel = cancel.clone();
            async move { service.execute("root-exact", cancel).await }
        });
        wait_runtime_dispatches(&controller, 2).await?;
        let claim = controller
            .load_snapshot("root-exact")
            .await?
            .tasks
            .iter()
            .find(|t| t.spec.id == "exact")
            .and_then(|t| t.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("exact claim missing".to_string()))?;
        assert!(
            service
                .request_attempt_interrupt("root-exact", "exact", &claim)
                .await?
                .requested
        );
        cancel.cancel();
        let observed = execution
            .await
            .map_err(|e| ReactError::Other(e.to_string()))?;
        assert!(
            matches!(observed,Err(ReactError::ExecutionNotSettled{run_id,execution_id:None})if run_id=="root-exact")
        );
        let snapshot = controller.load_snapshot("root-exact").await?;
        assert!(
            snapshot
                .tasks
                .iter()
                .all(|t| t.execution.status == TaskStatus::Running && t.execution.claim.is_some())
        );
        for release in releases.values() {
            release.notify_one();
        }
        wait_runtime_status(&controller, "exact", TaskStatus::Cancelled).await?;
        wait_runtime_status(&controller, "root", TaskStatus::Cancelled).await?;
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
    async fn unknown_dispatch_resolution_and_join_keep_typed_identity_claim_and_original_capacity()
    -> Result<()> {
        for stage in ["dispatch", "resolution", "settlement_panic"] {
            let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
                "unknown",
                TaskStatus::Pending,
                &[],
            )]));
            match stage {
                "dispatch" => {
                    controller
                        .dispatch_observation
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert("unknown".to_string());
                }
                "resolution" => {
                    controller
                        .resolve_observation
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert("unknown".to_string());
                }
                _ => {
                    controller
                        .settlement_owner_panics
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert("unknown".to_string());
                }
            }
            let admission = Arc::new(ExecutionAdmission::with_capacity(1));
            let service = super::super::RuntimeTaskService::new(
                controller.clone(),
                RuntimeTaskServiceConfig {
                    shared_admission: Some(admission.clone()),
                    ..Default::default()
                },
            );
            let observed = service.execute(stage, CancellationToken::new()).await;
            let snapshot = controller.load_snapshot(stage).await?;
            let claim = snapshot
                .tasks
                .first()
                .and_then(|t| t.execution.claim.as_ref())
                .ok_or_else(|| ReactError::Other("unknown owner claim released".to_string()))?;
            let execution_id = claim.execution_id(stage, "unknown");
            assert!(
                matches!(observed,Err(ReactError::ExecutionNotSettled{run_id,execution_id:Some(actual)}) if run_id==stage&&actual==execution_id),
                "unknown exit must not collapse to Other/Failed"
            );
            assert_eq!(
                controller.statuses().get("unknown"),
                Some(&TaskStatus::Running)
            );
            assert_eq!(
                admission.active_count(),
                1,
                "original unknown owner must retain the original admission lease"
            );
            assert!(admission.issue("must-not-enter").is_err());
            assert!(matches!(
                service.attempt_settlement_observation(&execution_id),
                Some(AttemptSettlementObservation::AuthorityUnknown { .. })
            ));
            drop(service);
            assert_eq!(
                admission.active_count(),
                1,
                "dropping the observer/service cannot release unknown producer capacity"
            );
            assert!(admission.issue("still-must-not-enter").is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejected_exact_request_does_not_start_observation_clock_or_exceed_pending_capacity()
    -> Result<()> {
        let tasks = (0..257)
            .map(|index| runtime_task(&format!("queued-{index}"), TaskStatus::Pending, &[]))
            .collect::<Vec<_>>();
        let controller = Arc::new(ScriptedController::with_tasks(tasks.clone()));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        for (index, task) in tasks.iter().enumerate() {
            let RuntimeTaskClaimOutcome::Claimed(claim) =
                controller.claim_task("pending-capacity", task, 1).await?
            else {
                return Err(ReactError::Other("claim rejected".to_string()));
            };
            let result = service
                .request_attempt_interrupt("pending-capacity", &task.spec.id, &claim)
                .await;
            let started = service
                .interruption_started_at(&claim.execution_id("pending-capacity", &task.spec.id));
            if index == 256 {
                assert!(matches!(
                    result,
                    Err(
                        super::super::RuntimeTaskAttemptInterruptError::PendingCapacityExceeded {
                            limit: 256
                        }
                    )
                ));
                assert!(
                    started.is_none(),
                    "a capacity rejection is not an admitted first request"
                );
            } else {
                assert!(result?.requested);
                assert!(started.is_some());
            }
        }
        assert_eq!(service.pending_attempt_interrupt_count(), 256);
        Ok(())
    }

    #[tokio::test]
    async fn normal_execution_longer_than_grace_has_no_observation_deadline() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "normal",
            TaskStatus::Pending,
            &[],
        )]));
        let release = Arc::new(tokio::sync::Notify::new());
        controller
            .dispatch_release
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("normal".to_string(), release.clone());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..Default::default()
            },
        ));
        let mut execution = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .execute("normal-long", CancellationToken::new())
                    .await
            }
        });
        wait_runtime_dispatches(&controller, 1).await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut execution)
                .await
                .is_err()
        );
        assert_eq!(
            controller.statuses().get("normal"),
            Some(&TaskStatus::Running)
        );
        release.notify_one();
        assert_eq!(
            execution
                .await
                .map_err(|e| ReactError::Other(e.to_string()))??,
            RuntimeDagOutcome::Completed
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_held_attempt_does_not_expire_or_stop_its_independent_sibling() -> Result<()> {
        let (controller, service, releases) = held_runtime_fixture(&["a", "b"]).await?;
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove("b");
        let cancel = CancellationToken::new();
        let mut execution = tokio::spawn({
            let service = service.clone();
            let cancel = cancel.clone();
            async move { service.execute("exact-sibling", cancel).await }
        });
        wait_runtime_dispatches(&controller, 2).await?;
        let claim = controller
            .load_snapshot("exact-sibling")
            .await?
            .tasks
            .iter()
            .find(|t| t.spec.id == "a")
            .and_then(|t| t.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("a claim missing".to_string()))?;
        assert!(
            service
                .request_attempt_interrupt("exact-sibling", "a", &claim)
                .await?
                .requested
        );
        releases
            .get("b")
            .ok_or_else(|| ReactError::Other("b release missing".to_string()))?
            .notify_one();
        wait_runtime_status(&controller, "b", TaskStatus::Completed).await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(60), &mut execution)
                .await
                .is_err()
        );
        assert!(!cancel.is_cancelled());
        assert_eq!(controller.statuses().get("a"), Some(&TaskStatus::Running));
        releases
            .get("a")
            .ok_or_else(|| ReactError::Other("a release missing".to_string()))?
            .notify_one();
        assert_eq!(
            execution
                .await
                .map_err(|e| ReactError::Other(e.to_string()))??,
            RuntimeDagOutcome::Cancelled
        );
        assert_eq!(controller.statuses().get("b"), Some(&TaskStatus::Completed));
        Ok(())
    }

    #[tokio::test]
    async fn restart_cancel_without_physical_receipt_never_clears_original_claim() -> Result<()> {
        let task = runtime_task("restored", TaskStatus::Pending, &[]);
        let mut snapshot = RuntimePlanSnapshot {
            revision: 1,
            tasks: vec![task.clone()],
        };
        let RuntimeTaskClaimOutcome::Claimed(claim) =
            super::super::claim_runtime_task(&mut snapshot, &task, 1)
                .map_err(|e| ReactError::Other(e.to_string()))?
        else {
            return Err(ReactError::Other("claim rejected".to_string()));
        };
        let controller = Arc::new(ScriptedController::with_tasks(snapshot.tasks.clone()));
        let service = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            service.execute("restart-unknown", cancel).await,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        let after = controller.load_snapshot("restart-unknown").await?;
        assert_eq!(
            after.tasks.first().and_then(|t| t.execution.claim.as_ref()),
            Some(&claim)
        );
        assert_eq!(
            controller.statuses().get("restored"),
            Some(&TaskStatus::Running)
        );
        assert!(
            controller
                .order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn observation_grace_keeps_original_dispatch_and_claim_until_physical_release()
    -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "held",
            TaskStatus::Pending,
            &[],
        )]));
        let release = Arc::new(tokio::sync::Notify::new());
        controller
            .dispatch_release
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("held".to_string(), release.clone());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        controller
            .dispatch_dropped
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("held".to_string(), dropped.clone());
        controller
            .wait_for_cancel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert("held".to_string());
        let service = Arc::new(super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig {
                cancellation_grace_period: Duration::from_millis(20),
                ..RuntimeTaskServiceConfig::default()
            },
        ));
        let cancel = CancellationToken::new();
        let observer = tokio::spawn({
            let service = service.clone();
            let cancel = cancel.clone();
            async move { service.execute("observation-held", cancel).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while controller
                .order
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("dispatch never entered".to_string()))?;
        let before = controller.load_snapshot("observation-held").await?;
        let original = before
            .tasks
            .first()
            .and_then(|task| task.execution.claim.clone())
            .ok_or_else(|| ReactError::Other("original claim missing".to_string()))?;
        cancel.cancel();
        let observed = tokio::time::timeout(Duration::from_secs(1), observer)
            .await
            .map_err(|_| ReactError::Other("cancellation observer exceeded its bound".to_string()))?
            .map_err(|error| ReactError::Other(format!("observer join: {error}")))?;
        // Record all facts and release the real owner before asserting. A red
        // run cannot strand this test's producer beyond the runtime lifetime.
        let after = controller.load_snapshot("observation-held").await?;
        let still_running = after.tasks.first().is_some_and(|task| {
            task.execution.status == TaskStatus::Running
                && task.execution.claim.as_ref() == Some(&original)
        });
        let request_retained = after.tasks.first().is_some_and(|task| {
            task.execution.interruption_requested == Some(RuntimeInterruptionDisposition::Cancelled)
        });
        let physically_dropped = dropped.load(std::sync::atomic::Ordering::SeqCst);
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !dropped.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| ReactError::Other("original producer did not close".to_string()))?;
        eprintln!(
            "OBSERVATION_HELD original_claim_retained={still_running} physical_drop_before_release={physically_dropped} observed={observed:?}"
        );
        assert!(
            still_running,
            "observer expiry cannot settle the held physical claim"
        );
        assert!(
            request_retained,
            "root cancellation must persist its request while cleanup is held"
        );
        assert!(
            !physically_dropped,
            "grace cannot destroy the original cleanup future"
        );
        assert!(
            observed.is_err(),
            "observation expiry is not a Cancelled terminal receipt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn root_cancellation_retains_noncooperative_claim_until_release() -> Result<()> {
        let (controller, service, releases) = held_runtime_fixture(&["stuck"]).await?;
        let cancel = CancellationToken::new();
        let execution = tokio::spawn({
            let service = service.clone();
            let cancel = cancel.clone();
            async move { service.execute("root-held", cancel).await }
        });
        wait_runtime_dispatches(&controller, 1).await?;
        cancel.cancel();
        let observed = execution
            .await
            .map_err(|e| ReactError::Other(e.to_string()))?;
        assert!(matches!(
            observed,
            Err(ReactError::ExecutionNotSettled { .. })
        ));
        assert_eq!(
            controller.statuses().get("stuck"),
            Some(&TaskStatus::Running)
        );
        releases
            .get("stuck")
            .ok_or_else(|| ReactError::Other("release missing".to_string()))?
            .notify_one();
        wait_runtime_status(&controller, "stuck", TaskStatus::Cancelled).await?;
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
