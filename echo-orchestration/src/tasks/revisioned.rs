//! Revisioned task-relation service shared by lightweight progress tracking and
//! executable task graphs.
//!
//! The framework owns patch semantics, structural validation, revisions, and
//! optimistic concurrency. Applications provide persistence and product policy
//! through narrow adapters.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use echo_core::tools::ToolContext;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;

use super::{
    PlanValidator, RuntimePlanSnapshot, RuntimeTaskClaimOutcome, Task, TaskClaim, TaskExecution,
    TaskId, TaskSpec, TaskStatus,
};

/// How a task graph should be driven by a product adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGraphExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

/// Generic plan-level context carried beside the runtime DAG snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskGraphContext {
    pub goal: String,
    pub assumptions: Vec<String>,
    pub risks: Vec<String>,
    pub execution_mode: TaskGraphExecutionMode,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// One coherent task graph revision plus plan-level context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionedTaskGraph {
    pub snapshot: RuntimePlanSnapshot,
    pub context: TaskGraphContext,
}

/// Product-neutral task input before product defaults and metadata are added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDraft {
    pub id: String,
    pub title: String,
    pub description: String,
    pub depends_on: Vec<TaskId>,
    pub max_retries: u32,
    pub extension: serde_json::Value,
}

impl TaskDraft {
    /// Attach a typed extension before the framework applies product policy.
    pub fn with_extension<T: Serialize>(
        mut self,
        extension: T,
    ) -> std::result::Result<Self, String> {
        self.extension = serde_json::to_value(extension)
            .map_err(|error| format!("failed to encode task draft extension: {error}"))?;
        Ok(self)
    }

    /// Decode the draft extension into the caller's typed value.
    pub fn extension_as<T: DeserializeOwned>(&self) -> std::result::Result<T, String> {
        serde_json::from_value(self.extension.clone())
            .map_err(|error| format!("failed to decode task draft extension: {error}"))
    }
}

/// Parsed `task_create` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCreateInput {
    pub tasks: Vec<TaskDraft>,
    pub base_revision: Option<u64>,
    pub reason: Option<String>,
    pub assumptions: Vec<String>,
    pub risks: Vec<String>,
    pub execution_mode: TaskGraphExecutionMode,
}

/// Partial update of one immutable task specification.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpecPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub depends_on: Option<Vec<TaskId>>,
    pub max_retries: Option<u32>,
    /// Product-owned partial extension. Object values are recursively merged;
    /// non-object values replace the existing extension.
    pub extension: Option<serde_json::Value>,
}

impl TaskSpecPatch {
    /// Set a complete typed extension on this patch. Use the raw `extension`
    /// field only for partial object merges.
    pub fn with_extension<T: Serialize>(
        mut self,
        extension: T,
    ) -> std::result::Result<Self, String> {
        self.extension = Some(
            serde_json::to_value(extension)
                .map_err(|error| format!("failed to encode task patch extension: {error}"))?,
        );
        Ok(self)
    }

    /// Decode a complete extension when this patch carries one.
    pub fn extension_as<T: DeserializeOwned>(&self) -> std::result::Result<Option<T>, String> {
        self.extension
            .clone()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| format!("failed to decode task patch extension: {error}"))
    }
}

/// Canonical operation applied by the framework patch engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPlanPatchOp {
    Insert {
        after_task_id: Option<TaskId>,
        task: TaskSpec,
    },
    Update {
        task_id: TaskId,
        patch: TaskSpecPatch,
    },
    Skip {
        task_id: TaskId,
    },
    Reorder {
        task_ids: Vec<TaskId>,
    },
    SetStatus {
        task_id: TaskId,
        status: TaskStatus,
    },
}

/// Canonical optimistic patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPlanPatch {
    pub base_revision: u64,
    pub reason: String,
    pub operations: Vec<TaskPlanPatchOp>,
}

/// Parsed update operation before Insert drafts receive product defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPlanPatchInputOp {
    Insert {
        after_task_id: Option<TaskId>,
        task: TaskDraft,
    },
    Update {
        task_id: TaskId,
        patch: TaskSpecPatch,
    },
    Skip {
        task_id: TaskId,
    },
    Reorder {
        task_ids: Vec<TaskId>,
    },
    SetStatus {
        task_id: TaskId,
        status: TaskStatus,
    },
}

/// Parsed `task_update` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskUpdateInput {
    pub base_revision: u64,
    pub reason: String,
    pub operations: Vec<TaskPlanPatchInputOp>,
}

/// Generic effects persisted alongside one committed revision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPatchEffects {
    pub inserted_task_ids: Vec<TaskId>,
    pub updated_task_ids: Vec<TaskId>,
    pub skipped_task_ids: Vec<TaskId>,
    pub reset_task_ids: Vec<TaskId>,
    pub progressed_task_ids: Vec<TaskId>,
    pub reordered: bool,
}

/// Already-computed candidate handed to a persistence adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGraphCommit {
    pub expected_revision: Option<u64>,
    /// Exact runtime execution state observed when a relation patch was
    /// prepared. Persistence adapters must reject the commit if any entry has
    /// changed, even when the relation revision is unchanged. `None` is kept
    /// for create and legacy/direct commits that do not carry this stronger
    /// precondition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_executions: Option<BTreeMap<TaskId, TaskExecution>>,
    pub next: RevisionedTaskGraph,
    pub reason: String,
    pub effects: TaskPatchEffects,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum RevisionedTaskStoreError {
    #[error("task graph not found for scope {scope_id}")]
    NotFound { scope_id: String },
    #[error("task graph revision conflict: expected {expected:?}, current {current:?}")]
    Conflict {
        expected: Option<u64>,
        current: Option<u64>,
    },
    #[error("task store does not support {operation}")]
    Unsupported { operation: String },
    #[error("{message}")]
    Rejected { message: String },
    #[error("{message}")]
    Backend { message: String },
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum TaskPolicyError {
    #[error("{message}")]
    ScopeUnavailable { message: String },
    #[error("{message}")]
    Rejected { message: String },
    #[error("{message}")]
    Backend { message: String },
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum TaskRevisionError {
    #[error("{message}")]
    InvalidInput { message: String },
    #[error("task graph not found for scope {scope_id}")]
    GraphNotFound { scope_id: String },
    #[error("task not found: {task_id}")]
    TaskNotFound { task_id: TaskId },
    #[error("plan revision conflict: expected {expected:?}, current {current:?}")]
    RevisionConflict {
        expected: Option<u64>,
        current: Option<u64>,
    },
    #[error("{message}")]
    InvalidPatch { message: String },
    #[error("{message}")]
    PolicyRejected { message: String },
    #[error("task store does not support {operation}")]
    StoreUnsupported { operation: String },
    #[error("{message}")]
    StoreRejected { message: String },
    #[error("{message}")]
    Backend { message: String },
}

impl From<RevisionedTaskStoreError> for TaskRevisionError {
    fn from(error: RevisionedTaskStoreError) -> Self {
        match error {
            RevisionedTaskStoreError::NotFound { scope_id } => Self::GraphNotFound { scope_id },
            RevisionedTaskStoreError::Conflict { expected, current } => {
                Self::RevisionConflict { expected, current }
            }
            RevisionedTaskStoreError::Unsupported { operation } => {
                Self::StoreUnsupported { operation }
            }
            RevisionedTaskStoreError::Rejected { message } => Self::StoreRejected { message },
            RevisionedTaskStoreError::Backend { message } => Self::Backend { message },
        }
    }
}

impl From<TaskPolicyError> for TaskRevisionError {
    fn from(error: TaskPolicyError) -> Self {
        match error {
            TaskPolicyError::ScopeUnavailable { message }
            | TaskPolicyError::Rejected { message } => Self::PolicyRejected { message },
            TaskPolicyError::Backend { message } => Self::Backend { message },
        }
    }
}

/// Execution-only transaction at an unchanged relation revision.
///
/// Backends atomically compare the relation revision and the complete execution
/// map before publishing `next`. Context, task specifications, order and graph
/// revision must remain unchanged. Results and terminal state therefore share
/// the same persistence transaction as claim, retry and interruption facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionCommit {
    pub expected_revision: u64,
    pub expected_executions: BTreeMap<TaskId, TaskExecution>,
    pub next: RevisionedTaskGraph,
}

/// Persistence boundary for revisioned task graphs.
#[async_trait]
pub trait RevisionedTaskStore: Send + Sync {
    /// Side-effect-free capability declaration. CRUD-only backends remain
    /// valid but cannot accept runtime execution. Executable adapters opt in
    /// only when their atomic execution CAS contract is available.
    fn supports_runtime_execution(&self) -> bool {
        false
    }

    async fn load(
        &self,
        scope_id: &str,
    ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError>;

    async fn compare_and_commit(
        &self,
        scope_id: &str,
        commit: TaskGraphCommit,
    ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError>;

    /// Optional backend primitive for executable graphs. CRUD-only backends
    /// reject execution explicitly; the framework never creates a second
    /// in-memory graph when the registered store lacks this transaction.
    async fn compare_and_commit_execution(
        &self,
        _scope_id: &str,
        _commit: TaskExecutionCommit,
    ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
        Err(RevisionedTaskStoreError::Unsupported {
            operation: "task execution CAS".to_string(),
        })
    }
}

/// Product defaults produced without allowing policy to rewrite generic task
/// fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTaskPolicy {
    pub extension: serde_json::Value,
}

/// Product policy boundary used by the task tools and revision service.
#[async_trait]
pub trait TaskToolPolicy: Send + Sync {
    fn task_input_schema_extensions(&self) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::from_iter([(
            "extension".to_string(),
            serde_json::json!({
                "type": "object",
                "description": "Product-owned task data ignored by framework scheduling"
            }),
        )])
    }

    fn required_task_input_extensions(&self) -> Vec<String> {
        Vec::new()
    }

    fn allow_manual_progress_updates(&self) -> bool {
        false
    }

    async fn resolve_scope(&self, context: &ToolContext) -> Result<String, TaskPolicyError>;

    /// Prepare the product scope used by one graph creation attempt.
    ///
    /// An implementation that returns `Err` must first remove every
    /// unpublished side effect it staged. [`Self::abort_scope_preparation`] is
    /// called only after this method returns `Ok`.
    async fn ensure_scope(
        &self,
        scope_id: &str,
        input: &TaskCreateInput,
        context: &ToolContext,
    ) -> Result<(), TaskPolicyError>;

    /// Roll back product resources staged by [`Self::ensure_scope`] when task
    /// preparation, validation, or the graph commit fails.
    ///
    /// Implementations must be idempotent and must not remove an already
    /// published scope. Every policy implements this explicitly so policies
    /// with side effects cannot silently omit rollback handling.
    async fn abort_scope_preparation(&self, scope_id: &str) -> Result<(), TaskPolicyError>;

    async fn prepare_task(
        &self,
        scope_id: &str,
        draft: &TaskDraft,
        position: usize,
    ) -> Result<PreparedTaskPolicy, TaskPolicyError>;

    async fn prepare_initial_context(
        &self,
        scope_id: &str,
        input: &TaskCreateInput,
    ) -> Result<TaskGraphContext, TaskPolicyError>;

    async fn finalize_task_extension(
        &self,
        scope_id: &str,
        task_id: &str,
        position: usize,
        extension: serde_json::Value,
    ) -> Result<serde_json::Value, TaskPolicyError>;

    async fn validate_candidate(
        &self,
        scope_id: &str,
        tasks: &[Task],
    ) -> Result<(), TaskPolicyError>;
}

/// Default policy for the framework's per-Agent lightweight task graph.
#[derive(Debug, Clone)]
pub struct DefaultTaskToolPolicy {
    default_scope_id: String,
}

impl Default for DefaultTaskToolPolicy {
    fn default() -> Self {
        Self::new(format!("agent-task-scope-{}", uuid::Uuid::new_v4()))
    }
}

impl DefaultTaskToolPolicy {
    pub fn new(default_scope_id: impl Into<String>) -> Self {
        Self {
            default_scope_id: default_scope_id.into(),
        }
    }
}

#[async_trait]
impl TaskToolPolicy for DefaultTaskToolPolicy {
    fn allow_manual_progress_updates(&self) -> bool {
        true
    }

    async fn resolve_scope(&self, context: &ToolContext) -> Result<String, TaskPolicyError> {
        Ok(context
            .run_id
            .clone()
            .or_else(|| context.conversation_id.clone())
            .unwrap_or_else(|| self.default_scope_id.clone()))
    }

    async fn ensure_scope(
        &self,
        _scope_id: &str,
        _input: &TaskCreateInput,
        _context: &ToolContext,
    ) -> Result<(), TaskPolicyError> {
        Ok(())
    }

    async fn abort_scope_preparation(&self, _scope_id: &str) -> Result<(), TaskPolicyError> {
        Ok(())
    }

    async fn prepare_task(
        &self,
        _scope_id: &str,
        draft: &TaskDraft,
        _position: usize,
    ) -> Result<PreparedTaskPolicy, TaskPolicyError> {
        Ok(PreparedTaskPolicy {
            extension: draft.extension.clone(),
        })
    }

    async fn prepare_initial_context(
        &self,
        _scope_id: &str,
        input: &TaskCreateInput,
    ) -> Result<TaskGraphContext, TaskPolicyError> {
        let goal = input
            .tasks
            .first()
            .map(|task| task.title.clone())
            .unwrap_or_else(|| "Task graph".to_string());
        Ok(TaskGraphContext {
            goal,
            assumptions: input.assumptions.clone(),
            risks: input.risks.clone(),
            execution_mode: input.execution_mode,
            metadata: serde_json::Value::Null,
        })
    }

    async fn finalize_task_extension(
        &self,
        _scope_id: &str,
        _task_id: &str,
        _position: usize,
        extension: serde_json::Value,
    ) -> Result<serde_json::Value, TaskPolicyError> {
        Ok(extension)
    }

    async fn validate_candidate(
        &self,
        _scope_id: &str,
        _tasks: &[Task],
    ) -> Result<(), TaskPolicyError> {
        Ok(())
    }
}

/// Per-instance in-memory CAS Store used by default framework Agents.
#[derive(Debug, Default)]
pub struct InMemoryRevisionedTaskStore {
    graphs: RwLock<HashMap<String, RevisionedTaskGraph>>,
}

impl InMemoryRevisionedTaskStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persist an exact-claim stop request before projecting live cancellation.
    pub async fn request_runtime_task_interruption(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        disposition: super::RuntimeInterruptionDisposition,
    ) -> Result<bool, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::request_runtime_task_interruption(
                snapshot,
                task_id,
                claim,
                disposition.clone(),
            )
        })
        .await
    }

    /// Atomically claim a pending runtime task without changing the immutable
    /// graph revision. Intended for [`RuntimeDagController`](super::RuntimeDagController)
    /// adapters backed by this store.
    pub async fn claim_runtime_task(
        &self,
        scope_id: &str,
        task: &Task,
        expected_revision: u64,
    ) -> Result<RuntimeTaskClaimOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::claim_runtime_task(snapshot, task, expected_revision)
        })
        .await
    }

    /// Atomically settle the exact physical claim.
    pub async fn settle_runtime_claim(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        status: TaskStatus,
    ) -> Result<super::RuntimeTaskSettlementOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::settle_runtime_claim(snapshot, task_id, claim, status.clone())
        })
        .await
    }

    /// Atomically requeue one exact claim using the canonical runtime mutation.
    pub async fn requeue_runtime_claim(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        failure_fingerprint: Option<String>,
        error: String,
        exhaustion: super::RuntimeRetryExhaustion,
    ) -> Result<super::RuntimeTaskRequeueOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::requeue_runtime_claim(
                snapshot,
                task_id,
                claim,
                failure_fingerprint.clone(),
                error.clone(),
                exhaustion,
            )
        })
        .await
    }

    /// Atomically apply one framework-owned dispatch resolution request.
    pub async fn settle_runtime_resolution(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        request: super::RuntimeTaskResolutionRequest,
    ) -> Result<super::RuntimeTaskResolution, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::settle_runtime_resolution(
                snapshot,
                task_id,
                claim,
                request.clone(),
            )
        })
        .await
    }

    /// Atomically restart one exact unclaimed task using the canonical runtime
    /// mutation. Product run/descendant policy remains outside this store.
    pub async fn retry_runtime_task(
        &self,
        scope_id: &str,
        expected_task: &Task,
        expected_revision: u64,
    ) -> Result<super::RuntimeTaskRetryOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::retry_runtime_task(snapshot, expected_task, expected_revision)
        })
        .await
    }

    /// Atomically resume one paused task without consuming retry budget.
    pub async fn resume_runtime_task(
        &self,
        scope_id: &str,
        expected_task: &Task,
        expected_revision: u64,
    ) -> Result<super::RuntimeTaskResumeOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::resume_runtime_task(snapshot, expected_task, expected_revision)
        })
        .await
    }

    /// Atomically settle one run-level interruption at an exact graph revision.
    pub async fn settle_runtime_interruption(
        &self,
        scope_id: &str,
        expected_revision: u64,
        disposition: super::RuntimeInterruptionDisposition,
    ) -> Result<super::RuntimeInterruptionSettlementOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self, scope_id, |snapshot| {
            super::runtime_service::settle_runtime_interruption(
                snapshot,
                expected_revision,
                disposition.clone(),
            )
        })
        .await
    }

    pub async fn runtime_claim_is_current(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
    ) -> Result<bool, RevisionedTaskStoreError> {
        let graph = load_runtime_graph(self, scope_id).await?;
        super::runtime_service::runtime_claim_is_current(&graph.snapshot, task_id, claim).map_err(
            |error| RevisionedTaskStoreError::Rejected {
                message: error.to_string(),
            },
        )
    }
}

async fn load_runtime_graph(
    store: &dyn RevisionedTaskStore,
    scope_id: &str,
) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
    store
        .load(scope_id)
        .await?
        .ok_or_else(|| RevisionedTaskStoreError::NotFound {
            scope_id: scope_id.to_string(),
        })
}

fn execution_map(graph: &RevisionedTaskGraph) -> BTreeMap<TaskId, TaskExecution> {
    graph
        .snapshot
        .tasks
        .iter()
        .map(|task| (task.spec.id.clone(), task.execution.clone()))
        .collect()
}

async fn mutate_task_execution<T>(
    store: &dyn RevisionedTaskStore,
    scope_id: &str,
    mut mutate: impl FnMut(&mut RuntimePlanSnapshot) -> Result<T, super::RuntimeTaskMutationError>,
) -> Result<T, RevisionedTaskStoreError> {
    if !store.supports_runtime_execution() {
        return Err(RevisionedTaskStoreError::Unsupported {
            operation: "task execution CAS".to_string(),
        });
    }
    const MAX_EXECUTION_CAS_ATTEMPTS: usize = 32;
    for attempt in 1..=MAX_EXECUTION_CAS_ATTEMPTS {
        let mut graph = load_runtime_graph(store, scope_id).await?;
        let expected_executions = execution_map(&graph);
        let expected_revision = graph.snapshot.revision;
        let outcome =
            mutate(&mut graph.snapshot).map_err(|error| RevisionedTaskStoreError::Rejected {
                message: error.to_string(),
            })?;
        if expected_executions == execution_map(&graph) {
            return Ok(outcome);
        }
        match store
            .compare_and_commit_execution(
                scope_id,
                TaskExecutionCommit {
                    expected_revision,
                    expected_executions,
                    next: graph,
                },
            )
            .await
        {
            Ok(_) => return Ok(outcome),
            Err(error @ RevisionedTaskStoreError::Conflict { .. }) => {
                if attempt == MAX_EXECUTION_CAS_ATTEMPTS {
                    // A permanently conflicting backend must not keep Host
                    // lifecycle operations alive indefinitely. The caller
                    // receives the last typed conflict and may retry later.
                    return Err(error);
                }
                tokio::task::yield_now().await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(RevisionedTaskStoreError::Backend {
        message: "execution CAS attempt budget was empty".to_string(),
    })
}

#[async_trait]
impl RevisionedTaskStore for InMemoryRevisionedTaskStore {
    fn supports_runtime_execution(&self) -> bool {
        true
    }

    async fn load(
        &self,
        scope_id: &str,
    ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError> {
        Ok(self.graphs.read().await.get(scope_id).cloned())
    }

    async fn compare_and_commit(
        &self,
        scope_id: &str,
        commit: TaskGraphCommit,
    ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
        let mut graphs = self.graphs.write().await;
        let current = graphs.get(scope_id).map(|graph| graph.snapshot.revision);
        if current != commit.expected_revision {
            return Err(RevisionedTaskStoreError::Conflict {
                expected: commit.expected_revision,
                current,
            });
        }
        if let Some(current_graph) = graphs.get(scope_id) {
            let execution_drifted = if let Some(expected_executions) = &commit.expected_executions {
                let current_executions: BTreeMap<TaskId, TaskExecution> = current_graph
                    .snapshot
                    .tasks
                    .iter()
                    .map(|task| (task.spec.id.clone(), task.execution.clone()))
                    .collect();
                &current_executions != expected_executions
            } else {
                let intentionally_progressed = commit
                    .effects
                    .progressed_task_ids
                    .iter()
                    .chain(&commit.effects.skipped_task_ids)
                    .chain(&commit.effects.reset_task_ids)
                    .collect::<HashSet<_>>();
                current_graph.snapshot.tasks.iter().any(|current_task| {
                    if intentionally_progressed.contains(&current_task.spec.id) {
                        return false;
                    }
                    commit
                        .next
                        .snapshot
                        .tasks
                        .iter()
                        .find(|next_task| next_task.spec.id == current_task.spec.id)
                        .is_some_and(|next_task| next_task.execution != current_task.execution)
                })
            };
            if execution_drifted {
                return Err(RevisionedTaskStoreError::Conflict {
                    expected: commit.expected_revision,
                    current,
                });
            }
        }
        let expected_next = match commit.expected_revision {
            Some(revision) => revision.checked_add(1),
            None => Some(1),
        }
        .ok_or_else(|| RevisionedTaskStoreError::Rejected {
            message: "task graph revision overflow".to_string(),
        })?;
        if commit.next.snapshot.revision != expected_next {
            return Err(RevisionedTaskStoreError::Rejected {
                message: format!(
                    "invalid next task graph revision: expected {expected_next}, got {}",
                    commit.next.snapshot.revision
                ),
            });
        }
        super::validate_runtime_snapshot_claims(&commit.next.snapshot).map_err(|error| {
            RevisionedTaskStoreError::Rejected {
                message: error.to_string(),
            }
        })?;
        graphs.insert(scope_id.to_string(), commit.next.clone());
        Ok(commit.next)
    }

    async fn compare_and_commit_execution(
        &self,
        scope_id: &str,
        commit: TaskExecutionCommit,
    ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
        let mut graphs = self.graphs.write().await;
        let current = graphs
            .get(scope_id)
            .ok_or_else(|| RevisionedTaskStoreError::NotFound {
                scope_id: scope_id.to_string(),
            })?;
        if current.snapshot.revision != commit.expected_revision
            || execution_map(current) != commit.expected_executions
        {
            return Err(RevisionedTaskStoreError::Conflict {
                expected: Some(commit.expected_revision),
                current: Some(current.snapshot.revision),
            });
        }
        if commit.next.snapshot.revision != current.snapshot.revision
            || commit.next.context != current.context
            || commit
                .next
                .snapshot
                .tasks
                .iter()
                .map(|task| &task.spec)
                .ne(current.snapshot.tasks.iter().map(|task| &task.spec))
        {
            return Err(RevisionedTaskStoreError::Rejected {
                message: "execution CAS cannot change task relations, context or revision"
                    .to_string(),
            });
        }
        super::validate_runtime_snapshot_claims(&commit.next.snapshot).map_err(|error| {
            RevisionedTaskStoreError::Rejected {
                message: error.to_string(),
            }
        })?;
        graphs.insert(scope_id.to_string(), commit.next.clone());
        Ok(commit.next)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPatchApplication {
    pub tasks: Vec<Task>,
    pub effects: TaskPatchEffects,
}

/// Pure, framework-owned task patch semantics.
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskPatchEngine;

impl TaskPatchEngine {
    pub fn apply_operations(
        current: &[Task],
        operations: Vec<TaskPlanPatchOp>,
        allow_manual_progress_updates: bool,
    ) -> Result<TaskPatchApplication, TaskRevisionError> {
        if operations.is_empty() {
            return Err(TaskRevisionError::InvalidPatch {
                message: "task_update requires at least one operation".to_string(),
            });
        }

        let mut tasks = current.to_vec();
        let mut effects = TaskPatchEffects::default();
        for operation in operations {
            match operation {
                TaskPlanPatchOp::Insert {
                    after_task_id,
                    task,
                } => {
                    if tasks.iter().any(|existing| existing.spec.id == task.id) {
                        return Err(TaskRevisionError::InvalidPatch {
                            message: format!("task '{}' already exists", task.id),
                        });
                    }
                    let position = match after_task_id {
                        Some(after) => tasks
                            .iter()
                            .position(|existing| existing.spec.id == after)
                            .map(|index| index.saturating_add(1))
                            .ok_or_else(|| TaskRevisionError::TaskNotFound {
                                task_id: after.clone(),
                            })?,
                        None => tasks.len(),
                    };
                    effects.inserted_task_ids.push(task.id.clone());
                    let execution = TaskExecution::pending(task.id.clone());
                    tasks.insert(
                        position,
                        Task {
                            spec: task,
                            execution,
                        },
                    );
                }
                TaskPlanPatchOp::Update { task_id, patch } => {
                    let task = tasks
                        .iter_mut()
                        .find(|task| task.spec.id == task_id)
                        .ok_or_else(|| TaskRevisionError::TaskNotFound {
                            task_id: task_id.clone(),
                        })?;
                    if !matches!(
                        task.execution.status,
                        TaskStatus::Pending | TaskStatus::Blocked(_)
                    ) {
                        return Err(TaskRevisionError::InvalidPatch {
                            message: format!(
                                "cannot modify task '{}' in {:?}",
                                task.spec.id, task.execution.status
                            ),
                        });
                    }
                    if matches!(task.execution.status, TaskStatus::Blocked(_)) {
                        task.execution.status = TaskStatus::Pending;
                        effects.reset_task_ids.push(task_id.clone());
                    }
                    apply_spec_patch(&mut task.spec, patch);
                    task.execution.claim = None;
                    task.execution.interruption_requested = None;
                    effects.updated_task_ids.push(task_id);
                }
                TaskPlanPatchOp::Skip { task_id } => {
                    let task = tasks
                        .iter_mut()
                        .find(|task| task.spec.id == task_id)
                        .ok_or_else(|| TaskRevisionError::TaskNotFound {
                            task_id: task_id.clone(),
                        })?;
                    if !matches!(
                        task.execution.status,
                        TaskStatus::Pending | TaskStatus::Blocked(_)
                    ) {
                        return Err(TaskRevisionError::InvalidPatch {
                            message: format!(
                                "cannot skip task '{}' in {:?}",
                                task.spec.id, task.execution.status
                            ),
                        });
                    }
                    task.execution.status = TaskStatus::Skipped;
                    task.execution.claim = None;
                    task.execution.interruption_requested = None;
                    effects.skipped_task_ids.push(task_id);
                }
                TaskPlanPatchOp::Reorder { task_ids } => {
                    let expected = tasks
                        .iter()
                        .map(|task| task.spec.id.clone())
                        .collect::<HashSet<_>>();
                    let actual = task_ids.iter().cloned().collect::<HashSet<_>>();
                    if expected != actual || task_ids.len() != tasks.len() {
                        return Err(TaskRevisionError::InvalidPatch {
                            message: "reorder must contain every task id exactly once".to_string(),
                        });
                    }
                    let positions = task_ids
                        .into_iter()
                        .enumerate()
                        .map(|(position, task_id)| (task_id, position))
                        .collect::<HashMap<_, _>>();
                    tasks.sort_by_key(|task| {
                        positions.get(&task.spec.id).copied().unwrap_or(usize::MAX)
                    });
                    effects.reordered = true;
                }
                TaskPlanPatchOp::SetStatus { task_id, status } => {
                    if !allow_manual_progress_updates {
                        return Err(TaskRevisionError::PolicyRejected {
                            message: "manual task status updates are disabled".to_string(),
                        });
                    }
                    if !matches!(
                        status,
                        TaskStatus::Pending
                            | TaskStatus::Running
                            | TaskStatus::Completed
                            | TaskStatus::Cancelled
                    ) {
                        return Err(TaskRevisionError::InvalidPatch {
                            message: "manual task status must be pending, in_progress, completed, or cancelled"
                                .to_string(),
                        });
                    }
                    let task = tasks
                        .iter_mut()
                        .find(|task| task.spec.id == task_id)
                        .ok_or_else(|| TaskRevisionError::TaskNotFound {
                            task_id: task_id.clone(),
                        })?;
                    if task.execution.status != status {
                        task.execution.status = task
                            .execution
                            .status
                            .transition_to(status)
                            .map_err(|message| TaskRevisionError::InvalidPatch { message })?;
                        task.execution.claim = None;
                        task.execution.interruption_requested = None;
                        effects.progressed_task_ids.push(task_id);
                    }
                }
            }
        }
        Ok(TaskPatchApplication { tasks, effects })
    }
}

fn apply_spec_patch(spec: &mut TaskSpec, patch: TaskSpecPatch) {
    if let Some(title) = patch.title {
        spec.title = title;
    }
    if let Some(description) = patch.description {
        spec.description = description;
    }
    if let Some(depends_on) = patch.depends_on {
        spec.depends_on = depends_on;
    }
    if let Some(max_retries) = patch.max_retries {
        spec.max_retries = max_retries;
    }
    if let Some(extension) = patch.extension {
        merge_extension(&mut spec.extension, extension);
    }
}

fn merge_extension(current: &mut serde_json::Value, patch: serde_json::Value) {
    match (current, patch) {
        (serde_json::Value::Object(current), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                if let Some(existing) = current.get_mut(&key) {
                    merge_extension(existing, value);
                } else {
                    current.insert(key, value);
                }
            }
        }
        (current, patch) => *current = patch,
    }
}

#[derive(Debug, Clone)]
pub struct TaskCreateOutcome {
    pub graph: RevisionedTaskGraph,
    pub created_count: usize,
    pub appended: bool,
}

/// Unique framework service for task creation, revision patches, and reads.
pub struct TaskRevisionService {
    store: Arc<dyn RevisionedTaskStore>,
    policy: Arc<dyn TaskToolPolicy>,
    validator: PlanValidator,
}

impl TaskRevisionService {
    /// Report the capability of the same registered graph backend without
    /// loading, writing or dispatching a task.
    pub fn supports_runtime_execution(&self) -> bool {
        self.store.supports_runtime_execution()
    }

    pub fn new(store: Arc<dyn RevisionedTaskStore>, policy: Arc<dyn TaskToolPolicy>) -> Self {
        Self {
            store,
            policy,
            validator: PlanValidator::default(),
        }
    }

    /// Persist an exact-claim stop request before projecting live cancellation.
    pub async fn request_runtime_task_interruption(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        disposition: super::RuntimeInterruptionDisposition,
    ) -> Result<bool, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::request_runtime_task_interruption(
                snapshot,
                task_id,
                claim,
                disposition.clone(),
            )
        })
        .await
    }

    /// Atomically claim a pending runtime task without changing the immutable
    /// graph revision. Intended for [`RuntimeDagController`](super::RuntimeDagController)
    /// adapters backed by this store.
    pub async fn claim_runtime_task(
        &self,
        scope_id: &str,
        task: &Task,
        expected_revision: u64,
    ) -> Result<RuntimeTaskClaimOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::claim_runtime_task(snapshot, task, expected_revision)
        })
        .await
    }

    /// Atomically settle the exact physical claim.
    pub async fn settle_runtime_claim(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        status: TaskStatus,
    ) -> Result<super::RuntimeTaskSettlementOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::settle_runtime_claim(snapshot, task_id, claim, status.clone())
        })
        .await
    }

    /// Atomically requeue one exact claim using the canonical runtime mutation.
    pub async fn requeue_runtime_claim(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        failure_fingerprint: Option<String>,
        error: String,
        exhaustion: super::RuntimeRetryExhaustion,
    ) -> Result<super::RuntimeTaskRequeueOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::requeue_runtime_claim(
                snapshot,
                task_id,
                claim,
                failure_fingerprint.clone(),
                error.clone(),
                exhaustion,
            )
        })
        .await
    }

    /// Atomically apply one framework-owned dispatch resolution request.
    pub async fn settle_runtime_resolution(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
        request: super::RuntimeTaskResolutionRequest,
    ) -> Result<super::RuntimeTaskResolution, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::settle_runtime_resolution(
                snapshot,
                task_id,
                claim,
                request.clone(),
            )
        })
        .await
    }

    /// Atomically restart one exact unclaimed task using the canonical runtime
    /// mutation. Product run/descendant policy remains outside this store.
    pub async fn retry_runtime_task(
        &self,
        scope_id: &str,
        expected_task: &Task,
        expected_revision: u64,
    ) -> Result<super::RuntimeTaskRetryOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::retry_runtime_task(snapshot, expected_task, expected_revision)
        })
        .await
    }

    /// Atomically resume one paused task without consuming retry budget.
    pub async fn resume_runtime_task(
        &self,
        scope_id: &str,
        expected_task: &Task,
        expected_revision: u64,
    ) -> Result<super::RuntimeTaskResumeOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::resume_runtime_task(snapshot, expected_task, expected_revision)
        })
        .await
    }

    /// Atomically settle one run-level interruption at an exact graph revision.
    pub async fn settle_runtime_interruption(
        &self,
        scope_id: &str,
        expected_revision: u64,
        disposition: super::RuntimeInterruptionDisposition,
    ) -> Result<super::RuntimeInterruptionSettlementOutcome, RevisionedTaskStoreError> {
        mutate_task_execution(self.store.as_ref(), scope_id, |snapshot| {
            super::runtime_service::settle_runtime_interruption(
                snapshot,
                expected_revision,
                disposition.clone(),
            )
        })
        .await
    }

    pub async fn runtime_claim_is_current(
        &self,
        scope_id: &str,
        task_id: &str,
        claim: &TaskClaim,
    ) -> Result<bool, RevisionedTaskStoreError> {
        let graph = load_runtime_graph(self.store.as_ref(), scope_id).await?;
        super::runtime_service::runtime_claim_is_current(&graph.snapshot, task_id, claim).map_err(
            |error| RevisionedTaskStoreError::Rejected {
                message: error.to_string(),
            },
        )
    }

    pub fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
    }

    pub fn task_input_schema_extensions(&self) -> serde_json::Map<String, serde_json::Value> {
        self.policy.task_input_schema_extensions()
    }

    pub fn required_task_input_extensions(&self) -> Vec<String> {
        self.policy.required_task_input_extensions()
    }

    pub fn allow_manual_progress_updates(&self) -> bool {
        self.policy.allow_manual_progress_updates()
    }

    pub async fn resolve_scope(&self, context: &ToolContext) -> Result<String, TaskRevisionError> {
        self.policy.resolve_scope(context).await.map_err(Into::into)
    }

    pub async fn load(
        &self,
        scope_id: &str,
    ) -> Result<Option<RevisionedTaskGraph>, TaskRevisionError> {
        self.store.load(scope_id).await.map_err(Into::into)
    }

    pub async fn create_from_tool(
        &self,
        input: TaskCreateInput,
        context: &ToolContext,
    ) -> Result<TaskCreateOutcome, TaskRevisionError> {
        if input.tasks.is_empty() {
            return Err(TaskRevisionError::InvalidInput {
                message: "task_create requires at least one task".to_string(),
            });
        }
        let scope_id = self.resolve_scope(context).await?;
        self.policy
            .ensure_scope(&scope_id, &input, context)
            .await
            .map_err(TaskRevisionError::from)?;
        match self.create_after_scope_preparation(&scope_id, input).await {
            Ok(outcome) => Ok(outcome),
            Err(create_error) => match self.policy.abort_scope_preparation(&scope_id).await {
                Ok(()) => Err(create_error),
                Err(abort_error) => Err(TaskRevisionError::Backend {
                    message: format!(
                        "task creation failed: {create_error}; scope preparation rollback failed: {abort_error}"
                    ),
                }),
            },
        }
    }

    async fn create_after_scope_preparation(
        &self,
        scope_id: &str,
        input: TaskCreateInput,
    ) -> Result<TaskCreateOutcome, TaskRevisionError> {
        let current = self.load(scope_id).await?;
        let start_position = current
            .as_ref()
            .map(|graph| graph.snapshot.tasks.len())
            .unwrap_or(0);
        let mut prepared_tasks = Vec::with_capacity(input.tasks.len());
        for (offset, draft) in input.tasks.iter().enumerate() {
            let position = start_position.saturating_add(offset);
            let prepared = self
                .policy
                .prepare_task(scope_id, draft, position)
                .await
                .map_err(TaskRevisionError::from)?;
            prepared_tasks.push(TaskSpec {
                id: draft.id.clone(),
                title: draft.title.clone(),
                description: draft.description.clone(),
                depends_on: draft.depends_on.clone(),
                max_retries: draft.max_retries,
                extension: prepared.extension,
            });
        }

        if let Some(graph) = current {
            let base_revision = input
                .base_revision
                .filter(|revision| *revision > 0)
                .ok_or_else(|| TaskRevisionError::InvalidInput {
                    message: "task_create requires base_revision when tasks already exist"
                        .to_string(),
                })?;
            let reason = input
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .unwrap_or("add tasks")
                .to_string();
            let operations = prepared_tasks
                .into_iter()
                .map(|task| TaskPlanPatchOp::Insert {
                    after_task_id: None,
                    task,
                })
                .collect();
            let graph = self
                .apply_patch_to_loaded(
                    scope_id,
                    graph,
                    TaskPlanPatch {
                        base_revision,
                        reason,
                        operations,
                    },
                )
                .await?;
            return Ok(TaskCreateOutcome {
                graph,
                created_count: input.tasks.len(),
                appended: true,
            });
        }

        let graph_context = self
            .policy
            .prepare_initial_context(scope_id, &input)
            .await
            .map_err(TaskRevisionError::from)?;
        let tasks = prepared_tasks
            .into_iter()
            .map(|spec| {
                let execution = TaskExecution::pending(spec.id.clone());
                Task { spec, execution }
            })
            .collect();
        let graph = self
            .create_prepared(
                scope_id,
                graph_context,
                tasks,
                "initial complete plan".to_string(),
            )
            .await?;
        Ok(TaskCreateOutcome {
            graph,
            created_count: input.tasks.len(),
            appended: false,
        })
    }

    pub async fn update_from_tool(
        &self,
        input: TaskUpdateInput,
        context: &ToolContext,
    ) -> Result<RevisionedTaskGraph, TaskRevisionError> {
        let scope_id = self.resolve_scope(context).await?;
        let current =
            self.load(&scope_id)
                .await?
                .ok_or_else(|| TaskRevisionError::GraphNotFound {
                    scope_id: scope_id.clone(),
                })?;
        let mut operations = Vec::with_capacity(input.operations.len());
        let mut insert_offset = 0usize;
        for operation in input.operations {
            let operation = match operation {
                TaskPlanPatchInputOp::Insert {
                    after_task_id,
                    task,
                } => {
                    let position = current.snapshot.tasks.len().saturating_add(insert_offset);
                    insert_offset = insert_offset.saturating_add(1);
                    let prepared = self
                        .policy
                        .prepare_task(&scope_id, &task, position)
                        .await
                        .map_err(TaskRevisionError::from)?;
                    TaskPlanPatchOp::Insert {
                        after_task_id,
                        task: TaskSpec {
                            id: task.id,
                            title: task.title,
                            description: task.description,
                            depends_on: task.depends_on,
                            max_retries: task.max_retries,
                            extension: prepared.extension,
                        },
                    }
                }
                TaskPlanPatchInputOp::Update { task_id, patch } => {
                    TaskPlanPatchOp::Update { task_id, patch }
                }
                TaskPlanPatchInputOp::Skip { task_id } => TaskPlanPatchOp::Skip { task_id },
                TaskPlanPatchInputOp::Reorder { task_ids } => TaskPlanPatchOp::Reorder { task_ids },
                TaskPlanPatchInputOp::SetStatus { task_id, status } => {
                    TaskPlanPatchOp::SetStatus { task_id, status }
                }
            };
            operations.push(operation);
        }
        self.apply_patch_to_loaded(
            &scope_id,
            current,
            TaskPlanPatch {
                base_revision: input.base_revision,
                reason: input.reason,
                operations,
            },
        )
        .await
    }

    pub async fn create_prepared(
        &self,
        scope_id: &str,
        context: TaskGraphContext,
        tasks: Vec<Task>,
        reason: String,
    ) -> Result<RevisionedTaskGraph, TaskRevisionError> {
        let tasks = self.finalize_and_validate(scope_id, tasks, 1).await?;
        let graph = RevisionedTaskGraph {
            snapshot: RuntimePlanSnapshot { revision: 1, tasks },
            context,
        };
        self.store
            .compare_and_commit(
                scope_id,
                TaskGraphCommit {
                    expected_revision: None,
                    expected_executions: None,
                    next: graph,
                    reason,
                    effects: TaskPatchEffects::default(),
                },
            )
            .await
            .map_err(Into::into)
    }

    pub async fn apply_patch(
        &self,
        scope_id: &str,
        patch: TaskPlanPatch,
    ) -> Result<RevisionedTaskGraph, TaskRevisionError> {
        let current =
            self.load(scope_id)
                .await?
                .ok_or_else(|| TaskRevisionError::GraphNotFound {
                    scope_id: scope_id.to_string(),
                })?;
        self.apply_patch_to_loaded(scope_id, current, patch).await
    }

    async fn apply_patch_to_loaded(
        &self,
        scope_id: &str,
        current: RevisionedTaskGraph,
        patch: TaskPlanPatch,
    ) -> Result<RevisionedTaskGraph, TaskRevisionError> {
        if patch.base_revision == 0 {
            return Err(TaskRevisionError::InvalidInput {
                message: "task_update requires base_revision".to_string(),
            });
        }
        if current.snapshot.revision != patch.base_revision {
            return Err(TaskRevisionError::RevisionConflict {
                expected: Some(patch.base_revision),
                current: Some(current.snapshot.revision),
            });
        }
        if patch.reason.trim().is_empty() {
            return Err(TaskRevisionError::InvalidPatch {
                message: "task_update requires a non-empty reason".to_string(),
            });
        }
        let application = TaskPatchEngine::apply_operations(
            &current.snapshot.tasks,
            patch.operations,
            self.policy.allow_manual_progress_updates(),
        )?;
        let expected_executions = current
            .snapshot
            .tasks
            .iter()
            .map(|task| (task.spec.id.clone(), task.execution.clone()))
            .collect();
        let next_revision = current.snapshot.revision.checked_add(1).ok_or_else(|| {
            TaskRevisionError::InvalidPatch {
                message: "task graph revision overflow".to_string(),
            }
        })?;
        let tasks = self
            .finalize_and_validate(scope_id, application.tasks, next_revision)
            .await?;
        let next = RevisionedTaskGraph {
            snapshot: RuntimePlanSnapshot {
                revision: next_revision,
                tasks,
            },
            context: current.context,
        };
        self.store
            .compare_and_commit(
                scope_id,
                TaskGraphCommit {
                    expected_revision: Some(patch.base_revision),
                    expected_executions: Some(expected_executions),
                    next,
                    reason: patch.reason,
                    effects: application.effects,
                },
            )
            .await
            .map_err(Into::into)
    }

    async fn finalize_and_validate(
        &self,
        scope_id: &str,
        mut tasks: Vec<Task>,
        revision: u64,
    ) -> Result<Vec<Task>, TaskRevisionError> {
        for (position, task) in tasks.iter_mut().enumerate() {
            let extension = std::mem::take(&mut task.spec.extension);
            task.spec.extension = self
                .policy
                .finalize_task_extension(scope_id, &task.spec.id, position, extension)
                .await
                .map_err(TaskRevisionError::from)?;
        }
        self.policy
            .validate_candidate(scope_id, &tasks)
            .await
            .map_err(TaskRevisionError::from)?;
        self.validator
            .validate_task_snapshot(&tasks)
            .map_err(|errors| TaskRevisionError::InvalidPatch {
                message: errors.join("; "),
            })?;
        super::validate_runtime_snapshot_claims(&RuntimePlanSnapshot {
            revision,
            tasks: tasks.clone(),
        })
        .map_err(|error| TaskRevisionError::InvalidPatch {
            message: error.to_string(),
        })?;
        Ok(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(id: &str, depends_on: &[&str]) -> TaskDraft {
        TaskDraft {
            id: id.to_string(),
            title: format!("Task {id}"),
            description: format!("Do {id}"),
            depends_on: depends_on.iter().map(|item| (*item).to_string()).collect(),
            max_retries: 3,
            extension: serde_json::json!({ "check": "accepted" }),
        }
    }

    fn create_input(tasks: Vec<TaskDraft>) -> TaskCreateInput {
        TaskCreateInput {
            tasks,
            base_revision: None,
            reason: None,
            assumptions: Vec::new(),
            risks: Vec::new(),
            execution_mode: TaskGraphExecutionMode::Parallel,
        }
    }

    fn service() -> TaskRevisionService {
        TaskRevisionService::new(
            Arc::new(InMemoryRevisionedTaskStore::new()),
            Arc::new(DefaultTaskToolPolicy::new("test-scope")),
        )
    }

    #[test]
    fn review_explicit_null_result_survives_graph_checkpoint_roundtrip() -> Result<(), String> {
        let mut task = Task {
            spec: TaskSpec {
                id: "null-result".to_string(),
                title: "Null is an explicit result".to_string(),
                description: "checkpoint".to_string(),
                depends_on: Vec::new(),
                max_retries: 0,
                extension: serde_json::Value::Null,
            },
            execution: TaskExecution::pending("null-result"),
        };
        task.execution.status = TaskStatus::Completed;
        task.execution.result = Some(serde_json::Value::Null);
        let graph = RevisionedTaskGraph {
            snapshot: RuntimePlanSnapshot {
                revision: 7,
                tasks: vec![task.clone()],
            },
            context: TaskGraphContext {
                goal: "restore null".to_string(),
                assumptions: Vec::new(),
                risks: Vec::new(),
                execution_mode: TaskGraphExecutionMode::Parallel,
                metadata: serde_json::Value::Null,
            },
        };
        let encoded = serde_json::to_string(&graph).map_err(|error| error.to_string())?;
        let restored: RevisionedTaskGraph =
            serde_json::from_str(&encoded).map_err(|error| error.to_string())?;
        assert_eq!(
            restored
                .snapshot
                .tasks
                .first()
                .map(|task| &task.execution.result),
            Some(&Some(serde_json::Value::Null))
        );
        task.execution.result = None;
        let encoded = serde_json::to_value(&task).map_err(|error| error.to_string())?;
        assert!(encoded.pointer("/execution/result").is_none());
        let absent: Task = serde_json::from_value(encoded).map_err(|error| error.to_string())?;
        assert_eq!(absent.execution.result, None);
        Ok(())
    }

    #[tokio::test]
    async fn review_relation_terminal_patch_clears_exact_interrupt_and_rejects_stale_execution()
    -> Result<(), String> {
        let (service, store) = proxy_service(true);
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let graph = service
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing graph".to_string())?;
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing task".to_string())?;
        let RuntimeTaskClaimOutcome::Claimed(claim) = service
            .claim_runtime_task("proxy-scope", task, 1)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err("claim rejected".to_string());
        };
        service
            .request_runtime_task_interruption(
                "proxy-scope",
                "a",
                &claim,
                super::super::RuntimeInterruptionDisposition::Paused {
                    reason: "pause".to_string(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let current = store
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing requested graph".to_string())?;
        let stale_execution = TaskExecutionCommit {
            expected_revision: 1,
            expected_executions: execution_map(&current),
            next: current,
        };
        let patched = service
            .apply_patch(
                "proxy-scope",
                TaskPlanPatch {
                    base_revision: 1,
                    reason: "explicit terminal progress".to_string(),
                    operations: vec![TaskPlanPatchOp::SetStatus {
                        task_id: "a".to_string(),
                        status: TaskStatus::Cancelled,
                    }],
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        super::super::validate_runtime_snapshot_claims(&patched.snapshot)
            .map_err(|error| error.to_string())?;
        assert_eq!(
            patched
                .snapshot
                .tasks
                .first()
                .and_then(|task| task.execution.interruption_requested.as_ref()),
            None
        );
        assert!(matches!(
            store
                .compare_and_commit_execution("proxy-scope", stale_execution)
                .await,
            Err(RevisionedTaskStoreError::Conflict { .. })
        ));
        Ok(())
    }

    /// A callback/proxy-shaped backend. Runtime callers can only use the
    /// public trait, so tests detect accidental reliance on concrete methods.
    struct ProxyExecutionStore {
        inner: InMemoryRevisionedTaskStore,
        commits: std::sync::atomic::AtomicUsize,
        enabled: bool,
    }

    #[async_trait]
    impl RevisionedTaskStore for ProxyExecutionStore {
        fn supports_runtime_execution(&self) -> bool {
            self.enabled
        }

        async fn load(
            &self,
            scope_id: &str,
        ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError> {
            self.inner.load(scope_id).await
        }
        async fn compare_and_commit(
            &self,
            scope_id: &str,
            commit: TaskGraphCommit,
        ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
            self.inner.compare_and_commit(scope_id, commit).await
        }
        async fn compare_and_commit_execution(
            &self,
            scope_id: &str,
            commit: TaskExecutionCommit,
        ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
            if !self.enabled {
                return Err(RevisionedTaskStoreError::Unsupported {
                    operation: "proxy execution".to_string(),
                });
            }
            self.commits
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner
                .compare_and_commit_execution(scope_id, commit)
                .await
        }
    }

    struct AlwaysConflictStore {
        inner: InMemoryRevisionedTaskStore,
        attempts: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl RevisionedTaskStore for AlwaysConflictStore {
        fn supports_runtime_execution(&self) -> bool {
            true
        }

        async fn load(
            &self,
            scope_id: &str,
        ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError> {
            self.inner.load(scope_id).await
        }
        async fn compare_and_commit(
            &self,
            scope_id: &str,
            commit: TaskGraphCommit,
        ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
            self.inner.compare_and_commit(scope_id, commit).await
        }
        async fn compare_and_commit_execution(
            &self,
            _scope_id: &str,
            commit: TaskExecutionCommit,
        ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(RevisionedTaskStoreError::Conflict {
                expected: Some(commit.expected_revision),
                current: Some(commit.expected_revision),
            })
        }
    }

    #[tokio::test]
    async fn persistent_backend_conflict_returns_a_bounded_typed_error() -> Result<(), String> {
        let store = Arc::new(AlwaysConflictStore {
            inner: InMemoryRevisionedTaskStore::new(),
            attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("bounded-cas")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let graph = service
            .load("bounded-cas")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing graph".to_string())?;
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing task".to_string())?;
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            service.claim_runtime_task("bounded-cas", task, 1),
        )
        .await
        .map_err(|_| "execution CAS retried forever".to_string())?;
        assert!(matches!(
            error,
            Err(RevisionedTaskStoreError::Conflict {
                expected: Some(1),
                current: Some(1)
            })
        ));
        assert_eq!(store.attempts.load(std::sync::atomic::Ordering::SeqCst), 32);
        assert_eq!(
            service
                .load("bounded-cas")
                .await
                .map_err(|error| error.to_string())?
                .and_then(|graph| graph
                    .snapshot
                    .tasks
                    .first()
                    .map(|task| task.execution.status.clone())),
            Some(TaskStatus::Pending)
        );
        Ok(())
    }

    struct CrudOnlyPreflightStore;
    #[async_trait]
    impl RevisionedTaskStore for CrudOnlyPreflightStore {
        async fn load(
            &self,
            _scope_id: &str,
        ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError> {
            Err(RevisionedTaskStoreError::Backend {
                message: "runtime preflight touched backend load".to_string(),
            })
        }
        async fn compare_and_commit(
            &self,
            _scope_id: &str,
            _commit: TaskGraphCommit,
        ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
            Err(RevisionedTaskStoreError::Backend {
                message: "runtime preflight mutated backend".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn runtime_capability_preflight_rejects_crud_only_before_loading_or_mutating()
    -> Result<(), String> {
        let service = TaskRevisionService::new(
            Arc::new(CrudOnlyPreflightStore),
            Arc::new(DefaultTaskToolPolicy::new("preflight")),
        );
        assert!(!service.supports_runtime_execution());
        let task = Task {
            spec: TaskSpec {
                id: "a".to_string(),
                title: "preflight".to_string(),
                description: "preflight".to_string(),
                depends_on: Vec::new(),
                max_retries: 0,
                extension: serde_json::Value::Null,
            },
            execution: TaskExecution::pending("a"),
        };
        assert!(matches!(
            service.claim_runtime_task("preflight", &task, 1).await,
            Err(RevisionedTaskStoreError::Unsupported { .. })
        ));
        Ok(())
    }

    fn proxy_service(enabled: bool) -> (TaskRevisionService, Arc<ProxyExecutionStore>) {
        let store = Arc::new(ProxyExecutionStore {
            inner: InMemoryRevisionedTaskStore::new(),
            commits: std::sync::atomic::AtomicUsize::new(0),
            enabled,
        });
        (
            TaskRevisionService::new(
                store.clone(),
                Arc::new(DefaultTaskToolPolicy::new("proxy-scope")),
            ),
            store,
        )
    }

    #[tokio::test]
    async fn arbitrary_backend_runtime_and_crud_share_one_canonical_graph_and_result()
    -> Result<(), String> {
        let (service, store) = proxy_service(true);
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let loaded = service
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing graph".to_string())?;
        let task = loaded
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing task".to_string())?;
        let RuntimeTaskClaimOutcome::Claimed(claim) = service
            .claim_runtime_task("proxy-scope", task, 1)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err("claim rejected".to_string());
        };
        assert_eq!(
            service
                .settle_runtime_resolution(
                    "proxy-scope",
                    "a",
                    &claim,
                    super::super::RuntimeTaskResolutionRequest::CompletedWithResult {
                        result: serde_json::json!({"output":"完成🚀", "nullable": null})
                    }
                )
                .await
                .map_err(|error| error.to_string())?,
            super::super::RuntimeTaskResolution::Completed
        );
        // Reconstructing the service must preserve output without a local cache.
        let restored = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("proxy-scope")),
        );
        let graph = restored
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing restored graph".to_string())?;
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing restored task".to_string())?;
        assert_eq!(graph.snapshot.revision, 1);
        assert_eq!(task.execution.status, TaskStatus::Completed);
        assert_eq!(
            task.execution.result,
            Some(serde_json::json!({"output":"完成🚀", "nullable":null}))
        );
        assert!(
            !restored
                .request_runtime_task_interruption(
                    "proxy-scope",
                    "a",
                    &claim,
                    super::super::RuntimeInterruptionDisposition::Cancelled
                )
                .await
                .map_err(|error| error.to_string())?
        );
        assert_eq!(store.commits.load(std::sync::atomic::Ordering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_runtime_backend_does_not_fallback_to_a_local_graph() -> Result<(), String>
    {
        let (service, store) = proxy_service(false);
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let graph = service
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing graph".to_string())?;
        assert!(!service.supports_runtime_execution());
        assert_eq!(store.commits.load(std::sync::atomic::Ordering::SeqCst), 0);
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing task".to_string())?;
        assert!(matches!(
            service.claim_runtime_task("proxy-scope", task, 1).await,
            Err(RevisionedTaskStoreError::Unsupported { .. })
        ));
        let actual = store
            .inner
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing authoritative graph".to_string())?;
        assert_eq!(
            actual
                .snapshot
                .tasks
                .first()
                .map(|task| &task.execution.status),
            Some(&TaskStatus::Pending)
        );
        Ok(())
    }

    #[tokio::test]
    async fn execution_cas_rejects_a_lost_relation_revision_and_runtime_state() -> Result<(), String>
    {
        let (service, store) = proxy_service(true);
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let graph = store
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing graph".to_string())?;
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing task".to_string())?;
        let stale_execution = TaskExecutionCommit {
            expected_revision: 1,
            expected_executions: execution_map(&graph),
            next: graph.clone(),
        };
        service
            .claim_runtime_task("proxy-scope", task, 1)
            .await
            .map_err(|error| error.to_string())?;
        assert!(matches!(
            store
                .compare_and_commit_execution("proxy-scope", stale_execution.clone())
                .await,
            Err(RevisionedTaskStoreError::Conflict { .. })
        ));
        let current = store
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing claimed graph".to_string())?;
        let mut next = current.clone();
        next.snapshot.revision = 2;
        store
            .compare_and_commit(
                "proxy-scope",
                TaskGraphCommit {
                    expected_revision: Some(1),
                    expected_executions: Some(execution_map(&current)),
                    next,
                    reason: "interleaved relation patch".to_string(),
                    effects: TaskPatchEffects::default(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        assert!(matches!(
            store
                .compare_and_commit_execution("proxy-scope", stale_execution)
                .await,
            Err(RevisionedTaskStoreError::Conflict { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn durable_exact_pause_wins_over_late_result_and_survives_service_reconstruction()
    -> Result<(), String> {
        let (service, store) = proxy_service(true);
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let graph = service
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing graph".to_string())?;
        let task = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing task".to_string())?;
        let RuntimeTaskClaimOutcome::Claimed(claim) = service
            .claim_runtime_task("proxy-scope", task, 1)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err("claim rejected".to_string());
        };
        let disposition = super::super::RuntimeInterruptionDisposition::Paused {
            reason: "requested pause".to_string(),
        };
        assert!(
            service
                .request_runtime_task_interruption("proxy-scope", "a", &claim, disposition.clone())
                .await
                .map_err(|error| error.to_string())?
        );
        let restored =
            TaskRevisionService::new(store, Arc::new(DefaultTaskToolPolicy::new("proxy-scope")));
        assert_eq!(
            restored
                .settle_runtime_resolution(
                    "proxy-scope",
                    "a",
                    &claim,
                    super::super::RuntimeTaskResolutionRequest::CompletedWithResult {
                        result: serde_json::json!("late result")
                    }
                )
                .await
                .map_err(|error| error.to_string())?,
            super::super::RuntimeTaskResolution::Interrupted { disposition }
        );
        let graph = restored
            .load("proxy-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "missing settled graph".to_string())?;
        let paused = graph
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "missing paused task".to_string())?;
        assert_eq!(
            paused.execution.status,
            TaskStatus::Paused("requested pause".to_string())
        );
        assert_eq!(paused.execution.result, None);
        assert_eq!(paused.execution.claim, None);
        assert_eq!(
            restored
                .resume_runtime_task("proxy-scope", paused, 1)
                .await
                .map_err(|error| error.to_string())?,
            super::super::RuntimeTaskResumeOutcome::Resumed
        );
        Ok(())
    }

    struct ClaimBeforePatchCommitStore {
        inner: Arc<InMemoryRevisionedTaskStore>,
    }

    #[async_trait]
    impl RevisionedTaskStore for ClaimBeforePatchCommitStore {
        async fn load(
            &self,
            scope_id: &str,
        ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError> {
            self.inner.load(scope_id).await
        }

        async fn compare_and_commit(
            &self,
            scope_id: &str,
            commit: TaskGraphCommit,
        ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError> {
            if commit.expected_revision.is_some() {
                let current = self.inner.load(scope_id).await?.ok_or_else(|| {
                    RevisionedTaskStoreError::Backend {
                        message: "interleaving store could not load the patch graph".to_string(),
                    }
                })?;
                let expected = commit.expected_executions.as_ref().ok_or_else(|| {
                    RevisionedTaskStoreError::Backend {
                        message: "canonical patch omitted its execution precondition".to_string(),
                    }
                })?;
                let current_executions: BTreeMap<TaskId, TaskExecution> = current
                    .snapshot
                    .tasks
                    .iter()
                    .map(|task| (task.spec.id.clone(), task.execution.clone()))
                    .collect();
                if expected != &current_executions {
                    return Err(RevisionedTaskStoreError::Backend {
                        message: "canonical patch did not preserve its loaded execution snapshot"
                            .to_string(),
                    });
                }
                let task = current.snapshot.tasks.first().ok_or_else(|| {
                    RevisionedTaskStoreError::Backend {
                        message: "interleaving store could not find the patch task".to_string(),
                    }
                })?;
                if matches!(
                    self.inner
                        .claim_runtime_task(scope_id, task, current.snapshot.revision)
                        .await?,
                    RuntimeTaskClaimOutcome::ReloadSnapshot
                ) {
                    return Err(RevisionedTaskStoreError::Backend {
                        message: "interleaving claim unexpectedly requested a reload".to_string(),
                    });
                }
            }
            self.inner.compare_and_commit(scope_id, commit).await
        }
    }

    fn commit_from_loaded(
        loaded: &RevisionedTaskGraph,
        operation: TaskPlanPatchOp,
    ) -> Result<TaskGraphCommit, String> {
        let application =
            TaskPatchEngine::apply_operations(&loaded.snapshot.tasks, vec![operation], true)
                .map_err(|error| error.to_string())?;
        let revision = loaded
            .snapshot
            .revision
            .checked_add(1)
            .ok_or_else(|| "test graph revision overflow".to_string())?;
        Ok(TaskGraphCommit {
            expected_revision: Some(loaded.snapshot.revision),
            expected_executions: Some(
                loaded
                    .snapshot
                    .tasks
                    .iter()
                    .map(|task| (task.spec.id.clone(), task.execution.clone()))
                    .collect(),
            ),
            next: RevisionedTaskGraph {
                snapshot: RuntimePlanSnapshot {
                    revision,
                    tasks: application.tasks,
                },
                context: loaded.context.clone(),
            },
            reason: "deterministic execution CAS test".to_string(),
            effects: application.effects,
        })
    }

    async fn assert_claim_conflicts_with_patch(
        scope_id: &str,
        operation: TaskPlanPatchOp,
    ) -> Result<(), String> {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new(scope_id)),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let loaded = store
            .load(scope_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "claim conflict graph is missing".to_string())?;
        let commit = commit_from_loaded(&loaded, operation)?;
        let task = loaded
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "claim conflict task is missing".to_string())?;
        let claim = match store
            .claim_runtime_task(scope_id, task, loaded.snapshot.revision)
            .await
            .map_err(|error| error.to_string())?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err("claim conflict task unexpectedly requested reload".to_string());
            }
        };
        let error = store
            .compare_and_commit(scope_id, commit)
            .await
            .err()
            .ok_or_else(|| "relation patch overwrote a concurrent claim".to_string())?;
        assert!(matches!(error, RevisionedTaskStoreError::Conflict { .. }));
        assert_eq!(
            store
                .settle_runtime_claim(scope_id, "a", &claim, TaskStatus::Completed)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskSettlementOutcome::Settled {
                status: TaskStatus::Completed
            }
        );
        Ok(())
    }

    #[test]
    fn extension_patch_merges_without_losing_product_fields() {
        let mut spec = TaskSpec {
            id: "task-1".to_string(),
            title: "Task".to_string(),
            description: "Do work".to_string(),
            depends_on: Vec::new(),
            max_retries: 3,
            extension: serde_json::json!({
                "kind": "implementation",
                "review": { "required": true, "role": "reviewer" },
            }),
        };
        apply_spec_patch(
            &mut spec,
            TaskSpecPatch {
                extension: Some(serde_json::json!({
                    "review": { "role": "senior-reviewer" },
                })),
                ..TaskSpecPatch::default()
            },
        );

        assert_eq!(
            spec.extension.get("kind"),
            Some(&serde_json::json!("implementation"))
        );
        assert_eq!(
            spec.extension.pointer("/review/required"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            spec.extension.pointer("/review/role"),
            Some(&serde_json::json!("senior-reviewer"))
        );
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TypedDraftExtension {
        role: String,
        priority: u32,
    }

    #[test]
    fn draft_and_patch_extensions_round_trip_through_typed_api() -> Result<(), String> {
        let extension = TypedDraftExtension {
            role: "explorer".to_string(),
            priority: 1,
        };
        let draft = draft("task-1", &[]).with_extension(extension.clone())?;
        assert_eq!(draft.extension_as::<TypedDraftExtension>()?, extension);

        let patch = TaskSpecPatch::default().with_extension(extension.clone())?;
        assert_eq!(
            patch.extension_as::<TypedDraftExtension>()?,
            Some(extension)
        );
        Ok(())
    }

    struct PreparationTrackingPolicy {
        delegate: DefaultTaskToolPolicy,
        abort_count: Arc<std::sync::atomic::AtomicUsize>,
        abort_error: Option<String>,
        ensure_error: Option<String>,
        scope_staged: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl TaskToolPolicy for PreparationTrackingPolicy {
        async fn resolve_scope(&self, context: &ToolContext) -> Result<String, TaskPolicyError> {
            self.delegate.resolve_scope(context).await
        }

        async fn ensure_scope(
            &self,
            scope_id: &str,
            input: &TaskCreateInput,
            context: &ToolContext,
        ) -> Result<(), TaskPolicyError> {
            if let Some(message) = &self.ensure_error {
                self.scope_staged
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.scope_staged
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                return Err(TaskPolicyError::Rejected {
                    message: message.clone(),
                });
            }
            self.delegate.ensure_scope(scope_id, input, context).await
        }

        async fn abort_scope_preparation(&self, _scope_id: &str) -> Result<(), TaskPolicyError> {
            self.abort_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &self.abort_error {
                Some(message) => Err(TaskPolicyError::Backend {
                    message: message.clone(),
                }),
                None => Ok(()),
            }
        }

        async fn prepare_task(
            &self,
            scope_id: &str,
            draft: &TaskDraft,
            position: usize,
        ) -> Result<PreparedTaskPolicy, TaskPolicyError> {
            self.delegate.prepare_task(scope_id, draft, position).await
        }

        async fn prepare_initial_context(
            &self,
            scope_id: &str,
            input: &TaskCreateInput,
        ) -> Result<TaskGraphContext, TaskPolicyError> {
            self.delegate.prepare_initial_context(scope_id, input).await
        }

        async fn finalize_task_extension(
            &self,
            scope_id: &str,
            task_id: &str,
            position: usize,
            extension: serde_json::Value,
        ) -> Result<serde_json::Value, TaskPolicyError> {
            self.delegate
                .finalize_task_extension(scope_id, task_id, position, extension)
                .await
        }

        async fn validate_candidate(
            &self,
            scope_id: &str,
            tasks: &[Task],
        ) -> Result<(), TaskPolicyError> {
            self.delegate.validate_candidate(scope_id, tasks).await
        }
    }

    fn preparation_tracking_service(
        ensure_error: Option<String>,
        abort_error: Option<String>,
    ) -> (
        TaskRevisionService,
        Arc<InMemoryRevisionedTaskStore>,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let abort_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scope_staged = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let policy = PreparationTrackingPolicy {
            delegate: DefaultTaskToolPolicy::new("preparation-scope"),
            abort_count: abort_count.clone(),
            abort_error,
            ensure_error,
            scope_staged: scope_staged.clone(),
        };
        (
            TaskRevisionService::new(store.clone(), Arc::new(policy)),
            store,
            abort_count,
            scope_staged,
        )
    }

    #[tokio::test]
    async fn ensure_scope_failure_self_cleans_without_invoking_abort() -> Result<(), String> {
        let (service, store, abort_count, scope_staged) =
            preparation_tracking_service(Some("scope staging failed".to_string()), None);
        let error = service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .err()
            .ok_or_else(|| "scope preparation unexpectedly succeeded".to_string())?;
        assert!(matches!(error, TaskRevisionError::PolicyRejected { .. }));
        assert!(!scope_staged.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(abort_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(
            store
                .load("preparation-scope")
                .await
                .map_err(|load_error| load_error.to_string())?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn failed_creation_aborts_scope_preparation_without_publishing_graph()
    -> Result<(), String> {
        let (service, store, abort_count, _scope_staged) = preparation_tracking_service(None, None);
        let error = service
            .create_from_tool(
                create_input(vec![draft("a", &["missing"])]),
                &ToolContext::default(),
            )
            .await
            .err()
            .ok_or_else(|| "invalid graph unexpectedly committed".to_string())?;
        assert!(matches!(error, TaskRevisionError::InvalidPatch { .. }));
        assert_eq!(abort_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            store
                .load("preparation-scope")
                .await
                .map_err(|load_error| load_error.to_string())?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn scope_preparation_rollback_failure_preserves_both_errors() -> Result<(), String> {
        let (service, store, abort_count, _scope_staged) =
            preparation_tracking_service(None, Some("staging cleanup refused".to_string()));
        let error = service
            .create_from_tool(
                create_input(vec![draft("a", &["missing"])]),
                &ToolContext::default(),
            )
            .await
            .err()
            .ok_or_else(|| "invalid graph unexpectedly committed".to_string())?;
        let TaskRevisionError::Backend { message } = error else {
            return Err("rollback failure did not surface as backend error".to_string());
        };
        assert!(message.contains("missing"));
        assert!(message.contains("staging cleanup refused"));
        assert_eq!(abort_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            store
                .load("preparation-scope")
                .await
                .map_err(|load_error| load_error.to_string())?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn creates_and_patches_one_canonical_graph() -> Result<(), String> {
        let service = service();
        let context = ToolContext::default();
        let created = service
            .create_from_tool(create_input(vec![draft("a", &[])]), &context)
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(created.graph.snapshot.revision, 1);

        let updated = service
            .update_from_tool(
                TaskUpdateInput {
                    base_revision: 1,
                    reason: "add dependency".to_string(),
                    operations: vec![TaskPlanPatchInputOp::Insert {
                        after_task_id: Some("a".to_string()),
                        task: draft("b", &["a"]),
                    }],
                },
                &context,
            )
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(updated.snapshot.revision, 2);
        assert_eq!(updated.snapshot.tasks.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn stale_patch_reports_conflict() -> Result<(), String> {
        let service = service();
        let context = ToolContext::default();
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &context)
            .await
            .map_err(|error| error.to_string())?;
        let error = service
            .apply_patch(
                "test-scope",
                TaskPlanPatch {
                    base_revision: 2,
                    reason: "stale".to_string(),
                    operations: vec![TaskPlanPatchOp::Skip {
                        task_id: "a".to_string(),
                    }],
                },
            )
            .await
            .err()
            .ok_or_else(|| "stale patch unexpectedly succeeded".to_string())?;
        assert!(matches!(error, TaskRevisionError::RevisionConflict { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn default_policy_supports_manual_progress() -> Result<(), String> {
        let service = service();
        let context = ToolContext::default();
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &context)
            .await
            .map_err(|error| error.to_string())?;
        let running = service
            .apply_patch(
                "test-scope",
                TaskPlanPatch {
                    base_revision: 1,
                    reason: "start".to_string(),
                    operations: vec![TaskPlanPatchOp::SetStatus {
                        task_id: "a".to_string(),
                        status: TaskStatus::Running,
                    }],
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let completed = service
            .apply_patch(
                "test-scope",
                TaskPlanPatch {
                    base_revision: running.snapshot.revision,
                    reason: "finish".to_string(),
                    operations: vec![TaskPlanPatchOp::SetStatus {
                        task_id: "a".to_string(),
                        status: TaskStatus::Completed,
                    }],
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let task = completed
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "completed graph lost its task".to_string())?;
        assert_eq!(task.execution.status, TaskStatus::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn in_memory_store_applies_explicit_retry_with_exact_snapshot_cas() -> Result<(), String>
    {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("retry-scope")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let pending = store
            .load("retry-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "pending retry graph is missing".to_string())?;
        let pending_task = pending
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "pending retry task is missing".to_string())?;
        let claim = match store
            .claim_runtime_task("retry-scope", pending_task, pending.snapshot.revision)
            .await
            .map_err(|error| error.to_string())?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err("pending retry task requested reload".to_string());
            }
        };
        assert_eq!(
            store
                .settle_runtime_claim(
                    "retry-scope",
                    "a",
                    &claim,
                    TaskStatus::Blocked("acceptance pending".to_string()),
                )
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskSettlementOutcome::Settled {
                status: TaskStatus::Blocked("acceptance pending".to_string())
            }
        );
        let blocked = store
            .load("retry-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "retry graph is missing".to_string())?;
        let expected = blocked
            .snapshot
            .tasks
            .first()
            .cloned()
            .ok_or_else(|| "retry task is missing".to_string())?;

        assert_eq!(
            store
                .retry_runtime_task("retry-scope", &expected, blocked.snapshot.revision)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskRetryOutcome::Retried { retry_count: 1 }
        );
        assert_eq!(
            store
                .retry_runtime_task("retry-scope", &expected, blocked.snapshot.revision)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskRetryOutcome::Superseded
        );
        let retried = store
            .load("retry-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "retried graph is missing".to_string())?;
        let task = retried
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "retried task is missing".to_string())?;
        assert_eq!(task.execution.status, TaskStatus::Pending);
        assert_eq!(task.execution.retry_count, 1);
        assert!(task.execution.claim.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn idempotent_running_patch_preserves_live_physical_claim() -> Result<(), String> {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("claim-preservation")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let pending = store
            .load("claim-preservation")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "claim preservation graph is missing".to_string())?;
        let task = pending
            .snapshot
            .tasks
            .first()
            .cloned()
            .ok_or_else(|| "claim preservation task is missing".to_string())?;
        let claim = match store
            .claim_runtime_task("claim-preservation", &task, pending.snapshot.revision)
            .await
            .map_err(|error| error.to_string())?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err("claim preservation unexpectedly reloaded".to_string());
            }
        };

        let patched = service
            .apply_patch(
                "claim-preservation",
                TaskPlanPatch {
                    base_revision: pending.snapshot.revision,
                    reason: "idempotent running observation".to_string(),
                    operations: vec![TaskPlanPatchOp::SetStatus {
                        task_id: "a".to_string(),
                        status: TaskStatus::Running,
                    }],
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let current = patched
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "patched claim task is missing".to_string())?;
        assert_eq!(current.execution.claim.as_ref(), Some(&claim));
        assert_eq!(
            store
                .settle_runtime_claim("claim-preservation", "a", &claim, TaskStatus::Completed,)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskSettlementOutcome::Settled {
                status: TaskStatus::Completed
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn relation_patch_loaded_before_claim_cannot_overwrite_live_claim() -> Result<(), String>
    {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("claim-patch-cas")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;

        let loaded = store
            .load("claim-patch-cas")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "claim patch graph is missing".to_string())?;
        let application = TaskPatchEngine::apply_operations(
            &loaded.snapshot.tasks,
            vec![TaskPlanPatchOp::Skip {
                task_id: "a".to_string(),
            }],
            true,
        )
        .map_err(|error| error.to_string())?;
        let next = RevisionedTaskGraph {
            snapshot: RuntimePlanSnapshot {
                revision: loaded.snapshot.revision.saturating_add(1),
                tasks: application.tasks,
            },
            context: loaded.context.clone(),
        };
        let task = loaded
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "claim patch task is missing".to_string())?;
        let claim = match store
            .claim_runtime_task("claim-patch-cas", task, loaded.snapshot.revision)
            .await
            .map_err(|error| error.to_string())?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err("claim patch task unexpectedly requested reload".to_string());
            }
        };

        let error = store
            .compare_and_commit(
                "claim-patch-cas",
                TaskGraphCommit {
                    expected_revision: Some(loaded.snapshot.revision),
                    expected_executions: Some(
                        loaded
                            .snapshot
                            .tasks
                            .iter()
                            .map(|task| (task.spec.id.clone(), task.execution.clone()))
                            .collect(),
                    ),
                    next,
                    reason: "skip stale loaded task".to_string(),
                    effects: application.effects,
                },
            )
            .await
            .err()
            .ok_or_else(|| "stale relation patch overwrote a live claim".to_string())?;
        assert!(matches!(error, RevisionedTaskStoreError::Conflict { .. }));

        let current = store
            .load("claim-patch-cas")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "claimed graph disappeared".to_string())?;
        let current_task = current
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "claimed task disappeared".to_string())?;
        assert_eq!(current_task.execution.claim.as_ref(), Some(&claim));
        Ok(())
    }

    #[tokio::test]
    async fn canonical_patch_carries_loaded_execution_precondition() -> Result<(), String> {
        let inner = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            Arc::new(ClaimBeforePatchCommitStore {
                inner: inner.clone(),
            }),
            Arc::new(DefaultTaskToolPolicy::new("canonical-patch-cas")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;

        let error = service
            .apply_patch(
                "canonical-patch-cas",
                TaskPlanPatch {
                    base_revision: 1,
                    reason: "exercise canonical execution precondition".to_string(),
                    operations: vec![TaskPlanPatchOp::Skip {
                        task_id: "a".to_string(),
                    }],
                },
            )
            .await
            .err()
            .ok_or_else(|| "canonical patch overwrote the interleaved claim".to_string())?;
        assert!(matches!(error, TaskRevisionError::RevisionConflict { .. }));

        let current = inner
            .load("canonical-patch-cas")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "canonical patch graph disappeared".to_string())?;
        let task = current
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "canonical patch task disappeared".to_string())?;
        assert_eq!(task.execution.status, TaskStatus::Running);
        assert!(task.execution.claim.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn set_status_and_spec_update_conflict_with_concurrent_claim() -> Result<(), String> {
        assert_claim_conflicts_with_patch(
            "claim-set-status-cas",
            TaskPlanPatchOp::SetStatus {
                task_id: "a".to_string(),
                status: TaskStatus::Running,
            },
        )
        .await?;
        assert_claim_conflicts_with_patch(
            "claim-spec-update-cas",
            TaskPlanPatchOp::Update {
                task_id: "a".to_string(),
                patch: TaskSpecPatch {
                    title: Some("Updated after stale load".to_string()),
                    ..TaskSpecPatch::default()
                },
            },
        )
        .await
    }

    #[tokio::test]
    async fn relation_patch_conflicts_with_runtime_settlement_and_retry() -> Result<(), String> {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("settlement-patch-cas")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        let pending = store
            .load("settlement-patch-cas")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "settlement graph is missing".to_string())?;
        let stale_skip = commit_from_loaded(
            &pending,
            TaskPlanPatchOp::Skip {
                task_id: "a".to_string(),
            },
        )?;
        let pending_task = pending
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "settlement task is missing".to_string())?;
        let claim = match store
            .claim_runtime_task(
                "settlement-patch-cas",
                pending_task,
                pending.snapshot.revision,
            )
            .await
            .map_err(|error| error.to_string())?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err("settlement task unexpectedly requested reload".to_string());
            }
        };
        assert_eq!(
            store
                .settle_runtime_claim("settlement-patch-cas", "a", &claim, TaskStatus::Completed,)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskSettlementOutcome::Settled {
                status: TaskStatus::Completed
            }
        );
        let settlement_error = store
            .compare_and_commit("settlement-patch-cas", stale_skip)
            .await
            .err()
            .ok_or_else(|| "relation patch overwrote runtime settlement".to_string())?;
        assert!(matches!(
            settlement_error,
            RevisionedTaskStoreError::Conflict { .. }
        ));

        let retry_store = Arc::new(InMemoryRevisionedTaskStore::new());
        let retry_service = TaskRevisionService::new(
            retry_store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("retry-patch-cas")),
        );
        retry_service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        {
            let mut graphs = retry_store.graphs.write().await;
            let graph = graphs
                .get_mut("retry-patch-cas")
                .ok_or_else(|| "retry graph is missing".to_string())?;
            let task = graph
                .snapshot
                .tasks
                .first_mut()
                .ok_or_else(|| "retry task is missing".to_string())?;
            task.execution.status = TaskStatus::Blocked("retry".to_string());
        }
        let blocked = retry_store
            .load("retry-patch-cas")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "blocked graph is missing".to_string())?;
        let stale_update = commit_from_loaded(
            &blocked,
            TaskPlanPatchOp::Update {
                task_id: "a".to_string(),
                patch: TaskSpecPatch {
                    title: Some("Updated after retry".to_string()),
                    ..TaskSpecPatch::default()
                },
            },
        )?;
        let blocked_task = blocked
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "blocked retry task is missing".to_string())?;
        assert_eq!(
            retry_store
                .retry_runtime_task("retry-patch-cas", blocked_task, blocked.snapshot.revision)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskRetryOutcome::Retried { retry_count: 1 }
        );
        let retry_error = retry_store
            .compare_and_commit("retry-patch-cas", stale_update)
            .await
            .err()
            .ok_or_else(|| "relation patch overwrote runtime retry".to_string())?;
        assert!(matches!(
            retry_error,
            RevisionedTaskStoreError::Conflict { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn patch_cannot_reduce_retry_budget_below_consumed_retries() -> Result<(), String> {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("retry-budget")),
        );
        service
            .create_from_tool(create_input(vec![draft("a", &[])]), &ToolContext::default())
            .await
            .map_err(|error| error.to_string())?;
        {
            let mut graphs = store.graphs.write().await;
            let graph = graphs
                .get_mut("retry-budget")
                .ok_or_else(|| "retry budget graph is missing".to_string())?;
            let task = graph
                .snapshot
                .tasks
                .first_mut()
                .ok_or_else(|| "retry budget task is missing".to_string())?;
            task.execution.status = TaskStatus::Blocked("review".to_string());
            task.execution.retry_count = 2;
        }

        let error = service
            .apply_patch(
                "retry-budget",
                TaskPlanPatch {
                    base_revision: 1,
                    reason: "invalid retry budget reduction".to_string(),
                    operations: vec![TaskPlanPatchOp::Update {
                        task_id: "a".to_string(),
                        patch: TaskSpecPatch {
                            max_retries: Some(1),
                            ..TaskSpecPatch::default()
                        },
                    }],
                },
            )
            .await
            .err()
            .ok_or_else(|| "invalid retry budget patch unexpectedly committed".to_string())?;
        assert!(
            error
                .to_string()
                .contains("retry_count 2 exceeds max_retries 1")
        );
        let current = store
            .load("retry-budget")
            .await
            .map_err(|load_error| load_error.to_string())?
            .ok_or_else(|| "retry budget graph disappeared".to_string())?;
        let task = current
            .snapshot
            .tasks
            .first()
            .ok_or_else(|| "retry budget task disappeared".to_string())?;
        assert_eq!(task.spec.max_retries, 3);
        assert_eq!(task.execution.retry_count, 2);
        assert_eq!(
            task.execution.status,
            TaskStatus::Blocked("review".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn runtime_claims_are_parallel_aba_safe_and_relation_commits_detect_drift()
    -> Result<(), String> {
        let store = Arc::new(InMemoryRevisionedTaskStore::new());
        let service = TaskRevisionService::new(
            store.clone(),
            Arc::new(DefaultTaskToolPolicy::new("runtime-scope")),
        );
        service
            .create_from_tool(
                create_input(vec![draft("a", &[]), draft("b", &[])]),
                &ToolContext::default(),
            )
            .await
            .map_err(|error| error.to_string())?;
        let stale = store
            .load("runtime-scope")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "runtime graph missing".to_string())?;
        let task_a = stale
            .snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "a")
            .ok_or_else(|| "task a missing".to_string())?;
        let task_b = stale
            .snapshot
            .tasks
            .iter()
            .find(|task| task.spec.id == "b")
            .ok_or_else(|| "task b missing".to_string())?;
        let claim_a = match store
            .claim_runtime_task("runtime-scope", task_a, stale.snapshot.revision)
            .await
            .map_err(|error| error.to_string())?
        {
            RuntimeTaskClaimOutcome::Claimed(claim) => claim,
            RuntimeTaskClaimOutcome::ReloadSnapshot => {
                return Err("first runtime claim requested reload".to_string());
            }
        };
        assert!(matches!(
            store
                .claim_runtime_task("runtime-scope", task_b, stale.snapshot.revision)
                .await
                .map_err(|error| error.to_string())?,
            RuntimeTaskClaimOutcome::Claimed(_)
        ));
        assert_eq!(
            store
                .settle_runtime_claim("runtime-scope", "a", &claim_a, TaskStatus::Completed,)
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskSettlementOutcome::Settled {
                status: TaskStatus::Completed
            }
        );
        assert_eq!(
            store
                .settle_runtime_claim(
                    "runtime-scope",
                    "a",
                    &claim_a,
                    TaskStatus::Failed("late result".to_string()),
                )
                .await
                .map_err(|error| error.to_string())?,
            crate::tasks::RuntimeTaskSettlementOutcome::Superseded
        );

        let mut stale_next = stale.clone();
        stale_next.snapshot.revision = 2;
        let stale_task = stale_next
            .snapshot
            .tasks
            .iter_mut()
            .find(|task| task.spec.id == "a")
            .ok_or_else(|| "stale task a missing".to_string())?;
        stale_task.spec.title = "stale title update".to_string();
        let error = store
            .compare_and_commit(
                "runtime-scope",
                TaskGraphCommit {
                    expected_revision: Some(1),
                    expected_executions: None,
                    next: stale_next,
                    reason: "stale relation edit".to_string(),
                    effects: TaskPatchEffects {
                        updated_task_ids: vec!["a".to_string()],
                        ..TaskPatchEffects::default()
                    },
                },
            )
            .await
            .err()
            .ok_or_else(|| "stale relation commit overwrote runtime claim".to_string())?;
        assert!(matches!(error, RevisionedTaskStoreError::Conflict { .. }));
        Ok(())
    }
}
