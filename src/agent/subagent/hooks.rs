//! Subagent execution hooks — lifecycle callbacks for subagent dispatch
//!
//! Mirrors the pattern from `crate::tasks::hooks` but tailored for
//! subagent-specific lifecycle points.

use std::sync::Arc;

use super::types::{ExecutionMode, SubagentResult};

// ── Hook Context ──────────────────────────────────────────────────────────────

/// Context passed to subagent hook callbacks.
#[derive(Debug, Clone)]
pub struct SubagentHookContext {
    /// Parent agent name.
    pub parent_agent: String,
    /// Subagent name being dispatched.
    pub subagent_name: String,
    /// Execution mode for this dispatch.
    pub execution_mode: ExecutionMode,
    /// The task being dispatched.
    pub task: String,
    /// Current attempt number (1-based, for retries).
    pub attempt: u32,
}

// ── Retry Decision ────────────────────────────────────────────────────────────

/// Decision returned by `on_failure` hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentRetryDecision {
    /// Retry the dispatch after a delay.
    Retry {
        /// Delay in seconds before retrying.
        delay_secs: u64,
    },
    /// Fail and propagate the error.
    Fail,
    /// Delegate to an alternative subagent.
    Delegate {
        /// Name of the alternative subagent to delegate to.
        alternative_agent: String,
    },
}

// ── Subagent Hooks Trait ──────────────────────────────────────────────────────

/// Hook trait for subagent lifecycle events.
///
/// All methods have default no-op implementations.
#[async_trait::async_trait]
pub trait SubagentHooks: Send + Sync {
    /// Called before a subagent is dispatched.
    async fn before_dispatch(&self, _ctx: &SubagentHookContext) {}

    /// Called after a subagent completes successfully.
    async fn after_dispatch(&self, _ctx: &SubagentHookContext, _result: &SubagentResult) {}

    /// Called when a subagent fails.
    ///
    /// Return a decision to control retry/delegation behavior.
    async fn on_failure(&self, _ctx: &SubagentHookContext, _error: &str) -> SubagentRetryDecision {
        SubagentRetryDecision::Fail
    }

    /// Called when a subagent is cancelled.
    async fn on_cancelled(&self, _ctx: &SubagentHookContext) {}
}

// ── Built-in Implementations ──────────────────────────────────────────────────

/// No-op hooks (default).
pub struct NoopSubagentHooks;

#[async_trait::async_trait]
impl SubagentHooks for NoopSubagentHooks {}

/// Logging hooks that emit tracing events.
pub struct LoggingSubagentHooks;

#[async_trait::async_trait]
impl SubagentHooks for LoggingSubagentHooks {
    async fn before_dispatch(&self, ctx: &SubagentHookContext) {
        tracing::info!(
            parent = %ctx.parent_agent,
            subagent = %ctx.subagent_name,
            mode = %ctx.execution_mode,
            task = %ctx.task,
            "subagent_before_dispatch"
        );
    }

    async fn after_dispatch(&self, ctx: &SubagentHookContext, result: &SubagentResult) {
        let preview = if result.output.len() > 100 {
            format!("{}...", &result.output[..100])
        } else {
            result.output.clone()
        };
        tracing::info!(
            parent = %ctx.parent_agent,
            subagent = %ctx.subagent_name,
            duration_ms = result.duration.as_millis(),
            output = %preview,
            "subagent_after_dispatch"
        );
    }

    async fn on_failure(&self, ctx: &SubagentHookContext, error: &str) -> SubagentRetryDecision {
        tracing::warn!(
            parent = %ctx.parent_agent,
            subagent = %ctx.subagent_name,
            attempt = ctx.attempt,
            error = %error,
            "subagent_on_failure"
        );
        SubagentRetryDecision::Fail
    }

    async fn on_cancelled(&self, ctx: &SubagentHookContext) {
        tracing::info!(
            parent = %ctx.parent_agent,
            subagent = %ctx.subagent_name,
            "subagent_on_cancelled"
        );
    }
}

// ── Hook Registry ─────────────────────────────────────────────────────────────

/// Registry for subagent hooks — allows multiple hooks to be registered
/// and called in sequence.
pub struct SubagentHookRegistry {
    hooks: Vec<Arc<dyn SubagentHooks>>,
}

impl SubagentHookRegistry {
    /// Create an empty hook registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Create a hook registry pre-configured with logging hooks.
    pub fn with_logging() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(LoggingSubagentHooks));
        registry
    }

    /// Register a hook implementation.
    ///
    /// # Parameters
    /// * `hook` - Hook implementation to add.
    pub fn register(&mut self, hook: Arc<dyn SubagentHooks>) {
        self.hooks.push(hook);
    }

    /// Call `before_dispatch` on all registered hooks.
    ///
    /// # Parameters
    /// * `ctx` - Hook context.
    pub async fn before_dispatch(&self, ctx: &SubagentHookContext) {
        for hook in &self.hooks {
            hook.before_dispatch(ctx).await;
        }
    }

    /// Call `after_dispatch` on all registered hooks.
    ///
    /// # Parameters
    /// * `ctx` - Hook context.
    /// * `result` - Subagent execution result.
    pub async fn after_dispatch(&self, ctx: &SubagentHookContext, result: &SubagentResult) {
        for hook in &self.hooks {
            hook.after_dispatch(ctx, result).await;
        }
    }

    /// Call `on_failure` on all hooks, returning the first non-Fail decision.
    pub async fn on_failure(
        &self,
        ctx: &SubagentHookContext,
        error: &str,
    ) -> SubagentRetryDecision {
        for hook in &self.hooks {
            let decision = hook.on_failure(ctx, error).await;
            if !matches!(decision, SubagentRetryDecision::Fail) {
                return decision;
            }
        }
        SubagentRetryDecision::Fail
    }

    /// Call `on_cancelled` on all registered hooks.
    ///
    /// # Parameters
    /// * `ctx` - Hook context.
    pub async fn on_cancelled(&self, ctx: &SubagentHookContext) {
        for hook in &self.hooks {
            hook.on_cancelled(ctx).await;
        }
    }

    /// Check if the registry contains any hooks.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Get the number of registered hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }
}

impl Default for SubagentHookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SubagentHookRegistry {
    fn clone(&self) -> Self {
        Self {
            hooks: self.hooks.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    struct TestHooks {
        before: AtomicU32,
        after: AtomicU32,
        failure: AtomicU32,
    }

    impl TestHooks {
        fn new() -> Self {
            Self {
                before: AtomicU32::new(0),
                after: AtomicU32::new(0),
                failure: AtomicU32::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl SubagentHooks for TestHooks {
        async fn before_dispatch(&self, _ctx: &SubagentHookContext) {
            self.before.fetch_add(1, Ordering::SeqCst);
        }

        async fn after_dispatch(&self, _ctx: &SubagentHookContext, _result: &SubagentResult) {
            self.after.fetch_add(1, Ordering::SeqCst);
        }

        async fn on_failure(
            &self,
            _ctx: &SubagentHookContext,
            _error: &str,
        ) -> SubagentRetryDecision {
            self.failure.fetch_add(1, Ordering::SeqCst);
            SubagentRetryDecision::Retry { delay_secs: 1 }
        }
    }

    fn make_ctx() -> SubagentHookContext {
        SubagentHookContext {
            parent_agent: "parent".into(),
            subagent_name: "child".into(),
            execution_mode: ExecutionMode::Sync,
            task: "test task".into(),
            attempt: 1,
        }
    }

    #[tokio::test]
    async fn test_hooks_called() {
        let hooks = Arc::new(TestHooks::new());
        let mut registry = SubagentHookRegistry::new();
        registry.register(hooks.clone());

        let ctx = make_ctx();
        let result = SubagentResult::sync_result("child", "ok".into(), Duration::from_millis(100));

        registry.before_dispatch(&ctx).await;
        registry.after_dispatch(&ctx, &result).await;
        registry.on_failure(&ctx, "error").await;

        assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.after.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.failure.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_registry_default() {
        let registry = SubagentHookRegistry::default();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_with_logging() {
        let registry = SubagentHookRegistry::with_logging();
        assert_eq!(registry.len(), 1);
    }
}
