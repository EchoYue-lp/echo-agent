//! Revisioned task graph planning and execution primitives.
//!
//! [`TaskRevisionService`] is the single CRUD/relation authority and
//! [`RuntimeTaskService`] is the single dependency execution entry point.

pub mod background_state;
mod events;
pub mod revisioned;
pub mod runtime;
mod runtime_executor;
pub mod runtime_service;
pub mod task_tools;
mod time;

pub mod background_task;
pub mod command_cell;
pub mod progress;

pub use crate::planning::PlanValidator;
pub use background_state::{BackgroundTaskState, CheckpointStore as BackgroundCheckpointStore};
pub use background_task::{
    AnyBackgroundTask, BackgroundTask, BackgroundTaskStatus, TaskSpawner, TaskSpawnerConfig,
    TaskSummary,
};
pub use command_cell::{
    BackgroundCommandManager, BackgroundCommandManagerConfig, CommandCellReservation,
};
pub use events::{
    AsyncTaskEventListener, LoggingListener, TaskEvent, TaskEventBus, TaskEventListener,
};
pub use progress::{Phase, PhasePlan, ProgressReporter, TaskProgress};
pub use revisioned::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, PreparedTaskPolicy, RevisionedTaskGraph,
    RevisionedTaskStore, RevisionedTaskStoreError, TaskCreateInput, TaskCreateOutcome, TaskDraft,
    TaskGraphCommit, TaskGraphContext, TaskGraphExecutionMode, TaskPatchApplication,
    TaskPatchEffects, TaskPatchEngine, TaskPlanPatch, TaskPlanPatchInputOp, TaskPlanPatchOp,
    TaskPolicyError, TaskRevisionError, TaskRevisionService, TaskSpecPatch, TaskToolPolicy,
    TaskUpdateInput,
};
pub use runtime::{
    DagDependencyState, DagExecutionState, DagRefresh, NestedDelegationPolicy,
    RuntimeInterruptionDisposition, SuggestedTask, Task, TaskClaim, TaskExecution,
    TaskExecutionSummary, TaskId, TaskSpec, TaskStatus, TaskSubagent, TaskSubagentContext,
};
pub use runtime_executor::{
    RuntimeClaimAbandonment, RuntimeDagController, RuntimeDagOutcome, RuntimePlanSnapshot,
    RuntimeRetryExhaustion, RuntimeStopDisposition, RuntimeTaskClaimOutcome, RuntimeTaskResolution,
    RuntimeTaskResolutionRequest, RuntimeTaskServiceConfig,
};
pub use runtime_service::{
    RuntimeInterruptionReceipt, RuntimeInterruptionSettlementOutcome, RuntimeTaskMutationError,
    RuntimeTaskRequeueOutcome, RuntimeTaskResumeOutcome, RuntimeTaskRetryOutcome,
    RuntimeTaskService, RuntimeTaskSettlementOutcome, cancel_unfinished_runtime_tasks,
    claim_runtime_task, requeue_runtime_claim, resume_runtime_task, retry_runtime_task,
    runtime_claim_is_current, settle_runtime_claim, settle_runtime_interruption,
    settle_runtime_resolution, validate_runtime_snapshot_claims,
};
pub use task_tools::{
    TaskCreateTool, TaskListTool, TaskUpdateTool, build_task_create_tool, build_task_list_tool,
    build_task_tools, build_task_update_tool,
};
