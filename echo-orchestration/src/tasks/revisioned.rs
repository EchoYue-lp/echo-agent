//! Revisioned task-relation service shared by lightweight progress tracking and
//! executable task graphs.
//!
//! The framework owns patch semantics, structural validation, revisions, and
//! optimistic concurrency. Applications provide persistence and product policy
//! through narrow adapters.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use echo_core::tools::ToolContext;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{
    PlanValidator, RuntimePlanSnapshot, Task, TaskExecution, TaskId, TaskKind, TaskSpec, TaskStatus,
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
#[derive(Debug, Clone)]
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
    pub kind: TaskKind,
    pub subagent: Option<String>,
    pub depends_on: Vec<TaskId>,
    pub files: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub required_artifacts: Vec<String>,
    pub execution_checks: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub max_retries: u32,
    pub extensions: serde_json::Value,
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
    pub kind: Option<TaskKind>,
    pub agent_role: Option<String>,
    pub depends_on: Option<Vec<TaskId>>,
    pub files: Option<Vec<String>>,
    pub allowed_tools: Option<Vec<String>>,
    pub required_artifacts: Option<Vec<String>>,
    pub execution_checks: Option<Vec<String>>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub max_retries: Option<u32>,
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
#[derive(Debug, Clone)]
pub struct TaskGraphCommit {
    pub expected_revision: Option<u64>,
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

/// Persistence boundary for revisioned task graphs.
#[async_trait]
pub trait RevisionedTaskStore: Send + Sync {
    async fn load(
        &self,
        scope_id: &str,
    ) -> Result<Option<RevisionedTaskGraph>, RevisionedTaskStoreError>;

    async fn compare_and_commit(
        &self,
        scope_id: &str,
        commit: TaskGraphCommit,
    ) -> Result<RevisionedTaskGraph, RevisionedTaskStoreError>;
}

/// Product defaults produced without allowing policy to rewrite generic task
/// fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTaskPolicy {
    pub agent_role: String,
    pub metadata: serde_json::Value,
}

/// Product policy boundary used by the task tools and revision service.
#[async_trait]
pub trait TaskToolPolicy: Send + Sync {
    fn task_input_schema_extensions(&self) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }

    fn allow_manual_progress_updates(&self) -> bool {
        false
    }

    async fn resolve_scope(&self, context: &ToolContext) -> Result<String, TaskPolicyError>;

    async fn ensure_scope(
        &self,
        scope_id: &str,
        input: &TaskCreateInput,
        context: &ToolContext,
    ) -> Result<(), TaskPolicyError>;

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

    async fn finalize_task_metadata(
        &self,
        scope_id: &str,
        task_id: &str,
        position: usize,
        metadata: serde_json::Value,
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

    async fn prepare_task(
        &self,
        _scope_id: &str,
        draft: &TaskDraft,
        _position: usize,
    ) -> Result<PreparedTaskPolicy, TaskPolicyError> {
        Ok(PreparedTaskPolicy {
            agent_role: draft
                .subagent
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            metadata: draft.extensions.clone(),
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

    async fn finalize_task_metadata(
        &self,
        _scope_id: &str,
        _task_id: &str,
        _position: usize,
        metadata: serde_json::Value,
    ) -> Result<serde_json::Value, TaskPolicyError> {
        Ok(metadata)
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
}

#[async_trait]
impl RevisionedTaskStore for InMemoryRevisionedTaskStore {
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
                    }
                    task.execution.claim = None;
                    effects.progressed_task_ids.push(task_id);
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
    if let Some(kind) = patch.kind {
        spec.kind = kind;
    }
    if let Some(agent_role) = patch.agent_role {
        spec.agent_role = agent_role;
    }
    if let Some(depends_on) = patch.depends_on {
        spec.depends_on = depends_on;
    }
    if let Some(files) = patch.files {
        spec.files = files;
    }
    if let Some(allowed_tools) = patch.allowed_tools {
        spec.allowed_tools = allowed_tools;
    }
    if let Some(required_artifacts) = patch.required_artifacts {
        spec.required_artifacts = required_artifacts;
    }
    if let Some(execution_checks) = patch.execution_checks {
        spec.execution_checks = execution_checks;
    }
    if let Some(acceptance_criteria) = patch.acceptance_criteria {
        spec.acceptance_criteria = acceptance_criteria;
    }
    if let Some(max_retries) = patch.max_retries {
        spec.max_retries = max_retries;
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
    pub fn new(store: Arc<dyn RevisionedTaskStore>, policy: Arc<dyn TaskToolPolicy>) -> Self {
        Self {
            store,
            policy,
            validator: PlanValidator::default(),
        }
    }

    pub fn with_validator(mut self, validator: PlanValidator) -> Self {
        self.validator = validator;
        self
    }

    pub fn task_input_schema_extensions(&self) -> serde_json::Map<String, serde_json::Value> {
        self.policy.task_input_schema_extensions()
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
        let current = self.load(&scope_id).await?;
        let start_position = current
            .as_ref()
            .map(|graph| graph.snapshot.tasks.len())
            .unwrap_or(0);
        let mut prepared_tasks = Vec::with_capacity(input.tasks.len());
        for (offset, draft) in input.tasks.iter().enumerate() {
            let position = start_position.saturating_add(offset);
            let prepared = self
                .policy
                .prepare_task(&scope_id, draft, position)
                .await
                .map_err(TaskRevisionError::from)?;
            prepared_tasks.push(TaskSpec {
                id: draft.id.clone(),
                title: draft.title.clone(),
                description: draft.description.clone(),
                kind: draft.kind,
                agent_role: prepared.agent_role,
                depends_on: draft.depends_on.clone(),
                files: draft.files.clone(),
                allowed_tools: draft.allowed_tools.clone(),
                required_artifacts: draft.required_artifacts.clone(),
                execution_checks: draft.execution_checks.clone(),
                acceptance_criteria: draft.acceptance_criteria.clone(),
                max_retries: draft.max_retries,
                metadata: prepared.metadata,
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
                    &scope_id,
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
            .prepare_initial_context(&scope_id, &input)
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
                &scope_id,
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
                            kind: task.kind,
                            agent_role: prepared.agent_role,
                            depends_on: task.depends_on,
                            files: task.files,
                            allowed_tools: task.allowed_tools,
                            required_artifacts: task.required_artifacts,
                            execution_checks: task.execution_checks,
                            acceptance_criteria: task.acceptance_criteria,
                            max_retries: task.max_retries,
                            metadata: prepared.metadata,
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
        let tasks = self.finalize_and_validate(scope_id, tasks).await?;
        let graph = RevisionedTaskGraph {
            snapshot: RuntimePlanSnapshot { revision: 1, tasks },
            context,
        };
        self.store
            .compare_and_commit(
                scope_id,
                TaskGraphCommit {
                    expected_revision: None,
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
        let tasks = self
            .finalize_and_validate(scope_id, application.tasks)
            .await?;
        let next_revision = current.snapshot.revision.checked_add(1).ok_or_else(|| {
            TaskRevisionError::InvalidPatch {
                message: "task graph revision overflow".to_string(),
            }
        })?;
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
    ) -> Result<Vec<Task>, TaskRevisionError> {
        for (position, task) in tasks.iter_mut().enumerate() {
            let metadata = std::mem::take(&mut task.spec.metadata);
            task.spec.metadata = self
                .policy
                .finalize_task_metadata(scope_id, &task.spec.id, position, metadata)
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
            kind: TaskKind::Implementation,
            subagent: None,
            depends_on: depends_on.iter().map(|item| (*item).to_string()).collect(),
            files: Vec::new(),
            allowed_tools: Vec::new(),
            required_artifacts: Vec::new(),
            execution_checks: vec!["check".to_string()],
            acceptance_criteria: vec!["accepted".to_string()],
            max_retries: 3,
            extensions: serde_json::Value::Null,
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
}
