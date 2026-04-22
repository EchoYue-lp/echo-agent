//! Task executor — parallel execution with timeout, retry, and cancellation support
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
use super::store::{CheckpointStore, ExecutionCheckpoint};
use super::task::{Task, TaskStatus};
use dashmap::DashMap;
use echo_core::error::{ReactError, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Configuration for task executor
#[derive(Debug, Clone)]
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
    /// Checkpoint interval: seconds (0 = no checkpoint).
    pub checkpoint_interval_secs: u64,
}

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
            checkpoint_interval_secs: 0,
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
    /// Task ID
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
    /// Task ID
    pub task_id: String,
    /// Task description
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

        let mut parts = vec!["上游依赖任务的执行结果：".to_string()];
        for (id, result) in &self.upstream_results {
            // UTF-8 safe truncation
            let preview = if result.len() > char_limit {
                let end = result
                    .char_indices()
                    .take_while(|(idx, _)| *idx < char_limit)
                    .last()
                    .map(|(idx, c)| idx + c.len_utf8())
                    .unwrap_or(0);
                format!("{}...", &result[..end])
            } else {
                result.clone()
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
pub struct TaskExecutor {
    task_manager: Arc<TaskManager>,
    config: TaskExecutorConfig,
    semaphore: Arc<Semaphore>,
    execute_fn: Option<TaskExecuteFn>,
    hooks: Arc<TaskHookRegistry>,
    checkpoint_store: Option<Arc<dyn CheckpointStore>>,
    /// Tracks cancellation tokens for running tasks.
    /// Used by `cancel_task()` to abort in-flight executions.
    running_tasks: Arc<DashMap<String, CancellationToken>>,
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
            checkpoint_store: None,
            running_tasks: Arc::new(DashMap::new()),
        }
    }

    /// Set a custom execution function
    pub fn with_execute_fn(mut self, f: TaskExecuteFn) -> Self {
        self.execute_fn = Some(f);
        self
    }

    /// Set checkpoint store for periodic saving during execute_all
    pub fn with_checkpoint_store(mut self, store: Arc<dyn CheckpointStore>) -> Self {
        self.checkpoint_store = Some(store);
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
    /// Returns the number of tasks executed
    pub async fn execute_ready_tasks(&self) -> Result<Vec<TaskExecutionResult>> {
        let ready_tasks: Vec<Task> = self.task_manager.get_ready_tasks();

        if ready_tasks.is_empty() {
            return Ok(Vec::new());
        }

        info!(
            tasks = ready_tasks.len(),
            max_concurrent = self.config.max_concurrent,
            "Executing {} ready tasks with max {} concurrent",
            ready_tasks.len(),
            self.config.max_concurrent
        );

        let mut handles = Vec::with_capacity(ready_tasks.len());

        for task in ready_tasks {
            let permit = self
                .semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| ReactError::Other(format!("Semaphore acquire error: {}", e)))?;
            let manager = self.task_manager.clone();
            let config = self.config.clone();
            let execute_fn = self.execute_fn.clone();
            let hooks = self.hooks.clone();
            let running_tasks = self.running_tasks.clone();
            let task_id = task.id.clone();
            let cancel = CancellationToken::new();
            let cancel_clone = cancel.clone();
            running_tasks.insert(task_id.clone(), cancel);

            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let start = Instant::now();

                // Check cancellation before starting
                if task.is_cancelled() || cancel_clone.is_cancelled() {
                    running_tasks.remove(&task_id);
                    return TaskExecutionResult::cancelled(&task_id);
                }

                // Execute with retry, racing against cancellation
                let manager2 = manager.clone();
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

        // Wait for all tasks to complete
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

    /// Run a single task with retry logic
    async fn run_task_with_retry(
        task: Task,
        manager: Arc<TaskManager>,
        config: TaskExecutorConfig,
        execute_fn: Option<TaskExecuteFn>,
        hooks: Arc<TaskHookRegistry>,
        cancel: CancellationToken,
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

        // Update status to InProgress
        let _ = manager.update_task_status(&task_id, TaskStatus::InProgress);

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
                let block_reason = format!("上游任务失败: {}", error_summary);
                let _ =
                    manager.update_task_status(&task_id, TaskStatus::Blocked(block_reason.clone()));
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

                        // Call on_timeout hook
                        if config.enable_hooks
                            && let Some(ctx) =
                                manager.create_hook_context(&task_id, current_attempt, None)
                        {
                            hooks.on_timeout(&ctx).await;
                        }

                        return result;
                    }
                    result = &mut execution => result,
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
                    let _ = manager.update_task_status(&task_id, TaskStatus::Completed);
                    manager.set_task_result(&task_id, output.clone());

                    // Call after_execute hook
                    if config.enable_hooks
                        && let Some(ctx) =
                            manager.create_hook_context(&task_id, current_attempt, None)
                    {
                        hooks.after_execute(&ctx, &output).await;
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

                                current_attempt += 1;
                                tokio::select! {
                                    biased;
                                    _ = cancel.cancelled() => {
                                        return TaskExecutionResult::cancelled(&task_id);
                                    }
                                    _ = tokio::time::sleep(Duration::from_secs(delay_secs)) => {}
                                }
                                continue;
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

                    // Final failure
                    let _ =
                        manager.update_task_status(&task_id, TaskStatus::Failed(error_str.clone()));
                    manager.record_task_execution(
                        &task_id,
                        current_attempt,
                        Some(error_str.clone()),
                        Some(start.elapsed().as_secs()),
                        None,
                    );

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

    /// Collect upstream dependency results and errors for a task
    ///
    /// Returns (successful_results, failed_errors).
    /// Only includes tasks that have reached a terminal state.
    fn collect_upstream_results_with_errors(
        task: &Task,
        manager: &Arc<TaskManager>,
    ) -> (Vec<(String, String)>, Vec<(String, String)>) {
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

    /// Run the full execution loop until all tasks complete or a deadlock is detected.
    ///
    /// This method repeatedly calls `execute_ready_tasks` and uses `wake_dependents`
    /// after each batch to discover newly-ready tasks, eliminating polling.
    ///
    /// Returns all execution results accumulated across batches.
    pub async fn execute_all(&self) -> Result<Vec<TaskExecutionResult>> {
        let mut all_results = Vec::new();
        let mut empty_rounds = 0;
        let mut batch_count: u64 = 0;

        loop {
            let results = self.execute_ready_tasks().await?;
            let batch_size = results.len();

            // Wake dependents for each completed task in this batch
            for r in &results {
                if matches!(r.status, TaskStatus::Completed) {
                    let newly_ready = self.task_manager.wake_dependents(&r.task_id);
                    if !newly_ready.is_empty() {
                        debug!(
                            task_id = %r.task_id,
                            newly_ready = newly_ready.len(),
                            "Wake dependents: {} new tasks ready",
                            newly_ready.len()
                        );
                    }
                }
            }

            all_results.extend(results);

            // Periodic checkpoint
            if batch_size > 0 {
                batch_count += 1;
                if self.config.checkpoint_interval_secs > 0
                    && let Some(ref store) = self.checkpoint_store
                {
                    let ckpt = ExecutionCheckpoint::from_manager(None, &self.task_manager);
                    if let Err(e) = store.save_checkpoint(&ckpt).await {
                        warn!(error = %e, "Failed to save checkpoint after batch {}", batch_count);
                    } else {
                        debug!(batch = batch_count, "Checkpoint saved");
                    }
                }
            }

            if self.is_completed() {
                break;
            }

            if batch_size == 0 {
                empty_rounds += 1;
                if empty_rounds >= 3 {
                    warn!("No tasks became ready after 3 consecutive rounds, possible deadlock");
                    if !self.is_completed() {
                        let (completed, total) = self.get_progress();
                        return Err(ReactError::Other(format!(
                            "Task execution stopped with incomplete tasks: {}/{} completed. Possible deadlock or unresolved dependencies.",
                            completed, total
                        )));
                    }
                    break;
                }
            } else {
                empty_rounds = 0;
            }
        }

        // Final checkpoint
        if let Some(ref store) = self.checkpoint_store {
            let ckpt = ExecutionCheckpoint::from_manager(None, &self.task_manager);
            if let Err(e) = store.save_checkpoint(&ckpt).await {
                warn!(error = %e, "Failed to save final checkpoint");
            }
        }

        Ok(all_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_status_transitions() {
        use TaskStatus::*;

        // Valid transitions
        assert!(Pending.can_transition_to(&InProgress));
        assert!(Pending.can_transition_to(&Cancelled));
        assert!(InProgress.can_transition_to(&Completed));
        assert!(InProgress.can_transition_to(&Failed("test".into())));

        // Invalid transitions
        assert!(!Completed.can_transition_to(&InProgress));
        assert!(!Failed("test".into()).can_transition_to(&InProgress));
        assert!(!Pending.can_transition_to(&Completed)); // Must go through InProgress
    }

    #[test]
    fn test_transition_to_valid() {
        use TaskStatus::*;

        // Valid: Pending → InProgress → Completed
        let result = Pending.transition_to(InProgress);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), InProgress);

        let result = InProgress.transition_to(Completed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_transition_to_invalid() {
        use TaskStatus::*;

        // Invalid: Completed → InProgress
        let result = Completed.transition_to(InProgress);
        assert!(result.is_err());

        // Invalid: Pending → Completed (must go through InProgress)
        let result = Pending.transition_to(Completed);
        assert!(result.is_err());
    }

    #[test]
    fn test_manager_update_task_validates() {
        let manager = TaskManager::new();
        manager.add_task(Task::new("t1", "Test"));

        // Valid: Pending → InProgress
        assert!(manager.update_task("t1", TaskStatus::InProgress).is_ok());

        // Valid: InProgress → Completed
        assert!(manager.update_task("t1", TaskStatus::Completed).is_ok());

        // Invalid: Completed → InProgress
        assert!(manager.update_task("t1", TaskStatus::InProgress).is_err());

        // Non-existent task
        assert!(manager.update_task("t99", TaskStatus::InProgress).is_err());
    }

    #[test]
    fn test_task_status_is_terminal() {
        use TaskStatus::*;

        assert!(!Pending.is_terminal());
        assert!(!InProgress.is_terminal());
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
        manager.add_task(Task::new("t1", "Task 1"));
        manager.add_task(Task::new("t2", "Task 2"));
        manager.add_task(Task::new("t3", "Task 3"));

        let config = TaskExecutorConfig {
            max_concurrent: 3,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            checkpoint_interval_secs: 0,
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
        manager.add_task(Task::new("t1", "First"));
        manager.add_task(Task::new("t2", "Second").with_dependencies(vec!["t1".into()]));
        manager.add_task(Task::new("t3", "Third").with_dependencies(vec!["t2".into()]));

        let config = TaskExecutorConfig {
            max_concurrent: 3,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            checkpoint_interval_secs: 0,
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
    async fn test_executor_custom_execute_fn() {
        let manager = Arc::new(TaskManager::new());
        manager.add_task(Task::new("t1", "Custom task"));

        let config = TaskExecutorConfig {
            max_concurrent: 1,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            checkpoint_interval_secs: 0,
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

    #[tokio::test]
    async fn test_executor_upstream_context_passed() {
        let manager = Arc::new(TaskManager::new());

        // t1 → t2
        manager.add_task(Task::new("t1", "First task"));
        manager.add_task(Task::new("t2", "Second task").with_dependencies(vec!["t1".into()]));

        let config = TaskExecutorConfig {
            max_concurrent: 2,
            default_timeout_secs: 10,
            enable_hooks: false,
            retry_delay_secs: 0,
            retry_backoff_factor: 2.0,
            retry_max_delay_secs: 60,
            retry_jitter: false,
            checkpoint_interval_secs: 0,
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
}
