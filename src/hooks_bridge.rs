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
//! YAML hooks.yaml ──→ HookRegistry
//!                          ↑
//! TaskHookBridge ──────┘  (fires Created/Started/Completed(status))
//! SubagentHookBridge ──┘  (fires Start/Stop(status))
//! ```
//!
//! Rust developers can still use the `TaskHooks` and `SubagentHooks`
//! traits directly. The bridges ensure that YAML-configured hooks also
//! see these lifecycle events.

use echo_core::hooks::HookContext;
use echo_execution::skills::hooks::HookRegistry;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Optional identifiers supplied by a product task runtime.
#[derive(Debug, Clone, Copy, Default)]
pub struct HookCorrelation<'a> {
    pub run_id: Option<&'a str>,
    pub plan_revision: Option<&'a str>,
    pub subagent_run_id: Option<&'a str>,
    pub attempt: Option<u32>,
}

fn apply_correlation(ctx: HookContext, correlation: HookCorrelation<'_>) -> HookContext {
    ctx.with_run_correlation(
        correlation.run_id,
        correlation.plan_revision,
        correlation.subagent_run_id,
        correlation.attempt,
    )
}

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

    /// Fire TaskCreated — a task node entered the executable graph.
    ///
    /// Task runtime adapters invoke this after the authoritative plan revision
    /// commits and before the canonical DAG controller makes the task
    /// executable.
    pub async fn on_created(&self, task_id: &str, task_subject: &str) {
        self.on_created_with_correlation(task_id, task_subject, HookCorrelation::default())
            .await;
    }

    pub async fn on_created_with_correlation(
        &self,
        task_id: &str,
        task_subject: &str,
        correlation: HookCorrelation<'_>,
    ) {
        let ctx = apply_correlation(
            HookContext::for_task_created(
                task_id,
                task_subject,
                &self.session_id,
                &self.agent_name,
            ),
            correlation,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire TaskStarted — the scheduler picked the task and is about to
    /// execute. This is the framework `TaskHooks::before_execute` mapping.
    pub async fn on_before_execute(&self, task_id: &str, task_subject: &str) {
        self.on_before_execute_with_correlation(task_id, task_subject, HookCorrelation::default())
            .await;
    }

    pub async fn on_before_execute_with_correlation(
        &self,
        task_id: &str,
        task_subject: &str,
        correlation: HookCorrelation<'_>,
    ) {
        let ctx = apply_correlation(
            HookContext::for_task_started(
                task_id,
                task_subject,
                &self.session_id,
                &self.agent_name,
            ),
            correlation,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire TaskCompleted — the task reached a terminal state.
    ///
    /// `status` is the structured terminal reason. The former separate
    /// `TaskTimeout`/`TaskCancelled` events are gone — timeout/cancelled are
    /// `status` values here (industry-aligned: Codex CommandExecutionStatus).
    pub async fn on_after_execute(
        &self,
        task_id: &str,
        task_subject: &str,
        result: &str,
        status: echo_core::hooks::TaskTerminalStatus,
    ) {
        self.on_after_execute_with_correlation(
            task_id,
            task_subject,
            result,
            status,
            HookCorrelation::default(),
        )
        .await;
    }

    pub async fn on_after_execute_with_correlation(
        &self,
        task_id: &str,
        task_subject: &str,
        result: &str,
        status: echo_core::hooks::TaskTerminalStatus,
        correlation: HookCorrelation<'_>,
    ) {
        let ctx = apply_correlation(
            HookContext::for_task_completed(
                task_id,
                task_subject,
                result,
                status,
                &self.session_id,
                &self.agent_name,
            ),
            correlation,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire StopFailure event (maps to on_failure).
    pub async fn on_failure(&self, _task_id: &str, task_subject: &str, error: &str) {
        let ctx =
            HookContext::for_stop_failure(task_subject, error, &self.session_id, &self.agent_name);
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
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
        self.on_before_dispatch_with_correlation(
            subagent_name,
            execution_mode,
            task,
            HookCorrelation::default(),
        )
        .await;
    }

    pub async fn on_before_dispatch_with_correlation(
        &self,
        subagent_name: &str,
        execution_mode: &str,
        task: &str,
        correlation: HookCorrelation<'_>,
    ) {
        let ctx = apply_correlation(
            HookContext::for_subagent_start(
                subagent_name,
                execution_mode,
                task,
                &self.session_id,
                &self.agent_name,
            ),
            correlation,
        );
        let registry = self.hook_registry.read().await;
        let _ = registry.run_lifecycle_hooks(&ctx).await;
    }

    /// Fire SubagentStop event (maps to after_dispatch).
    ///
    /// Always emits exactly one SubagentStop carrying the terminal `status`
    /// (completed/failed/cancelled/timed_out). This is the single convergence
    /// point for all subagent terminal states — callers must NOT also fire a
    /// separate cancelled event. Industry-aligned model (Claude Code / Codex /
    /// OpenAI Agents SDK / AGTP: two boundary events + status enum).
    pub async fn on_after_dispatch(
        &self,
        subagent_name: &str,
        execution_mode: &str,
        result: &str,
        status: echo_core::hooks::SubagentStopStatus,
    ) {
        self.on_after_dispatch_with_correlation(
            subagent_name,
            execution_mode,
            result,
            status,
            HookCorrelation::default(),
        )
        .await;
    }

    pub async fn on_after_dispatch_with_correlation(
        &self,
        subagent_name: &str,
        execution_mode: &str,
        result: &str,
        status: echo_core::hooks::SubagentStopStatus,
        correlation: HookCorrelation<'_>,
    ) {
        let ctx = apply_correlation(
            HookContext::for_subagent_stop(
                subagent_name,
                execution_mode,
                result,
                status,
                &self.session_id,
                &self.agent_name,
            ),
            correlation,
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
}

// ── TaskHooks adapter ───────────────────────────────────────────────

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

        // Three-stage task lifecycle: created (graph entry) → started
        // (scheduler pick) → completed (terminal, with status). The former
        // on_timeout/on_cancelled are now on_after_execute(status).
        bridge.on_created("task-1", "Build project").await;
        bridge.on_before_execute("task-1", "Build project").await;
        bridge
            .on_after_execute(
                "task-1",
                "Build project",
                "ok",
                echo_core::hooks::TaskTerminalStatus::Completed,
            )
            .await;
        bridge
            .on_failure("task-1", "Build project", "compile error")
            .await;
        // Verify timeout/cancelled reach consumers as TaskCompleted status.
        bridge
            .on_after_execute(
                "task-1",
                "Build project",
                "deadline exceeded",
                echo_core::hooks::TaskTerminalStatus::TimedOut,
            )
            .await;
        bridge
            .on_after_execute(
                "task-1",
                "Build project",
                "aborted",
                echo_core::hooks::TaskTerminalStatus::Cancelled,
            )
            .await;
    }

    #[tokio::test]
    async fn test_subagent_bridge_fires_events() {
        let registry = Arc::new(RwLock::new(HookRegistry::new()));
        let bridge = SubagentHookBridge::new(
            registry.clone(),
            "session-1".to_string(),
            "agent-1".to_string(),
        );

        // These should not panic even with an empty registry.
        // SubagentStop is the single convergence point for all terminal
        // states (completed/failed/cancelled/timed_out) — no separate
        // on_cancelled anymore.
        bridge
            .on_before_dispatch("researcher", "sync", "Find papers")
            .await;
        bridge
            .on_after_dispatch(
                "researcher",
                "sync",
                "Find papers",
                echo_core::hooks::SubagentStopStatus::Completed,
            )
            .await;
        bridge.on_failure("researcher", "timeout").await;
        // Verify cancelled is delivered as a SubagentStop status, not a
        // separate event.
        bridge
            .on_after_dispatch(
                "researcher",
                "sync",
                "cancelled mid-run",
                echo_core::hooks::SubagentStopStatus::Cancelled,
            )
            .await;
    }
}
