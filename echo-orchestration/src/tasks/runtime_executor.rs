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
use echo_core::error::{ReactError, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::runtime::{
    DagExecutionState, NestedDelegationPolicy, Task, TaskClaim, TaskId, TaskStatus,
    TaskSubagentContext,
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

/// Application resolution of one completed dispatch attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskResolution {
    Completed,
    Pending,
    Skipped,
    Failed {
        error: String,
    },
    Blocked {
        error: String,
        disposition: RuntimeStopDisposition,
    },
    Cancelled,
    /// The dispatch completed after its durable claim was replaced or cleared.
    Superseded,
}

/// Result of atomically claiming a task from one loaded plan revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTaskClaimOutcome {
    Claimed(TaskClaim),
    /// The revision, status, or specification changed. Reload the snapshot;
    /// this is optimistic-concurrency control, not a task failure.
    ReloadSnapshot,
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
        failed_task_id: TaskId,
        error: String,
    },
    Cancelled,
}

/// Persistence, dispatch, and product-policy adapter for [`RuntimeDagExecutor`].
///
/// `resolve_dispatch` owns application-specific review and persistence. It must
/// commit the returned task state before completing so the next safe-point
/// snapshot remains authoritative.
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
    ) -> Result<RuntimeTaskResolution>;

    async fn block_task(&self, run_id: &str, task: &Task, reason: &str) -> Result<()>;

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

    async fn interruption_outcome(&self, _run_id: &str) -> Result<RuntimeDagOutcome> {
        Ok(RuntimeDagOutcome::Cancelled)
    }

    async fn note_stalled(&self, _run_id: &str, _reason: &str) -> Result<()> {
        Ok(())
    }
}

/// Configuration for the dynamic runtime DAG executor.
#[derive(Debug, Clone)]
pub struct RuntimeDagExecutorConfig {
    pub max_concurrent_subagents: usize,
    pub external_progress_poll_interval: Duration,
    pub delegation_policy: NestedDelegationPolicy,
}

impl Default for RuntimeDagExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_subagents: 4,
            external_progress_poll_interval: Duration::from_millis(250),
            delegation_policy: NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 0,
                max_delegate_depth: 2,
            },
        }
    }
}

/// The framework's executor for revisioned dynamic Agent plans.
pub struct RuntimeDagExecutor<C: RuntimeDagController> {
    controller: Arc<C>,
    config: RuntimeDagExecutorConfig,
    validator: PlanValidator,
}

impl<C: RuntimeDagController> RuntimeDagExecutor<C> {
    pub fn new(controller: Arc<C>, config: RuntimeDagExecutorConfig) -> Self {
        Self {
            controller,
            config,
            validator: PlanValidator::default(),
        }
    }

    /// Override structural limits while retaining the canonical validator.
    pub fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
    }

    pub async fn execute(
        &self,
        run_id: &str,
        cancel: CancellationToken,
    ) -> Result<RuntimeDagOutcome> {
        let subagent_semaphore =
            Arc::new(Semaphore::new(self.config.max_concurrent_subagents.max(1)));
        let mut active_revision: Option<u64> = None;
        let mut failure_errors: HashMap<TaskId, String> = HashMap::new();

        loop {
            if cancel.is_cancelled() {
                return self.controller.interruption_outcome(run_id).await;
            }

            // Every loop boundary is a safe point: all locally-dispatched
            // handles from the previous wave have been joined and resolved.
            let snapshot = self.controller.load_snapshot(run_id).await?;
            if let Err(errors) = self.validator.validate_task_snapshot(&snapshot.tasks) {
                return Err(ReactError::Other(format!(
                    "invalid runtime plan snapshot: {}",
                    errors.join("; ")
                )));
            }
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

            if let Some(failed_task) = tasks
                .iter()
                .find(|task| state.failed.contains(&task.spec.id))
            {
                for blocked_id in state.blocked_by_failures(&tasks) {
                    if let Some(blocked_task) = tasks.iter().find(|task| task.spec.id == blocked_id)
                    {
                        self.controller
                            .block_task(run_id, blocked_task, "blocked: upstream task failed")
                            .await?;
                    }
                }

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

            if !state.cancelled.is_empty() {
                return self.controller.interruption_outcome(run_id).await;
            }

            if state.all_completed(&tasks) {
                return Ok(RuntimeDagOutcome::Completed);
            }

            let ready_task_ids = state.ready_task_ids(&tasks);
            if ready_task_ids.is_empty() {
                if !state.in_flight.is_empty() {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            return self.controller.interruption_outcome(run_id).await;
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
                return Ok(RuntimeDagOutcome::Failed {
                    failed_task_id: "<none>".to_string(),
                    error: reason.to_string(),
                });
            }

            let selected_ids = self
                .controller
                .select_ready_wave(&tasks, ready_task_ids.clone());
            let selected_ids = validate_selected_wave(&ready_task_ids, selected_ids)?;
            let selected_set: HashSet<&str> = selected_ids.iter().map(String::as_str).collect();
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
            for task in selected_tasks {
                let controller = self.controller.clone();
                let semaphore = subagent_semaphore.clone();
                let task_cancel = cancel.clone();
                let expected_revision = snapshot.revision;
                let context = TaskSubagentContext::new(run_id.to_string())
                    .with_cancel(task_cancel)
                    .with_delegation_policy(self.config.delegation_policy);
                join_set.spawn(async move {
                    let permit = semaphore.acquire_owned().await.map_err(|error| {
                        ReactError::Other(format!("Subagent semaphore closed: {error}"))
                    })?;
                    let claim = match controller
                        .claim_task(&context.run_id, &task, expected_revision)
                        .await?
                    {
                        RuntimeTaskClaimOutcome::Claimed(claim) => claim,
                        RuntimeTaskClaimOutcome::ReloadSnapshot => return Ok(None),
                    };
                    let task_for_dispatch = task.clone();
                    let result = controller
                        .dispatch_task(context, claim.clone(), task_for_dispatch)
                        .await;
                    drop(permit);
                    Ok::<_, ReactError>(Some((task, claim, result)))
                });
            }

            let mut wave_results = Vec::new();
            while !join_set.is_empty() {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        join_set.abort_all();
                        while join_set.join_next().await.is_some() {}
                        return self.controller.interruption_outcome(run_id).await;
                    }
                    joined = join_set.join_next() => {
                        match joined {
                            Some(Ok(Ok(Some(result)))) => wave_results.push(result),
                            Some(Ok(Ok(None))) => {}
                            Some(Ok(Err(error))) => {
                                return Err(error);
                            }
                            Some(Err(error)) => {
                                return Err(ReactError::Other(format!(
                                    "Subagent dispatch task failed to join: {error}"
                                )));
                            }
                            None => {}
                        }
                    }
                }
            }

            let mut pending_outcome: Option<RuntimeDagOutcome> = None;
            for (task, claim, dispatch) in wave_results {
                let resolution = self
                    .controller
                    .resolve_dispatch(run_id, claim, task.clone(), dispatch)
                    .await?;
                match resolution {
                    RuntimeTaskResolution::Completed
                    | RuntimeTaskResolution::Pending
                    | RuntimeTaskResolution::Skipped
                    | RuntimeTaskResolution::Superseded => {}
                    RuntimeTaskResolution::Failed { error } => {
                        failure_errors.entry(task.spec.id).or_insert(error);
                    }
                    RuntimeTaskResolution::Blocked { error, disposition } => {
                        if pending_outcome.is_none() {
                            pending_outcome = Some(stop_outcome(disposition, task.spec.id, error));
                        }
                    }
                    RuntimeTaskResolution::Cancelled => {
                        if pending_outcome.is_none() {
                            pending_outcome = Some(RuntimeDagOutcome::Cancelled);
                        }
                    }
                }
            }

            // Resolve the whole wave before honoring a stop request so completed
            // siblings are never replayed after resume.
            if let Some(outcome) = pending_outcome {
                return Ok(outcome);
            }
        }
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
            failed_task_id,
            error,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::tasks::{TaskKind, TaskStatus};

    #[derive(Default)]
    struct ScriptedController {
        snapshot: Mutex<Option<RuntimePlanSnapshot>>,
        order: Mutex<Vec<TaskId>>,
        fail: Mutex<HashMap<TaskId, String>>,
        insert_after: Mutex<Option<TaskId>>,
        reload_claim_once: Mutex<bool>,
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
            if snapshot.revision != expected_revision {
                return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
            }
            let Some(current) = snapshot
                .tasks
                .iter_mut()
                .find(|current| current.spec.id == task.spec.id)
            else {
                return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
            };
            if current.spec != task.spec || current.execution.status != TaskStatus::Pending {
                return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
            }
            let claim = TaskClaim {
                revision: expected_revision,
                attempt: current.execution.retry_count.saturating_add(1),
                spec_hash: current.spec.stable_hash().map_err(ReactError::Other)?,
            };
            current.execution.status = TaskStatus::Running;
            current.execution.claim = Some(claim.clone());
            Ok(RuntimeTaskClaimOutcome::Claimed(claim))
        }

        async fn dispatch_task(
            &self,
            _context: TaskSubagentContext,
            _claim: TaskClaim,
            task: Task,
        ) -> Result<Self::DispatchOutput> {
            self.order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(task.spec.id.clone());
            let failure = self
                .fail
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&task.spec.id)
                .cloned();
            match failure {
                Some(error) => Err(ReactError::Other(error)),
                None => Ok(task.spec.id),
            }
        }

        async fn resolve_dispatch(
            &self,
            _run_id: &str,
            claim: TaskClaim,
            task: Task,
            dispatch: Result<Self::DispatchOutput>,
        ) -> Result<RuntimeTaskResolution> {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let snapshot = snapshot
                .as_mut()
                .ok_or_else(|| ReactError::Other("missing snapshot".to_string()))?;
            if let Some(current) = snapshot
                .tasks
                .iter_mut()
                .find(|current| current.spec.id == task.spec.id)
            {
                if current.execution.claim.as_ref() != Some(&claim)
                    || current.execution.status != TaskStatus::Running
                {
                    return Ok(RuntimeTaskResolution::Superseded);
                }
                current.execution.status = match &dispatch {
                    Ok(_) => TaskStatus::Completed,
                    Err(error) => TaskStatus::Failed(error.to_string()),
                };
            }

            let insert_after = self
                .insert_after
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if insert_after.as_deref() == Some(task.spec.id.as_str()) {
                snapshot.revision = snapshot.revision.saturating_add(1);
                snapshot.tasks.push(runtime_task(
                    "inserted",
                    TaskStatus::Pending,
                    &[task.spec.id.as_str()],
                ));
            }

            Ok(match dispatch {
                Ok(_) => RuntimeTaskResolution::Completed,
                Err(error) => RuntimeTaskResolution::Failed {
                    error: error.to_string(),
                },
            })
        }

        async fn block_task(&self, _run_id: &str, task: &Task, reason: &str) -> Result<()> {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(current) = snapshot.as_mut().and_then(|snapshot| {
                snapshot
                    .tasks
                    .iter_mut()
                    .find(|item| item.spec.id == task.spec.id)
            }) {
                current.execution.status = TaskStatus::Blocked(reason.to_string());
            }
            Ok(())
        }
    }

    fn runtime_task(id: &str, status: TaskStatus, dependencies: &[&str]) -> Task {
        Task {
            spec: crate::tasks::TaskSpec {
                id: id.to_string(),
                title: id.to_string(),
                description: format!("execute {id}"),
                kind: TaskKind::Investigation,
                agent_role: "explorer".to_string(),
                depends_on: dependencies
                    .iter()
                    .map(|dependency| dependency.to_string())
                    .collect(),
                files: Vec::new(),
                allowed_tools: Vec::new(),
                required_artifacts: Vec::new(),
                execution_checks: Vec::new(),
                acceptance_criteria: Vec::new(),
                max_retries: 1,
                metadata: serde_json::Value::Null,
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
            RuntimeDagExecutor::new(controller.clone(), RuntimeDagExecutorConfig::default());

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
        let executor = RuntimeDagExecutor::new(controller, RuntimeDagExecutorConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn executor_treats_cancelled_snapshot_as_interrupted() -> Result<()> {
        let controller = Arc::new(ScriptedController::with_tasks(vec![runtime_task(
            "cancelled",
            TaskStatus::Cancelled,
            &[],
        )]));
        let executor = RuntimeDagExecutor::new(controller, RuntimeDagExecutorConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(outcome, RuntimeDagOutcome::Cancelled);
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
            RuntimeDagExecutor::new(controller.clone(), RuntimeDagExecutorConfig::default());

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
            let executor = RuntimeDagExecutor::new(controller, RuntimeDagExecutorConfig::default());

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
    async fn executor_blocks_downstream_and_fails_exhausted_graph() -> Result<()> {
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
            RuntimeDagExecutor::new(controller.clone(), RuntimeDagExecutorConfig::default());

        let outcome = executor.execute("run", CancellationToken::new()).await?;

        assert_eq!(
            outcome,
            RuntimeDagOutcome::Failed {
                failed_task_id: "a".to_string(),
                error: "boom".to_string(),
            }
        );
        assert_eq!(
            controller.statuses().get("b"),
            Some(&TaskStatus::Blocked(
                "blocked: upstream task failed".to_string()
            ))
        );
        Ok(())
    }
}
