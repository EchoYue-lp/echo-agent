//! ManagedTask execution hooks — lifecycle callbacks for task execution
//!
//! Hooks allow custom behavior to be injected at key points in the task lifecycle:
//! - `before_execute`: Called before a task starts execution
//! - `after_execute`: Called after a task completes successfully
//! - `on_failure`: Called when a task fails, can control retry behavior
//!
//! # Example
//!
//! ```rust
//! use async_trait::async_trait;
//! use echo_orchestration::tasks::{RetryDecision, TaskHookContext, TaskHooks};
//!
//! struct LoggingHooks;
//!
//! #[async_trait]
//! impl TaskHooks for LoggingHooks {
//!     async fn before_execute(&self, ctx: &TaskHookContext) {
//!         println!("Starting task: {}", ctx.task.subject);
//!     }
//!
//!     async fn after_execute(&self, ctx: &TaskHookContext, result: &str) {
//!         println!("Completed task: {} -> {}", ctx.task.subject, result);
//!     }
//!
//!     async fn on_failure(&self, ctx: &TaskHookContext, error: &str) -> RetryDecision {
//!         if ctx.task.retry_count < ctx.task.max_retries {
//!             RetryDecision::Retry { delay_secs: 1 }
//!         } else {
//!             RetryDecision::Fail
//!         }
//!     }
//! }
//!
//! let _ = LoggingHooks;
//! ```

use super::task::ManagedTask;
use std::sync::Arc;

/// Context passed to hook callbacks
#[derive(Debug, Clone)]
pub struct TaskHookContext {
    /// The task being executed
    pub task: ManagedTask,
    /// Current attempt number (1-based)
    pub attempt: u32,
    /// Agent executing the task
    pub executor: Option<String>,
}

/// Decision returned by `on_failure` hook
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    /// Retry the task after a delay
    Retry { delay_secs: u64 },
    /// Skip this task and continue
    Skip,
    /// Mark the task as failed
    Fail,
    /// Ignore the error and treat as success.
    ///
    /// **Warning**: Using this decision may cause data inconsistency.
    /// The task is marked as completed with the given message, but the
    /// underlying error was not resolved. Downstream tasks that depend
    /// on this task's output may receive incorrect or incomplete data.
    /// Only use this when you are certain the error is benign and
    /// will not affect subsequent processing.
    Ignore { message: String },
}

/// Trait for task execution hooks
///
/// Implement this trait to add custom behavior at key points in the task lifecycle.
/// All methods have default implementations that do nothing.
#[async_trait::async_trait]
pub trait TaskHooks: Send + Sync {
    /// Called before a task starts execution
    ///
    /// Use this for setup, logging, or validation before execution.
    async fn before_execute(&self, _ctx: &TaskHookContext) {}

    /// Called after a task completes successfully
    ///
    /// Use this for cleanup, logging, or post-processing.
    async fn after_execute(&self, _ctx: &TaskHookContext, _result: &str) {}

    /// Called when a task fails
    ///
    /// Return a `RetryDecision` to control what happens next.
    /// Default implementation returns `RetryDecision::Fail`.
    async fn on_failure(&self, _ctx: &TaskHookContext, _error: &str) -> RetryDecision {
        RetryDecision::Fail
    }

    /// Called when a task times out
    ///
    /// Default delegates to `on_failure`.
    async fn on_timeout(&self, ctx: &TaskHookContext) -> RetryDecision {
        self.on_failure(ctx, "Task timed out").await
    }

    /// Called when a task is cancelled
    ///
    /// Use this for cleanup when a task is cancelled.
    async fn on_cancelled(&self, _ctx: &TaskHookContext) {}

    /// Called when a task is about to be retried
    ///
    /// Return false to prevent the retry.
    async fn should_retry(&self, _ctx: &TaskHookContext, _error: &str) -> bool {
        true
    }
}

/// Default no-op hooks implementation
pub struct NoopHooks;

#[async_trait::async_trait]
impl TaskHooks for NoopHooks {}

/// Logging hooks that emit tracing events
pub struct LoggingHooks;

#[async_trait::async_trait]
impl TaskHooks for LoggingHooks {
    async fn before_execute(&self, ctx: &TaskHookContext) {
        tracing::info!(
            task_id = %ctx.task.id,
            subject = %ctx.task.subject,
            attempt = ctx.attempt,
            "task_before_execute"
        );
    }

    async fn after_execute(&self, ctx: &TaskHookContext, result: &str) {
        let preview = if result.chars().count() > 100 {
            let truncated: String = result.chars().take(100).collect();
            format!("{truncated}...")
        } else {
            result.to_string()
        };
        tracing::info!(
            task_id = %ctx.task.id,
            subject = %ctx.task.subject,
            result = %preview,
            "task_after_execute"
        );
    }

    async fn on_failure(&self, ctx: &TaskHookContext, error: &str) -> RetryDecision {
        tracing::warn!(
            task_id = %ctx.task.id,
            subject = %ctx.task.subject,
            attempt = ctx.attempt,
            error = %error,
            "task_on_failure"
        );
        if ctx.task.retry_count < ctx.task.max_retries {
            RetryDecision::Retry { delay_secs: 1 }
        } else {
            RetryDecision::Fail
        }
    }

    async fn on_timeout(&self, ctx: &TaskHookContext) -> RetryDecision {
        tracing::warn!(
            task_id = %ctx.task.id,
            subject = %ctx.task.subject,
            timeout_secs = ctx.task.timeout_secs,
            "task_on_timeout"
        );
        RetryDecision::Fail
    }

    async fn on_cancelled(&self, ctx: &TaskHookContext) {
        tracing::info!(
            task_id = %ctx.task.id,
            subject = %ctx.task.subject,
            "task_on_cancelled"
        );
    }
}

/// Registry for task hooks
///
/// Allows multiple hooks to be registered and called in sequence.
pub struct TaskHookRegistry {
    hooks: Vec<Arc<dyn TaskHooks>>,
}

impl TaskHookRegistry {
    /// Create an empty registry
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Create a registry with default logging hooks
    pub fn with_logging() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(LoggingHooks));
        registry
    }

    /// Register a hook
    pub fn register(&mut self, hook: Arc<dyn TaskHooks>) {
        self.hooks.push(hook);
    }

    /// Call `before_execute` on all registered hooks
    pub async fn before_execute(&self, ctx: &TaskHookContext) {
        for hook in &self.hooks {
            hook.before_execute(ctx).await;
        }
    }

    /// Call `after_execute` on all registered hooks
    pub async fn after_execute(&self, ctx: &TaskHookContext, result: &str) {
        for hook in &self.hooks {
            hook.after_execute(ctx, result).await;
        }
    }

    /// Call `on_failure` on all hooks, returning the first non-default decision
    pub async fn on_failure(&self, ctx: &TaskHookContext, error: &str) -> RetryDecision {
        for hook in &self.hooks {
            let decision = hook.on_failure(ctx, error).await;
            if decision != RetryDecision::Fail {
                return decision;
            }
        }
        RetryDecision::Fail
    }

    /// Call `on_timeout` on all hooks
    pub async fn on_timeout(&self, ctx: &TaskHookContext) -> RetryDecision {
        for hook in &self.hooks {
            let decision = hook.on_timeout(ctx).await;
            if decision != RetryDecision::Fail {
                return decision;
            }
        }
        RetryDecision::Fail
    }

    /// Call `on_cancelled` on all hooks
    pub async fn on_cancelled(&self, ctx: &TaskHookContext) {
        for hook in &self.hooks {
            hook.on_cancelled(ctx).await;
        }
    }

    /// Check if any hook wants to prevent retry
    pub async fn should_retry(&self, ctx: &TaskHookContext, error: &str) -> bool {
        for hook in &self.hooks {
            if !hook.should_retry(ctx, error).await {
                return false;
            }
        }
        true
    }

    /// Check if any hooks are registered
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Get the number of registered hooks
    pub fn len(&self) -> usize {
        self.hooks.len()
    }
}

impl Default for TaskHookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for TaskHookRegistry {
    fn clone(&self) -> Self {
        Self {
            hooks: self.hooks.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHooks {
        before_count: std::sync::atomic::AtomicU32,
        after_count: std::sync::atomic::AtomicU32,
        failure_count: std::sync::atomic::AtomicU32,
    }

    impl TestHooks {
        fn new() -> Self {
            Self {
                before_count: std::sync::atomic::AtomicU32::new(0),
                after_count: std::sync::atomic::AtomicU32::new(0),
                failure_count: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl TaskHooks for TestHooks {
        async fn before_execute(&self, _ctx: &TaskHookContext) {
            self.before_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        async fn after_execute(&self, _ctx: &TaskHookContext, _result: &str) {
            self.after_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        async fn on_failure(&self, _ctx: &TaskHookContext, _error: &str) -> RetryDecision {
            self.failure_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            RetryDecision::Skip
        }
    }

    #[tokio::test]
    async fn test_hooks_called() {
        let hooks = Arc::new(TestHooks::new());
        let mut registry = TaskHookRegistry::new();
        registry.register(hooks.clone());

        let task = ManagedTask::new("test", "Test task");
        let ctx = TaskHookContext {
            task,
            attempt: 1,
            executor: None,
        };

        registry.before_execute(&ctx).await;
        registry.after_execute(&ctx, "done").await;
        registry.on_failure(&ctx, "error").await;

        assert_eq!(
            hooks.before_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            hooks.after_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            hooks
                .failure_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[test]
    fn test_registry_default() {
        let registry = TaskHookRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_with_logging() {
        let registry = TaskHookRegistry::with_logging();
        assert_eq!(registry.len(), 1);
    }
}
