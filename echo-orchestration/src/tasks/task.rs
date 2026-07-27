//! Task definitions

use serde::{Deserialize, Serialize};

// ── Enhanced Task Types (Step 1) ──────────────────────────────────────────────

/// Task type classification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskType {
    /// Discovery: search, read, analyze
    Discovery,
    /// Implementation: modify, create, implement
    #[default]
    Implementation,
    /// Verification: test, verify, check
    Verification,
    /// Background: long-running background task
    Background,
    /// Delegation: delegate to sub-agent
    Delegation,
}

/// Task input specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskInput {
    pub name: String,
    pub input_type: InputType,
    pub source: String,
    pub required: bool,
}

/// Input type classification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    File,
    Data,
    Artifact,
    Context,
}

/// Task output specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOutput {
    pub name: String,
    pub output_type: OutputType,
    pub target: String,
    pub validation: Option<String>,
}

/// Output type classification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    File,
    Artifact,
    Result,
    Status,
}

/// Context scope for task execution
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ContextScope {
    /// Only include task description
    Minimal,
    /// Include relevant files/artifacts
    #[default]
    Relevant,
    /// Inherit parent context
    Full,
    /// Completely isolated
    Isolated,
}

/// Risk level classification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RiskLevel {
    /// Read-only, no side effects
    Low,
    /// Write operations, reversible
    #[default]
    Medium,
    /// Write operations, irreversible, requires verification
    High,
}

/// Verification specification
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationSpec {
    #[serde(default)]
    pub verification_type: VerificationType,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub expected: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub fallback_on_failure: FallbackStrategy,
}

fn default_timeout_secs() -> u64 {
    300
}

impl Default for VerificationSpec {
    fn default() -> Self {
        Self {
            verification_type: VerificationType::None,
            command: None,
            expected: None,
            timeout_secs: 300,
            retry_count: 0,
            fallback_on_failure: FallbackStrategy::Abort,
        }
    }
}

/// Verification type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum VerificationType {
    Command,
    FileExists,
    DiffCheck,
    Test,
    HumanReview,
    LlmReview,
    #[default]
    None,
}

/// Fallback strategy on verification failure
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FallbackStrategy {
    Retry,
    Replan,
    AskUser,
    #[default]
    Abort,
}

/// Checkpoint policy
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CheckpointPolicy {
    AfterEach,
    OnMilestone,
    #[default]
    OnFailure,
    Never,
}

/// Task execution attempt record
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskAttempt {
    pub attempt_id: u32,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub status: AttemptStatus,
    pub evidence: Vec<Evidence>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Attempt status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Running,
    Success,
    Failed,
    Timeout,
    Cancelled,
}

/// Evidence record
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub evidence_type: EvidenceType,
    pub content: String,
    pub source: String,
    pub timestamp: u64,
}

/// Evidence type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    CommandOutput,
    FileContent,
    TestResult,
    LlmOutput,
    UserInput,
}

/// File change record
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change_type: ChangeType,
    pub diff: Option<String>,
    pub checksum: Option<String>,
}

/// Change type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

/// Artifact record
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub artifact_type: ArtifactType,
    pub path: String,
    pub size_bytes: u64,
    pub metadata: std::collections::HashMap<String, String>,
}

/// Artifact type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    File,
    Report,
    Model,
    Data,
    Image,
    Other,
}

/// Command execution record
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandRecord {
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Verification result
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationResult {
    pub verification_type: VerificationType,
    pub passed: bool,
    pub output: String,
    pub duration_ms: u64,
    pub retry_count: u32,
}

/// Task state with execution details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskState {
    pub task_id: String,
    pub status: TaskStatus,
    pub evidence: Vec<Evidence>,
    pub changed_files: Vec<FileChange>,
    pub artifacts: Vec<Artifact>,
    pub commands_run: Vec<CommandRecord>,
    pub verification_result: Option<VerificationResult>,
    pub remaining_risks: Vec<String>,
    pub next_unblocked_tasks: Vec<String>,
    pub context_summary: Option<String>,
    pub retry_count: u32,
    pub parent_task_id: Option<String>,
    pub checkpoint_at: u64,
}

/// Task status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Pending
    Pending,
    /// In progress
    InProgress,
    /// Completed
    Completed,
    /// Cancelled
    Cancelled,
    /// Failed
    Failed(String),
    /// Blocked
    Blocked(String),
    /// Timed out
    TimedOut { error: String },
    /// Retrying
    Retrying { attempt: u32, last_error: String },
    /// Deliberately skipped because the task is no longer relevant.
    Skipped,
    /// Paused by the embedding application pending an external decision.
    Paused(String),
}

impl TaskStatus {
    /// Whether this is a terminal state (will not change further)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed
                | TaskStatus::Cancelled
                | TaskStatus::Failed(_)
                | TaskStatus::TimedOut { .. }
                | TaskStatus::Skipped
        )
    }

    /// Whether the state transition is valid
    pub fn can_transition_to(&self, target: &TaskStatus) -> bool {
        match self {
            TaskStatus::Pending => {
                matches!(
                    target,
                    TaskStatus::InProgress
                        | TaskStatus::Cancelled
                        | TaskStatus::Skipped
                        | TaskStatus::Paused(_)
                )
            }
            TaskStatus::InProgress => matches!(
                target,
                TaskStatus::Completed
                    | TaskStatus::Cancelled
                    | TaskStatus::Failed(_)
                    | TaskStatus::TimedOut { .. }
                    | TaskStatus::Retrying { .. }
                    | TaskStatus::Blocked(_)
                    | TaskStatus::Paused(_)
            ),
            TaskStatus::Retrying { .. } => matches!(
                target,
                TaskStatus::Completed
                    | TaskStatus::Cancelled
                    | TaskStatus::Failed(_)
                    | TaskStatus::TimedOut { .. }
                    | TaskStatus::Retrying { .. }
            ),
            TaskStatus::Blocked(_) => matches!(target, TaskStatus::Pending | TaskStatus::Cancelled),
            TaskStatus::Paused(_) => {
                matches!(target, TaskStatus::InProgress | TaskStatus::Cancelled)
            }
            _ => false,
        }
    }

    /// Execute state transition, return new state after validating legality
    ///
    /// If the transition is invalid, return `Err` with detailed error info.
    pub fn transition_to(&self, target: TaskStatus) -> Result<TaskStatus, String> {
        if !self.can_transition_to(&target) {
            return Err(format!(
                "Invalid task state transition: {:?} → {:?}",
                self, target
            ));
        }
        Ok(target)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Task {
    /// Task ID
    pub id: String,
    /// Task description
    pub description: String,
    /// Task status
    pub status: TaskStatus,
    /// List of dependent task IDs
    pub dependencies: Vec<String>,
    /// Priority (0-10, 10 is highest)
    pub priority: u8,
    /// Task result
    pub result: Option<String>,
    /// Execution rationale or notes
    pub reasoning: Option<String>,
    /// Name of the Agent assigned to execute this task
    pub assigned_agent: Option<String>,
    /// Tags (for categorization and filtering)
    pub tags: Vec<String>,
    pub parent_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Task topic/title (for logging and events)
    pub subject: String,
    /// Timeout in seconds, 0 means no timeout
    pub timeout_secs: u64,
    /// Maximum retry count
    pub max_retries: u32,
    /// Current retry count
    pub retry_count: u32,
    /// Optional per-task execution function.
    ///
    /// When set, this overrides the executor's global `execute_fn` for this task.
    /// Not serialized — callers must re-register after deserialization.
    #[serde(skip)]
    pub execute_fn: Option<super::executor::TaskExecuteFn>,

    /// Serializable typed metadata (survives persistence/serialization).
    ///
    /// Application layers can store domain-specific data (e.g., task kind,
    /// pipeline parameters, UI hints) as JSON. Use [`with_metadata`](Self::with_metadata)
    /// to set both this field and the typed [`metadata`](Self::metadata) simultaneously.
    pub metadata_json: Option<serde_json::Value>,

    /// Typed metadata (not serialized — re-register after deserialization).
    ///
    /// Provides zero-cost downcast access to the original typed value.
    /// Paired with [`metadata_json`](Self::metadata_json) for round-tripping.
    #[serde(skip)]
    pub metadata: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,

    // ── Enhanced Fields (Step 1) ────────────────────────────────────────────
    /// Task type classification (discovery/implementation/verification/background/delegation)
    pub task_type: TaskType,

    /// Acceptance criteria - conditions that must be met for task completion
    pub acceptance_criteria: Vec<String>,

    /// Input specifications
    pub inputs: Vec<TaskInput>,

    /// Expected output specifications
    pub expected_outputs: Vec<TaskOutput>,

    /// Allowed tools for this task (None = all tools allowed)
    pub allowed_tools: Option<Vec<String>>,

    /// Context scope for task execution
    pub context_scope: ContextScope,

    /// Risk level classification
    pub risk_level: RiskLevel,

    /// Whether this task can be parallelized with other tasks
    pub can_parallelize: bool,

    /// Whether this task requires write access
    pub requires_write_access: bool,

    /// Verification specification
    pub verification: VerificationSpec,

    /// Checkpoint policy
    pub checkpoint_policy: CheckpointPolicy,

    /// Execution attempt history
    pub attempts: Vec<TaskAttempt>,

    /// Task start timestamp
    pub started_at: Option<u64>,

    /// Task completion timestamp
    pub completed_at: Option<u64>,

    /// Structured error code (for programmatic error handling)
    pub error_code: Option<String>,

    // ── Execution State Fields (Step 4) ──────────────────────────────────────
    /// Evidence collected during task execution
    pub evidence: Vec<Evidence>,

    /// Files changed during task execution
    pub changed_files: Vec<FileChange>,

    /// Artifacts produced during task execution
    pub artifacts: Vec<Artifact>,

    /// Commands executed during task execution
    pub commands_run: Vec<CommandRecord>,

    /// Verification result (if verification was performed)
    pub verification_result: Option<VerificationResult>,

    /// Remaining risks identified during task execution
    pub remaining_risks: Vec<String>,

    /// Tasks that will be unblocked when this task completes
    pub next_unblocked_tasks: Vec<String>,

    /// Context summary for task resumption
    pub context_summary: Option<String>,

    // ── Task metadata fields ────────────────────────────────────────────────
    /// Run ID this task belongs to (run-level isolation).
    /// When Some, this task is scoped to a specific run; when None, it's a
    /// standalone framework-level task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,

    /// Short display title (distinct from the longer `description`, which is
    /// the full task prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Subagent role assigned to execute this task (e.g. "project_explorer",
    /// "code_reviewer"). Used by delegation dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,

    /// Task kind classification: "read_only_review" / "investigation" /
    /// "test_plan" / "implementation" / "debugging" / "verification" /
    /// "review" / "summary". Determines read-only vs mutating dispatch path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Target files for write tasks (file-level write lock conflict detection).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,

    /// Execution result summary (concise, for display + answer composition).
    /// Distinct from `result`, which is the full output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("description", &self.description)
            .field("status", &self.status)
            .field("dependencies", &self.dependencies)
            .field("priority", &self.priority)
            .field("result", &self.result)
            .field("reasoning", &self.reasoning)
            .field("assigned_agent", &self.assigned_agent)
            .field("tags", &self.tags)
            .field("parent_id", &self.parent_id)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("subject", &self.subject)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_retries", &self.max_retries)
            .field("retry_count", &self.retry_count)
            .field(
                "execute_fn",
                &self.execute_fn.as_ref().map(|_| "Some(<fn>)"),
            )
            .field("metadata_json", &self.metadata_json)
            .field("task_type", &self.task_type)
            .field("acceptance_criteria", &self.acceptance_criteria)
            .field("inputs", &self.inputs)
            .field("expected_outputs", &self.expected_outputs)
            .field("allowed_tools", &self.allowed_tools)
            .field("context_scope", &self.context_scope)
            .field("risk_level", &self.risk_level)
            .field("can_parallelize", &self.can_parallelize)
            .field("requires_write_access", &self.requires_write_access)
            .field("verification", &self.verification)
            .field("checkpoint_policy", &self.checkpoint_policy)
            .field("attempts", &self.attempts)
            .field("started_at", &self.started_at)
            .field("completed_at", &self.completed_at)
            .field("error_code", &self.error_code)
            .field("run_id", &self.run_id)
            .field("title", &self.title)
            .field("agent_role", &self.agent_role)
            .field("kind", &self.kind)
            .field("files", &self.files)
            .field("summary", &self.summary)
            .finish()
    }
}

impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.description == other.description
            && self.status == other.status
            && self.dependencies == other.dependencies
            && self.priority == other.priority
            && self.result == other.result
            && self.reasoning == other.reasoning
            && self.assigned_agent == other.assigned_agent
            && self.tags == other.tags
            && self.parent_id == other.parent_id
            && self.created_at == other.created_at
            && self.updated_at == other.updated_at
            && self.subject == other.subject
            && self.timeout_secs == other.timeout_secs
            && self.max_retries == other.max_retries
            && self.retry_count == other.retry_count
            && self.task_type == other.task_type
            && self.acceptance_criteria == other.acceptance_criteria
            && self.inputs == other.inputs
            && self.expected_outputs == other.expected_outputs
            && self.allowed_tools == other.allowed_tools
            && self.context_scope == other.context_scope
            && self.risk_level == other.risk_level
            && self.can_parallelize == other.can_parallelize
            && self.requires_write_access == other.requires_write_access
            && self.verification == other.verification
            && self.checkpoint_policy == other.checkpoint_policy
            && self.attempts == other.attempts
            && self.started_at == other.started_at
            && self.completed_at == other.completed_at
            && self.error_code == other.error_code
            && self.run_id == other.run_id
            && self.title == other.title
            && self.agent_role == other.agent_role
            && self.kind == other.kind
            && self.files == other.files
            && self.summary == other.summary
        // execute_fn, metadata_json, metadata intentionally excluded — not comparable
    }
}

impl Task {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        let description = description.into();
        Self {
            id: id.into(),
            subject: description.clone(),
            description,
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
            priority: 5,
            result: None,
            reasoning: None,
            assigned_agent: None,
            tags: Vec::new(),
            parent_id: None,
            created_at: 0,
            updated_at: 0,
            timeout_secs: 0,
            max_retries: 0,
            retry_count: 0,
            execute_fn: None,
            metadata_json: None,
            metadata: None,
            // Enhanced fields (Step 1) - all with sensible defaults
            task_type: TaskType::default(),
            acceptance_criteria: Vec::new(),
            inputs: Vec::new(),
            expected_outputs: Vec::new(),
            allowed_tools: None,
            context_scope: ContextScope::default(),
            risk_level: RiskLevel::default(),
            can_parallelize: true,
            requires_write_access: false,
            verification: VerificationSpec::default(),
            checkpoint_policy: CheckpointPolicy::default(),
            attempts: Vec::new(),
            started_at: None,
            completed_at: None,
            error_code: None,
            // Execution state fields (Step 4) - all empty/None by default
            evidence: Vec::new(),
            changed_files: Vec::new(),
            artifacts: Vec::new(),
            commands_run: Vec::new(),
            verification_result: None,
            remaining_risks: Vec::new(),
            next_unblocked_tasks: Vec::new(),
            context_summary: None,
            // Optional runtime metadata is unset for a generic task.
            run_id: None,
            title: None,
            agent_role: None,
            kind: None,
            files: Vec::new(),
            summary: None,
        }
    }

    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    pub fn add_dependency(&mut self, dep: String) {
        self.dependencies.push(dep);
    }

    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.min(10);
        self
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Specify the Agent to execute
    pub fn with_assigned_agent(mut self, agent: impl Into<String>) -> Self {
        self.assigned_agent = Some(agent.into());
        self
    }

    /// Add tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Add a single tag
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        self.tags.push(tag.into());
    }

    /// Set a per-task execution function that overrides the executor's global execute_fn.
    pub fn with_execute_fn(mut self, f: super::executor::TaskExecuteFn) -> Self {
        self.execute_fn = Some(f);
        self
    }

    /// Set typed metadata.
    ///
    /// Stores both the typed value (for zero-cost downcast access) and its
    /// JSON serialization (for persistence). After deserialization from a
    /// store, only `metadata_json` survives — call [`get_metadata`] to
    /// attempt a typed read, or re-register with `with_metadata`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// #[derive(Serialize)]
    /// struct ResearchParams { topic: String, max_papers: u32 }
    ///
    /// let task = Task::new("r1", "Research task")
    ///     .with_metadata(ResearchParams { topic: "AI".into(), max_papers: 20 });
    ///
    /// // Later, retrieve typed access:
    /// let params = task.get_metadata::<ResearchParams>().unwrap();
    /// ```
    pub fn with_metadata<T: serde::Serialize + Send + Sync + 'static>(mut self, meta: T) -> Self {
        self.metadata_json = serde_json::to_value(&meta).ok();
        self.metadata = Some(std::sync::Arc::new(meta));
        self
    }

    /// Set raw JSON metadata (without a typed value).
    pub fn with_metadata_json(mut self, json: serde_json::Value) -> Self {
        self.metadata_json = Some(json);
        self
    }

    /// Attempt to downcast the typed metadata to a concrete type.
    ///
    /// Returns `None` if no metadata was set or the type doesn't match.
    pub fn get_metadata<T: 'static>(&self) -> Option<&T> {
        self.metadata.as_ref()?.downcast_ref::<T>()
    }

    // ── Enhanced Builder Methods (Step 1) ────────────────────────────────────

    /// Set task type
    pub fn with_task_type(mut self, task_type: TaskType) -> Self {
        self.task_type = task_type;
        self
    }

    /// Set acceptance criteria
    pub fn with_acceptance_criteria(mut self, criteria: Vec<String>) -> Self {
        self.acceptance_criteria = criteria;
        self
    }

    /// Add a single acceptance criterion
    pub fn add_acceptance_criterion(&mut self, criterion: impl Into<String>) {
        self.acceptance_criteria.push(criterion.into());
    }

    /// Set input specifications
    pub fn with_inputs(mut self, inputs: Vec<TaskInput>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Add a single input
    pub fn add_input(&mut self, input: TaskInput) {
        self.inputs.push(input);
    }

    /// Set expected output specifications
    pub fn with_expected_outputs(mut self, outputs: Vec<TaskOutput>) -> Self {
        self.expected_outputs = outputs;
        self
    }

    /// Add a single expected output
    pub fn add_expected_output(&mut self, output: TaskOutput) {
        self.expected_outputs.push(output);
    }

    /// Set allowed tools
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Set context scope
    pub fn with_context_scope(mut self, scope: ContextScope) -> Self {
        self.context_scope = scope;
        self
    }

    /// Set risk level
    pub fn with_risk_level(mut self, level: RiskLevel) -> Self {
        self.risk_level = level;
        self
    }

    /// Set whether task can be parallelized
    pub fn with_can_parallelize(mut self, can_parallelize: bool) -> Self {
        self.can_parallelize = can_parallelize;
        self
    }

    /// Set whether task requires write access
    pub fn with_requires_write_access(mut self, requires: bool) -> Self {
        self.requires_write_access = requires;
        self
    }

    /// Set verification specification
    pub fn with_verification(mut self, verification: VerificationSpec) -> Self {
        self.verification = verification;
        self
    }

    /// Set checkpoint policy
    pub fn with_checkpoint_policy(mut self, policy: CheckpointPolicy) -> Self {
        self.checkpoint_policy = policy;
        self
    }

    /// Record task start
    pub fn mark_started(&mut self) {
        self.started_at = Some(super::time::now_secs());
        self.status = TaskStatus::InProgress;
        self.updated_at = super::time::now_secs();
    }

    /// Record task completion
    pub fn mark_completed(&mut self, result: Option<String>) {
        self.completed_at = Some(super::time::now_secs());
        self.status = TaskStatus::Completed;
        self.result = result;
        self.updated_at = super::time::now_secs();
    }

    /// Record an execution attempt
    pub fn record_attempt(&mut self, attempt: TaskAttempt) {
        self.attempts.push(attempt);
        self.retry_count = self.attempts.len() as u32;
        self.updated_at = super::time::now_secs();
    }

    /// Whether already cancelled
    pub fn is_cancelled(&self) -> bool {
        self.status == TaskStatus::Cancelled
    }

    /// Cancel the task (using state machine validation)
    ///
    /// Succeeds only when the current state allows transition to `Cancelled`.
    /// Returns `true` if cancellation succeeded, `false` if current state does not allow cancellation.
    pub fn cancel(&mut self) -> bool {
        match self.status.transition_to(TaskStatus::Cancelled) {
            Ok(new_status) => {
                self.status = new_status;
                true
            }
            Err(_) => false,
        }
    }

    /// Record an execution result
    pub fn record_execution(
        &mut self,
        attempt: u32,
        error: Option<String>,
        duration_secs: Option<u64>,
        result: Option<String>,
    ) {
        self.retry_count = attempt.saturating_sub(1);
        self.updated_at = super::time::now_secs();
        if let Some(r) = result {
            self.result = Some(r);
        }
        if let Some(dur) = duration_secs {
            let _ = dur; // Record execution duration (usable for future statistics)
        }
        if let Some(err) = error {
            self.reasoning = Some(format!("Attempt {} failed: {}", attempt, err));
        }
    }
}
