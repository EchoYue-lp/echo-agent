//! Revisioned task graph planning and execution primitives.
//!
//! [`TaskRevisionService`] is the single CRUD/relation authority and
//! [`RuntimeDagExecutor`] is the single dependency execution kernel.

pub mod background_state;
mod events;
pub mod revisioned;
pub mod runtime;
pub mod runtime_executor;
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
    DagExecutionState, DagRefresh, NestedDelegationPolicy, SuggestedTask, Task, TaskClaim,
    TaskExecution, TaskExecutionSummary, TaskId, TaskSpec, TaskStatus, TaskSubagent,
    TaskSubagentContext,
};
pub use runtime_executor::{
    RuntimeClaimAbandonment, RuntimeDagController, RuntimeDagExecutor, RuntimeDagExecutorConfig,
    RuntimeDagOutcome, RuntimePlanSnapshot, RuntimeStopDisposition, RuntimeTaskClaimOutcome,
    RuntimeTaskResolution,
};
pub use runtime_service::{
    RuntimeTaskMutationError, RuntimeTaskRequeueOutcome, RuntimeTaskRetryOutcome,
    RuntimeTaskService, block_runtime_task, claim_runtime_task, requeue_runtime_claim,
    retry_runtime_task, runtime_claim_is_current, settle_runtime_claim,
};
pub use task_tools::{
    TaskCreateTool, TaskListTool, TaskUpdateTool, build_task_create_tool, build_task_list_tool,
    build_task_tools, build_task_update_tool,
};
