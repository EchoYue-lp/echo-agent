//! Store- and policy-driven DAG execution for dynamic Agent plans.
//!
//! The framework owns dependency traversal, revision safe points, bounded
//! Subagent waves, cancellation, failure propagation, and stall detection.
//! Applications provide persistence, dispatch, review, and product policy
//! through [`RuntimeDagController`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use echo_core::agent::ExecutionAdmission;
use echo_core::error::{ReactError, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::runtime::{
    DagDependencyState, DagExecutionState, NestedDelegationPolicy, RuntimeInterruptionDisposition,
    Task, TaskClaim, TaskId, TaskStatus, TaskSubagentContext,
};
use super::runtime_service::{
    RuntimeInterruptionSettlementOutcome, RuntimeTaskSettlementOutcome,
    validate_runtime_snapshot_claims,
};
use crate::planning::PlanValidator;

/// One coherent plan revision loaded from the runtime authority.
#[derive(Debug, Clone)]
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
        claim: TaskClaim,
        task: Task,
    ) -> Result<Self::DispatchOutput>;

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
#[derive(Debug, Clone)]
pub struct RuntimeTaskServiceConfig {
    pub max_concurrent_subagents: usize,
    pub external_progress_poll_interval: Duration,
    /// Maximum time to let cancellation-aware Subagents finish their durable
    /// terminal writes before remaining non-cooperative dispatches are aborted.
    pub cancellation_grace_period: Duration,
    pub delegation_policy: NestedDelegationPolicy,
    /// Optional process-wide admission shared with subagent dispatchers.
    pub shared_admission: Option<Arc<ExecutionAdmission>>,
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
        }
    }
}

/// The framework's executor for revisioned dynamic Agent plans.
pub(crate) struct RuntimeDagExecutor<C: RuntimeDagController> {
    controller: Arc<C>,
    config: RuntimeTaskServiceConfig,
    validator: PlanValidator,
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
        }
    }

    /// Override structural limits while retaining the canonical validator.
    pub(crate) fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
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
                let task_cancel = cancel.clone();
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
                outstanding_claims.insert(claim_id.clone(), (task.clone(), claim.clone()));
                let dispatch_run_id = run_id.to_string();
                let delegation_policy = self.config.delegation_policy;
                let waived_dependency_ids = match dependency_states.get(&task.spec.id) {
                    Some(DagDependencyState::Satisfied {
                        waived_dependency_ids,
                    }) => waived_dependency_ids.clone(),
                    _ => Vec::new(),
                };
                join_set.spawn(async move {
                    let dispatch = if let Some(admission) = shared_admission {
                        let lease = tokio::select! {
                            _ = task_cancel.cancelled() => {
                                Err(ReactError::Agent(Box::new(echo_core::error::AgentError::Cancelled("cancelled while waiting for shared execution admission".to_string()))))
                            }
                            lease = admission.issue_wait(format!("runtime:{dispatch_run_id}:{claim_id}")) => lease
                                .map_err(|error| ReactError::Other(format!("shared execution admission rejected task: {error}"))),
                        };
                        match lease {
                            Ok(lease) => {
                                let context = TaskSubagentContext::new(dispatch_run_id)
                                    .with_cancel(task_cancel)
                                    .with_delegation_policy(delegation_policy)
                                    .with_waived_dependencies(waived_dependency_ids);
                                let result = controller.dispatch_task(context, claim, task).await;
                                drop(lease);
                                result
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        match semaphore.acquire_owned().await {
                            Ok(permit) => {
                                let context = TaskSubagentContext::new(dispatch_run_id)
                                    .with_cancel(task_cancel)
                                    .with_delegation_policy(delegation_policy)
                                    .with_waived_dependencies(waived_dependency_ids);
                                let result = controller.dispatch_task(context, claim, task).await;
                                drop(permit);
                                result
                            }
                            Err(error) => Err(ReactError::Other(format!(
                                "Subagent semaphore closed: {error}"
                            ))),
                        }
                    };
                    (claim_id, dispatch)
                });
            }

            let mut wave_results = Vec::new();
            let mut cancellation_observed = false;
            let cancellation_grace = tokio::time::sleep(Duration::ZERO);
            tokio::pin!(cancellation_grace);
            while !join_set.is_empty() {
                tokio::select! {
                    biased;
                    joined = join_set.join_next() => {
                        match joined {
                            Some(Ok(result)) => wave_results.push((
                                result,
                                !cancellation_observed && !cancel.is_cancelled(),
                            )),
                            Some(Err(error)) => {
                                wave_errors.push(format!(
                                    "Subagent dispatch task failed to join: {error}"
                                ));
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
                        while let Some(joined) = join_set.join_next().await {
                            match joined {
                                Ok(result) => wave_results.push((result, false)),
                                Err(error) if error.is_cancelled() => {}
                                Err(error) => {
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
                    if let Err(error) = self
                        .settle_abandonment(
                            run_id,
                            &claim,
                            &task,
                            RuntimeClaimAbandonment::Interrupted { disposition },
                        )
                        .await
                    {
                        wave_errors.push(error.to_string());
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
                        if let Err(abandon_error) = self
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
                            wave_errors.push(abandon_error.to_string());
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
                        if let Err(abandon_error) = self
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
                            wave_errors.push(abandon_error.to_string());
                        }
                        wave_errors.push(message);
                        continue;
                    }
                };
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
                if let Err(error) = self
                    .settle_abandonment(run_id, &claim, &task, abandonment)
                    .await
                {
                    wave_errors.push(error.to_string());
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
        Ok(settlement)
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
    use crate::tasks::TaskStatus;

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
            super::super::runtime_service::claim_runtime_task(snapshot, task, expected_revision)
                .map_err(|error| ReactError::Other(error.to_string()))
        }

        async fn claim_is_current(
            &self,
            _run_id: &str,
            task_id: &str,
            claim: &TaskClaim,
        ) -> Result<bool> {
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

        async fn dispatch_task(
            &self,
            context: TaskSubagentContext,
            _claim: TaskClaim,
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
                context.cancel.cancelled().await;
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
            Ok(resolution)
        }

        async fn abandon_claim(
            &self,
            _run_id: &str,
            claim: &TaskClaim,
            task: &Task,
            abandonment: RuntimeClaimAbandonment,
        ) -> Result<RuntimeTaskSettlementOutcome> {
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
        let runtime_tasks = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
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
        let runtime_tasks = super::super::RuntimeTaskService::new(
            controller.clone(),
            RuntimeTaskServiceConfig::default(),
        );
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
