//! Subagent executor — unified dispatch engine for Sync / Fork / Teammate modes
//!
//! The executor receives a [`DispatchRequest`] and routes it to the appropriate
//! execution strategy based on the definition's [`ExecutionMode`].

use crate::error::{AgentError, ReactError, Result};
use echo_core::agent::{Agent, AgentEvent, AgentInvocationContext, CancellationToken};
use echo_core::llm::types::Message;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use super::context::SubagentContext;
use super::events::SubagentEvent;
use super::hooks::{SubagentHookContext, SubagentHookRegistry};
use super::registry::SubagentRegistry;
use super::types::{
    ExecutionMode, ObservedIsolation, SubagentArtifact, SubagentOutcome, SubagentResult,
    SubagentStatus, SubagentTouchedFiles, SubagentVerification, SubagentVerificationSource,
    SubagentVerificationStatus,
};
use crate::tasks::NestedDelegationPolicy;

// ── Dispatch Request ──────────────────────────────────────────────────────────

/// A request to dispatch a task to a subagent.
#[derive(Clone)]
pub struct DispatchRequest {
    /// Target subagent name.
    pub agent_name: String,
    /// Task description.
    pub task: String,
    /// Override the definition's default execution mode.
    pub mode_override: Option<ExecutionMode>,
    /// Cancellation token (propagated from parent).
    pub cancel: CancellationToken,
    /// Parent agent name (for logging and topology).
    pub parent_agent: String,
    /// Parent context for inheritance (Fork mode).
    pub parent_context: Option<SubagentContext>,
    /// Nested delegation policy (prevents unbounded delegation chains).
    pub delegation_policy: NestedDelegationPolicy,
    /// 应用层 run 级上下文（跨 spawn 安全，值传递）。
    ///
    /// dispatch_fork 将它放入本次 `AgentInvocationContext`，使 subagent 内的工具
    /// 能经 ToolContext 读到 run_id/cancel/trace_sink——不修改共享 agent。
    pub runtime_context: Option<echo_core::tools::ExternalRunContext>,
    /// Optional multimodal message (images/files). When `Some`, the subagent is
    /// dispatched via `execute_stream_message_with_invocation_context` instead of the text
    /// `task` path, so it sees user-uploaded attachments. `None` = plain text
    /// dispatch (the default for all existing callers).
    pub message: Option<Message>,
    /// When true, this dispatch was (or will be) started via
    /// [`SubagentExecutor::dispatch_background`]. Propagated onto
    /// [`SubagentEvent::DispatchStarted`] so UI can mark background cards.
    pub background: bool,
}

impl std::fmt::Debug for DispatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchRequest")
            .field("agent_name", &self.agent_name)
            .field("task", &self.task)
            .field("mode_override", &self.mode_override)
            .field("parent_agent", &self.parent_agent)
            .field("delegation_policy", &self.delegation_policy)
            .field(
                "runtime_context",
                &self.runtime_context.as_ref().map(|c| &c.run_id),
            )
            .finish()
    }
}

impl DispatchRequest {
    /// Build a request delegation policy from the legacy depth parameter used
    /// by older public helper methods.
    pub fn policy_from_depth(depth: u32) -> NestedDelegationPolicy {
        NestedDelegationPolicy {
            can_spawn_subagents: true,
            delegate_depth: depth.min(u8::MAX as u32) as u8,
            max_delegate_depth: 3,
        }
    }
}

fn invocation_disabled_tools(
    tool_names: Vec<String>,
    allowed_tools: Option<&[String]>,
) -> Option<HashSet<String>> {
    let allowed_tools = allowed_tools.filter(|tools| !tools.is_empty())?;
    let allowed = allowed_tools
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    Some(
        tool_names
            .into_iter()
            .filter(|tool| !allowed.contains(tool.as_str()))
            .collect(),
    )
}

/// Map framework errors to the runtime-owned subagent terminal status.
pub fn subagent_status_from_error(error: &ReactError) -> SubagentStatus {
    match error {
        ReactError::Agent(agent_error) => match agent_error.as_ref() {
            AgentError::Timeout(_) => SubagentStatus::TimedOut,
            AgentError::Interrupted | AgentError::Cancelled(_) => SubagentStatus::Cancelled,
            _ => SubagentStatus::Failed,
        },
        _ => SubagentStatus::Failed,
    }
}

fn verification_check_from_tool(name: &str, args: &serde_json::Value) -> Option<String> {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    if !matches!(
        normalized.as_str(),
        "shell" | "bash" | "terminal" | "run_code" | "execute_command"
    ) {
        return None;
    }
    ["command", "cmd", "code", "script"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn file_access_from_tool(name: &str, args: &serde_json::Value) -> Option<(bool, String)> {
    let normalized = name.to_ascii_lowercase().replace('-', "_");
    let write = normalized.contains("write")
        || normalized.contains("edit")
        || normalized.contains("delete")
        || normalized.contains("patch");
    let read = normalized.contains("read")
        || normalized.contains("search")
        || normalized.contains("glob")
        || normalized.contains("grep");
    if !write && !read {
        return None;
    }
    ["path", "file_path", "target", "directory"]
        .iter()
        .find_map(|key| args.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|path| (write, path.to_string()))
}

fn bounded_detail(text: &str) -> String {
    text.chars().take(500).collect()
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

/// Merge runtime-observed evidence into a parsed result.
///
/// Observed checks replace older facts for the same exact check, while file
/// access and artifacts are unioned under the shared result bounds.
pub fn merge_observed_evidence(
    outcome: &mut SubagentOutcome,
    verification: Vec<SubagentVerification>,
    touched_files: SubagentTouchedFiles,
    artifacts: Vec<SubagentArtifact>,
) {
    for observed in verification {
        outcome
            .verification
            .retain(|existing| existing.check != observed.check);
        outcome.verification.push(observed);
    }
    for path in touched_files.read {
        push_unique(&mut outcome.touched_files.read, path);
    }
    for path in touched_files.written {
        push_unique(&mut outcome.touched_files.written, path);
    }
    for artifact in artifacts {
        outcome
            .artifacts
            .retain(|existing| existing.path != artifact.path);
        outcome.artifacts.push(artifact);
    }
    super::types::normalize_outcome(outcome);
}

// ── Teammate Handle ───────────────────────────────────────────────────────────

/// Handle to a running teammate agent.
///
/// Used to poll for results or cancel execution.
#[derive(Debug)]
pub struct TeammateHandle {
    /// Unique handle ID.
    pub id: String,
    /// The teammate agent name.
    pub agent_name: String,
    /// Cancellation token for this teammate.
    pub cancel: CancellationToken,
    /// Join handle for the spawned task.
    join_handle: tokio::task::JoinHandle<Result<SubagentResult>>,
}

/// Handle returned immediately by [`SubagentExecutor::dispatch_background`].
///
/// The subagent continues on a spawned task; lifecycle events
/// (`DispatchStarted` / `DispatchCompleted` / `DispatchFailed`) still fire on
/// the registry event bus. Unlike [`TeammateHandle`], this does not expose a
/// join handle — callers observe completion via events / UI.
#[derive(Debug, Clone)]
pub struct BackgroundSubagentHandle {
    /// Stable execution id (also on `DispatchStarted.execution_id`).
    pub execution_id: String,
    /// Target subagent name.
    pub agent_name: String,
}

impl TeammateHandle {
    /// Check if the teammate has completed.
    pub fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }

    /// Await the teammate's result.
    pub async fn join(self) -> Result<SubagentResult> {
        self.join_handle
            .await
            .map_err(|e| ReactError::Other(format!("Teammate join error: {}", e)))?
    }
}

// ── Executor Config ───────────────────────────────────────────────────────────

/// Configuration for the subagent executor.
pub struct SubagentExecutorConfig {
    /// Maximum concurrent Fork dispatches.
    pub max_concurrent_forks: usize,
    /// Default timeout (seconds) for ALL dispatch modes (Sync/Fork/Teammate).
    /// 0 = no timeout. Sourced from `AgentConfig.subagent_timeout_secs` (default
    /// 600 = 10 min). Per-subagent `SubagentDefinition.timeout_secs` (>0) overrides.
    pub default_timeout_secs: u64,
    /// Enable hooks.
    pub enable_hooks: bool,
    /// Optional bridge to the unified lifecycle hook system (echo-core).
    /// When set, SubagentStart/SubagentStop events are fired into the
    /// unified HookRegistry alongside the trait-based SubagentHooks.
    pub unified_hook_executor: Option<crate::skills::hooks::UnifiedHookExecutorFn>,
    /// Optional worktree-isolation factory (Sprint 8). When set, Fork-dispatched
    /// subagents whose `SubagentDefinition.isolate_worktree == true` run inside an
    /// isolated git worktree created by this factory. `None` = isolation
    /// unavailable: subagents declaring `isolate_worktree` **hard-fail** (do not
    /// silently share the main tree). Application supplies a git-backed impl;
    /// framework stays free of git deps.
    pub worktree_factory: Option<super::worktree::SharedWorktreeFactory>,
    /// Optional data-workspace factory (Sprint 10). When set, Fork-dispatched
    /// subagents whose `SubagentDefinition.isolate_workspace == true` run inside
    /// an isolated per-subagent working directory (tmpdir) created by this
    /// factory — for data/research subagents emitting disjoint output artifacts.
    /// `None` = no workspace isolation available. Application supplies a
    /// tmpdir-backed impl.
    pub data_workspace_factory: Option<super::workspace::SharedDataWorkspaceFactory>,
    /// Sprint 11: optional state store for team-mode checkpoint/resume. When
    /// set AND a team subagent's `TeamSpec` is dispatched, `dispatch_team`
    /// plumbs this into `TeamAgent` so `ManagerSubagentOrchestrator` can
    /// read/write checkpoint nodes keyed by `run_id`. `None` → teams run
    /// in-memory (no persistence, today's behavior).
    pub runtime_state_store: Option<std::sync::Arc<dyn crate::state::RuntimeStateStore>>,
}

impl Default for SubagentExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_forks: 5,
            default_timeout_secs: 600,
            enable_hooks: true,
            unified_hook_executor: None,
            worktree_factory: None,
            data_workspace_factory: None,
            runtime_state_store: None,
        }
    }
}

// ── Subagent Executor ─────────────────────────────────────────────────────────

/// Unified dispatch engine for all subagent execution modes.
pub struct SubagentExecutor {
    registry: Arc<SubagentRegistry>,
    hooks: Arc<SubagentHookRegistry>,
    config: SubagentExecutorConfig,
    semaphore: Arc<Semaphore>,
}

impl SubagentExecutor {
    /// Create a new executor backed by the given registry.
    pub fn new(registry: Arc<SubagentRegistry>, config: SubagentExecutorConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_forks));
        Self {
            registry,
            hooks: Arc::new(SubagentHookRegistry::new()),
            config,
            semaphore,
        }
    }

    /// Create with a specific hook registry.
    pub fn with_hooks(
        registry: Arc<SubagentRegistry>,
        config: SubagentExecutorConfig,
        hooks: SubagentHookRegistry,
    ) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_forks));
        Self {
            registry,
            hooks: Arc::new(hooks),
            config,
            semaphore,
        }
    }

    /// Get the hook registry (for external registration).
    pub fn hooks(&self) -> &SubagentHookRegistry {
        &self.hooks
    }

    /// Shared registry handle (for callers that need definition lookups).
    pub fn registry(&self) -> &Arc<SubagentRegistry> {
        &self.registry
    }

    /// Main dispatch entry point.
    ///
    /// Routes to the appropriate mode based on the definition or override.
    /// Uses a loop for retry/delegation instead of recursion to prevent stack overflow.
    pub async fn dispatch(&self, mut req: DispatchRequest) -> Result<SubagentResult> {
        let mut retry_count: u32 = 0;
        let max_retries: u32 = 3; // Prevent infinite retry loops
        // Save parent cancel token so retry/delegate paths propagate cancellation
        // instead of creating independent tokens (P1 — CancellationToken propagation).
        let parent_cancel = req.cancel.clone();

        // 入口取消检查:若 dispatch 前 token 已取消,立即返回,不进入执行 loop。
        // (与 dispatch_teammate:704 的 "Cancelled before execution" 语义一致;
        //  test_dispatch_cancelled 验证此行为。)
        if parent_cancel.is_cancelled() {
            return Ok(SubagentResult::cancelled(
                req.agent_name.clone(),
                "Cancelled before execution",
                req.mode_override.clone().unwrap_or(ExecutionMode::Fork),
            ));
        }

        loop {
            // Guard against infinite delegation chains
            if req.delegation_policy.delegate_depth > req.delegation_policy.max_delegate_depth {
                return Err(ReactError::Other(format!(
                    "Delegation depth exceeded (max {}): agent '{}'",
                    req.delegation_policy.max_delegate_depth, req.agent_name
                )));
            }

            // Guard against excessive retries
            if retry_count > max_retries {
                return Err(ReactError::Agent(Box::new(
                    AgentError::ContextLimitExceeded(format!(
                        "Max retry count exceeded ({}): agent '{}'",
                        max_retries, req.agent_name
                    )),
                )));
            }

            // Look up definition
            let registered = self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;

            let mode = req
                .mode_override
                .as_ref()
                .unwrap_or(&registered.definition.execution_mode)
                .clone();
            let runtime_run_id = req
                .runtime_context
                .as_ref()
                .and_then(|ctx| ctx.run_id.as_deref())
                .unwrap_or("<none>");
            // Extract stable identity for event payload (Option<String>).
            // Used by all Dispatch* events so the bridge/frontend can route
            // thinking/tool/token streams without temp id allocation.
            let event_execution_id = req
                .runtime_context
                .as_ref()
                .and_then(|ctx| ctx.execution_id.clone());
            let event_run_id = req
                .runtime_context
                .as_ref()
                .and_then(|ctx| ctx.run_id.clone());
            let event_conversation_id = req
                .runtime_context
                .as_ref()
                .and_then(|ctx| ctx.conversation_id.clone());
            let event_message_id = req
                .runtime_context
                .as_ref()
                .and_then(|ctx| ctx.message_id.clone());
            let has_trace_sink = req
                .runtime_context
                .as_ref()
                .is_some_and(|ctx| ctx.trace_sink.is_some());
            let has_cancel = req
                .runtime_context
                .as_ref()
                .is_some_and(|ctx| ctx.cancel.is_some());
            info!(
                parent = %req.parent_agent,
                subagent = %req.agent_name,
                mode = ?mode,
                attempt = retry_count + 1,
                delegate_depth = req.delegation_policy.delegate_depth,
                runtime_run_id = %runtime_run_id,
                has_runtime_context = req.runtime_context.is_some(),
                has_trace_sink,
                has_cancel,
                task_chars = req.task.chars().count(),
                "subagent_dispatch_start"
            );

            // Build hook context
            let hook_ctx = SubagentHookContext {
                parent_agent: req.parent_agent.clone(),
                subagent_name: req.agent_name.clone(),
                execution_mode: mode.clone(),
                task: req.task.clone(),
                attempt: 1 + retry_count,
            };

            // Emit event and call before_dispatch hook
            self.registry
                .event_bus()
                .emit(SubagentEvent::DispatchStarted {
                    parent: req.parent_agent.clone(),
                    agent: req.agent_name.clone(),
                    mode: mode.clone(),
                    task: req.task.clone(),
                    execution_id: event_execution_id.clone(),
                    run_id: event_run_id.clone(),
                    conversation_id: event_conversation_id.clone(),
                    message_id: event_message_id.clone(),
                    background: req.background,
                });

            if self.config.enable_hooks {
                self.hooks.before_dispatch(&hook_ctx).await;
            }

            // Snapshot fields needed in error path before `req` is moved
            let req_agent_name = req.agent_name.clone();
            let req_parent_agent = req.parent_agent.clone();
            let delegation_policy = req.delegation_policy;

            // Fire unified SubagentStart hook
            if let Some(ref executor) = self.config.unified_hook_executor {
                let ctx = crate::skills::hooks::HookContext::for_subagent_start(
                    &req_agent_name,
                    &format!("{:?}", mode),
                    &req.task,
                    "", // session_id not available at this layer
                    &req_parent_agent,
                );
                executor(ctx).await;
            }

            // Dispatch based on mode
            let start = Instant::now();
            let result = match mode {
                ExecutionMode::Sync => self.dispatch_sync(&req).await,
                ExecutionMode::Fork => self.dispatch_fork(&req).await,
                ExecutionMode::Teammate => {
                    // Teammate mode: spawn independently, then await result
                    match self.dispatch_teammate(req.clone()).await {
                        Ok(handle) => handle.join().await,
                        Err(e) => Err(e),
                    }
                }
                // Sprint 11: Team mode routes to dispatch_team (Task 5 fills
                // the body). Stub returns a clear error until Task 5.
                ExecutionMode::Team => self.dispatch_team(&req).await,
            };

            let duration = start.elapsed();

            match result {
                Ok(mut sub_result) => {
                    sub_result.duration = duration;
                    sub_result.mode = mode.clone();
                    info!(
                        parent = %req_parent_agent,
                        subagent = %req_agent_name,
                        mode = ?mode,
                        duration_ms = duration.as_millis() as u64,
                        output_chars = sub_result.output.chars().count(),
                        tokens_used = ?sub_result.tokens_used,
                        iterations = sub_result.iterations,
                        "subagent_dispatch_complete"
                    );

                    if sub_result.outcome.status == SubagentStatus::Cancelled {
                        self.registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchCancelled {
                                parent: req_parent_agent.clone(),
                                agent: req_agent_name.clone(),
                                result: sub_result.outcome.clone(),
                                execution_id: event_execution_id.clone(),
                                run_id: event_run_id.clone(),
                            });
                    } else {
                        self.registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchCompleted {
                                parent: req_parent_agent.clone(),
                                agent: req_agent_name.clone(),
                                duration_ms: duration.as_millis() as u64,
                                tokens_used: sub_result.tokens_used.map(|t| t as u64),
                                iterations: Some(sub_result.iterations as u64),
                                output: sub_result.output.clone(),
                                result: sub_result.outcome.clone(),
                                execution_id: event_execution_id.clone(),
                                run_id: event_run_id.clone(),
                            });
                    }

                    if self.config.enable_hooks {
                        self.hooks.after_dispatch(&hook_ctx, &sub_result).await;
                    }

                    // Fire unified SubagentStop hook (success)
                    if let Some(ref executor) = self.config.unified_hook_executor {
                        let ctx = crate::skills::hooks::HookContext::for_subagent_stop(
                            &req_agent_name,
                            &format!("{:?}", mode),
                            &format!("{:?}", sub_result.output),
                            "",
                            &req_parent_agent,
                        );
                        executor(ctx).await;
                    }

                    return Ok(sub_result);
                }
                Err(e) => {
                    let error_str = e.to_string();
                    let status = subagent_status_from_error(&e);
                    let terminal_result = SubagentOutcome::terminal(
                        status,
                        error_str.clone(),
                        vec![error_str.clone()],
                    );
                    warn!(
                        parent = %req_parent_agent,
                        subagent = %req_agent_name,
                        mode = ?mode,
                        error = %error_str,
                        "subagent_dispatch_failed"
                    );

                    if status == SubagentStatus::Cancelled {
                        self.registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchCancelled {
                                parent: req_parent_agent.clone(),
                                agent: req_agent_name.clone(),
                                result: terminal_result,
                                execution_id: event_execution_id.clone(),
                                run_id: event_run_id.clone(),
                            });
                        return Err(e);
                    }

                    if self.config.enable_hooks {
                        let decision = self.hooks.on_failure(&hook_ctx, &error_str).await;
                        match decision {
                            super::hooks::SubagentRetryDecision::Delegate { alternative_agent } => {
                                if let Some(child_policy) = delegation_policy.child_policy() {
                                    info!(
                                        from = %hook_ctx.subagent_name,
                                        to = %alternative_agent,
                                        depth = child_policy.delegate_depth,
                                        "Delegating to alternative subagent"
                                    );
                                    retry_count = retry_count.saturating_add(1);
                                    let rt_ctx = req.runtime_context.clone();
                                    let retry_msg = req.message.clone();
                                    req = DispatchRequest {
                                        agent_name: alternative_agent,
                                        task: hook_ctx.task.clone(),
                                        mode_override: Some(hook_ctx.execution_mode.clone()),
                                        cancel: parent_cancel.child_token(),
                                        parent_agent: hook_ctx.parent_agent.clone(),
                                        parent_context: None,
                                        delegation_policy: child_policy,
                                        runtime_context: rt_ctx,
                                        message: retry_msg,
                                        background: false,
                                    };
                                    // This attempt is recoverable, so it is not a terminal event.
                                    continue;
                                }
                                warn!(
                                    agent = %hook_ctx.subagent_name,
                                    max_depth = delegation_policy.max_delegate_depth,
                                    "subagent delegation rejected at depth limit"
                                );
                            }
                            super::hooks::SubagentRetryDecision::Retry { delay_secs } => {
                                if retry_count < max_retries {
                                    info!(
                                        delay_secs,
                                        attempt = retry_count.saturating_add(2),
                                        "Retrying subagent dispatch"
                                    );
                                    tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                                    retry_count = retry_count.saturating_add(1);
                                    let rt_ctx = req.runtime_context.clone();
                                    let retry_msg = req.message.clone();
                                    req = DispatchRequest {
                                        agent_name: hook_ctx.subagent_name.clone(),
                                        task: hook_ctx.task.clone(),
                                        mode_override: Some(hook_ctx.execution_mode.clone()),
                                        cancel: parent_cancel.child_token(),
                                        parent_agent: hook_ctx.parent_agent.clone(),
                                        parent_context: None,
                                        delegation_policy,
                                        runtime_context: rt_ctx,
                                        message: retry_msg,
                                        background: false,
                                    };
                                    // This attempt is recoverable, so it is not a terminal event.
                                    continue;
                                }
                                warn!(
                                    agent = %hook_ctx.subagent_name,
                                    max_retries,
                                    "subagent retry limit reached"
                                );
                            }
                            super::hooks::SubagentRetryDecision::Fail => {}
                        }
                    }

                    self.registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchFailed {
                            parent: req_parent_agent.clone(),
                            agent: req_agent_name.clone(),
                            error: error_str.clone(),
                            status,
                            result: terminal_result,
                            execution_id: event_execution_id.clone(),
                            run_id: event_run_id.clone(),
                        });

                    // Fire unified SubagentStop hook (failure)
                    if let Some(ref executor) = self.config.unified_hook_executor {
                        let ctx = crate::skills::hooks::HookContext::for_subagent_stop(
                            &req_agent_name,
                            &format!("{:?}", mode),
                            &format!("error: {}", error_str),
                            "",
                            &req_parent_agent,
                        );
                        executor(ctx).await;
                    }

                    return Err(e);
                }
            }
        }
    }

    /// Clone internals so a background subagent can own an executor on a spawned task.
    fn clone_for_spawn(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            hooks: self.hooks.clone(),
            config: SubagentExecutorConfig {
                max_concurrent_forks: self.config.max_concurrent_forks,
                default_timeout_secs: self.config.default_timeout_secs,
                enable_hooks: self.config.enable_hooks,
                unified_hook_executor: self.config.unified_hook_executor.clone(),
                worktree_factory: self.config.worktree_factory.clone(),
                data_workspace_factory: self.config.data_workspace_factory.clone(),
                runtime_state_store: self.config.runtime_state_store.clone(),
            },
            semaphore: self.semaphore.clone(),
        }
    }

    /// Ensure `runtime_context.execution_id` is set (generate `agent_tool-{uuid}` if missing).
    fn ensure_background_execution_id(req: &mut DispatchRequest) -> String {
        let ctx = req
            .runtime_context
            .get_or_insert_with(|| echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some(format!("bg-{}", uuid::Uuid::new_v4().as_simple())),
                turn_id: None,
                execution_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            });
        if ctx
            .execution_id
            .as_ref()
            .map(|id| id.is_empty())
            .unwrap_or(true)
        {
            ctx.execution_id = Some(format!("agent_tool-{}", uuid::Uuid::new_v4()));
        }
        ctx.execution_id
            .clone()
            .unwrap_or_else(|| format!("agent_tool-{}", uuid::Uuid::new_v4()))
    }

    /// Non-blocking dispatch: spawn the normal `dispatch` path and return a handle
    /// immediately. Lifecycle events still fire on the registry event bus.
    ///
    /// Mirrors the `tokio::spawn` pattern of [`Self::dispatch_teammate`], but
    /// reuses Sync/Fork/Team routing via [`Self::dispatch`] rather than the
    /// teammate-only streaming path.
    pub async fn dispatch_background(
        &self,
        mut req: DispatchRequest,
    ) -> Result<BackgroundSubagentHandle> {
        // Fail fast if the target role is missing (same as sync dispatch).
        if self.registry.get(&req.agent_name).await.is_none() {
            return Err(ReactError::Other(format!(
                "Subagent '{}' not found",
                req.agent_name
            )));
        }

        let execution_id = Self::ensure_background_execution_id(&mut req);
        req.background = true;
        let agent_name = req.agent_name.clone();
        let agent_name_for_handle = agent_name.clone();
        let spawned = self.clone_for_spawn();

        tokio::spawn(async move {
            if let Err(e) = spawned.dispatch(req).await {
                warn!(
                    agent = %agent_name,
                    error = %e,
                    "background subagent dispatch failed"
                );
            }
        });

        Ok(BackgroundSubagentHandle {
            execution_id,
            agent_name: agent_name_for_handle,
        })
    }

    /// Dispatch a teammate, returning a handle for async polling.
    pub async fn dispatch_teammate(&self, req: DispatchRequest) -> Result<TeammateHandle> {
        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;

        let agent_arc = self.isolated_dispatch_agent(&req.agent_name).await?;

        let child_token = req.cancel.child_token();
        let handle_cancel = child_token.clone();
        let task = Self::enhance_task(
            &req.task,
            req.parent_context.as_ref(),
            registered.definition.inherit_history,
        );
        let agent_name = req.agent_name.clone();
        let parent_agent = req.parent_agent.clone();
        let registry = self.registry.clone();
        let message = req.message.clone();
        let timeout_secs = if registered.definition.timeout_secs > 0 {
            registered.definition.timeout_secs
        } else {
            self.config.default_timeout_secs
        };

        let handle_id = format!("tm_{}", uuid::Uuid::new_v4().as_simple());

        // Extract stable identity for event payload (moved into the spawn).
        let event_execution_id = req
            .runtime_context
            .as_ref()
            .and_then(|ctx| ctx.execution_id.clone());
        let event_run_id = req
            .runtime_context
            .as_ref()
            .and_then(|ctx| ctx.run_id.clone());
        let invocation = AgentInvocationContext {
            runtime: req.runtime_context.clone(),
            ..AgentInvocationContext::default()
        };

        let join_handle = tokio::spawn(async move {
            let _permit = child_token.clone();
            let start = Instant::now();

            let agent = agent_arc.as_ref();

            if timeout_secs > 0 {
                // Race between timeout, cancellation, and execution
                tokio::select! {
                    biased; // Check cancellation first
                    _ = child_token.cancelled() => {
                        Err(ReactError::Agent(Box::new(AgentError::Interrupted)))
                    }
                    _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                        child_token.cancel();
                        Err(ReactError::Agent(Box::new(AgentError::Timeout(format!(
                            "Teammate '{}' timed out after {}s",
                            agent_name, timeout_secs
                        )))))
                    }
                    r = Self::execute_agent_streaming(
                        registry,
                        agent,
                        &task,
                        message.clone(),
                        child_token.clone(),
                        invocation.clone(),
                        &parent_agent,
                        &agent_name,
                        ExecutionMode::Teammate,
                        start,
                        event_execution_id.clone(),
                        event_run_id.clone(),
                    ) => r,
                }
            } else {
                // Race between cancellation and execution
                tokio::select! {
                    biased;
                    _ = child_token.cancelled() => {
                        Err(ReactError::Agent(Box::new(AgentError::Interrupted)))
                    }
                    r = Self::execute_agent_streaming(
                        registry,
                        agent,
                        &task,
                        message.clone(),
                        child_token.clone(),
                        invocation.clone(),
                        &parent_agent,
                        &agent_name,
                        ExecutionMode::Teammate,
                        start,
                        event_execution_id.clone(),
                        event_run_id.clone(),
                    ) => r,
                }
            }
        });

        Ok(TeammateHandle {
            id: handle_id,
            agent_name: req.agent_name.clone(),
            cancel: handle_cancel,
            join_handle,
        })
    }

    /// Sprint 11: dispatch a team-mode subagent. Builds a `TeamAgent` from the
    /// definition's `TeamSpec` (manager + subagents resolved by name from the
    /// registry), plumbs `run_id` + `state_store` for checkpoint/resume, and
    /// runs it.
    ///
    /// Timeout: relies on `TeamAgent::execute`'s own `tokio::time::timeout`
    /// wrapper (uses `TeamConfig.default_timeout_secs`) — no second timeout
    /// here (would double-wrap). Subagents are wrapped in `ArcAgentBox` since
    /// the builder consumes `Box<dyn Agent>` but the registry returns
    /// `Arc<dyn Agent>` (shared singletons).
    async fn dispatch_team(
        &self,
        req: &DispatchRequest,
    ) -> std::result::Result<SubagentResult, crate::error::ReactError> {
        use super::team::{ArcAgentBox, TeamAgent, TeamExecutionResult};

        let registered = self.registry.get(&req.agent_name).await.ok_or_else(|| {
            crate::error::ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
        })?;
        let spec = registered.definition.team.as_ref().ok_or_else(|| {
            crate::error::ReactError::Other(
                "Team mode requested but definition has no TeamSpec".into(),
            )
        })?;

        // Resolve manager + subagents by name (late binding, D-11-team-2).
        let manager_def = self
            .registry
            .get(&spec.manager)
            .await
            .ok_or_else(|| {
                crate::error::ReactError::Other(format!(
                    "Team manager '{}' not registered",
                    spec.manager
                ))
            })?
            .definition
            .clone();
        let manager_agent = self
            .registry
            .get_agent(&spec.manager)
            .await
            .ok_or_else(|| {
                crate::error::ReactError::Other(format!(
                    "Cannot get manager agent instance '{}'",
                    spec.manager
                ))
            })?;

        let mut builder = TeamAgent::builder()
            .manager(
                &spec.manager,
                Box::new(ArcAgentBox(manager_agent.clone())),
                manager_def,
            )
            .strategy(spec.strategy.clone())
            .run_id(
                req.runtime_context
                    .as_ref()
                    .and_then(|context| context.run_id.clone()),
            )
            .state_store(self.config.runtime_state_store.clone());

        for name in &spec.subagents {
            let subagent_definition = self
                .registry
                .get(name)
                .await
                .ok_or_else(|| {
                    crate::error::ReactError::Other(format!(
                        "Team subagent '{}' not registered",
                        name
                    ))
                })?
                .definition
                .clone();
            let subagent_agent = self.registry.get_agent(name).await.ok_or_else(|| {
                crate::error::ReactError::Other(format!(
                    "Cannot get team subagent instance '{}'",
                    name
                ))
            })?;
            builder = builder.subagent(
                name,
                Box::new(ArcAgentBox(subagent_agent.clone())),
                subagent_definition,
            );
        }
        let team_agent = builder.build();

        let start = std::time::Instant::now();
        let task = Self::enhance_task(
            &req.task,
            req.parent_context.as_ref(),
            registered.definition.inherit_history,
        );
        let TeamExecutionResult { output, usage } = team_agent
            .execute_with_usage(&task)
            .await
            .map_err(|error| {
                if error.to_ascii_lowercase().contains("timed out") {
                    crate::error::ReactError::Agent(Box::new(AgentError::Timeout(error)))
                } else {
                    crate::error::ReactError::Other(format!("Team execution failed: {error}"))
                }
            })?;
        let tokens_used = usage
            .as_ref()
            .map(|stats| usize::try_from(stats.total_tokens).unwrap_or(usize::MAX));

        Ok(SubagentResult {
            agent_name: req.agent_name.clone(),
            output,
            outcome: SubagentOutcome {
                status: SubagentStatus::Completed,
                ..SubagentOutcome::default()
            },
            duration: start.elapsed(),
            iterations: 1,
            tokens_used,
            was_truncated: false,
            mode: ExecutionMode::Team,
            isolation_observed: ObservedIsolation::Subagent,
            usage,
        }
        .with_structured(
            req.runtime_context
                .as_ref()
                .and_then(|context| context.execution_id.as_deref()),
            std::env::current_dir().ok().as_deref(),
        ))
    }

    // ── Internal dispatch methods ──────────────────────────────────────────

    /// Enhance the task description with inherited parent context.
    ///
    /// Prepends the scoped user request, inherited system prompt, and a
    /// **sliced** conversation history to the task, giving the subagent the
    /// minimum parent context selected for this dispatch.
    ///
    /// # History slicing (Sprint 6b)
    ///
    /// `inherit_history` controls how many trailing messages are joined:
    /// - `None`  → no history is inherited (system prompt only, if any).
    /// - `Some(0)` → inherit all messages already present in `parent_ctx`
    ///   (these were themselves capped by the Fork mode default when the
    ///   context was built via `SubagentContext::from_parent`).
    /// - `Some(n)` → inherit the **last n** messages.
    ///
    /// Before Sprint 6b this function ignored `inherit_history` and dumped the
    /// entire `parent_ctx.messages`, so per-subagent `inherit_history` settings
    /// (e.g. from a subagent `.md` frontmatter) had no effect.
    fn enhance_task(
        task: &str,
        parent_ctx: Option<&super::context::SubagentContext>,
        inherit_history: Option<usize>,
    ) -> String {
        let mut parts = Vec::new();
        if let Some(ctx) = parent_ctx {
            if let Some(parent_goal) = ctx
                .parent_goal
                .as_deref()
                .filter(|goal| !goal.trim().is_empty())
                .filter(|_| !task.contains("[user_request"))
            {
                // The user's original request is the only language anchor. The
                // role prompt, inherited system context, conversation history,
                // and the [Subagent Result Contract] below are all English;
                // mark this block so the subagent does not drift to English.
                parts.push(format!(
                    "[user_request (language anchor — reply in this language)]\n{}\n[/user_request]",
                    parent_goal.trim()
                ));
            }
            if !ctx.system_prompt.is_empty() {
                parts.push(format!("[Inherited System Context]\n{}", ctx.system_prompt));
            }

            // Pick the message slice dictated by inherit_history.
            // Some(0) = everything already in ctx.messages (capped upstream);
            // Some(n) = last n; None = none.
            let selected: &[echo_core::llm::types::Message] = match inherit_history {
                None => &[],
                Some(0) => &ctx.messages,
                Some(n) => {
                    let start = ctx.messages.len().saturating_sub(n);
                    ctx.messages.get(start..).unwrap_or_default()
                }
            };

            if !selected.is_empty() {
                let history: Vec<String> = selected
                    .iter()
                    .filter_map(|m| {
                        m.content
                            .as_text()
                            .map(|c| format!("[{}] {}", m.role.as_str(), c))
                    })
                    .collect();
                if !history.is_empty() {
                    parts.push(format!(
                        "[Inherited Conversation History]\n{}",
                        history.join("\n")
                    ));
                }
            }
        }

        let enriched = if parts.is_empty() {
            task.to_string()
        } else {
            format!("{}\n\n---\n\n{}", parts.join("\n\n"), task)
        };
        if enriched.contains("## Result") && enriched.contains("\"contract_version\":1") {
            return enriched;
        }
        format!(
            "{enriched}\n\n[Subagent Result Contract]\nEnd with `## Result` and exactly one fenced JSON object:\n\
             ```json\n\
             {{\"contract_version\":1,\"status\":\"completed\",\"summary\":\"bounded result\",\
             \"artifacts\":[{{\"path\":\"actual path\",\"kind\":\"file|report|chart|other\"}}],\
             \"verification\":[{{\"check\":\"exact check\",\"status\":\"passed|failed|not_run\",\
             \"details\":\"bounded evidence\",\"source\":\"reported\"}}],\"remaining_work\":[],\
             \"touched_files\":{{\"read\":[],\"written\":[]}}}}\n\
             ```\nRuntime owns terminal status and observed evidence. Report only real paths and checks; put incomplete or blocked work in remaining_work."
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_agent_streaming(
        registry: Arc<SubagentRegistry>,
        agent: &(dyn Agent + Send + Sync),
        task: &str,
        message: Option<Message>,
        cancel: CancellationToken,
        invocation: AgentInvocationContext,
        parent: &str,
        subagent: &str,
        mode: ExecutionMode,
        start: Instant,
        execution_id: Option<String>,
        run_id: Option<String>,
    ) -> Result<SubagentResult> {
        let artifact_base_dir = invocation
            .working_dir
            .clone()
            .or_else(|| std::env::current_dir().ok());
        // Multimodal path: when a Message is supplied, run it so the subagent
        // sees images/files. Falls back to the text task otherwise.
        let event_identity = echo_core::agent::EventIdentity::from_invocation(&invocation);
        let raw_stream = if let Some(msg) = message {
            agent
                .execute_stream_message_with_invocation_context(msg, cancel, invocation)
                .await?
        } else {
            agent
                .execute_stream_with_invocation_context(task, cancel, invocation)
                .await?
        };
        let mut stream = echo_core::agent::envelope_event_stream(raw_stream, event_identity);
        let mut output = String::new();
        let mut in_thinking = false;
        let mut prompt_tokens: usize = 0;
        let mut completion_tokens: usize = 0;
        let mut cancelled = false;
        let mut usage_stats = super::usage::LlmUsageStats::default();
        let mut pending_verification = HashMap::<String, String>::new();
        let mut pending_file_access = HashMap::<String, (bool, String)>::new();
        let mut observed_verification = Vec::new();
        let mut touched_files = SubagentTouchedFiles::default();
        let mut observed_artifacts = Vec::new();

        while let Some(event_result) = stream.next().await {
            let event = event_result?.payload;
            match event {
                AgentEvent::Token(content) => {
                    if in_thinking {
                        registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchThinkingDelta {
                                parent: parent.to_string(),
                                agent: subagent.to_string(),
                                content,
                                execution_id: execution_id.clone(),
                                run_id: run_id.clone(),
                            });
                    } else {
                        output.push_str(&content);
                        registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchTokenDelta {
                                parent: parent.to_string(),
                                agent: subagent.to_string(),
                                content,
                                execution_id: execution_id.clone(),
                                run_id: run_id.clone(),
                            });
                    }
                }
                AgentEvent::ThinkStart => {
                    in_thinking = true;
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchThinkingStarted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::ThinkEnd {
                    prompt_tokens: pt,
                    completion_tokens: ct,
                } => {
                    in_thinking = false;
                    prompt_tokens = prompt_tokens.saturating_add(pt);
                    completion_tokens = completion_tokens.saturating_add(ct);
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchThinkingEnded {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            prompt_tokens: pt,
                            completion_tokens: ct,
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::LlmUsage {
                    model,
                    prompt_tokens: pt,
                    completion_tokens: ct,
                    total_tokens: tt,
                    cached_prompt_tokens: cpt,
                    cache_creation_prompt_tokens: ccpt,
                    usage_reported,
                } => {
                    usage_stats.record(&model, pt, ct, tt, cpt, ccpt, usage_reported);
                    registry.event_bus().emit(SubagentEvent::DispatchLlmUsage {
                        parent: parent.to_string(),
                        agent: subagent.to_string(),
                        model: model.clone(),
                        prompt_tokens: pt,
                        completion_tokens: ct,
                        total_tokens: tt,
                        cached_prompt_tokens: cpt,
                        cache_creation_prompt_tokens: ccpt,
                        usage_reported,
                        execution_id: execution_id.clone(),
                        run_id: run_id.clone(),
                    });
                }
                AgentEvent::ToolCall {
                    call_id,
                    name,
                    args,
                } => {
                    if let Some(check) = verification_check_from_tool(&name, &args) {
                        pending_verification.insert(call_id.clone(), check);
                    }
                    if let Some(access) = file_access_from_tool(&name, &args) {
                        pending_file_access.insert(call_id.clone(), access);
                    }
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolStarted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            call_id,
                            name,
                            args,
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::ToolResult {
                    call_id,
                    name,
                    output,
                } => {
                    if let Some(check) = pending_verification.remove(&call_id) {
                        observed_verification.push(SubagentVerification {
                            check,
                            status: SubagentVerificationStatus::Passed,
                            details: bounded_detail(&output),
                            source: SubagentVerificationSource::Observed,
                        });
                    }
                    if let Some((write, path)) = pending_file_access.remove(&call_id) {
                        if write {
                            push_unique(&mut touched_files.written, path);
                        } else {
                            push_unique(&mut touched_files.read, path);
                        }
                    }
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolCompleted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            call_id,
                            name,
                            result: output,
                            success: true,
                            failure: None,
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::ToolError {
                    call_id,
                    name,
                    error,
                    failure,
                } => {
                    if let Some(check) = pending_verification.remove(&call_id) {
                        observed_verification.push(SubagentVerification {
                            check,
                            status: SubagentVerificationStatus::Failed,
                            details: bounded_detail(&error),
                            source: SubagentVerificationSource::Observed,
                        });
                    }
                    pending_file_access.remove(&call_id);
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolCompleted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            call_id,
                            name,
                            result: error,
                            success: false,
                            failure: Some(failure),
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::ToolStream {
                    event: echo_core::tools::ToolStreamEvent::Complete(result),
                    ..
                } => {
                    if let Some(artifact) =
                        echo_core::tools::artifact::ToolOutputArtifactRef::from_metadata(
                            &result.metadata,
                        )
                    {
                        observed_artifacts.push(SubagentArtifact {
                            path: artifact.path.to_string_lossy().to_string(),
                            kind: "tool_log".to_string(),
                            bytes: Some(artifact.artifact_bytes),
                            sha256: Some(artifact.sha256),
                            producer_execution_id: execution_id.clone(),
                            available: artifact.path.is_file(),
                        });
                    }
                }
                AgentEvent::FinalAnswer(answer) if !answer.is_empty() => {
                    output = answer;
                }
                AgentEvent::FinalAnswer(_) => {}
                AgentEvent::Cancelled => {
                    cancelled = true;
                    break;
                }
                AgentEvent::Error { source, message } => {
                    return Err(ReactError::Other(format!("{source}: {message}")));
                }
                _ => {}
            }
        }

        if cancelled {
            output = "Cancelled during execution".to_string();
        }

        let tokens_used = Some(prompt_tokens.saturating_add(completion_tokens));
        let usage = if usage_stats.call_count > 0 {
            // Also update tokens_used from real usage stats for consistency.
            Some(usage_stats)
        } else {
            None
        };

        let status = if cancelled {
            SubagentStatus::Cancelled
        } else {
            SubagentStatus::Completed
        };
        let mut result = SubagentResult {
            agent_name: subagent.to_string(),
            output,
            outcome: SubagentOutcome {
                status,
                ..SubagentOutcome::default()
            },
            duration: start.elapsed(),
            iterations: 1,
            tokens_used,
            was_truncated: false,
            mode,
            isolation_observed: ObservedIsolation::Unknown,
            usage,
        }
        .with_structured(execution_id.as_deref(), artifact_base_dir.as_deref());
        if cancelled {
            result.outcome.remaining_work = vec!["cancelled during execution".to_string()];
        }
        merge_observed_evidence(
            &mut result.outcome,
            observed_verification,
            touched_files,
            observed_artifacts,
        );
        Ok(result)
    }

    async fn isolated_dispatch_agent(&self, agent_name: &str) -> Result<Arc<dyn Agent>> {
        let agent = match self.registry.create_fresh_agent(agent_name).await? {
            Some(agent) => agent,
            None => self.registry.get_agent(agent_name).await.ok_or_else(|| {
                ReactError::Other(format!(
                    "Subagent '{}' not found or not instantiated",
                    agent_name
                ))
            })?,
        };
        Ok(agent)
    }

    /// Sync mode: execute one isolated invocation and return its result.
    async fn dispatch_sync(&self, req: &DispatchRequest) -> Result<SubagentResult> {
        let agent_arc = self.isolated_dispatch_agent(&req.agent_name).await?;

        // Per-subagent override (0 = executor default). Sync now enforces a
        // timeout too (previously it blocked the parent indefinitely) — one
        // config (AgentConfig.subagent_timeout_secs) governs all three modes.
        let timeout_secs = match self.registry.get(&req.agent_name).await {
            Some(r) if r.definition.timeout_secs > 0 => r.definition.timeout_secs,
            _ => self.config.default_timeout_secs,
        };

        let start = Instant::now();
        let inherit_history = match self.registry.get(&req.agent_name).await {
            Some(r) => r.definition.inherit_history,
            None => None,
        };
        let task = Self::enhance_task(&req.task, req.parent_context.as_ref(), inherit_history);
        let execution_cancel = req.cancel.child_token();
        let event_execution_id = req
            .runtime_context
            .as_ref()
            .and_then(|ctx| ctx.execution_id.clone());
        let event_run_id = req
            .runtime_context
            .as_ref()
            .and_then(|ctx| ctx.run_id.clone());
        let invocation = AgentInvocationContext {
            runtime: req.runtime_context.clone(),
            ..AgentInvocationContext::default()
        };

        if timeout_secs > 0 {
            tokio::select! {
                biased;
                _ = execution_cancel.cancelled() => Err(ReactError::Agent(Box::new(
                    AgentError::Cancelled(format!("Sync subagent '{}' cancelled", req.agent_name))
                ))),
                r = tokio::time::timeout(
                    Duration::from_secs(timeout_secs),
                    Self::execute_agent_streaming(
                        self.registry.clone(),
                        agent_arc.as_ref(),
                        &task,
                        req.message.clone(),
                        execution_cancel.clone(),
                        invocation.clone(),
                        &req.parent_agent,
                        &req.agent_name,
                        ExecutionMode::Sync,
                        start,
                        event_execution_id.clone(),
                        event_run_id.clone(),
                    )
                ) => match r {
                    Ok(r) => r,
                    Err(_) => {
                        execution_cancel.cancel();
                        Err(ReactError::Agent(Box::new(AgentError::Timeout(format!(
                            "Sync subagent '{}' timed out after {}s",
                            req.agent_name, timeout_secs
                        )))))
                    }
                },
            }
        } else {
            Self::execute_agent_streaming(
                self.registry.clone(),
                agent_arc.as_ref(),
                &task,
                req.message.clone(),
                execution_cancel,
                invocation,
                &req.parent_agent,
                &req.agent_name,
                ExecutionMode::Sync,
                start,
                event_execution_id,
                event_run_id,
            )
            .await
        }
    }

    /// Fork mode: acquire semaphore, spawn task, await with timeout.
    async fn dispatch_fork(&self, req: &DispatchRequest) -> Result<SubagentResult> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ReactError::Other(format!("Semaphore error: {}", e)))?;

        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;

        let agent_arc = self.isolated_dispatch_agent(&req.agent_name).await?;

        let timeout_secs = if registered.definition.timeout_secs > 0 {
            registered.definition.timeout_secs
        } else {
            self.config.default_timeout_secs
        };

        let task = req.task.clone();
        let agent_name = req.agent_name.clone();
        let parent_agent = req.parent_agent.clone();
        let cancel = req.cancel.clone();
        let registry = self.registry.clone();
        let message = req.message.clone();
        let enhanced_task = Self::enhance_task(
            &task,
            req.parent_context.as_ref(),
            registered.definition.inherit_history,
        );
        // 跨 spawn 安全的值传递: 把外部 run context 带进 spawn 块。
        // Carry run context as an invocation value. Factory-backed Fork roles
        // receive a fresh agent instance; legacy pre-built roles still use the
        // cached instance without mutating shared runtime/working_dir fields.
        let mut runtime_context = req.runtime_context.clone();
        if let Some(ctx) = runtime_context.as_mut() {
            ctx.delegation_policy = Some(req.delegation_policy);
        }
        // Extract stable identity for event payload (moved into the spawn below).
        let event_execution_id = runtime_context
            .as_ref()
            .and_then(|ctx| ctx.execution_id.clone());
        let event_run_id = runtime_context.as_ref().and_then(|ctx| ctx.run_id.clone());

        // Sprint 8: worktree isolation for writer subagents. Resolve the intent
        // before spawning so the closure can capture a shared factory clone.
        let isolate = registered.definition.isolate_worktree;
        let worktree_factory = if isolate {
            self.config.worktree_factory.clone()
        } else {
            None
        };
        if isolate && worktree_factory.is_none() {
            // Hard-fail: a subagent that declared isolate_worktree must not
            // silently share the main tree (multi-implementer safety). Local
            // personal-assistant threat model still needs this — otherwise
            // parallel writers corrupt each other's edits.
            return Err(ReactError::Other(format!(
                "Subagent '{}' declares isolate_worktree but no WorktreeFactory is configured; \
                 refusing to run without isolation",
                agent_name
            )));
        }
        // Sprint 10: data-workspace isolation for data/research subagents.
        // Mutually exclusive with worktree in intent — if a subagent declares
        // both, worktree wins (it also provides disjoint FS). Resolve the
        // workspace factory only when worktree isolation isn't being used.
        let isolate_workspace = registered.definition.isolate_workspace && !isolate;
        let data_workspace_factory = if isolate_workspace {
            self.config.data_workspace_factory.clone()
        } else {
            None
        };
        if isolate_workspace && data_workspace_factory.is_none() {
            tracing::warn!(
                subagent = %agent_name,
                "Subagent declares isolate_workspace but no DataWorkspaceFactory is configured; \
                 running without a workspace"
            );
        }
        let runtime_run_id = runtime_context
            .as_ref()
            .and_then(|ctx| ctx.run_id.as_deref())
            .unwrap_or("<none>");
        let has_trace_sink = runtime_context
            .as_ref()
            .is_some_and(|ctx| ctx.trace_sink.is_some());
        info!(
            parent = %parent_agent,
            subagent = %agent_name,
            runtime_run_id = %runtime_run_id,
            has_runtime_context = runtime_context.is_some(),
            has_trace_sink,
            timeout_secs,
            isolate_worktree = isolate,
            isolate_workspace,
            "subagent_fork_start"
        );
        // Label identifies this dispatch for worktree/workspace naming. run_id
        // (if available from the runtime context) disambiguates concurrent runs.
        let worktree_identity = runtime_context
            .as_ref()
            .and_then(|context| {
                context
                    .execution_id
                    .as_deref()
                    .or(context.run_id.as_deref())
                    .or(context.turn_id.as_deref())
            })
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().as_simple().to_string());
        let worktree_label = format!("{agent_name}-{worktree_identity}");
        let execution_cancel = cancel.child_token();
        let invocation_allowed_tools = req
            .parent_context
            .as_ref()
            .and_then(|context| context.allowed_tools.clone())
            .filter(|allowed| !allowed.is_empty());

        let result = tokio::spawn(async move {
            let _permit = permit;
            let start = Instant::now();

            // Check cancellation
            if execution_cancel.is_cancelled() {
                let mut result = SubagentResult::cancelled(
                    agent_name.clone(),
                    "Cancelled before execution",
                    ExecutionMode::Fork,
                );
                result.duration = start.elapsed();
                return Ok(result);
            }

            let agent = agent_arc.as_ref();

            // Sprint 8: if isolation was requested and a factory is available,
            // create a worktree and bind it as the subagent's working_dir BEFORE
            // execution. Creation failure is a hard error — never silently run
            // a writer without the promised isolation (would let it touch the
            // main checkout, a data-loss hazard).
            //
            // `create` may block on a git subprocess; the application's factory
            // is responsible for offloading that to spawn_blocking if needed.
            // We capture the handle to finalize (diff summary) after the run.
            let mut worktree_handle: Option<super::worktree::WorktreeHandle> = None;
            if let Some(factory) = &worktree_factory {
                match factory.create(&worktree_label) {
                    Ok(handle) => {
                        worktree_handle = Some(handle);
                    }
                    Err(e) => {
                        return Err(ReactError::Other(format!(
                            "Worktree isolation for Fork subagent '{agent_name}' failed: {e}"
                        )));
                    }
                }
            }

            // Sprint 10: data-workspace isolation. Same shape as worktree above
            // but for data/research subagents: a per-subagent tmpdir bound as
            // working_dir so output files are disjoint across parallel subagents.
            // Creation failure is a hard error (consistent with worktree).
            let mut workspace_handle: Option<super::workspace::DataWorkspaceHandle> = None;
            if let Some(factory) = &data_workspace_factory {
                match factory.create(&worktree_label) {
                    Ok(handle) => {
                        workspace_handle = Some(handle);
                    }
                    Err(e) => {
                        return Err(ReactError::Other(format!(
                            "Data workspace for Fork subagent '{agent_name}' failed: {e}"
                        )));
                    }
                }
            }

            let isolation_observed = if worktree_handle.is_some() {
                ObservedIsolation::Worktree
            } else if workspace_handle.is_some() {
                ObservedIsolation::Workspace
            } else {
                ObservedIsolation::Context
            };
            let disabled_tools =
                invocation_disabled_tools(agent.tool_names(), invocation_allowed_tools.as_deref());
            let invocation = AgentInvocationContext {
                runtime: runtime_context.clone(),
                working_dir: worktree_handle
                    .as_ref()
                    .map(|handle| handle.path.clone())
                    .or_else(|| workspace_handle.as_ref().map(|handle| handle.path.clone())),
                cancel: None,
                disabled_tools,
                run_budget: None,
            };
            registry
                .event_bus()
                .emit(SubagentEvent::DispatchIsolationObserved {
                    parent: parent_agent.clone(),
                    agent: agent_name.clone(),
                    isolation: isolation_observed,
                    execution_id: event_execution_id.clone(),
                    run_id: event_run_id.clone(),
                });

            let mut result = if timeout_secs > 0 {
                tokio::select! {
                    biased;
                    _ = execution_cancel.cancelled() => Err(ReactError::Agent(Box::new(
                        AgentError::Cancelled(format!("Fork subagent '{}' cancelled", agent_name))
                    ))),
                    r = tokio::time::timeout(
                        Duration::from_secs(timeout_secs),
                        Self::execute_agent_streaming(
                            registry,
                            agent,
                            &enhanced_task,
                            message.clone(),
                            execution_cancel.clone(),
                            invocation.clone(),
                            &parent_agent,
                            &agent_name,
                            ExecutionMode::Fork,
                            start,
                            event_execution_id.clone(),
                            event_run_id.clone(),
                        )
                    ) => {
                        match r {
                            Ok(r) => r,
                            Err(_) => {
                                execution_cancel.cancel();
                                Err(ReactError::Agent(Box::new(AgentError::Timeout(format!(
                                    "Fork subagent '{}' timed out after {}s",
                                    agent_name, timeout_secs
                                )))))
                            }
                        }
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = execution_cancel.cancelled() => Err(ReactError::Agent(Box::new(
                        AgentError::Cancelled(format!("Fork subagent '{}' cancelled", agent_name))
                    ))),
                    r = Self::execute_agent_streaming(
                        registry,
                        agent,
                        &enhanced_task,
                        message.clone(),
                        execution_cancel.clone(),
                        invocation.clone(),
                        &parent_agent,
                        &agent_name,
                        ExecutionMode::Fork,
                        start,
                        event_execution_id.clone(),
                        event_run_id.clone(),
                    ) => r,
                }
            };
            if let Ok(subagent_result) = &mut result {
                subagent_result.isolation_observed = isolation_observed;
            }

            // Sprint 8: finalize the worktree and append its diff summary. The
            // working directory is invocation-scoped, so reusable subagents keep
            // no mutable directory state that needs restoring after dispatch.
            if let Some(handle) = worktree_handle {
                match (handle.finalize)() {
                    Ok(diff) => {
                        if let Ok(mut r) = result {
                            if !diff.trim().is_empty() {
                                r.output =
                                    format!("{}\n\n--- worktree diff ---\n{}", r.output, diff);
                            }
                            return Ok(r);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            subagent = %agent_name,
                            error = %e,
                            "Worktree finalize (diff summary) failed; result preserved"
                        );
                    }
                }
            }

            // Sprint 10: finalize the invocation-scoped data workspace and
            // append its file listing so downstream subagents can find outputs.
            if let Some(handle) = workspace_handle {
                match (handle.finalize)() {
                    Ok(listing) => {
                        if let Ok(mut r) = result {
                            if !listing.trim().is_empty() {
                                r.output = format!(
                                    "{}\n\n--- workspace outputs ---\n{}",
                                    r.output, listing
                                );
                            }
                            return Ok(r);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            subagent = %agent_name,
                            error = %e,
                            "Data workspace finalize (file listing) failed; result preserved"
                        );
                    }
                }
            }

            result
        })
        .await
        .map_err(|e| ReactError::Other(format!("Fork task join error: {}", e)))??;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::context::SubagentContext;
    use crate::agent::subagent::registry::FnAgentFactory;
    use crate::testing::{FailingMockAgent, MockAgent};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn invocation_allowlist_hides_every_unlisted_tool() {
        let tools = vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "shell".to_string(),
        ];
        let allowed = vec!["read_file".to_string()];
        let disabled = invocation_disabled_tools(tools.clone(), Some(&allowed)).unwrap_or_default();
        assert_eq!(
            disabled,
            HashSet::from(["write_file".to_string(), "shell".to_string()])
        );
        assert!(invocation_disabled_tools(tools.clone(), None).is_none());
        assert!(invocation_disabled_tools(tools, Some(&[])).is_none());
    }

    struct DelegateFailedDispatch;

    struct CancellationAwareStreamAgent {
        cancellation_seen: Arc<tokio::sync::Notify>,
    }

    impl Agent for CancellationAwareStreamAgent {
        fn name(&self) -> &str {
            "cancellation-aware"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(&'a self, _task: &'a str) -> futures::future::BoxFuture<'a, Result<String>> {
            Box::pin(async move { std::future::pending::<Result<String>>().await })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> futures::future::BoxFuture<
            'a,
            Result<futures::stream::BoxStream<'a, Result<AgentEvent>>>,
        > {
            Box::pin(async move {
                let (_sender, receiver) = tokio::sync::mpsc::channel(1);
                Ok(
                    Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver))
                        as futures::stream::BoxStream<'a, Result<AgentEvent>>,
                )
            })
        }

        fn execute_stream_with_invocation_context<'a>(
            &'a self,
            _task: &'a str,
            cancel: CancellationToken,
            _invocation: AgentInvocationContext,
        ) -> futures::future::BoxFuture<
            'a,
            Result<futures::stream::BoxStream<'a, Result<AgentEvent>>>,
        > {
            let cancellation_seen = Arc::clone(&self.cancellation_seen);
            Box::pin(async move {
                let (sender, receiver) = tokio::sync::mpsc::channel(1);
                tokio::spawn(async move {
                    cancel.cancelled().await;
                    cancellation_seen.notify_one();
                    let _ = sender.send(Ok(AgentEvent::Cancelled)).await;
                });
                Ok(
                    Box::pin(tokio_stream::wrappers::ReceiverStream::new(receiver))
                        as futures::stream::BoxStream<'a, Result<AgentEvent>>,
                )
            })
        }
    }

    #[async_trait::async_trait]
    impl super::super::hooks::SubagentHooks for DelegateFailedDispatch {
        async fn on_failure(
            &self,
            _ctx: &super::super::hooks::SubagentHookContext,
            _error: &str,
        ) -> super::super::hooks::SubagentRetryDecision {
            super::super::hooks::SubagentRetryDecision::Delegate {
                alternative_agent: "recovery".to_string(),
            }
        }
    }

    async fn make_executor() -> (Arc<SubagentRegistry>, SubagentExecutor) {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
        (registry, executor)
    }

    fn collect_terminal_events(
        events: &mut tokio::sync::broadcast::Receiver<Arc<SubagentEvent>>,
    ) -> Vec<Arc<SubagentEvent>> {
        let mut terminal_events = Vec::new();
        while let Ok(event) = events.try_recv() {
            if matches!(
                event.as_ref(),
                SubagentEvent::DispatchCompleted { .. }
                    | SubagentEvent::DispatchFailed { .. }
                    | SubagentEvent::DispatchCancelled { .. }
            ) {
                terminal_events.push(event);
            }
        }
        terminal_events
    }

    /// Build a SubagentContext with N numbered user messages and no system prompt.
    fn ctx_with_messages(n: usize) -> SubagentContext {
        let mut ctx = SubagentContext::empty();
        ctx.messages = (0..n)
            .map(|i| echo_core::llm::types::Message::user(format!("msg{i}")))
            .collect();
        ctx
    }

    #[test]
    fn dispatch_request_uses_nested_delegation_policy_as_depth_authority() {
        let policy = DispatchRequest::policy_from_depth(2);

        assert!(policy.can_spawn_subagents);
        assert_eq!(policy.delegate_depth, 2);
        assert_eq!(policy.max_delegate_depth, 3);

        let child = policy.child_policy().unwrap_or_default();
        assert_eq!(child.delegate_depth, 3);
        assert!(!child.can_delegate());
    }

    #[test]
    fn merge_observed_evidence_keeps_latest_check_result() {
        let mut outcome = SubagentOutcome {
            verification: vec![SubagentVerification {
                check: "cargo test".to_string(),
                status: SubagentVerificationStatus::Passed,
                details: "model claim".to_string(),
                source: SubagentVerificationSource::Reported,
            }],
            ..SubagentOutcome::default()
        };
        merge_observed_evidence(
            &mut outcome,
            vec![
                SubagentVerification {
                    check: "cargo test".to_string(),
                    status: SubagentVerificationStatus::Failed,
                    details: "first run failed".to_string(),
                    source: SubagentVerificationSource::Observed,
                },
                SubagentVerification {
                    check: "cargo test".to_string(),
                    status: SubagentVerificationStatus::Passed,
                    details: "retry passed".to_string(),
                    source: SubagentVerificationSource::Observed,
                },
            ],
            SubagentTouchedFiles::default(),
            Vec::new(),
        );
        assert!(matches!(
            outcome.verification.as_slice(),
            [SubagentVerification {
                status: SubagentVerificationStatus::Passed,
                source: SubagentVerificationSource::Observed,
                ..
            }]
        ));
    }

    #[test]
    fn enhance_task_no_context_appends_result_contract() {
        let out = SubagentExecutor::enhance_task("do thing", None, None);
        assert!(out.starts_with("do thing"));
        assert!(out.contains("## Result"));
        assert!(out.contains("\"contract_version\":1"));
    }

    #[test]
    fn enhance_task_empty_fresh_context_still_appends_result_contract() {
        let ctx = super::super::context::SubagentContext::empty();
        let out = SubagentExecutor::enhance_task("do thing", Some(&ctx), None);
        assert!(out.starts_with("do thing"));
        assert!(out.contains("## Result"));
    }

    #[test]
    fn enhance_task_prepends_scoped_user_request_once() {
        let mut ctx = super::super::context::SubagentContext::empty();
        ctx.parent_goal = Some("请核对并发问题 🧭".to_string());

        let out = SubagentExecutor::enhance_task("inspect executor", Some(&ctx), None);

        assert!(out.starts_with(
            "[user_request (language anchor — reply in this language)]\n请核对并发问题 🧭\n[/user_request]\n\n---\n\ninspect executor"
        ));
        assert_eq!(out.matches("[user_request").count(), 1);
    }

    #[test]
    fn enhance_task_does_not_duplicate_existing_user_request() {
        let mut ctx = super::super::context::SubagentContext::empty();
        ctx.parent_goal = Some("parent request".to_string());
        let task = "[user_request (language anchor — reply in this language)]\nexplicit request\n[/user_request]\n\ninspect executor";

        let out = SubagentExecutor::enhance_task(task, Some(&ctx), None);

        assert!(out.starts_with(task));
        assert_eq!(out.matches("[user_request").count(), 1);
        assert!(!out.contains("parent request"));
    }

    #[test]
    fn enhance_task_does_not_duplicate_existing_result_contract() {
        let task = "do thing\n\n## Result\n```json\n{\"contract_version\":1}\n```";
        let out = SubagentExecutor::enhance_task(task, None, None);
        assert_eq!(out, task);
    }

    #[test]
    fn enhance_task_inherit_history_none_omits_history() {
        // Sprint 6b: inherit_history=None → no history joined, even though
        // parent_ctx has messages. System prompt still joined if present.
        let mut ctx = ctx_with_messages(5);
        ctx.system_prompt = "SYS".to_string();
        let out = SubagentExecutor::enhance_task("task", Some(&ctx), None);
        assert!(out.contains("[Inherited System Context]\nSYS"));
        assert!(out.contains("task"));
        assert!(
            !out.contains("msg"),
            "with inherit_history=None no history should be joined"
        );
    }

    #[test]
    fn enhance_task_inherit_history_n_takes_last_n() {
        let ctx = ctx_with_messages(5);
        // Some(2) → only last 2 messages (msg3, msg4).
        let out = SubagentExecutor::enhance_task("task", Some(&ctx), Some(2));
        assert!(out.contains("[user] msg3"));
        assert!(out.contains("[user] msg4"));
        assert!(!out.contains("msg0"));
        assert!(!out.contains("msg2"));
    }

    #[test]
    fn enhance_task_inherit_history_zero_takes_all() {
        let ctx = ctx_with_messages(3);
        let out = SubagentExecutor::enhance_task("task", Some(&ctx), Some(0));
        assert!(out.contains("msg0"));
        assert!(out.contains("msg1"));
        assert!(out.contains("msg2"));
    }

    #[test]
    fn enhance_task_inherit_history_larger_than_available_takes_all() {
        // saturating_sub keeps this panic-free.
        let ctx = ctx_with_messages(2);
        let out = SubagentExecutor::enhance_task("task", Some(&ctx), Some(10));
        assert!(out.contains("msg0"));
        assert!(out.contains("msg1"));
    }

    #[tokio::test]
    async fn test_dispatch_sync() {
        let (registry, executor) = make_executor().await;

        let agent = MockAgent::new("subagent").with_response("done");
        let def = super::super::types::SubagentDefinition::new("subagent", "Subagent");
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "subagent".into(),
            task: "do work".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "done");
        assert_eq!(result.mode, ExecutionMode::Sync);
    }

    #[tokio::test]
    async fn dispatch_background_returns_before_completion() {
        let (registry, executor) = make_executor().await;
        let agent = MockAgent::new("slow")
            .with_delay_ms(200)
            .with_response("## Summary\nbg done");
        let def = super::super::types::SubagentDefinition::new("slow", "Slow subagent");
        registry.register(def, Box::new(agent)).await;

        let mut events = registry.event_bus().subscribe();
        let req = DispatchRequest {
            agent_name: "slow".into(),
            task: "take your time".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let started_at = Instant::now();
        let handle = executor.dispatch_background(req).await.unwrap();
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "dispatch_background must return before the slow subagent finishes"
        );
        assert!(!handle.execution_id.is_empty());
        assert_eq!(handle.agent_name, "slow");
        assert!(handle.execution_id.starts_with("agent_tool-"));

        let mut saw_started_bg = false;
        let mut saw_completed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline && !(saw_started_bg && saw_completed) {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Ok(ev)) => match ev.as_ref() {
                    SubagentEvent::DispatchStarted {
                        background: true,
                        execution_id: Some(id),
                        ..
                    } if id == &handle.execution_id => {
                        saw_started_bg = true;
                    }
                    SubagentEvent::DispatchCompleted {
                        output,
                        execution_id: Some(id),
                        ..
                    } if id == &handle.execution_id => {
                        assert!(output.contains("bg done"));
                        saw_completed = true;
                    }
                    _ => {}
                },
                _ => break,
            }
        }
        assert!(
            saw_started_bg,
            "expected DispatchStarted with background=true"
        );
        assert!(
            saw_completed,
            "expected DispatchCompleted after background work"
        );
    }

    #[tokio::test]
    async fn dispatch_started_preserves_chat_identity() -> std::result::Result<(), String> {
        let (registry, executor) = make_executor().await;
        let agent = MockAgent::new("identity").with_response("done");
        let definition =
            super::super::types::SubagentDefinition::new("identity", "Identity subagent");
        registry.register(definition, Box::new(agent)).await;
        let mut events = registry.event_bus().subscribe();
        let request = DispatchRequest {
            agent_name: "identity".to_string(),
            task: "preserve identity".to_string(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: Some(echo_core::tools::ExternalRunContext {
                conversation_id: Some("conversation-identity".to_string()),
                run_id: None,
                turn_id: Some("turn-identity".to_string()),
                execution_id: Some("agent_tool-identity".to_string()),
                message_id: Some("message-identity".to_string()),
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            message: None,
            background: false,
        };

        executor
            .dispatch(request)
            .await
            .map_err(|error| error.to_string())?;
        let started = events.recv().await.map_err(|error| error.to_string())?;
        match started.as_ref() {
            SubagentEvent::DispatchStarted {
                conversation_id,
                message_id,
                run_id,
                ..
            } => {
                assert_eq!(conversation_id.as_deref(), Some("conversation-identity"));
                assert_eq!(message_id.as_deref(), Some("message-identity"));
                assert!(run_id.is_none());
            }
            other => return Err(format!("expected DispatchStarted, got {other:?}")),
        }
        Ok(())
    }

    // NOTE: a unit test for multimodal dispatch forwarding (verifying a subagent
    // receives the Message via execute_stream_message_with_cancel) is not
    // feasible with MockAgent — the trait-object vtable routes the default
    // trait method rather than MockAgent's override for this added method, so
    // the message path can't be exercised in isolation. Subagent multimodal
    // forwarding is instead verified by: (1) the text-path dispatch tests
    // above, (2) compile-time coverage of the DispatchRequest.message field
    // and execute_agent_streaming branch, and (3) GUI manual testing of
    // attachment-bearing complex tasks. MockAgent retains the override +
    // message-recording fields so real agents and future test harnesses can
    // use them.

    #[tokio::test]
    async fn test_dispatch_not_found() {
        let (_registry, executor) = make_executor().await;

        let req = DispatchRequest {
            agent_name: "missing".into(),
            task: "task".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_dispatch_cancelled() {
        let (registry, executor) = make_executor().await;

        let agent = MockAgent::new("c").with_response("ok");
        let def = super::super::types::SubagentDefinition::new("c", "C");
        registry.register(def, Box::new(agent)).await;

        let cancel = CancellationToken::new();
        cancel.cancel();

        let req = DispatchRequest {
            agent_name: "c".into(),
            task: "task".into(),
            mode_override: None,
            cancel,
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert!(result.output.contains("Cancelled"));
        assert_eq!(result.outcome.status, SubagentStatus::Cancelled);
    }

    #[tokio::test]
    async fn recovered_dispatch_emits_only_one_terminal_event() -> Result<()> {
        let registry = Arc::new(SubagentRegistry::new());
        let mut hooks = super::super::hooks::SubagentHookRegistry::new();
        hooks.register(Arc::new(DelegateFailedDispatch));
        let executor = SubagentExecutor::with_hooks(
            registry.clone(),
            SubagentExecutorConfig::default(),
            hooks,
        );
        registry
            .register(
                super::super::types::SubagentDefinition::new("primary", "Primary"),
                Box::new(FailingMockAgent::new("primary", "first attempt failed")),
            )
            .await;
        registry
            .register(
                super::super::types::SubagentDefinition::new("recovery", "Recovery"),
                Box::new(MockAgent::new("recovery").with_response("recovered")),
            )
            .await;
        let mut events = registry.event_bus().subscribe();

        let result = executor
            .dispatch(DispatchRequest {
                agent_name: "primary".to_string(),
                task: "recover this task".to_string(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await?;
        assert_eq!(result.outcome.status, SubagentStatus::Completed);

        let terminal_events = collect_terminal_events(&mut events);
        assert_eq!(terminal_events.len(), 1);
        assert!(matches!(
            terminal_events.first().map(AsRef::as_ref),
            Some(SubagentEvent::DispatchCompleted { result, .. })
                if result.status == SubagentStatus::Completed
        ));
        Ok(())
    }

    #[tokio::test]
    async fn failed_dispatch_emits_failed_terminal_status() -> std::result::Result<(), String> {
        let (registry, executor) = make_executor().await;
        registry
            .register(
                super::super::types::SubagentDefinition::new("failing", "Failing"),
                Box::new(FailingMockAgent::new("failing", "boom")),
            )
            .await;
        let mut events = registry.event_bus().subscribe();
        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "failing".to_string(),
                task: "fail".to_string(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| "failing subagent unexpectedly completed".to_string())?;
        assert_eq!(subagent_status_from_error(&error), SubagentStatus::Failed);
        let terminal_events = collect_terminal_events(&mut events);
        assert!(matches!(
            terminal_events.as_slice(),
            [event]
                if matches!(
                    event.as_ref(),
                    SubagentEvent::DispatchFailed {
                        status: SubagentStatus::Failed,
                        ..
                    }
                )
        ));
        Ok(())
    }

    #[tokio::test]
    async fn running_dispatch_cancel_emits_cancelled_terminal_status()
    -> std::result::Result<(), String> {
        let (registry, executor) = make_executor().await;
        registry
            .register(
                super::super::types::SubagentDefinition::new("slow-cancel", "Slow cancel"),
                Box::new(MockAgent::new("slow-cancel").with_delay_ms(500)),
            )
            .await;
        let mut events = registry.event_bus().subscribe();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            trigger.cancel();
        });
        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "slow-cancel".to_string(),
                task: "wait".to_string(),
                mode_override: None,
                cancel,
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| "cancelled subagent unexpectedly completed".to_string())?;
        assert_eq!(
            subagent_status_from_error(&error),
            SubagentStatus::Cancelled
        );
        let terminal_events = collect_terminal_events(&mut events);
        assert!(matches!(
            terminal_events.as_slice(),
            [event]
                if matches!(event.as_ref(), SubagentEvent::DispatchCancelled { result, .. }
                    if result.status == SubagentStatus::Cancelled)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn timed_out_dispatch_emits_timed_out_terminal_status() -> std::result::Result<(), String>
    {
        let (registry, executor) = make_executor().await;
        let mut definition =
            super::super::types::SubagentDefinition::new("slow-timeout", "Slow timeout");
        definition.timeout_secs = 1;
        registry
            .register(
                definition,
                Box::new(MockAgent::new("slow-timeout").with_delay_ms(1_500)),
            )
            .await;
        let mut events = registry.event_bus().subscribe();
        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "slow-timeout".to_string(),
                task: "wait".to_string(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| "timed-out subagent unexpectedly completed".to_string())?;
        assert_eq!(subagent_status_from_error(&error), SubagentStatus::TimedOut);
        let terminal_events = collect_terminal_events(&mut events);
        assert!(matches!(
            terminal_events.as_slice(),
            [event]
                if matches!(
                    event.as_ref(),
                    SubagentEvent::DispatchFailed {
                        status: SubagentStatus::TimedOut,
                        result,
                        ..
                    } if result.status == SubagentStatus::TimedOut
                )
        ));
        Ok(())
    }

    #[tokio::test]
    async fn test_dispatch_mode_override() {
        let (registry, executor) = make_executor().await;

        let agent = MockAgent::new("forker").with_response("forked");
        let mut def = super::super::types::SubagentDefinition::new("forker", "Fork agent");
        def.execution_mode = ExecutionMode::Sync; // Default is Sync
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "forker".into(),
            task: "task".into(),
            mode_override: Some(ExecutionMode::Fork),
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "forked");
        assert_eq!(result.mode, ExecutionMode::Fork);
    }

    #[tokio::test]
    async fn sync_dispatch_creates_fresh_agent_per_request() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let mut definition = super::super::types::SubagentDefinition::new("explorer", "Explorer");
        definition.execution_mode = ExecutionMode::Sync;
        registry
            .register(
                definition.clone(),
                Box::new(MockAgent::new("explorer").with_response("cached")),
            )
            .await;

        let creations = Arc::new(AtomicUsize::new(0));
        let factory_creations = Arc::clone(&creations);
        let factory = Arc::new(FnAgentFactory::new(move || {
            let instance = factory_creations
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            Box::pin(async move {
                Ok(Box::new(
                    MockAgent::new("explorer")
                        .with_delay_ms(50)
                        .with_response(format!("instance-{instance}")),
                ) as Box<dyn Agent>)
            })
        }));
        assert!(registry.register_factory_sync(definition, factory));

        let request = || DispatchRequest {
            agent_name: "explorer".to_string(),
            task: "inspect".to_string(),
            mode_override: Some(ExecutionMode::Sync),
            cancel: CancellationToken::new(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let (first, second) =
            tokio::join!(executor.dispatch(request()), executor.dispatch(request()));
        let first = first?;
        let second = second?;

        assert_eq!(creations.load(Ordering::SeqCst), 2);
        assert_ne!(first.output, second.output);
        assert_ne!(first.output, "cached");
        assert_ne!(second.output, "cached");
        Ok(())
    }

    #[tokio::test]
    async fn sync_timeout_cancels_detached_stream_producer() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let cancellation_seen = Arc::new(tokio::sync::Notify::new());
        let mut definition = super::super::types::SubagentDefinition::new("slow", "Slow stream");
        definition.execution_mode = ExecutionMode::Sync;
        definition.timeout_secs = 1;
        registry
            .register(
                definition,
                Box::new(CancellationAwareStreamAgent {
                    cancellation_seen: Arc::clone(&cancellation_seen),
                }),
            )
            .await;

        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "slow".to_string(),
                task: "wait".to_string(),
                mode_override: Some(ExecutionMode::Sync),
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| ReactError::Other("slow Sync unexpectedly completed".to_string()))?;

        assert_eq!(subagent_status_from_error(&error), SubagentStatus::TimedOut);
        tokio::time::timeout(Duration::from_secs(1), cancellation_seen.notified())
            .await
            .map_err(|_| {
                ReactError::Other(
                    "Sync timeout did not cancel the detached stream producer".to_string(),
                )
            })?;
        Ok(())
    }

    #[tokio::test]
    async fn fork_dispatch_creates_fresh_agent_per_request() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let mut definition = super::super::types::SubagentDefinition::new("explorer", "Explorer");
        definition.execution_mode = ExecutionMode::Fork;
        registry
            .register(
                definition.clone(),
                Box::new(MockAgent::new("explorer").with_response("cached")),
            )
            .await;

        let creations = Arc::new(AtomicUsize::new(0));
        let factory_creations = Arc::clone(&creations);
        let factory = Arc::new(FnAgentFactory::new(move || {
            let instance = factory_creations
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            Box::pin(async move {
                Ok(Box::new(
                    MockAgent::new("explorer")
                        .with_delay_ms(50)
                        .with_response(format!("instance-{instance}")),
                ) as Box<dyn Agent>)
            })
        }));
        assert!(registry.register_factory_sync(definition, factory));

        let request = || DispatchRequest {
            agent_name: "explorer".to_string(),
            task: "inspect".to_string(),
            mode_override: Some(ExecutionMode::Fork),
            cancel: CancellationToken::new(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let (first, second) =
            tokio::join!(executor.dispatch(request()), executor.dispatch(request()));
        let first = first?;
        let second = second?;

        assert_eq!(creations.load(Ordering::SeqCst), 2);
        assert_ne!(first.output, second.output);
        assert_ne!(first.output, "cached");
        assert_ne!(second.output, "cached");
        Ok(())
    }

    #[tokio::test]
    async fn fork_timeout_cancels_detached_stream_producer() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let cancellation_seen = Arc::new(tokio::sync::Notify::new());
        let mut definition = super::super::types::SubagentDefinition::new("slow", "Slow stream");
        definition.execution_mode = ExecutionMode::Fork;
        definition.timeout_secs = 1;
        registry
            .register(
                definition,
                Box::new(CancellationAwareStreamAgent {
                    cancellation_seen: Arc::clone(&cancellation_seen),
                }),
            )
            .await;

        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "slow".to_string(),
                task: "wait".to_string(),
                mode_override: Some(ExecutionMode::Fork),
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| ReactError::Other("slow Fork unexpectedly completed".to_string()))?;

        assert_eq!(subagent_status_from_error(&error), SubagentStatus::TimedOut);
        tokio::time::timeout(Duration::from_secs(1), cancellation_seen.notified())
            .await
            .map_err(|_| {
                ReactError::Other(
                    "Fork timeout did not cancel the detached stream producer".to_string(),
                )
            })?;
        Ok(())
    }

    #[tokio::test]
    async fn test_teammate_dispatch() {
        let (registry, executor) = make_executor().await;

        let agent = MockAgent::new("tm").with_response("team result");
        let mut def = super::super::types::SubagentDefinition::new("tm", "Teammate");
        def.execution_mode = ExecutionMode::Teammate;
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "tm".into(),
            task: "team task".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "leader".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let handle = executor.dispatch_teammate(req).await.unwrap();
        assert_eq!(handle.agent_name, "tm");

        let result = handle.join().await.unwrap();
        assert_eq!(result.output, "team result");
    }

    #[tokio::test]
    async fn teammate_dispatch_creates_fresh_agent_per_request() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let mut definition = super::super::types::SubagentDefinition::new("explorer", "Explorer");
        definition.execution_mode = ExecutionMode::Teammate;
        registry
            .register(
                definition.clone(),
                Box::new(MockAgent::new("explorer").with_response("cached")),
            )
            .await;

        let creations = Arc::new(AtomicUsize::new(0));
        let factory_creations = Arc::clone(&creations);
        let factory = Arc::new(FnAgentFactory::new(move || {
            let instance = factory_creations
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            Box::pin(async move {
                Ok(Box::new(
                    MockAgent::new("explorer")
                        .with_delay_ms(50)
                        .with_response(format!("instance-{instance}")),
                ) as Box<dyn Agent>)
            })
        }));
        assert!(registry.register_factory_sync(definition, factory));

        let request = || DispatchRequest {
            agent_name: "explorer".to_string(),
            task: "inspect".to_string(),
            mode_override: Some(ExecutionMode::Teammate),
            cancel: CancellationToken::new(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let (first, second) =
            tokio::join!(executor.dispatch(request()), executor.dispatch(request()));
        let first = first?;
        let second = second?;

        assert_eq!(creations.load(Ordering::SeqCst), 2);
        assert_ne!(first.output, second.output);
        assert_ne!(first.output, "cached");
        assert_ne!(second.output, "cached");
        Ok(())
    }

    #[tokio::test]
    async fn teammate_timeout_cancels_detached_stream_producer() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let cancellation_seen = Arc::new(tokio::sync::Notify::new());
        let mut definition = super::super::types::SubagentDefinition::new("slow", "Slow stream");
        definition.execution_mode = ExecutionMode::Teammate;
        definition.timeout_secs = 1;
        registry
            .register(
                definition,
                Box::new(CancellationAwareStreamAgent {
                    cancellation_seen: Arc::clone(&cancellation_seen),
                }),
            )
            .await;

        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "slow".to_string(),
                task: "wait".to_string(),
                mode_override: Some(ExecutionMode::Teammate),
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| ReactError::Other("slow Teammate unexpectedly completed".to_string()))?;

        assert_eq!(subagent_status_from_error(&error), SubagentStatus::TimedOut);
        tokio::time::timeout(Duration::from_secs(1), cancellation_seen.notified())
            .await
            .map_err(|_| {
                ReactError::Other(
                    "Teammate timeout did not cancel the detached stream producer".to_string(),
                )
            })?;
        Ok(())
    }

    #[tokio::test]
    async fn teammate_handle_cancel_does_not_cancel_parent() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let definition = super::super::types::SubagentDefinition::new("slow", "Slow stream");
        registry
            .register(
                definition,
                Box::new(CancellationAwareStreamAgent {
                    cancellation_seen: Arc::new(tokio::sync::Notify::new()),
                }),
            )
            .await;

        let parent_cancel = CancellationToken::new();
        let handle = executor
            .dispatch_teammate(DispatchRequest {
                agent_name: "slow".to_string(),
                task: "wait".to_string(),
                mode_override: Some(ExecutionMode::Teammate),
                cancel: parent_cancel.clone(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                background: false,
            })
            .await?;

        handle.cancel.cancel();
        if parent_cancel.is_cancelled() {
            return Err(ReactError::Other(
                "cancelling a Teammate handle cancelled its parent run".to_string(),
            ));
        }
        let error = handle
            .join()
            .await
            .err()
            .ok_or_else(|| ReactError::Other("cancelled Teammate completed".to_string()))?;
        assert!(
            matches!(error, ReactError::Agent(inner) if matches!(*inner, AgentError::Interrupted))
        );
        Ok(())
    }

    // ── Sprint 11: Team dispatch (ExecutionMode::Team + TeamSpec) ─────────

    #[tokio::test]
    async fn test_dispatch_team_routes_and_runs() {
        // Register a team-mode subagent + its named manager + subagent. Dispatch
        // must route to dispatch_team and return the synthesized result.
        let (registry, executor) = make_executor().await;

        // Manager: MockAgent returns plan first, then synthesis.
        let manager = MockAgent::new("mgr")
            .with_response("sub1\nsub2")
            .with_response("SYNTH");
        let mgr_def = super::super::types::SubagentDefinition::new("mgr", "Manager");
        registry.register(mgr_def, Box::new(manager)).await;

        // Subagent: returns a canned result.
        let subagent = MockAgent::new("wk").with_response("subagent-out");
        let w_def = super::super::types::SubagentDefinition::new("wk", "Subagent");
        registry.register(w_def, Box::new(subagent)).await;

        // Team definition: references mgr + wk by name.
        let team_spec = super::super::types::TeamSpec {
            strategy: super::super::team::strategy::TeamStrategy::ManagerSubagent,
            manager: "mgr".to_string(),
            subagents: vec!["wk".to_string()],
            config: super::super::team::TeamConfig::default(),
        };
        let mut team_def =
            super::super::types::SubagentDefinition::new("team-research", "team dispatcher");
        team_def.execution_mode = ExecutionMode::Team;
        team_def.team = Some(team_spec);
        // The team definition itself needs an agent instance registered too
        // (dispatch looks up the definition by name), but it's never executed
        // as an agent — use a placeholder mock.
        let placeholder = MockAgent::new("team-research");
        registry.register(team_def, Box::new(placeholder)).await;

        let req = DispatchRequest {
            agent_name: "team-research".into(),
            task: "research X".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.mode, ExecutionMode::Team);
        // Manager's second execute() is synthesis → "SYNTH".
        assert_eq!(result.output, "SYNTH");
    }

    #[tokio::test]
    async fn test_dispatch_team_without_spec_errors() {
        // A Team-mode definition with no TeamSpec → clear error.
        let (registry, executor) = make_executor().await;
        let agent = MockAgent::new("broken");
        let mut def = super::super::types::SubagentDefinition::new("broken", "no spec");
        def.execution_mode = ExecutionMode::Team;
        def.team = None;
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "broken".into(),
            task: "task".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(err.to_string().contains("no TeamSpec"));
    }

    // ── Sprint 8: Fork worktree isolation ──────────────────────────────────

    use crate::agent::subagent::worktree::{WorktreeError, WorktreeFactory, WorktreeHandle};
    use std::sync::Mutex as StdMutex;

    /// A mock factory whose `create` always succeeds, records the label, and
    /// whose `finalize` returns a canned diff. `should_fail` toggles hard-fail.
    struct MockWorktreeFactory {
        labels: StdMutex<Vec<String>>,
        should_fail: bool,
    }

    impl WorktreeFactory for MockWorktreeFactory {
        fn create(&self, label: &str) -> std::result::Result<WorktreeHandle, WorktreeError> {
            if self.should_fail {
                return Err(WorktreeError::new("mock worktree create failed"));
            }
            self.labels.lock().unwrap().push(label.to_string());
            let path = std::path::PathBuf::from(format!("/tmp/mock-wt-{label}"));
            Ok(WorktreeHandle {
                path,
                finalize: Box::new(|| {
                    Ok::<String, WorktreeError>("=== mock diff ===\nfoo.rs | 1 +".to_string())
                }),
            })
        }
    }

    /// Build an executor with a worktree factory wired into its config.
    fn make_executor_with_factory(
        factory: Arc<dyn WorktreeFactory>,
    ) -> (Arc<SubagentRegistry>, SubagentExecutor) {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                worktree_factory: Some(factory),
                ..SubagentExecutorConfig::default()
            },
        );
        (registry, executor)
    }

    #[tokio::test]
    async fn fork_isolate_worktree_binds_path_and_appends_diff() {
        // A writer subagent declaring isolate_worktree should observe Worktree
        // isolation for this invocation and append the finalized diff.
        let factory = Arc::new(MockWorktreeFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn WorktreeFactory> = factory.clone();
        let (registry, executor) = make_executor_with_factory(factory_obs);

        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: true,
            ..super::super::types::SubagentDefinition::new("writer", "Writer")
        };
        registry.register(def, Box::new(agent.clone())).await;

        let req = DispatchRequest {
            agent_name: "writer".into(),
            task: "edit foo".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        // Output = subagent's answer + appended diff (the diff append is the
        // observable proof that the worktree was created and finalized).
        assert!(result.output.contains("done"));
        assert!(result.output.contains("=== mock diff ==="));
        // Factory was invoked once with a label derived from the agent name.
        let labels = factory.labels.lock().unwrap().clone();
        assert_eq!(labels.len(), 1);
        assert!(labels[0].starts_with("writer-"));
    }

    #[tokio::test]
    async fn fork_worktree_observation_precedes_subagent_completion()
    -> std::result::Result<(), String> {
        let factory = Arc::new(MockWorktreeFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn WorktreeFactory> = factory;
        let (registry, executor) = make_executor_with_factory(factory_obs);
        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: true,
            ..super::super::types::SubagentDefinition::new("writer", "Writer")
        };
        registry.register(def, Box::new(agent)).await;
        let mut events = registry.event_bus().subscribe();
        let req = DispatchRequest {
            agent_name: "writer".into(),
            task: "edit foo".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor
            .dispatch(req)
            .await
            .map_err(|error| error.to_string())?;
        if result.isolation_observed != ObservedIsolation::Worktree {
            return Err(format!(
                "expected worktree result observation, got {:?}",
                result.isolation_observed
            ));
        }
        let started = events.recv().await.map_err(|error| error.to_string())?;
        let observed = events.recv().await.map_err(|error| error.to_string())?;
        let completed = events.recv().await.map_err(|error| error.to_string())?;
        if !matches!(started.as_ref(), SubagentEvent::DispatchStarted { .. }) {
            return Err(format!("first event was not started: {started:?}"));
        }
        if !matches!(
            observed.as_ref(),
            SubagentEvent::DispatchIsolationObserved {
                isolation: ObservedIsolation::Worktree,
                ..
            }
        ) {
            return Err(format!(
                "second event was not worktree observation: {observed:?}"
            ));
        }
        if !matches!(completed.as_ref(), SubagentEvent::DispatchCompleted { .. }) {
            return Err(format!("third event was not completed: {completed:?}"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_forks_carry_distinct_worktree_invocations()
    -> std::result::Result<(), String> {
        let factory = Arc::new(MockWorktreeFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn WorktreeFactory> = factory;
        let (registry, executor) = make_executor_with_factory(factory_obs);
        let agent = MockAgent::new("writer").with_responses(["done-a", "done-b"]);
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: true,
            ..super::super::types::SubagentDefinition::new("writer", "Writer")
        };
        registry.register(def, Box::new(agent.clone())).await;
        let executor = Arc::new(executor);
        let request = |run_id: &str| DispatchRequest {
            agent_name: "writer".into(),
            task: format!("edit for {run_id}"),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: Some(echo_core::tools::ExternalRunContext {
                conversation_id: None,
                run_id: Some(run_id.to_string()),
                turn_id: None,
                execution_id: Some(format!("execution-{run_id}")),
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            message: None,
            background: false,
        };

        let first = {
            let executor = Arc::clone(&executor);
            async move { executor.dispatch(request("run-a")).await }
        };
        let second = {
            let executor = Arc::clone(&executor);
            async move { executor.dispatch(request("run-b")).await }
        };
        let (first_result, second_result) = tokio::join!(first, second);
        first_result.map_err(|error| error.to_string())?;
        second_result.map_err(|error| error.to_string())?;

        let invocations = agent.invocation_contexts();
        if invocations.len() != 2 {
            return Err(format!(
                "expected two value-scoped invocations, got {invocations:?}"
            ));
        }
        for invocation in invocations {
            let runtime = invocation
                .runtime
                .ok_or_else(|| "fork invocation missing runtime context".to_string())?;
            let run_id = runtime
                .run_id
                .ok_or_else(|| "fork invocation missing run id".to_string())?;
            let working_dir = invocation
                .working_dir
                .ok_or_else(|| format!("{run_id} missing worktree path"))?;
            let path = working_dir.to_string_lossy();
            if !path.contains(run_id.as_str()) {
                return Err(format!(
                    "cross-worktree invocation: run {} received {}",
                    run_id, path
                ));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn fork_isolate_worktree_create_failure_fails_dispatch() {
        // A factory that hard-fails → dispatch must fail, never run unisolated.
        let factory = Arc::new(MockWorktreeFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: true,
        });
        let factory_obs: Arc<dyn WorktreeFactory> = factory;
        let (registry, executor) = make_executor_with_factory(factory_obs);

        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: true,
            ..super::super::types::SubagentDefinition::new("writer", "Writer")
        };
        registry.register(def, Box::new(agent.clone())).await;

        let req = DispatchRequest {
            agent_name: "writer".into(),
            task: "edit foo".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(err.to_string().contains("Worktree isolation"), "got: {err}");
        // The dispatch hard-failed (the safety gate) — never silently continued
        // without isolation. The error message itself is the proof; we don't
        // assert on MockAgent's recorded working_dir_calls because the registry
        // stores the agent behind an Arc<dyn Agent> and the recorded-state
        // sharing across the clone boundary is not reliably observable here.
        let _ = agent; // suppress unused warning
    }

    #[tokio::test]
    async fn fork_isolate_without_factory_hard_fails() {
        // isolate_worktree=true but NO factory configured → hard-fail
        // (must not silently share the main tree with other writers).
        let (registry, executor) = make_executor().await; // default config, no factory

        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: true,
            ..super::super::types::SubagentDefinition::new("writer", "Writer")
        };
        registry.register(def, Box::new(agent.clone())).await;

        let req = DispatchRequest {
            agent_name: "writer".into(),
            task: "edit foo".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(
            err.to_string().contains("no WorktreeFactory")
                || err
                    .to_string()
                    .contains("refusing to run without isolation"),
            "got: {err}"
        );
        let _ = agent;
    }

    #[tokio::test]
    async fn fork_no_isolate_does_not_touch_worktree() {
        // A readonly subagent (isolate_worktree=false) never creates a worktree
        // even when a factory is configured.
        let factory = Arc::new(MockWorktreeFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn WorktreeFactory> = factory.clone();
        let (registry, executor) = make_executor_with_factory(factory_obs);

        let agent = MockAgent::new("reader").with_response("ok");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: false, // readonly
            ..super::super::types::SubagentDefinition::new("reader", "Reader")
        };
        registry.register(def, Box::new(agent.clone())).await;

        let req = DispatchRequest {
            agent_name: "reader".into(),
            task: "read foo".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "ok");
        assert_eq!(result.isolation_observed, ObservedIsolation::Context);
        // Factory never invoked — readonly subagents don't request isolation.
        assert!(factory.labels.lock().unwrap().is_empty());
        let _ = agent;
    }

    // ── Sprint 10: Fork data-workspace isolation ───────────────────────────

    use crate::agent::subagent::workspace::{
        DataWorkspaceFactory, DataWorkspaceHandle, WorkspaceError,
    };

    /// A mock data-workspace factory mirroring MockWorktreeFactory.
    struct MockWorkspaceFactory {
        labels: StdMutex<Vec<String>>,
        should_fail: bool,
    }

    impl DataWorkspaceFactory for MockWorkspaceFactory {
        fn create(&self, label: &str) -> std::result::Result<DataWorkspaceHandle, WorkspaceError> {
            if self.should_fail {
                return Err(WorkspaceError::new("mock workspace create failed"));
            }
            self.labels.lock().unwrap().push(label.to_string());
            Ok(DataWorkspaceHandle {
                path: std::path::PathBuf::from(format!("/tmp/mock-ws-{label}")),
                finalize: Box::new(|| {
                    Ok::<String, WorkspaceError>(
                        "run_001_clean.parquet\nrun_001_stats.json".to_string(),
                    )
                }),
            })
        }
    }

    /// Build an executor with a data-workspace factory wired into its config.
    fn make_executor_with_workspace_factory(
        factory: Arc<dyn DataWorkspaceFactory>,
    ) -> (Arc<SubagentRegistry>, SubagentExecutor) {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                data_workspace_factory: Some(factory),
                ..SubagentExecutorConfig::default()
            },
        );
        (registry, executor)
    }

    #[tokio::test]
    async fn fork_isolate_workspace_appends_file_listing() {
        // A data subagent declaring isolate_workspace, dispatched in Fork mode
        // with a configured factory: the workspace is created and the finalize
        // file listing is appended to the output (proof of creation+finalize).
        let factory = Arc::new(MockWorkspaceFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn DataWorkspaceFactory> = factory.clone();
        let (registry, executor) = make_executor_with_workspace_factory(factory_obs);

        let agent = MockAgent::new("analyst").with_response("analysis done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_workspace: true,
            ..super::super::types::SubagentDefinition::new("analyst", "Analyst")
        };
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "analyst".into(),
            task: "analyze data".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert!(result.output.contains("analysis done"));
        assert!(result.output.contains("run_001_clean.parquet"));
        assert!(result.output.contains("--- workspace outputs ---"));
        assert_eq!(result.isolation_observed, ObservedIsolation::Workspace);
        // Factory invoked once with a label derived from the agent name.
        let labels = factory.labels.lock().unwrap().clone();
        assert_eq!(labels.len(), 1);
        assert!(labels[0].starts_with("analyst-"));
    }

    #[tokio::test]
    async fn fork_isolate_workspace_create_failure_fails_dispatch() {
        // Workspace factory hard-fails → dispatch fails (safety gate).
        let factory = Arc::new(MockWorkspaceFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: true,
        });
        let factory_obs: Arc<dyn DataWorkspaceFactory> = factory;
        let (registry, executor) = make_executor_with_workspace_factory(factory_obs);

        let agent = MockAgent::new("analyst").with_response("ok");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_workspace: true,
            ..super::super::types::SubagentDefinition::new("analyst", "Analyst")
        };
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "analyst".into(),
            task: "analyze".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(err.to_string().contains("Data workspace"), "got: {err}");
    }

    #[tokio::test]
    async fn fork_worktree_takes_precedence_over_workspace() {
        // If a subagent declares BOTH isolate_worktree and isolate_workspace,
        // worktree wins (it also provides disjoint FS) — workspace factory is
        // not invoked. Guards against double-isolation.
        let ws_factory = Arc::new(MockWorkspaceFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let wt_factory = Arc::new(MockWorktreeFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                worktree_factory: Some(wt_factory.clone()),
                data_workspace_factory: Some(ws_factory.clone()),
                ..SubagentExecutorConfig::default()
            },
        );

        let agent = MockAgent::new("w").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolate_worktree: true,
            isolate_workspace: true, // both — worktree should win
            ..super::super::types::SubagentDefinition::new("w", "Writer")
        };
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "w".into(),
            task: "work".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        // Worktree was used (mock diff appended), workspace was NOT.
        assert!(result.output.contains("=== mock diff ==="));
        assert!(
            ws_factory.labels.lock().unwrap().is_empty(),
            "workspace factory must not be invoked when worktree is active"
        );
        assert!(
            !wt_factory.labels.lock().unwrap().is_empty(),
            "worktree factory should have been invoked"
        );
    }
}
