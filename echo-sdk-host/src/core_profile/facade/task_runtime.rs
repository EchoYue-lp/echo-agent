//! Task execution runtime for the facade (plan 07, todo 3).
//!
//! [`FacadeTaskController`] implements the framework's
//! [`RuntimeDagController`] over the Session's own concrete task store and
//! [`SubagentExecutor`]: snapshots, claims, retries, interruptions and
//! terminals all settle through the canonical store mutations — the Host
//! never recomputes a frontier, claim or terminal. Dispatch sends each
//! claimed PlanTask to the subagent named by the task extension
//! (`{"subagent": "<name>"}`); the dispatch prompt carries completed
//! dependency outputs like the team runtime does.
//!
//! `task/execute` drives the graph through
//! [`echo_agent::tasks::RuntimeTaskService`] on the official connection
//! task and answers with the TaskRun handle immediately; terminals are
//! observed through `task/list`. `task/control` settles one exact task
//! claim through the same store: pause keeps the task resumable, cancel is
//! terminal, resume clears a pause without consuming retry budget.

use agent_client_protocol::{Client, ConnectionTo, Responder};
use echo_agent::agent::subagent::{
    DispatchRequest, SubagentExecutor, SubagentResult, SubagentStatus,
};
use echo_agent::error::{ReactError, Result};
use echo_agent::tasks::{
    InMemoryRevisionedTaskStore, RevisionedTaskStore as _, RuntimeClaimAbandonment,
    RuntimeDagController, RuntimeInterruptionDisposition, RuntimeInterruptionSettlementOutcome,
    RuntimeTaskService, RuntimeTaskServiceConfig, Task, TaskClaim, TaskStatus, TaskSubagentContext,
};
use echo_agent::tasks::{
    RuntimeTaskClaimOutcome, RuntimeTaskResolution, RuntimeTaskResolutionRequest,
    RuntimeTaskSettlementOutcome,
};
use echo_sdk_protocol::capability::ExtensionCapability;
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::HandleKind;
use echo_sdk_protocol::methods::{
    ControlAction, TaskControlRequest, TaskControlResponse, TaskExecuteRequest,
    TaskExecuteResponse, WireTaskStatus,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::handler::{require_capability, require_extended};
use super::super::state::CoreProfileState;
use super::super::wire;
use super::task::session_services;
use crate::factory::SessionAuthorityServices;

/// Per-run stop policy, flipped by RPC before the cancel token fires.
#[derive(Default)]
struct InterruptionChoice {
    pause: AtomicBool,
}

/// The facade DAG controller. One instance per live TaskRun execution.
pub(crate) struct FacadeTaskController {
    store: Arc<InMemoryRevisionedTaskStore>,
    executor: Arc<SubagentExecutor>,
    /// Completed dependency outputs keyed by task id (prompt enrichment).
    outputs: tokio::sync::Mutex<HashMap<String, SubagentResult>>,
    /// Staged outputs of in-flight claims keyed by claim id.
    staged: tokio::sync::Mutex<HashMap<String, SubagentResult>>,
    interruption: std::sync::Mutex<HashMap<String, Arc<InterruptionChoice>>>,
}

impl FacadeTaskController {
    pub fn new(store: Arc<InMemoryRevisionedTaskStore>, executor: Arc<SubagentExecutor>) -> Self {
        Self {
            store,
            executor,
            outputs: tokio::sync::Mutex::new(HashMap::new()),
            staged: tokio::sync::Mutex::new(HashMap::new()),
            interruption: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn choose_interruption(&self, run_id: &str, pause: bool) {
        let choice = self
            .interruption
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(run_id.to_string())
            .or_default()
            .clone();
        choice.pause.store(pause, Ordering::SeqCst);
    }

    fn subagent_name(task: &Task) -> Result<String> {
        task.spec
            .extension
            .get("subagent")
            .and_then(|value| value.as_str())
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "task '{}' does not name a dispatchable subagent (extension.subagent)",
                    task.spec.id
                ))
            })
    }
}

#[async_trait::async_trait]
impl RuntimeDagController for FacadeTaskController {
    type DispatchOutput = SubagentResult;

    async fn load_snapshot(&self, run_id: &str) -> Result<echo_agent::tasks::RuntimePlanSnapshot> {
        self.store
            .load(run_id)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?
            .map(|graph| graph.snapshot)
            .ok_or_else(|| {
                ReactError::Other(format!("task graph '{run_id}' has no committed revision"))
            })
    }

    async fn claim_task(
        &self,
        run_id: &str,
        task: &Task,
        expected_revision: u64,
    ) -> Result<RuntimeTaskClaimOutcome> {
        self.store
            .claim_runtime_task(run_id, task, expected_revision)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }

    async fn claim_is_current(
        &self,
        run_id: &str,
        task_id: &str,
        claim: &TaskClaim,
    ) -> Result<bool> {
        self.store
            .runtime_claim_is_current(run_id, task_id, claim)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }

    async fn dispatch_task(
        &self,
        context: TaskSubagentContext,
        _claim: TaskClaim,
        task: Task,
    ) -> Result<Self::DispatchOutput> {
        let agent_name = Self::subagent_name(&task)?;
        let mut prompt = task.spec.description.clone();
        {
            let outputs = self.outputs.lock().await;
            for dependency in &task.spec.depends_on {
                if let Some(output) = outputs.get(dependency) {
                    prompt.push_str("\n\nCompleted dependency ");
                    prompt.push_str(dependency);
                    prompt.push_str(":\n");
                    prompt.push_str(&output.output);
                }
            }
        }
        self.executor
            .dispatch(DispatchRequest {
                agent_name,
                task: prompt,
                mode_override: None,
                cancel: context.cancel,
                parent_agent: "sdk-task-run".to_string(),
                parent_context: None,
                delegation_policy: context.delegation_policy,
                runtime_context: None,
                message: None,
                prompt_payload: None,
                prompt_context: None,
                constraints: Vec::new(),
                background: false,
            })
            .await
    }

    async fn resolve_dispatch(
        &self,
        _run_id: &str,
        claim: TaskClaim,
        _task: Task,
        dispatch: Result<Self::DispatchOutput>,
    ) -> Result<RuntimeTaskResolutionRequest> {
        let (request, output) = match dispatch {
            Ok(output) if output.outcome.status == SubagentStatus::Completed => {
                (RuntimeTaskResolutionRequest::Completed, Some(output))
            }
            Ok(output) if output.outcome.status == SubagentStatus::Cancelled => {
                (RuntimeTaskResolutionRequest::Cancelled, None)
            }
            Ok(output) => {
                let error = if output.outcome.summary.is_empty() {
                    output.output.clone()
                } else {
                    output.outcome.summary.clone()
                };
                (RuntimeTaskResolutionRequest::Failed { error }, None)
            }
            Err(error) => (
                RuntimeTaskResolutionRequest::Failed {
                    error: error.to_string(),
                },
                None,
            ),
        };
        if let Some(output) = output {
            self.staged
                .lock()
                .await
                .insert(claim.claim_id.clone(), output);
        }
        Ok(request)
    }

    async fn settle_resolution(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        request: RuntimeTaskResolutionRequest,
    ) -> Result<RuntimeTaskResolution> {
        let staged = self.staged.lock().await.remove(&claim.claim_id);
        let resolution = self
            .store
            .settle_runtime_resolution(run_id, &task.spec.id, claim, request)
            .await
            .map_err(|error| ReactError::Other(error.to_string()));
        if matches!(resolution, Ok(RuntimeTaskResolution::Completed))
            && let Some(output) = staged
        {
            self.outputs
                .lock()
                .await
                .insert(task.spec.id.clone(), output);
        }
        resolution
    }

    async fn abandon_claim(
        &self,
        run_id: &str,
        claim: &TaskClaim,
        task: &Task,
        abandonment: RuntimeClaimAbandonment,
    ) -> Result<RuntimeTaskSettlementOutcome> {
        let status = match abandonment {
            RuntimeClaimAbandonment::Interrupted { disposition } => match disposition {
                RuntimeInterruptionDisposition::Cancelled => TaskStatus::Cancelled,
                RuntimeInterruptionDisposition::Paused { reason } => TaskStatus::Paused(reason),
            },
            RuntimeClaimAbandonment::Failed { error } => TaskStatus::Failed(error),
        };
        self.store
            .settle_runtime_claim(run_id, &task.spec.id, claim, status)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }

    async fn interruption_disposition(
        &self,
        run_id: &str,
    ) -> Result<RuntimeInterruptionDisposition> {
        let pause = self
            .interruption
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .is_some_and(|choice| choice.pause.load(Ordering::SeqCst));
        Ok(if pause {
            RuntimeInterruptionDisposition::Paused {
                reason: "paused by rpc task control".to_string(),
            }
        } else {
            RuntimeInterruptionDisposition::Cancelled
        })
    }

    async fn settle_interruption(
        &self,
        run_id: &str,
        expected_revision: u64,
        disposition: RuntimeInterruptionDisposition,
    ) -> Result<RuntimeInterruptionSettlementOutcome> {
        self.store
            .settle_runtime_interruption(run_id, expected_revision, disposition)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))
    }
}

fn runtime_error(message: impl std::fmt::Display, method: &str) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&message.to_string()),
        Retryability::Never,
        method,
    )
}

fn wire_status(status: &TaskStatus) -> WireTaskStatus {
    match status {
        TaskStatus::Pending => WireTaskStatus::Pending,
        TaskStatus::Running => WireTaskStatus::Running,
        TaskStatus::Blocked(reason) => WireTaskStatus::Blocked {
            reason: reason.clone(),
        },
        TaskStatus::Completed => WireTaskStatus::Completed,
        TaskStatus::Failed(error) => WireTaskStatus::Failed {
            error: error.clone(),
        },
        TaskStatus::Skipped => WireTaskStatus::Skipped,
        TaskStatus::Cancelled => WireTaskStatus::Cancelled,
        TaskStatus::TimedOut { error } => WireTaskStatus::TimedOut {
            error: error.clone(),
        },
        TaskStatus::Retrying {
            attempt,
            last_error,
        } => WireTaskStatus::Retrying {
            attempt: *attempt,
            last_error: last_error.clone(),
        },
        TaskStatus::Paused(reason) => WireTaskStatus::Paused {
            reason: reason.clone(),
        },
    }
}

/// The concrete store and executor backing task execution for one session.
fn task_runtime_parts(
    authorities: &SessionAuthorityServices,
    method: &str,
) -> std::result::Result<(Arc<InMemoryRevisionedTaskStore>, Arc<SubagentExecutor>), EchoSdkError> {
    #[cfg(feature = "framework-subagent")]
    {
        let store = authorities.task_store.clone().ok_or_else(|| {
            runtime_error("task execution is not available in this Host build", method)
        })?;
        Ok((store, authorities.subagent_executor.clone()))
    }
    #[cfg(not(feature = "framework-subagent"))]
    {
        let _ = authorities;
        Err(runtime_error(
            "task execution requires the subagent feature in this Host build",
            method,
        ))
    }
}

pub(crate) async fn task_execute(
    state: Arc<CoreProfileState>,
    request: TaskExecuteRequest,
    responder: Responder<TaskExecuteResponse>,
    connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/task/execute";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::TaskGraph, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    let authorities = match session_services(&state, &request.task_run, method) {
        Ok(authorities) => authorities,
        Err(error) => fail!(error),
    };
    let (store, executor) = match task_runtime_parts(&authorities, method) {
        Ok(parts) => parts,
        Err(error) => fail!(error),
    };
    let scope = request.task_run.id.clone();
    // One controller per live run; the spawn captures it directly so no
    // later lookup can race the execution.
    let controller = Arc::new(FacadeTaskController::new(store, executor));
    let cancel = tokio_util::sync::CancellationToken::new();
    state.facade.register_task_execution(
        scope.clone(),
        super::TaskRunExecution {
            cancel: cancel.clone(),
            controller: controller.clone(),
        },
    );
    let state_for_task = state.clone();
    let scope_for_task = scope.clone();
    let _ = connection.spawn(async move {
        let service = RuntimeTaskService::new(controller, RuntimeTaskServiceConfig::default());
        if let Err(error) = service.execute(&scope_for_task, cancel).await {
            tracing::warn!("task graph execution for {scope_for_task} failed: {error}");
        }
        state_for_task.facade.remove_task_execution(&scope_for_task);
        Ok::<(), agent_client_protocol::Error>(())
    });
    responder.respond(TaskExecuteResponse {
        run: request.task_run,
    })
}

pub(crate) async fn task_control(
    state: Arc<CoreProfileState>,
    request: TaskControlRequest,
    responder: Responder<TaskControlResponse>,
    _connection: ConnectionTo<Client>,
) -> std::result::Result<(), agent_client_protocol::Error> {
    let method = "_echo_agent/task/control";
    let _services = require_extended(&state, method).await?;
    require_capability(&state, ExtensionCapability::TaskGraph, method)?;
    macro_rules! fail {
        ($error:expr) => {
            return responder.respond_with_error(wire::into_jsonrpc_error($error))
        };
    }
    if let Err(error) =
        state
            .handles
            .check_shape_and_generation(&request.task, HandleKind::PlanTask, method)
    {
        fail!(error);
    }
    let authorities = match session_services(&state, &request.task_run, method) {
        Ok(authorities) => authorities,
        Err(error) => fail!(error),
    };
    let Some(store) = authorities.task_store.clone() else {
        fail!(runtime_error(
            "task control is not available in this Host build",
            method,
        ));
    };
    let graph = match store.load(&request.task_run.id).await {
        Ok(Some(graph)) => graph,
        Ok(None) => {
            fail!(runtime_error(
                "task graph has no committed revision",
                method
            ));
        }
        Err(error) => {
            fail!(runtime_error(error, method));
        }
    };
    let Some(task) = graph
        .snapshot
        .tasks
        .iter()
        .find(|task| task.execution.task_id == request.task.id)
        .cloned()
    else {
        fail!(runtime_error("task id is not part of the graph", method));
    };
    let revision = graph.snapshot.revision;
    let outcome: std::result::Result<(), echo_agent::tasks::RevisionedTaskStoreError> =
        match request.action {
            ControlAction::Pause => match task.execution.claim.clone() {
                Some(claim) if task.execution.status == TaskStatus::Running => {
                    if let Some(execution) = state.facade.task_execution_of(&request.task_run.id) {
                        execution
                            .controller
                            .choose_interruption(&request.task_run.id, true);
                        execution.cancel.cancel();
                    }
                    store
                        .settle_runtime_claim(
                            &request.task_run.id,
                            &request.task.id,
                            &claim,
                            TaskStatus::Paused("paused by rpc task control".to_string()),
                        )
                        .await
                        .map(|_| ())
                }
                _ => Err(not_running_claim_error()),
            },
            ControlAction::Resume => {
                if let Some(execution) = state.facade.task_execution_of(&request.task_run.id) {
                    execution
                        .controller
                        .choose_interruption(&request.task_run.id, false);
                }
                store
                    .resume_runtime_task(&request.task_run.id, &task, revision)
                    .await
                    .map(|_| ())
            }
            ControlAction::Cancel => match task.execution.claim.clone() {
                Some(claim) => {
                    if let Some(execution) = state.facade.task_execution_of(&request.task_run.id) {
                        execution
                            .controller
                            .choose_interruption(&request.task_run.id, false);
                        execution.cancel.cancel();
                    }
                    store
                        .settle_runtime_claim(
                            &request.task_run.id,
                            &request.task.id,
                            &claim,
                            TaskStatus::Cancelled,
                        )
                        .await
                        .map(|_| ())
                }
                None => Err(echo_agent::tasks::RevisionedTaskStoreError::Rejected {
                    message: "cancel requires a live claim; the task never started".to_string(),
                }),
            },
        };
    let mut accepted = true;
    if let Err(error) = outcome {
        // Claim-settling semantics: verbs without a live claim are reported
        // as not accepted (never a fake state transition).
        if error.to_string().contains("requires a live claim") {
            accepted = false;
        } else {
            fail!(runtime_error(error, method));
        }
    }
    let updated = match store.load(&request.task_run.id).await {
        Ok(Some(graph)) => graph,
        _ => {
            fail!(runtime_error("task graph vanished during control", method));
        }
    };
    let status = updated
        .snapshot
        .tasks
        .iter()
        .find(|task| task.execution.task_id == request.task.id)
        .map(|task| task.execution.status.clone())
        .unwrap_or(TaskStatus::Pending);
    responder.respond(TaskControlResponse {
        accepted,
        status: wire_status(&status),
    })
}

fn not_running_claim_error() -> echo_agent::tasks::RevisionedTaskStoreError {
    echo_agent::tasks::RevisionedTaskStoreError::Rejected {
        message: "pause requires a running claim".to_string(),
    }
}
