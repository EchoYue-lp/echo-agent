//! Unified hook bridge — connects task and subagent hook systems to the
//! central `HookRegistry` event model.
//!
//! The skills hooks system (in `echo-execution`) already supports
//! `TaskCreated`, `TaskCompleted`, `SubagentStart`, `SubagentStop` etc.
//! as `HookEvent` variants. This module provides adapter implementations
//! that fire those events when the trait-based task/subagent hook
//! systems trigger their callbacks.
//!
//! ## Architecture
//!
//! ```text
//! YAML hooks.yaml ──→ HookRegistry (25 events, 5 action types)
//!                          ↑
//! TaskHookBridge ──────┘  (fires TaskCreated/Completed/Timeout/Cancelled)
//! SubagentHookBridge ──┘  (fires SubagentStart/Stop/Cancelled)
//! ```
//!
//! Rust developers can still use the `TaskHooks` and `SubagentHooks`
//! traits directly. The bridges ensure that YAML-configured hooks also
//! see these lifecycle events.

use echo_core::hooks::{HookContext, HookEvent};
use echo_execution::skills::hooks::HookRegistry;
use std::sync::Arc;
use tokio::sync::RwLock;

// ── Task Hook Bridge ────────────────────────────────────────────────

/// Bridges `TaskHooks` trait callbacks to the unified `HookRegistry`.
///
/// When task lifecycle events occur (before_execute, after_execute, etc.),
/// this bridge fires the corresponding `HookEvent` in the central
/// `HookRegistry`, allowing YAML-configured hooks to react.
pub struct TaskHookBridge {
    hook_registry: Arc<RwLock<HookRegistry>>,
    session_id: String,
    agent_name: String,
}

impl TaskHookBridge {
    /// Create a new bridge.
    pub fn new(
        hook_registry: Arc<RwLock<HookRegistry>>,
        session_id: String,
        agent_name: String,
    ) -> Self {
        Self {
            hook_registry,
            session_id,
            agent_name,
        }
    }

    /// Fire a task lifecycle event in the central hook registry.
    async fn fire_event(&self, event: HookEvent, _task_id: &str, task_subject: &str) {
        let ctx =
            HookContext::for_lifecycle(event, task_subject, &self.session_id, &self.agent_name);
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire TaskCreated event (maps to before_execute).
    pub async fn on_before_execute(&self, task_id: &str, task_subject: &str) {
        self.fire_event(HookEvent::TaskCreated, task_id, task_subject)
            .await;
    }

    /// Fire TaskCompleted event (maps to after_execute).
    pub async fn on_after_execute(&self, task_id: &str, task_subject: &str) {
        self.fire_event(HookEvent::TaskCompleted, task_id, task_subject)
            .await;
    }

    /// Fire StopFailure event (maps to on_failure).
    pub async fn on_failure(&self, _task_id: &str, task_subject: &str, error: &str) {
        let ctx =
            HookContext::for_stop_failure(task_subject, error, &self.session_id, &self.agent_name);
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire TaskTimeout event.
    pub async fn on_timeout(&self, task_id: &str, task_subject: &str) {
        self.fire_event(HookEvent::TaskTimeout, task_id, task_subject)
            .await;
    }

    /// Fire TaskCancelled event.
    pub async fn on_cancelled(&self, task_id: &str, task_subject: &str) {
        self.fire_event(HookEvent::TaskCancelled, task_id, task_subject)
            .await;
    }
}

// ── Subagent Hook Bridge ────────────────────────────────────────────

/// Bridges `SubagentHooks` trait callbacks to the unified `HookRegistry`.
///
/// When subagent lifecycle events occur (before_dispatch, after_dispatch, etc.),
/// this bridge fires the corresponding `HookEvent` in the central
/// `HookRegistry`, allowing YAML-configured hooks to react.
pub struct SubagentHookBridge {
    hook_registry: Arc<RwLock<HookRegistry>>,
    session_id: String,
    agent_name: String,
}

impl SubagentHookBridge {
    /// Create a new bridge.
    pub fn new(
        hook_registry: Arc<RwLock<HookRegistry>>,
        session_id: String,
        agent_name: String,
    ) -> Self {
        Self {
            hook_registry,
            session_id,
            agent_name,
        }
    }

    /// Fire SubagentStart event (maps to before_dispatch).
    pub async fn on_before_dispatch(&self, subagent_name: &str, execution_mode: &str, task: &str) {
        let ctx = HookContext::for_subagent_start(
            subagent_name,
            execution_mode,
            task,
            &self.session_id,
            &self.agent_name,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire SubagentStop event (maps to after_dispatch).
    pub async fn on_after_dispatch(&self, subagent_name: &str, execution_mode: &str, task: &str) {
        let ctx = HookContext::for_subagent_stop(
            subagent_name,
            execution_mode,
            task,
            &self.session_id,
            &self.agent_name,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire StopFailure event (maps to on_failure).
    pub async fn on_failure(&self, subagent_name: &str, error: &str) {
        let ctx =
            HookContext::for_stop_failure(subagent_name, error, &self.session_id, &self.agent_name);
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire SubagentCancelled event.
    pub async fn on_cancelled(&self, subagent_name: &str) {
        let ctx = HookContext::for_lifecycle(
            HookEvent::SubagentCancelled,
            subagent_name,
            &self.session_id,
            &self.agent_name,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }
}

// ── TaskHooks adapter ───────────────────────────────────────────────

/// A `TaskHooks` implementation that delegates to a `TaskHookBridge`.
///
/// Register this alongside (or instead of) other `TaskHooks` to ensure
/// YAML-configured hooks see task lifecycle events.
pub struct BridgedTaskHooks {
    bridge: Arc<TaskHookBridge>,
}

impl BridgedTaskHooks {
    pub fn new(bridge: Arc<TaskHookBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait::async_trait]
impl echo_orchestration::tasks::TaskHooks for BridgedTaskHooks {
    async fn before_execute(&self, ctx: &echo_orchestration::tasks::TaskHookContext) {
        self.bridge
            .on_before_execute(&ctx.task.id, &ctx.task.subject)
            .await;
    }

    async fn after_execute(&self, ctx: &echo_orchestration::tasks::TaskHookContext, _result: &str) {
        self.bridge
            .on_after_execute(&ctx.task.id, &ctx.task.subject)
            .await;
    }

    async fn on_failure(
        &self,
        ctx: &echo_orchestration::tasks::TaskHookContext,
        error: &str,
    ) -> echo_orchestration::tasks::RetryDecision {
        self.bridge
            .on_failure(&ctx.task.id, &ctx.task.subject, error)
            .await;
        echo_orchestration::tasks::RetryDecision::Fail
    }

    async fn on_timeout(
        &self,
        ctx: &echo_orchestration::tasks::TaskHookContext,
    ) -> echo_orchestration::tasks::RetryDecision {
        self.bridge
            .on_timeout(&ctx.task.id, &ctx.task.subject)
            .await;
        echo_orchestration::tasks::RetryDecision::Fail
    }

    async fn on_cancelled(&self, ctx: &echo_orchestration::tasks::TaskHookContext) {
        self.bridge
            .on_cancelled(&ctx.task.id, &ctx.task.subject)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_hook_bridge_creation() {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        let bridge = TaskHookBridge::new(
            registry,
            "test-session".to_string(),
            "test-agent".to_string(),
        );
        // Bridge should be creatable without panicking
        assert_eq!(bridge.session_id, "test-session");
        assert_eq!(bridge.agent_name, "test-agent");
    }

    #[test]
    fn test_subagent_hook_bridge_creation() {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        let bridge = SubagentHookBridge::new(
            registry,
            "test-session".to_string(),
            "test-agent".to_string(),
        );
        assert_eq!(bridge.session_id, "test-session");
        assert_eq!(bridge.agent_name, "test-agent");
    }

    #[tokio::test]
    async fn test_task_bridge_fires_events() {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        let bridge = TaskHookBridge::new(
            registry.clone(),
            "session-1".to_string(),
            "agent-1".to_string(),
        );

        // These should not panic even with an empty registry
        bridge.on_before_execute("task-1", "Build project").await;
        bridge.on_after_execute("task-1", "Build project").await;
        bridge
            .on_failure("task-1", "Build project", "compile error")
            .await;
        bridge.on_timeout("task-1", "Build project").await;
        bridge.on_cancelled("task-1", "Build project").await;
    }

    #[tokio::test]
    async fn test_subagent_bridge_fires_events() {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        let bridge = SubagentHookBridge::new(
            registry.clone(),
            "session-1".to_string(),
            "agent-1".to_string(),
        );

        // These should not panic even with an empty registry
        bridge
            .on_before_dispatch("researcher", "sync", "Find papers")
            .await;
        bridge
            .on_after_dispatch("researcher", "sync", "Find papers")
            .await;
        bridge.on_failure("researcher", "timeout").await;
        bridge.on_cancelled("researcher").await;
    }
}
