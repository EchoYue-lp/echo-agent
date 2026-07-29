//! ManagedTask executor — parallel execution with timeout, retry, and cancellation support
//!
//! ## Architecture
//!
//! ```text
//! TaskManager → TaskExecutor → execute_ready_tasks()
//!                              ↓
//!                         [Semaphore limited parallelism]
//!                              ↓
//!                         tokio::spawn for each task
//!                              ↓
//!                         run_task_with_retry()
//!                              ↓
//!                         [timeout] → [retry loop] → execute_fn()
//! ```
//!
//! ## Example
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_orchestration::tasks::{TaskExecutor, TaskExecutorConfig, TaskManager};
//! use std::sync::Arc;
//!
//! async fn example() -> Result<()> {
//!     let manager = Arc::new(TaskManager::new());
//!     let config = TaskExecutorConfig {
//!         max_concurrent: 5,
//!         default_timeout_secs: 60,
//!         ..Default::default()
//!     };
//!
//!     let executor = TaskExecutor::new(manager, config);
//!
//!     // Execute all ready tasks in parallel
//!     while !executor.is_completed() {
//!         executor.execute_ready_tasks().await;
//!     }
//!     Ok(())
//! }
//! ```

use super::hooks::{RetryDecision, TaskHookRegistry};
use super::manager::TaskManager;
use super::replanner::Replanner;
use super::runtime::TaskStatus;
use super::runtime::{NestedDelegationPolicy, Task, TaskClaim, TaskSubagentContext};
use super::runtime_executor::{
    RuntimeDagController, RuntimeDagExecutor, RuntimeDagExecutorConfig, RuntimeDagOutcome,
    RuntimePlanSnapshot, RuntimeStopDisposition, RuntimeTaskClaimOutcome, RuntimeTaskResolution,
};
use super::scheduler::TaskScheduler;
use super::task::ManagedTask;
use super::verifier::Verifier;
use crate::planning::PlanValidator;
use crate::tasks::BackgroundCheckpointStore;
use async_trait::async_trait;
use dashmap::DashMap;
use echo_core::error::{ReactError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Configuration for task executor
pub struct TaskExecutorConfig {
    /// Maximum concurrent task executions
    pub max_concurrent: usize,
    /// Default timeout in seconds (0 = no timeout)
    pub default_timeout_secs: u64,
    /// Base retry delay in seconds (used as initial delay for exponential backoff)
    pub retry_delay_secs: u64,
    /// Backoff multiplier for retry delay (e.g. 2.0 = delay doubles each attempt)
    pub retry_backoff_factor: f64,
    /// Maximum retry delay cap in seconds (prevents unbounded growth)
    pub retry_max_delay_secs: u64,
    /// Whether to add jitter to retry delays (recommended for production)
    pub retry_jitter: bool,
    /// Enable task hooks
    pub enable_hooks: bool,
    /// Optional bridge to the unified lifecycle hook system (echo-core).
    /// When set, TaskCreated/TaskCompleted events are fired into the
    /// unified HookRegistry alongside the trait-based TaskHooks.
    pub unified_hook_executor: Option<echo_core::hooks::UnifiedHookExecutorFn>,
}

impl Clone for TaskExecutorConfig {
    fn clone(&self) -> Self {
        Self {
            max_concurrent: self.max_concurrent,
            default_timeout_secs: self.default_timeout_secs,
            retry_delay_secs: self.retry_delay_secs,
            retry_backoff_factor: self.retry_backoff_factor,
            retry_max_delay_secs: self.retry_max_delay_secs,
            retry_jitter: self.retry_jitter,
            enable_hooks: self.enable_hooks,
            unified_hook_executor: self.unified_hook_executor.clone(),
        }
    }
}

type TaskOutputPair = (String, String);
type UpstreamResults = (Vec<TaskOutputPair>, Vec<TaskOutputPair>);

impl Default for TaskExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            default_timeout_secs: 300, // 5 minutes
            retry_delay_secs: 1,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: true,
            enable_hooks: true,
            unified_hook_executor: None,
        }
    }
}

impl TaskExecutorConfig {
    /// Compute retry delay for the given attempt (1-based), applying exponential backoff + optional jitter.
    pub fn retry_delay_for_attempt(&self, attempt: u32) -> Duration {
        let base = self.retry_delay_secs as f64;
        let delay = base
            * self
                .retry_backoff_factor
                .powi((attempt as i32).saturating_sub(1));
        let capped = delay.min(self.retry_max_delay_secs as f64);

        let secs = if self.retry_jitter {
            // Full jitter: random in [0, capped]

            fastrand::f64() * capped
        } else {
            capped
        };

        Duration::from_secs_f64(secs)
    }
}

/// Result of task execution
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    /// ManagedTask ID
    pub task_id: String,
    /// Final status
    pub status: TaskStatus,
    /// Output/result string
    pub output: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution duration
    pub duration: Duration,
    /// Number of attempts made
    pub attempts: u32,
}

impl TaskExecutionResult {
    pub fn success(task_id: &str, output: String, duration: Duration, attempts: u32) -> Self {
        Self {
            task_id: task_id.to_string(),
            status: TaskStatus::Completed,
            output: Some(output),
            error: None,
            duration,
            attempts,
        }
    }

    pub fn failure(task_id: &str, error: String, duration: Duration, attempts: u32) -> Self {
        Self {
            task_id: task_id.to_string(),
            status: TaskStatus::Failed(error.clone()),
            output: None,
            error: Some(error),
            duration,
            attempts,
        }
    }

    pub fn timeout(task_id: &str, timeout_secs: u64, attempts: u32) -> Self {
        Self {
            task_id: task_id.to_string(),
            status: TaskStatus::TimedOut {
                error: format!("Task timed out after {}s", timeout_secs),
            },
            output: None,
            error: Some(format!("Timeout after {}s", timeout_secs)),
            duration: Duration::from_secs(timeout_secs),
            attempts,
        }
    }

    pub fn cancelled(task_id: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            status: TaskStatus::Cancelled,
            output: None,
            error: Some("Task was cancelled".to_string()),
            duration: Duration::ZERO,
            attempts: 0,
        }
    }
}

/// Context provided to task execution functions
///
/// Contains the task description, upstream dependency results, and metadata
/// so the executor can make informed decisions.
#[derive(Debug, Clone)]
pub struct TaskContext {
    /// ManagedTask ID
    pub task_id: String,
    /// ManagedTask description
    pub description: String,
    /// Results from completed upstream dependencies (task_id → output)
    pub upstream_results: Vec<(String, String)>,
    /// Errors from failed upstream dependencies (task_id → error)
    pub upstream_errors: Vec<(String, String)>,
    /// Attempt number (1-based)
    pub attempt: u32,
}

impl TaskContext {
    /// Create a minimal context with no upstream results
    pub fn new(task_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            upstream_results: Vec::new(),
            upstream_errors: Vec::new(),
            attempt: 1,
        }
    }

    /// Create context with upstream results from a TaskManager snapshot
    pub fn with_upstream(
        task_id: impl Into<String>,
        description: impl Into<String>,
        upstream_results: Vec<(String, String)>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            upstream_results,
            upstream_errors: Vec::new(),
            attempt: 1,
        }
    }

    /// Create context with both upstream results and errors
    pub fn with_upstream_and_errors(
        task_id: impl Into<String>,
        description: impl Into<String>,
        upstream_results: Vec<(String, String)>,
        upstream_errors: Vec<(String, String)>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            upstream_results,
            upstream_errors,
            attempt: 1,
        }
    }

    /// Format upstream results as a context string for LLM injection
    pub fn format_upstream_context(&self) -> String {
        self.format_upstream_context_with_limit(300)
    }

    /// Format upstream results with a configurable character limit per result
    pub fn format_upstream_context_with_limit(&self, char_limit: usize) -> String {
        if self.upstream_results.is_empty() && self.upstream_errors.is_empty() {
            return String::new();
        }

        let mut parts = vec!["Execution results of upstream dependent tasks:".to_string()];
        for (id, result) in &self.upstream_results {
            let truncated = result.chars().count() > char_limit;
            let preview: String = result.chars().take(char_limit).collect();
            let preview = if truncated {
                format!("{preview}...")
            } else {
                preview
            };
            parts.push(format!("  - [{}]: {}", id, preview));
        }
        for (id, error) in &self.upstream_errors {
            parts.push(format!("  - [{}]: (FAILED) {}", id, error));
        }
        parts.join("\n")
    }
}

/// Function type for task execution
///
/// Receives a [`TaskContext`] with task description and upstream results.
pub type TaskExecuteFn =
    Arc<dyn Fn(TaskContext) -> futures::future::BoxFuture<'static, Result<String>> + Send + Sync>;

/// Parallel task executor with timeout and retry support.
///
/// Works directly with `Arc<TaskManager>` — since `TaskManager` uses `DashMap` internally,
/// no external `RwLock` is needed for concurrent access.
#[derive(Clone)]
pub struct TaskExecutor {
    task_manager: Arc<TaskManager>,
    config: TaskExecutorConfig,
    semaphore: Arc<Semaphore>,
    execute_fn: Option<TaskExecuteFn>,
    hooks: Arc<TaskHookRegistry>,
    /// Optional persistent task store for cross-restart resumption.
    task_store: Option<Arc<dyn super::store::TaskStore>>,
    /// Tracks cancellation tokens for running tasks.
    /// Used by `cancel_task()` to abort in-flight executions.
    running_tasks: Arc<DashMap<String, CancellationToken>>,
    /// Optional shared [`TaskSpawner`] for `execute_all_async()`.
    ///
    /// When set, all async DAG executions share the same spawner so tasks
    /// appear in a unified `list()` and share concurrency control. When
    /// `None`, each `execute_all_async()` call creates an isolated spawner.
    shared_spawner: Option<Arc<super::background_task::TaskSpawner>>,
    /// Cooperative cancellation token for the entire executor.
    cancel: CancellationToken,

    // ── Step 10: Integrated components ──────────────────────────────────
    /// Optional replanner for automatic plan adjustment on failure/blocking.
    replanner: Option<Arc<dyn Replanner>>,
    /// Optional verifier for task completion verification.
    verifier: Option<Arc<dyn Verifier>>,
    /// Optional scheduler for strategy-based task scheduling.
    scheduler: Option<Arc<TaskScheduler>>,
    /// Optional background task checkpoint store.
    background_checkpoint_store: Option<Arc<dyn BackgroundCheckpointStore>>,
}

struct ManagedTaskDagController {
    executor: TaskExecutor,
    results: Mutex<Vec<TaskExecutionResult>>,
    claims: Mutex<HashMap<String, TaskClaim>>,
}

impl ManagedTaskDagController {
    fn new(executor: TaskExecutor) -> Self {
        Self {
            executor,
            results: Mutex::new(Vec::new()),
            claims: Mutex::new(HashMap::new()),
        }
    }

    async fn take_results(&self) -> Vec<TaskExecutionResult> {
        std::mem::take(&mut *self.results.lock().await)
    }

    async fn persist_task(&self, task: &ManagedTask) -> Result<()> {
        if let Some(store) = self.executor.task_store.as_ref() {
            store.save_task(task).await?;
        }
        Ok(())
    }
}

impl TaskExecutor {
    /// Create a new task executor
    pub fn new(task_manager: Arc<TaskManager>, config: TaskExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent));
        let hooks = Arc::new(task_manager.hooks().clone());
        Self {
            task_manager,
            config,
            semaphore,
            execute_fn: None,
            hooks,
            task_store: None,
            running_tasks: Arc::new(DashMap::new()),
            shared_spawner: None,
            cancel: CancellationToken::new(),
            // Step 10: Initialize integrated components
            replanner: None,
            verifier: None,
            scheduler: None,
            background_checkpoint_store: None,
        }
    }

    /// Get a clone of the executor's cancellation token.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Set a custom execution function
    pub fn with_execute_fn(mut self, f: TaskExecuteFn) -> Self {
        self.execute_fn = Some(f);
        self
    }

    /// Register a `TaskHooks` implementation for task lifecycle callbacks.
    ///
    /// Use this to wire `BridgedTaskHooks` so YAML-configured hooks see
    /// task events (TaskCreated, TaskCompleted, etc.).
    pub fn with_task_hook(mut self, hook: Arc<dyn super::hooks::TaskHooks>) -> Self {
        // Safe: we own the only reference during builder phase
        if let Some(registry) = Arc::get_mut(&mut self.hooks) {
            registry.register(hook);
        }
        self
    }

    /// Set a persistent task store for cross-restart task resumption.
    pub fn with_task_store(mut self, store: Arc<dyn super::store::TaskStore>) -> Self {
        self.task_store = Some(store);
        self
    }

    /// Set a shared [`TaskSpawner`] for `execute_all_async()`.
    ///
    /// When set, all async DAG executions share the same spawner so tasks
    /// appear in a unified `list()` and share concurrency control.
    pub fn with_task_spawner(mut self, spawner: Arc<super::background_task::TaskSpawner>) -> Self {
        self.shared_spawner = Some(spawner);
        self
    }

    // ── Step 10: Integrated component builders ────────────────────────────

    /// Set a replanner for automatic plan adjustment on failure/blocking.
    ///
    /// When set, the executor will trigger replanning when tasks fail or get blocked.
    pub fn with_replanner(mut self, replanner: Arc<dyn Replanner>) -> Self {
        self.replanner = Some(replanner);
        self
    }

    /// Set a verifier for task completion verification.
    ///
    /// When set, the executor will verify task completion before marking as completed.
    pub fn with_verifier(mut self, verifier: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }

    /// Set a scheduler for strategy-based task scheduling.
    ///
    /// When set, the executor will use the scheduler to determine execution strategy
    /// (parallel, serial, worktree-isolated, etc.).
    pub fn with_scheduler(mut self, scheduler: Arc<TaskScheduler>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Set a background task checkpoint store.
    ///
    /// When set, background task states will be persisted to this store.
    pub fn with_background_checkpoint_store(
        mut self,
        store: Arc<dyn BackgroundCheckpointStore>,
    ) -> Self {
        self.background_checkpoint_store = Some(store);
        self
    }

    /// Check if all tasks are completed
    pub fn is_completed(&self) -> bool {
        self.task_manager.is_all_completed()
    }

    /// Get progress statistics
    pub fn get_progress(&self) -> (usize, usize) {
        self.task_manager.get_progress()
    }

    /// Execute all ready tasks in parallel
    ///
    /// When a [`TaskScheduler`] is configured, the executor respects the
    /// scheduling strategy: parallel groups are executed group-by-group
    /// (tasks within a group run concurrently), then serial-sequence tasks
    /// run one at a time.  Without a scheduler, all ready tasks run in
    /// parallel (legacy behaviour).
    ///
    /// Returns the number of tasks executed
    pub async fn execute_ready_tasks(&self) -> Result<Vec<TaskExecutionResult>> {
        let mut ready_tasks: Vec<ManagedTask> = self.task_manager.get_ready_tasks();

        if ready_tasks.is_empty() {
            return Ok(Vec::new());
        }

        // Sort by priority descending (highest priority first).
        ready_tasks.sort_by_key(|t| std::cmp::Reverse(t.priority));

        // If a scheduler is configured, use it to determine execution order.
        if let Some(ref scheduler) = self.scheduler {
            return self
                .execute_with_scheduler(ready_tasks, scheduler.as_ref())
                .await;
        }

        // ── Legacy path: all ready tasks run in parallel ──
        info!(
            tasks = ready_tasks.len(),
            max_concurrent = self.config.max_concurrent,
            "Executing {} ready tasks with max {} concurrent",
            ready_tasks.len(),
            self.config.max_concurrent
        );

        self.spawn_parallel_batch(ready_tasks).await
    }

    /// Execute ready tasks following the scheduler's plan.
    ///
    /// Parallel groups run group-by-group (group-internal concurrency),
    /// serial-sequence tasks run one at a time.
    async fn execute_with_scheduler(
        &self,
        ready_tasks: Vec<ManagedTask>,
        scheduler: &TaskScheduler,
    ) -> Result<Vec<TaskExecutionResult>> {
        let plan = scheduler.schedule(&ready_tasks);

        if !plan.conflicts.is_empty() {
            warn!(
                conflicts = ?plan.conflicts,
                strategy = ?plan.strategy,
                "Write conflicts detected; serialising conflicting tasks"
            );
        }

        info!(
            tasks = ready_tasks.len(),
            parallel_groups = plan.parallel_groups.len(),
            serial_tasks = plan.serial_sequence.len(),
            strategy = ?plan.strategy,
            "Executing ready tasks with scheduler"
        );

        // Build a lookup: task ID → ManagedTask for quick access
        let task_map: std::collections::HashMap<String, ManagedTask> =
            ready_tasks.into_iter().map(|t| (t.id.clone(), t)).collect();

        let mut all_results = Vec::new();

        // 1. Execute parallel groups sequentially (group-internal concurrency)
        for group_ids in &plan.parallel_groups {
            let group_tasks: Vec<ManagedTask> = group_ids
                .iter()
                .filter_map(|id| task_map.get(id).cloned())
                .collect();
            if group_tasks.is_empty() {
                continue;
            }
            debug!(group_size = group_tasks.len(), "Executing parallel group");
            let results = self.spawn_parallel_batch(group_tasks).await?;
            all_results.extend(results);
        }

        // 2. Execute serial-sequence tasks one at a time
        for task_id in &plan.serial_sequence {
            if let Some(task) = task_map.get(task_id).cloned() {
                debug!(task_id = %task_id, "Executing serial task");
                let result = self
                    .spawn_and_run_single(task, self.config.max_concurrent.max(1) as u32)
                    .await;
                all_results.push(result);
            }
        }

        Ok(all_results)
    }

    /// Spawn a batch of tasks to run concurrently (subject to semaphore).
    async fn spawn_parallel_batch(
        &self,
        tasks: Vec<ManagedTask>,
    ) -> Result<Vec<TaskExecutionResult>> {
        let mut handles = Vec::with_capacity(tasks.len());

        for task in tasks {
            // Fire unified TaskCreated hook at scheduling time
            if let Some(ref executor) = self.config.unified_hook_executor {
                let ctx = echo_core::hooks::HookContext::for_task_created(
                    &task.id,
                    &task.subject,
                    "",
                    "",
                );
                executor(ctx).await;
            }

            let permit = self
                .semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| ReactError::Other(format!("Semaphore acquire error: {}", e)))?;
            let manager = self.task_manager.clone();
            let config = self.config.clone();
            let execute_fn = task.execute_fn.clone().or_else(|| self.execute_fn.clone());
            let hooks = self.hooks.clone();
            let running_tasks = self.running_tasks.clone();
            let task_store = self.task_store.clone();
            let verifier = self.verifier.clone();
            let replanner = self.replanner.clone();
            let task_id = task.id.clone();
            let cancel = self.cancel.child_token();
            let cancel_clone = cancel.clone();
            running_tasks.insert(task_id.clone(), cancel);

            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let start = Instant::now();

                if task.is_cancelled() || cancel_clone.is_cancelled() {
                    running_tasks.remove(&task_id);
                    return TaskExecutionResult::cancelled(&task_id);
                }

                let manager2 = manager.clone();
                let verifier_clone = verifier.clone();
                let result = tokio::select! {
                    biased;
                    _ = cancel_clone.cancelled() => {
                        let _ = manager.cancel_task(&task_id);
                        TaskExecutionResult::cancelled(&task_id)
                    }
                    result = Self::run_task_with_retry(
                        task,
                        manager2,
                        config,
                        execute_fn,
                        hooks,
                        cancel_clone.clone(),
                        task_store,
                        verifier_clone,
                        replanner,
                    ) => {
                        result
                    }
                };

                running_tasks.remove(&task_id);

                debug!(
                    task_id = %task_id,
                    duration_ms = start.elapsed().as_millis(),
                    status = ?result.status,
                    "Task execution completed"
                );

                result
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    warn!(error = %e, "Task join error");
                }
            }
        }

        Ok(results)
    }

    /// Execute a single task with semaphore permit, returning its result.
    async fn spawn_and_run_single(
        &self,
        task: ManagedTask,
        _max_concurrent: u32,
    ) -> TaskExecutionResult {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| TaskExecutionResult::cancelled(&task.id));

        let permit = match permit {
            Ok(p) => p,
            Err(r) => return r,
        };

        let _permit = permit;
        let start = Instant::now();
        let task_id = task.id.clone();
        let cancel = self.cancel.child_token();
        let cancel_clone = cancel.clone();
        self.running_tasks.insert(task_id.clone(), cancel);

        let result = tokio::select! {
            biased;
            _ = cancel_clone.cancelled() => {
                let _ = self.task_manager.cancel_task(&task_id);
                TaskExecutionResult::cancelled(&task_id)
            }
            result = Self::run_task_with_retry(
                task,
                self.task_manager.clone(),
                self.config.clone(),
                self.execute_fn.clone(),
                self.hooks.clone(),
                cancel_clone.clone(),
                self.task_store.clone(),
                self.verifier.clone(),
                self.replanner.clone(),
            ) => {
                result
            }
        };

        self.running_tasks.remove(&task_id);
        debug!(
            task_id = %task_id,
            duration_ms = start.elapsed().as_millis(),
            status = ?result.status,
            "Serial task execution completed"
        );
        result
    }

    /// Execute one node selected by the canonical DAG kernel.
    ///
    /// Global concurrency is already bounded by `RuntimeDagExecutor`, so this
    /// adapter deliberately does not acquire the legacy batch semaphore.
    async fn execute_selected_task(
        &self,
        task: ManagedTask,
        parent_cancel: CancellationToken,
    ) -> TaskExecutionResult {
        let task_id = task.id.clone();
        if let Some(ref executor) = self.config.unified_hook_executor {
            let ctx =
                echo_core::hooks::HookContext::for_task_created(&task.id, &task.subject, "", "");
            executor(ctx).await;
        }

        let cancel = parent_cancel.child_token();
        let cancel_for_execution = cancel.clone();
        let cancel_wait = cancel_for_execution.clone();
        self.running_tasks.insert(task_id.clone(), cancel);
        let result = tokio::select! {
            biased;
            _ = cancel_wait.cancelled() => {
                let _ = self.task_manager.cancel_task(&task_id);
                TaskExecutionResult::cancelled(&task_id)
            }
            result = Self::run_task_with_retry(
                task,
                self.task_manager.clone(),
                self.config.clone(),
                self.execute_fn.clone(),
                self.hooks.clone(),
                cancel_for_execution,
                self.task_store.clone(),
                self.verifier.clone(),
                self.replanner.clone(),
            ) => result,
        };
        if matches!(result.status, TaskStatus::Cancelled) {
            let _ = self.task_manager.cancel_task(&task_id);
        }
        self.running_tasks.remove(&task_id);
        result
    }

    /// Run a single task with retry logic
    // These values form one internal execution context; grouping them is a
    // separate executor refactor, not part of workspace migration.
    #[allow(clippy::too_many_arguments)]
    async fn run_task_with_retry(
        task: ManagedTask,
        manager: Arc<TaskManager>,
        config: TaskExecutorConfig,
        execute_fn: Option<TaskExecuteFn>,
        hooks: Arc<TaskHookRegistry>,
        cancel: CancellationToken,
        task_store: Option<Arc<dyn super::store::TaskStore>>,
        verifier: Option<Arc<dyn Verifier>>,
        replanner: Option<Arc<dyn Replanner>>,
    ) -> TaskExecutionResult {
        let task_id = task.id.clone();
        let timeout_secs = if task.timeout_secs > 0 {
            task.timeout_secs
        } else {
            config.default_timeout_secs
        };
        let max_retries = task.max_retries;
        let mut current_attempt = task.retry_count + 1;
        let start = Instant::now();

        // Update status to Running
        let _ = manager.update_task_status(&task_id, TaskStatus::Running);

        // Call before_execute hook
        if config.enable_hooks
            && let Some(ctx) = manager.create_hook_context(&task_id, current_attempt, None)
        {
            hooks.before_execute(&ctx).await;
        }

        loop {
            // Check cancellation
            if cancel.is_cancelled()
                || manager
                    .get_task(&task_id)
                    .map(|t| t.is_cancelled())
                    .unwrap_or(false)
            {
                return TaskExecutionResult::cancelled(&task_id);
            }

            // Re-collect upstream results on each retry attempt.
            // Upstream tasks may complete during retry backoff, so we need
            // fresh data each time.
            let (upstream_results, upstream_errors) =
                Self::collect_upstream_results_with_errors(&task, &manager);

            // Check if any upstream task failed — if so, mark this task as blocked
            if !upstream_errors.is_empty() {
                let error_summary = upstream_errors
                    .iter()
                    .map(|(id, err)| format!("{}: {}", id, err))
                    .collect::<Vec<_>>()
                    .join("; ");
                let block_reason = format!("Upstream task failed: {}", error_summary);
                let _ =
                    manager.update_task_status(&task_id, TaskStatus::Blocked(block_reason.clone()));

                // ── Replanner: TaskBlocked trigger ──
                Self::try_replan_on_blocked(replanner.clone(), &manager, &task_id, &block_reason)
                    .await;

                return TaskExecutionResult::failure(
                    &task_id,
                    block_reason,
                    start.elapsed(),
                    current_attempt,
                );
            }

            let ctx = TaskContext {
                task_id: task_id.clone(),
                description: task.description.clone(),
                upstream_results,
                upstream_errors: Vec::new(), // All upstream succeeded at this point
                attempt: current_attempt,
            };

            // Execute with timeout
            let execute_result = if let Some(ref f) = execute_fn {
                let f = f.clone();
                let execution = f(ctx);
                tokio::pin!(execution);

                let cancel_token = cancel.clone();
                let cancel_wait = cancel_token.cancelled();
                tokio::pin!(cancel_wait);

                if timeout_secs == 0 {
                    tokio::select! {
                        biased;
                        _ = &mut cancel_wait => {
                            return TaskExecutionResult::cancelled(&task_id);
                        }
                        result = &mut execution => result,
                    }
                } else {
                    let timeout_wait = tokio::time::sleep(Duration::from_secs(timeout_secs));
                    tokio::pin!(timeout_wait);

                    tokio::select! {
                        biased;
                        _ = &mut cancel_wait => {
                            return TaskExecutionResult::cancelled(&task_id);
                        }
                        _ = &mut timeout_wait => {
                            let result =
                                TaskExecutionResult::timeout(&task_id, timeout_secs, current_attempt);

                            // Update manager status to TimedOut
                            let _ = manager.update_task_status(&task_id, TaskStatus::TimedOut {
                                error: format!("Task timed out after {}s", timeout_secs),
                            });

                            // Call on_timeout hook
                            if config.enable_hooks
                                && let Some(ctx) =
                                    manager.create_hook_context(&task_id, current_attempt, None)
                            {
                                hooks.on_timeout(&ctx).await;
                            }

                            // Immediately persist timed-out task to store
                            if let Some(ref store) = task_store
                                && let Some(task_snapshot) = manager.get_task(&task_id)
                                    && let Err(e) = store.save_task(&task_snapshot).await {
                                        warn!(task_id = %task_id, error = %e, "Failed to persist timed-out task");
                                    }

                            return result;
                        }
                        result = &mut execution => result,
                    }
                }
            } else {
                // No execute_fn provided - return success with description
                Ok(task.description.clone())
            };

            match execute_result {
                Ok(output) => {
                    // Record execution
                    manager.record_task_execution(
                        &task_id,
                        current_attempt,
                        None,
                        Some(start.elapsed().as_secs()),
                        None,
                    );

                    // Verify task completion if verifier is set
                    if let Some(ref verifier) = verifier
                        && let Some(task_snapshot) = manager.get_task(&task_id)
                    {
                        match verifier.verify(&task_snapshot).await {
                            Ok(verification_result) => {
                                if verification_result.passed {
                                    info!(task_id = %task_id, "Task verification passed");
                                } else {
                                    warn!(task_id = %task_id, "Task verification failed: {}", verification_result.output);
                                    let fail_reason = format!(
                                        "Verification failed: {}",
                                        verification_result.output
                                    );

                                    // ── Replanner: verification failure trigger ──
                                    Self::try_replan_on_failure(
                                        replanner.clone(),
                                        &manager,
                                        &task_id,
                                        &fail_reason,
                                    )
                                    .await;

                                    // Mark as failed due to verification failure
                                    let _ = manager.update_task_status(
                                        &task_id,
                                        TaskStatus::Failed(fail_reason.clone()),
                                    );
                                    return TaskExecutionResult::failure(
                                        &task_id,
                                        fail_reason,
                                        start.elapsed(),
                                        current_attempt,
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(task_id = %task_id, error = %e, "Verification error");
                                // Continue with completion even if verification errors
                            }
                        }
                    }

                    let _ = manager.update_task_status(&task_id, TaskStatus::Completed);
                    manager.set_task_result(&task_id, output.clone());

                    // Call after_execute hook
                    if config.enable_hooks
                        && let Some(ctx) =
                            manager.create_hook_context(&task_id, current_attempt, None)
                    {
                        hooks.after_execute(&ctx, &output).await;
                    }

                    // Fire unified TaskCompleted hook (success)
                    if let Some(ref executor) = config.unified_hook_executor {
                        let ctx = echo_core::hooks::HookContext::for_task_completed(
                            &task_id,
                            &task.subject,
                            &output,
                            "", // session_id not available at this layer
                            "", // agent_name not available at this layer
                        );
                        executor(ctx).await;
                    }

                    // Immediately persist completed task to store (close crash window)
                    if let Some(ref store) = task_store
                        && let Some(task_snapshot) = manager.get_task(&task_id)
                        && let Err(e) = store.save_task(&task_snapshot).await
                    {
                        warn!(task_id = %task_id, error = %e, "Failed to persist completed task");
                    }

                    return TaskExecutionResult::success(
                        &task_id,
                        output,
                        start.elapsed(),
                        current_attempt,
                    );
                }
                Err(e) => {
                    let error_str = e.to_string();

                    // Check if should retry
                    if current_attempt <= max_retries {
                        // Call on_failure hook
                        let decision = if config.enable_hooks {
                            if let Some(ctx) =
                                manager.create_hook_context(&task_id, current_attempt, None)
                            {
                                hooks.on_failure(&ctx, &error_str).await
                            } else {
                                RetryDecision::Retry {
                                    delay_secs: config
                                        .retry_delay_for_attempt(current_attempt)
                                        .as_secs(),
                                }
                            }
                        } else {
                            RetryDecision::Retry {
                                delay_secs: config
                                    .retry_delay_for_attempt(current_attempt)
                                    .as_secs(),
                            }
                        };

                        match decision {
                            RetryDecision::Retry { delay_secs } => {
                                info!(
                                    task_id = %task_id,
                                    attempt = current_attempt,
                                    max_retries = max_retries,
                                    delay_secs = delay_secs,
                                    "Retrying task after failure"
                                );

                                // Update status to Retrying
                                let _ = manager.update_task_status(
                                    &task_id,
                                    TaskStatus::Retrying {
                                        attempt: current_attempt,
                                        last_error: error_str.clone(),
                                    },
                                );
                                manager.record_task_execution(
                                    &task_id,
                                    current_attempt,
                                    Some(error_str.clone()),
                                    Some(start.elapsed().as_secs()),
                                    None,
                                );

                                if let Some(next_attempt) = current_attempt.checked_add(1) {
                                    current_attempt = next_attempt;
                                    tokio::select! {
                                        biased;
                                        _ = cancel.cancelled() => {
                                            return TaskExecutionResult::cancelled(&task_id);
                                        }
                                        _ = tokio::time::sleep(Duration::from_secs(delay_secs)) => {}
                                    }
                                    continue;
                                }
                                warn!(task_id = %task_id, "Retry counter exhausted");
                            }
                            RetryDecision::Skip => {
                                // Mark as completed but with warning
                                let _ = manager.update_task_status(&task_id, TaskStatus::Completed);
                                manager
                                    .set_task_result(&task_id, format!("Skipped: {}", error_str));
                                return TaskExecutionResult::success(
                                    &task_id,
                                    format!("Skipped: {}", error_str),
                                    start.elapsed(),
                                    current_attempt,
                                );
                            }
                            RetryDecision::Fail => {
                                // Fall through to failure handling
                            }
                            RetryDecision::Ignore { message } => {
                                let _ = manager.update_task_status(&task_id, TaskStatus::Completed);
                                manager.set_task_result(&task_id, message.clone());
                                return TaskExecutionResult::success(
                                    &task_id,
                                    message,
                                    start.elapsed(),
                                    current_attempt,
                                );
                            }
                        }
                    }

                    // Final failure — try replanning before giving up
                    Self::try_replan_on_failure(replanner.clone(), &manager, &task_id, &error_str)
                        .await;

                    let _ =
                        manager.update_task_status(&task_id, TaskStatus::Failed(error_str.clone()));

                    // Fire unified TaskCompleted hook (failure)
                    if let Some(ref executor) = config.unified_hook_executor {
                        let ctx = echo_core::hooks::HookContext::for_task_completed(
                            &task_id,
                            &task.subject,
                            &format!("error: {}", error_str),
                            "", // session_id not available at this layer
                            "", // agent_name not available at this layer
                        );
                        executor(ctx).await;
                    }
                    manager.record_task_execution(
                        &task_id,
                        current_attempt,
                        Some(error_str.clone()),
                        Some(start.elapsed().as_secs()),
                        None,
                    );

                    // Immediately persist failed task to store (close crash window)
                    if let Some(ref store) = task_store
                        && let Some(task_snapshot) = manager.get_task(&task_id)
                        && let Err(e) = store.save_task(&task_snapshot).await
                    {
                        warn!(task_id = %task_id, error = %e, "Failed to persist failed task");
                    }

                    return TaskExecutionResult::failure(
                        &task_id,
                        error_str,
                        start.elapsed(),
                        current_attempt,
                    );
                }
            }
        }
    }

    /// Attempt replanning when a task fails (exhausted retries / verification failure).
    async fn try_replan_on_failure(
        replanner: Option<Arc<dyn Replanner>>,
        manager: &Arc<TaskManager>,
        task_id: &str,
        error: &str,
    ) {
        let Some(replanner) = replanner else { return };
        let trigger = super::replanner::ReplanTrigger::TaskFailure {
            task_id: task_id.to_string(),
            error: error.to_string(),
        };
        if matches!(
            replanner.should_replan(&trigger),
            super::replanner::ReplanDecision::Replan { .. }
        ) {
            let current_plan = manager.get_summary();
            let tasks_snapshot = manager.get_all_tasks();
            match replanner
                .replan(&trigger, &current_plan, &tasks_snapshot)
                .await
            {
                Ok(_new_plan_json) => {
                    info!(task_id = %task_id, "Replanner generated new plan after task failure");
                }
                Err(e) => warn!(task_id = %task_id, error = %e, "Replan attempt failed"),
            }
        }
    }

    /// Attempt replanning when a task is blocked by upstream failure.
    async fn try_replan_on_blocked(
        replanner: Option<Arc<dyn Replanner>>,
        manager: &Arc<TaskManager>,
        task_id: &str,
        reason: &str,
    ) {
        let Some(replanner) = replanner else { return };
        let trigger = super::replanner::ReplanTrigger::TaskBlocked {
            task_id: task_id.to_string(),
            reason: reason.to_string(),
        };
        if matches!(
            replanner.should_replan(&trigger),
            super::replanner::ReplanDecision::Replan { .. }
        ) {
            let current_plan = manager.get_summary();
            let tasks_snapshot = manager.get_all_tasks();
            // Fire-and-forget: don't block the current task's failure path
            let replanner = Arc::clone(&replanner);
            let task_id_owned = task_id.to_string();
            tokio::spawn(async move {
                match replanner
                    .replan(&trigger, &current_plan, &tasks_snapshot)
                    .await
                {
                    Ok(_new_plan_json) => {
                        info!(task_id = %task_id_owned, "Replanner generated new plan after task blocked");
                    }
                    Err(e) => warn!(task_id = %task_id_owned, error = %e, "Replan on block failed"),
                }
            });
        }
    }

    /// Collect upstream dependency results and errors for a task
    ///
    /// Returns (successful_results, failed_errors).
    /// Only includes tasks that have reached a terminal state.
    fn collect_upstream_results_with_errors(
        task: &ManagedTask,
        manager: &Arc<TaskManager>,
    ) -> UpstreamResults {
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for dep_id in &task.dependencies {
            if let Some(dep) = manager.get_task(dep_id) {
                match &dep.status {
                    TaskStatus::Completed => {
                        if let Some(r) = dep.result {
                            results.push((dep_id.clone(), r));
                        }
                    }
                    TaskStatus::Failed(err) => {
                        errors.push((dep_id.clone(), err.clone()));
                    }
                    TaskStatus::TimedOut { error } => {
                        errors.push((dep_id.clone(), format!("TimedOut: {}", error)));
                    }
                    TaskStatus::Blocked(reason) => {
                        errors.push((dep_id.clone(), format!("Blocked: {}", reason)));
                    }
                    _ => {} // Still running or pending - not terminal
                }
            }
        }

        (results, errors)
    }

    /// Cancel a specific task
    ///
    /// Marks the task as cancelled in the manager AND cancels the in-flight
    /// execution via CancellationToken, so a running spawned task is aborted.
    pub fn cancel_task(&self, task_id: &str) -> bool {
        let cancelled = self.task_manager.cancel_task(task_id);
        if let Some((_, token)) = self.running_tasks.remove(task_id) {
            token.cancel();
        }
        cancelled
    }

    /// Cancel all tasks
    pub fn cancel_all(&self) {
        self.task_manager.cancel_all();
        // Cancel all in-flight tokens
        let tokens: Vec<_> = self
            .running_tasks
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        for token in tokens {
            token.cancel();
        }
        self.running_tasks.clear();
    }

    /// Run the full task DAG through the canonical runtime kernel.
    ///
    /// Hooks, retries, verification, replanning, timeout, and persistence stay
    /// in this executor's per-task pipeline. Dependency traversal, bounded
    /// waves, cancellation, failure propagation, and stall detection have one
    /// authority: [`RuntimeDagExecutor`].
    pub async fn execute_all(&self) -> Result<Vec<TaskExecutionResult>> {
        if self.task_manager.get_all_tasks().is_empty() {
            return Ok(Vec::new());
        }

        let max_concurrent = self.config.max_concurrent.max(1);
        let controller = Arc::new(ManagedTaskDagController::new(self.clone()));
        let runtime = RuntimeDagExecutor::new(
            controller.clone(),
            RuntimeDagExecutorConfig {
                max_concurrent_subagents: max_concurrent,
                external_progress_poll_interval: Duration::from_millis(250),
                cancellation_grace_period: Duration::from_secs(5),
                delegation_policy: NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: 0,
                    max_delegate_depth: 2,
                },
            },
        )
        .with_validator(PlanValidator {
            max_tasks: usize::MAX,
            max_depth: usize::MAX,
            require_acceptance_criteria: false,
            require_verification: false,
            max_retries: u32::MAX,
        });
        let outcome = runtime
            .execute("framework-task-executor", self.cancel.clone())
            .await?;
        let results = controller.take_results().await;
        match outcome {
            RuntimeDagOutcome::Completed | RuntimeDagOutcome::Cancelled => Ok(results),
            RuntimeDagOutcome::Failed { error, .. } | RuntimeDagOutcome::Paused { error, .. } => {
                Err(ReactError::Other(error))
            }
        }
    }

    /// Spawn the current ready frontier as non-blocking background tasks.
    ///
    /// Returns handles immediately — the caller's ReAct loop is not blocked.
    /// Each returned [`BackgroundTask`] can be polled for status, awaited, or cancelled.
    ///
    /// This is a one-wave primitive, not a second full DAG executor. Completion
    /// wakes dependents; callers may invoke this method again for the next
    /// frontier. Use [`execute_all`](Self::execute_all) for framework-owned
    /// traversal through [`RuntimeDagExecutor`].
    pub fn execute_all_async(
        &self,
    ) -> Vec<super::background_task::BackgroundTask<TaskExecutionResult>> {
        let ready_tasks = self.task_manager.get_ready_tasks();
        if ready_tasks.is_empty() {
            return Vec::new();
        }

        info!(
            tasks = ready_tasks.len(),
            "Spawning {} tasks as background tasks (non-blocking)",
            ready_tasks.len()
        );

        let spawner = self.shared_spawner.clone().unwrap_or_else(|| {
            Arc::new(super::background_task::TaskSpawner::new(
                super::background_task::TaskSpawnerConfig {
                    max_concurrent: self.config.max_concurrent,
                    default_timeout_secs: self.config.default_timeout_secs,
                },
            ))
        });

        let mut handles = Vec::with_capacity(ready_tasks.len());
        for task in ready_tasks {
            let task_id = task.id.clone();
            let task_name = format!("dag-task-{}", &task_id);

            // Clone everything needed for the spawn
            let manager = self.task_manager.clone();
            let config = self.config.clone();
            let execute_fn = task.execute_fn.clone().or_else(|| self.execute_fn.clone());
            let hooks = self.hooks.clone();
            let semaphore = self.semaphore.clone();
            let task_store = self.task_store.clone();
            let verifier = self.verifier.clone();
            let parent_cancel = self.cancel.clone();

            let handle = spawner.spawn(&task_name, async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|e| ReactError::Other(format!("Semaphore error: {e}")))?;

                let result = Self::run_task_with_retry(
                    task,
                    manager.clone(),
                    config,
                    execute_fn,
                    hooks,
                    parent_cancel.child_token(),
                    task_store,
                    verifier,
                    None, // replanner
                )
                .await;

                // Wake dependents so the next batch becomes ready
                if matches!(result.status, TaskStatus::Completed) {
                    manager.wake_dependents(&task_id);
                }

                Ok(result)
            });

            handles.push(handle);
        }

        handles
    }

    /// Resume incomplete tasks from a persistent task store after a process restart.
    ///
    /// Tasks that were `Running` when the process died are reset to `Pending`
    /// so the executor can pick them up. The `retry_count` is preserved so
    /// retry logic continues from where it left off.
    ///
    /// **Important:** Since `execute_fn` is not serializable, callers must
    /// re-register execute functions (via `with_execute_fn` or per-task
    /// `execute_fn`) before calling this method.
    pub async fn resume_from_store(&self) -> Result<Vec<TaskExecutionResult>> {
        let store = self
            .task_store
            .as_ref()
            .ok_or_else(|| ReactError::Other("No task store configured".into()))?;

        let all_tasks = store.load_all().await?;
        let incomplete: Vec<super::ManagedTask> = all_tasks
            .into_iter()
            .filter(|t| !t.status.is_terminal())
            .collect();

        if incomplete.is_empty() {
            info!("No incomplete tasks to resume from store");
            return Ok(Vec::new());
        }

        info!(
            count = incomplete.len(),
            "Resuming {} incomplete tasks from store",
            incomplete.len()
        );

        // Re-add incomplete tasks to the TaskManager
        for mut task in incomplete {
            // Reset Running → Pending so execute_ready_tasks picks them up
            if matches!(task.status, TaskStatus::Running) {
                task.status = TaskStatus::Pending;
            }
            self.task_manager.add_task(task);
        }

        // Now run the normal execution loop for the resumed tasks
        self.execute_all().await
    }
}

#[async_trait]
impl RuntimeDagController for ManagedTaskDagController {
    type DispatchOutput = TaskExecutionResult;

    async fn load_snapshot(&self, _run_id: &str) -> Result<RuntimePlanSnapshot> {
        let mut tasks = self.executor.task_manager.get_all_tasks();
        tasks.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(RuntimePlanSnapshot {
            // TaskManager is an in-memory authority without revision commits.
            // The kernel still reloads it at every safe point.
            revision: 0,
            tasks: tasks.iter().map(ManagedTask::to_task).collect(),
        })
    }

    async fn claim_task(
        &self,
        _run_id: &str,
        task: &Task,
        expected_revision: u64,
    ) -> Result<RuntimeTaskClaimOutcome> {
        if expected_revision != 0
            || !self
                .executor
                .task_manager
                .claim_pending_task(&task.spec.id, &task.spec)
                .map_err(ReactError::Other)?
        {
            return Ok(RuntimeTaskClaimOutcome::ReloadSnapshot);
        }
        let claim = TaskClaim {
            revision: expected_revision,
            attempt: task.execution.retry_count.saturating_add(1),
            spec_hash: task.spec.stable_hash().map_err(ReactError::Other)?,
        };
        self.claims
            .lock()
            .await
            .insert(task.spec.id.clone(), claim.clone());
        Ok(RuntimeTaskClaimOutcome::Claimed(claim))
    }

    fn select_ready_wave(&self, _tasks: &[Task], ready_task_ids: Vec<String>) -> Vec<String> {
        let mut ready_tasks: Vec<ManagedTask> = ready_task_ids
            .iter()
            .filter_map(|task_id| self.executor.task_manager.get_task(task_id))
            .collect();
        ready_tasks.sort_by_key(|task| std::cmp::Reverse(task.priority));

        let Some(scheduler) = self.executor.scheduler.as_ref() else {
            return ready_tasks.into_iter().map(|task| task.id).collect();
        };
        let schedule = scheduler.schedule(&ready_tasks);
        if let Some(group) = schedule
            .parallel_groups
            .into_iter()
            .find(|group| !group.is_empty())
        {
            return group;
        }
        if let Some(task_id) = schedule.serial_sequence.into_iter().next() {
            return vec![task_id];
        }

        ready_tasks
            .into_iter()
            .next()
            .map(|task| vec![task.id])
            .unwrap_or_default()
    }

    async fn dispatch_task(
        &self,
        context: TaskSubagentContext,
        _claim: TaskClaim,
        runtime_task: Task,
    ) -> Result<Self::DispatchOutput> {
        let task = self
            .executor
            .task_manager
            .get_task(&runtime_task.spec.id)
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "runtime-selected task '{}' no longer exists",
                    runtime_task.spec.id
                ))
            })?;

        match &task.status {
            TaskStatus::Running => Ok(self
                .executor
                .execute_selected_task(task, context.cancel)
                .await),
            TaskStatus::Completed => Ok(TaskExecutionResult::success(
                &task.id,
                task.result.clone().unwrap_or_default(),
                Duration::ZERO,
                task.retry_count.saturating_add(1),
            )),
            TaskStatus::Cancelled => Ok(TaskExecutionResult::cancelled(&task.id)),
            TaskStatus::Skipped => Ok(TaskExecutionResult {
                task_id: task.id,
                status: TaskStatus::Skipped,
                output: task.result,
                error: None,
                duration: Duration::ZERO,
                attempts: task.retry_count,
            }),
            TaskStatus::Failed(error)
            | TaskStatus::Blocked(error)
            | TaskStatus::Paused(error)
            | TaskStatus::TimedOut { error } => Ok(TaskExecutionResult {
                task_id: task.id,
                status: task.status.clone(),
                output: task.result,
                error: Some(error.clone()),
                duration: Duration::ZERO,
                attempts: task.retry_count,
            }),
            TaskStatus::Pending | TaskStatus::Retrying { .. } => Err(ReactError::Other(format!(
                "runtime-selected task '{}' was not claimed for dispatch",
                task.id
            ))),
        }
    }

    async fn resolve_dispatch(
        &self,
        _run_id: &str,
        claim: TaskClaim,
        runtime_task: Task,
        dispatch: Result<Self::DispatchOutput>,
    ) -> Result<RuntimeTaskResolution> {
        let current_claim = self.claims.lock().await.get(&runtime_task.spec.id).cloned();
        if current_claim.as_ref() != Some(&claim) {
            return Ok(RuntimeTaskResolution::Superseded);
        }
        let result = match dispatch {
            Ok(result) => result,
            Err(error) => {
                let error = error.to_string();
                if let Some(task) = self.executor.task_manager.get_task(&runtime_task.spec.id)
                    && matches!(task.status, TaskStatus::Running)
                {
                    self.executor
                        .task_manager
                        .update_task_status(&task.id, TaskStatus::Failed(error.clone()))
                        .map_err(ReactError::Other)?;
                }
                TaskExecutionResult::failure(&runtime_task.spec.id, error, Duration::ZERO, 0)
            }
        };

        if matches!(result.status, TaskStatus::Cancelled) {
            let _ = self
                .executor
                .task_manager
                .cancel_task(&runtime_task.spec.id);
        }
        self.results.lock().await.push(result.clone());

        let current = self.executor.task_manager.get_task(&runtime_task.spec.id);
        if let Some(task) = current.as_ref()
            && matches!(
                task.status,
                TaskStatus::Cancelled
                    | TaskStatus::Blocked(_)
                    | TaskStatus::Paused(_)
                    | TaskStatus::Skipped
            )
        {
            self.persist_task(task).await?;
        }
        let status = current.map(|task| task.status).unwrap_or(result.status);
        let resolution = match status {
            TaskStatus::Pending => Ok(RuntimeTaskResolution::Pending),
            TaskStatus::Completed => Ok(RuntimeTaskResolution::Completed),
            TaskStatus::Skipped => Ok(RuntimeTaskResolution::Skipped),
            TaskStatus::Cancelled => Ok(RuntimeTaskResolution::Cancelled),
            TaskStatus::Failed(error) | TaskStatus::TimedOut { error } => {
                Ok(RuntimeTaskResolution::Failed { error })
            }
            TaskStatus::Blocked(error) => Ok(RuntimeTaskResolution::Blocked {
                error,
                disposition: RuntimeStopDisposition::Fail,
            }),
            TaskStatus::Paused(error) => Ok(RuntimeTaskResolution::Blocked {
                error,
                disposition: RuntimeStopDisposition::Pause,
            }),
            TaskStatus::Running | TaskStatus::Retrying { .. } => Err(ReactError::Other(format!(
                "task '{}' remained running after dispatch resolved",
                runtime_task.spec.id
            ))),
        };
        self.claims.lock().await.remove(&runtime_task.spec.id);
        resolution
    }

    async fn block_task(&self, _run_id: &str, task: &Task, reason: &str) -> Result<()> {
        let Some(current) = self.executor.task_manager.get_task(&task.spec.id) else {
            return Ok(());
        };
        if matches!(current.status, TaskStatus::Pending | TaskStatus::Running) {
            self.executor
                .task_manager
                .update_task_status(&task.spec.id, TaskStatus::Blocked(reason.to_string()))
                .map_err(ReactError::Other)?;
            if let Some(blocked) = self.executor.task_manager.get_task(&task.spec.id) {
                self.persist_task(&blocked).await?;
            }
        }
        Ok(())
    }

    async fn failed_task_disposition(
        &self,
        _run_id: &str,
        _task: &Task,
        _all_unfinished_failed_or_blocked: bool,
    ) -> Result<RuntimeStopDisposition> {
        Ok(RuntimeStopDisposition::Fail)
    }

    async fn interruption_outcome(&self, _run_id: &str) -> Result<RuntimeDagOutcome> {
        self.executor.cancel_all();
        let cancelled_tasks: Vec<ManagedTask> = self
            .executor
            .task_manager
            .get_all_tasks()
            .into_iter()
            .filter(|task| matches!(task.status, TaskStatus::Cancelled))
            .collect();
        for task in &cancelled_tasks {
            self.persist_task(task).await?;
        }
        let mut results = self.results.lock().await;
        for task in cancelled_tasks {
            if !results.iter().any(|result| result.task_id == task.id) {
                results.push(TaskExecutionResult::cancelled(&task.id));
            }
        }
        Ok(RuntimeDagOutcome::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_status_transitions() {
        use TaskStatus::*;

        // Valid transitions
        assert!(Pending.can_transition_to(&Running));
        assert!(Pending.can_transition_to(&Cancelled));
        assert!(Running.can_transition_to(&Completed));
        assert!(Running.can_transition_to(&Failed("test".into())));

        // Invalid transitions
        assert!(!Completed.can_transition_to(&Running));
        assert!(!Failed("test".into()).can_transition_to(&Running));
        assert!(!Pending.can_transition_to(&Completed)); // Must go through Running
    }

    #[test]
    fn test_transition_to_valid() {
        use TaskStatus::*;

        // Valid: Pending → Running → Completed
        let result = Pending.transition_to(Running);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Running);

        let result = Running.transition_to(Completed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_transition_to_invalid() {
        use TaskStatus::*;

        // Invalid: Completed → Running
        let result = Completed.transition_to(Running);
        assert!(result.is_err());

        // Invalid: Pending → Completed (must go through Running)
        let result = Pending.transition_to(Completed);
        assert!(result.is_err());
    }

    #[test]
    fn test_manager_update_task_validates() {
        let manager = TaskManager::new();
        manager.add_task(ManagedTask::new("t1", "Test"));

        // Valid: Pending → Running
        assert!(manager.update_task("t1", TaskStatus::Running).is_ok());

        // Valid: Running → Completed
        assert!(manager.update_task("t1", TaskStatus::Completed).is_ok());

        // Invalid: Completed → Running
        assert!(manager.update_task("t1", TaskStatus::Running).is_err());

        // Non-existent task
        assert!(manager.update_task("t99", TaskStatus::Running).is_err());
    }

    #[test]
    fn test_task_status_is_terminal() {
        use TaskStatus::*;

        assert!(!Pending.is_terminal());
        assert!(!Running.is_terminal());
        assert!(Completed.is_terminal());
        assert!(Cancelled.is_terminal());
        assert!(Failed("test".into()).is_terminal());
        assert!(
            TimedOut {
                error: "test".into(),
            }
            .is_terminal()
        );
    }

    #[test]
    fn test_execution_result() {
        let result =
            TaskExecutionResult::success("task1", "output".to_string(), Duration::from_secs(5), 1);
        assert_eq!(result.task_id, "task1");
        assert_eq!(result.status, TaskStatus::Completed);
        assert!(result.output.is_some());

        let result =
            TaskExecutionResult::failure("task1", "error".to_string(), Duration::from_secs(5), 1);
        assert_eq!(result.status, TaskStatus::Failed("error".to_string()));
    }

    #[tokio::test]
    async fn test_executor_parallel_execution() {
        let manager = Arc::new(TaskManager::new());

        // Add independent tasks
        manager.add_task(ManagedTask::new("t1", "Task 1"));
        manager.add_task(ManagedTask::new("t2", "Task 2"));
        manager.add_task(ManagedTask::new("t3", "Task 3"));

        let config = TaskExecutorConfig {
            max_concurrent: 3,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };

        let executor = TaskExecutor::new(manager.clone(), config);
        let results = executor.execute_ready_tasks().await.unwrap();

        assert_eq!(results.len(), 3);
        assert!(executor.is_completed());
    }

    #[tokio::test]
    async fn test_executor_dependency_order() {
        let manager = Arc::new(TaskManager::new());

        // t1 → t2 → t3 (linear chain)
        manager.add_task(ManagedTask::new("t1", "First"));
        manager.add_task(ManagedTask::new("t2", "Second").with_dependencies(vec!["t1".into()]));
        manager.add_task(ManagedTask::new("t3", "Third").with_dependencies(vec!["t2".into()]));

        let config = TaskExecutorConfig {
            max_concurrent: 3,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };

        let executor = TaskExecutor::new(manager.clone(), config);

        // First round: only t1 is ready
        let results = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, "t1");

        // Second round: t2 is now ready
        let results = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, "t2");

        // Third round: t3 is now ready
        let results = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].task_id, "t3");

        assert!(executor.is_completed());
    }

    #[tokio::test]
    async fn execute_all_delegates_dependency_order_to_runtime_kernel() -> Result<()> {
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "First"));
        manager.add_task(ManagedTask::new("t2", "Second").with_dependencies(vec!["t1".into()]));
        manager.add_task(ManagedTask::new("t3", "Third").with_dependencies(vec!["t2".into()]));
        let execution_order = Arc::new(Mutex::new(Vec::new()));
        let order_for_execution = execution_order.clone();
        let executor = TaskExecutor::new(manager, TaskExecutorConfig::default()).with_execute_fn(
            Arc::new(move |context| {
                let order = order_for_execution.clone();
                Box::pin(async move {
                    order.lock().await.push(context.task_id.clone());
                    Ok(format!("{} complete", context.task_id))
                })
            }),
        );

        let results = executor.execute_all().await?;
        assert_eq!(
            results
                .iter()
                .map(|result| result.task_id.as_str())
                .collect::<Vec<_>>(),
            vec!["t1", "t2", "t3"]
        );
        assert_eq!(execution_order.lock().await.as_slice(), ["t1", "t2", "t3"]);
        Ok(())
    }

    #[tokio::test]
    async fn execute_all_uses_runtime_kernel_failure_propagation() -> Result<()> {
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "Failing task"));
        manager.add_task(
            ManagedTask::new("t2", "Dependent task").with_dependencies(vec!["t1".into()]),
        );
        let executor = TaskExecutor::new(manager.clone(), TaskExecutorConfig::default())
            .with_execute_fn(Arc::new(|context| {
                Box::pin(async move {
                    if context.task_id == "t1" {
                        Err(ReactError::Other("planned failure".to_string()))
                    } else {
                        Ok("dependent should not run".to_string())
                    }
                })
            }));

        let outcome = executor.execute_all().await;
        assert!(outcome.is_err());
        let failed = manager
            .get_task("t1")
            .ok_or_else(|| ReactError::Other("failed task is missing".to_string()))?;
        let blocked = manager
            .get_task("t2")
            .ok_or_else(|| ReactError::Other("blocked task is missing".to_string()))?;
        assert!(matches!(failed.status, TaskStatus::Failed(_)));
        assert!(matches!(blocked.status, TaskStatus::Blocked(_)));
        Ok(())
    }

    #[tokio::test]
    async fn test_executor_custom_execute_fn() {
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "Custom task"));

        let config = TaskExecutorConfig {
            max_concurrent: 1,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };

        let executor =
            TaskExecutor::new(manager.clone(), config).with_execute_fn(Arc::new(|_ctx| {
                Box::pin(async { Ok("custom result".to_string()) })
            }));

        let results = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output.as_deref(), Some("custom result"));
    }

    #[test]
    fn test_task_context_format_upstream() {
        let ctx = TaskContext::with_upstream(
            "t2",
            "Second task",
            vec![
                ("Step A".to_string(), "result A".to_string()),
                ("Step B".to_string(), "result B".to_string()),
            ],
        );
        let text = ctx.format_upstream_context();
        assert!(text.contains("Step A"));
        assert!(text.contains("result A"));
        assert!(text.contains("Step B"));
    }

    #[test]
    fn test_task_context_empty_upstream() {
        let ctx = TaskContext::new("t1", "Simple task");
        assert!(ctx.format_upstream_context().is_empty());
    }

    #[test]
    fn task_context_limit_counts_unicode_characters() {
        let ctx = TaskContext::with_upstream(
            "t2",
            "Second task",
            vec![("t1".to_string(), "你好世界".to_string())],
        );
        assert!(
            ctx.format_upstream_context_with_limit(2)
                .contains("你好...")
        );
    }

    #[tokio::test]
    async fn zero_timeout_disables_task_timeout() -> Result<()> {
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "No timeout"));
        let config = TaskExecutorConfig {
            default_timeout_secs: 0,
            enable_hooks: false,
            retry_jitter: false,
            ..TaskExecutorConfig::default()
        };
        let executor = TaskExecutor::new(manager, config).with_execute_fn(Arc::new(|_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Ok("completed".to_string())
            })
        }));

        let results = executor.execute_all().await?;
        assert_eq!(results.len(), 1);
        let result = results
            .first()
            .ok_or_else(|| ReactError::Other("missing task result".to_string()))?;
        assert!(matches!(result.status, TaskStatus::Completed));
        Ok(())
    }

    #[tokio::test]
    async fn test_executor_upstream_context_passed() {
        let manager = Arc::new(TaskManager::new());

        // t1 → t2
        manager.add_task(ManagedTask::new("t1", "First task"));
        manager
            .add_task(ManagedTask::new("t2", "Second task").with_dependencies(vec!["t1".into()]));

        let config = TaskExecutorConfig {
            max_concurrent: 2,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };

        let executor = TaskExecutor::new(manager.clone(), config).with_execute_fn(Arc::new(
            |ctx: TaskContext| {
                Box::pin(async move {
                    // If this is t2, upstream should contain t1's result
                    if ctx.task_id == "t1" {
                        Ok("first result".to_string())
                    } else {
                        // t2 should have upstream context
                        let upstream = ctx.format_upstream_context();
                        if upstream.contains("first result") {
                            Ok("second result with context".to_string())
                        } else {
                            Ok("second result without context".to_string())
                        }
                    }
                })
            },
        ));

        // Execute t1 first
        let r1 = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(r1.len(), 1);
        assert_eq!(r1[0].output.as_deref(), Some("first result"));

        // Execute t2 — should receive t1's result as upstream context
        let r2 = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(r2.len(), 1);
        assert_eq!(r2[0].output.as_deref(), Some("second result with context"));
    }
    /// Phase 3.1: run_dag failure propagation — when a dependency fails,
    /// dependents should be marked as Blocked, not run.
    #[tokio::test]
    async fn test_run_dag_failure_propagation() {
        use std::sync::Arc;
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "Failing task"));
        manager.add_task(
            ManagedTask::new("t2", "Dependent task").with_dependencies(vec!["t1".into()]),
        );
        let config = TaskExecutorConfig {
            max_concurrent: 2,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };
        let executor = TaskExecutor::new(manager.clone(), config).with_execute_fn(Arc::new(
            |ctx: TaskContext| {
                Box::pin(async move {
                    if ctx.task_id == "t1" {
                        Err(ReactError::Other("t1 failed".into()))
                    } else {
                        Ok("t2 should not run".to_string())
                    }
                })
            },
        ));
        let r1 = executor.execute_ready_tasks().await.unwrap();
        assert_eq!(r1.len(), 1);
        assert!(r1[0].error.is_some(), "t1 should have failed");
        let r2 = executor.execute_ready_tasks().await.unwrap();
        assert!(r2.is_empty(), "t2 should not run after t1 failure");
    }

    #[tokio::test]
    async fn test_run_dag_cancel_propagation() {
        use std::sync::Arc;
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "Long task"));
        let config = TaskExecutorConfig {
            max_concurrent: 1,
            default_timeout_secs: 60,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };
        let executor = TaskExecutor::new(manager.clone(), config).with_execute_fn(Arc::new(
            |_ctx: TaskContext| {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    Ok("finished".to_string())
                })
            },
        ));
        let exec = Arc::new(executor);
        let exec_clone = exec.clone();
        let h = tokio::spawn(async move { exec_clone.execute_all().await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        exec.cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), h)
            .await
            .expect("timeout")
            .expect("join");
        assert!(result.is_ok(), "execute_all should return Ok after cancel");
    }

    /// Phase 3: verify `cancel_all()` propagates to multiple concurrent child
    /// tasks. Registers 3 independent tasks, all sleeping, then calls
    /// `cancel_all()`. All must transition to Cancelled and none should produce
    /// a finished result (the shared counter stays at 0).
    #[tokio::test]
    async fn test_cancel_all_propagates_to_concurrent_children() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("t1", "Long A"));
        manager.add_task(ManagedTask::new("t2", "Long B"));
        manager.add_task(ManagedTask::new("t3", "Long C"));

        let finished = Arc::new(AtomicUsize::new(0));
        let config = TaskExecutorConfig {
            max_concurrent: 3,
            default_timeout_secs: 60,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            unified_hook_executor: None,
        };
        let finished_clone = finished.clone();
        let executor = TaskExecutor::new(manager.clone(), config).with_execute_fn(Arc::new(
            move |_ctx: TaskContext| {
                let fc = finished_clone.clone();
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    fc.fetch_add(1, Ordering::Relaxed);
                    Ok("finished".to_string())
                })
            },
        ));
        let exec = Arc::new(executor);
        let exec_clone = exec.clone();
        let h = tokio::spawn(async move { exec_clone.execute_all().await });

        // Let all 3 tasks enter Running.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Cancel ALL (not just root token — the real API).
        exec.cancel_all();

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), h)
            .await
            .expect("timeout")
            .expect("join");
        assert!(
            result.is_ok(),
            "execute_all should return Ok after cancel_all"
        );

        // None should have finished (all interrupted mid-sleep).
        assert_eq!(
            finished.load(Ordering::Relaxed),
            0,
            "no task should have produced a result after cancel_all"
        );

        // All three tasks should be in a terminal (Cancelled or Failed) state.
        for id in &["t1", "t2", "t3"] {
            let task = manager.get_task(id).expect("task exists");
            assert!(
                task.status.is_terminal(),
                "task {} should be terminal after cancel_all, got {:?}",
                id,
                task.status
            );
        }
    }

    /// Phase 3: verify a Blocked task resumes execution after being reset to
    /// Pending. Uses the manager API directly to simulate Blocked → Pending
    /// transition (the same transition the replanner or external recovery
    /// would trigger).
    #[tokio::test]
    async fn test_blocked_task_resumes_after_reset() {
        use std::sync::Arc;
        let manager = Arc::new(TaskManager::new());
        manager.add_task(ManagedTask::new("bt", "Blocked task"));

        // Simulate the real lifecycle: Pending → Running → Blocked (which
        // is the executor's actual path when an upstream task fails).
        manager
            .update_task_status("bt", TaskStatus::Running)
            .expect("Pending→Running is legal");
        manager
            .update_task_status("bt", TaskStatus::Blocked("upstream died".into()))
            .expect("Running→Blocked is legal");
        assert!(matches!(
            manager.get_task("bt").unwrap().status,
            TaskStatus::Blocked(_)
        ));

        let config = TaskExecutorConfig::default();
        let executor =
            TaskExecutor::new(manager.clone(), config).with_execute_fn(Arc::new(|_ctx| {
                Box::pin(async { Ok("recovered".to_string()) })
            }));
        let exec = Arc::new(executor);

        // execute_ready_tasks should NOT pick up a Blocked task.
        let result = exec.execute_ready_tasks().await;
        assert!(
            result.is_ok(),
            "blocked task scan should succeed: {result:?}"
        );
        assert!(
            !matches!(
                manager.get_task("bt").unwrap().status,
                TaskStatus::Completed
            ),
            "Blocked task must not execute until reset to Pending"
        );

        // Reset Blocked → Pending (the recovery path).
        manager
            .update_task_status("bt", TaskStatus::Pending)
            .expect("Blocked→Pending is legal");

        // Now execute_ready_tasks should pick it up and complete it.
        let result = exec.execute_ready_tasks().await;
        assert!(
            result.is_ok(),
            "pending task execution should succeed: {result:?}"
        );
        assert!(
            matches!(
                manager.get_task("bt").unwrap().status,
                TaskStatus::Completed
            ),
            "Reset task should complete after Pending reset"
        );
    }
}
