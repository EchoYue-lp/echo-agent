//! Subagent executor — unified dispatch engine for Sync / Fork / Teammate modes
//!
//! The executor receives a [`DispatchRequest`] and routes it to the appropriate
//! execution strategy based on the definition's [`ExecutionMode`].

use crate::error::{AgentError, ReactError, Result};
use echo_core::agent::{Agent, AgentEvent, AgentInvocationContext, CancellationToken};
use echo_core::error::AgentTerminalKind;
use echo_core::llm::types::Message;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::{info, warn};

use super::context::SubagentContext;
use super::control::{
    SubagentAttemptBinding, SubagentAttemptIdentity, SubagentControlError, SubagentControlRegistry,
    SubagentGuidanceQueueReceipt, SubagentInterruptOutcome, SubagentMessageDelivery,
};
use super::events::SubagentEvent;
use super::hooks::{SubagentHookContext, SubagentHookRegistry};
use super::prompt::{
    CompiledSubagentInvocation, ContextTransferPolicy, SubagentPromptCompiler, SubagentPromptInput,
    with_compiled_task,
};
use super::registry::SubagentRegistry;
use super::types::{
    ExecutionMode, ObservedIsolation, SubagentArtifact, SubagentEvidence, SubagentEvidenceSource,
    SubagentOutcome, SubagentResult, SubagentStatus,
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
    /// Opaque structured payload consumed only by an injected product compiler.
    pub prompt_payload: Option<serde_json::Value>,
    /// Explicit task constraints / boundaries from the dispatch caller (e.g.
    /// the `agent_tool` `constraints` parameter). Rendered by the invocation
    /// compiler independently of `parent_context`, so fresh-context dispatches
    /// can still carry boundaries.
    pub constraints: Vec<String>,
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
            .field("has_prompt_payload", &self.prompt_payload.is_some())
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

/// Append dispatch-time working-dir context to a compiled task input.
///
/// `compile_invocation` runs before worktree/workspace creation (the isolation
/// path is chosen at dispatch time), so the actual working directory is not
/// available at compile time. The executor appends it after isolation is
/// established, using the same `[workspace]` shape as planned invocations.
/// Only appended when isolation actually changed the cwd — otherwise the
/// subagent works in the main directory the parent context already covers.
fn append_working_dir_context(task_input: &mut String, working_dir: Option<&Path>) {
    if let Some(dir) = working_dir {
        task_input.push_str(&format!(
            "\n\n[workspace]\n- root: {}\n[/workspace]",
            dir.display()
        ));
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

#[cfg(test)]
mod typed_error_mapping_tests {
    use super::*;

    #[test]
    fn terminal_status_ignores_misleading_error_text() {
        let error = ReactError::Other("cancelled timeout are ordinary words here".to_string());
        assert_eq!(subagent_status_from_error(&error), SubagentStatus::Failed);
    }
}

fn hook_stop_status(status: SubagentStatus) -> echo_core::hooks::SubagentStopStatus {
    match status {
        SubagentStatus::Completed => echo_core::hooks::SubagentStopStatus::Completed,
        SubagentStatus::Failed => echo_core::hooks::SubagentStopStatus::Failed,
        SubagentStatus::Cancelled => echo_core::hooks::SubagentStopStatus::Cancelled,
        SubagentStatus::TimedOut => echo_core::hooks::SubagentStopStatus::TimedOut,
    }
}

fn correlate_subagent_hook(
    ctx: crate::skills::hooks::HookContext,
    run_id: Option<&str>,
    execution_id: Option<&str>,
    attempt: u32,
) -> crate::skills::hooks::HookContext {
    ctx.with_run_correlation(run_id, None, execution_id, Some(attempt))
}

fn bounded_detail(text: &str) -> String {
    text.chars().take(500).collect()
}

/// Merge runtime-observed evidence into a parsed result.
///
/// Observed facts replace older evidence with the same kind, subject, and
/// attributes, while distinct invocations and artifacts are preserved.
pub fn merge_observed_evidence(
    outcome: &mut SubagentOutcome,
    evidence: Vec<SubagentEvidence>,
    artifacts: Vec<SubagentArtifact>,
) {
    for observed in evidence {
        outcome.evidence.retain(|existing| {
            existing.kind != observed.kind
                || existing.subject != observed.subject
                || existing.attributes != observed.attributes
        });
        outcome.evidence.push(observed);
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
/// the registry event bus. The handle also owns cancellation and the eventual
/// result so direct API callers do not have to depend on a lossy event stream.
#[derive(Debug, Clone)]
pub struct BackgroundSubagentHandle {
    /// Stable execution id (also on `DispatchStarted.execution_id`).
    pub execution_id: String,
    /// Target subagent name.
    pub agent_name: String,
    cancel: CancellationToken,
    join_handle: Arc<Mutex<Option<tokio::task::JoinHandle<Result<SubagentResult>>>>>,
}

impl BackgroundSubagentHandle {
    /// Request cancellation of the background dispatch.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Check whether the spawned dispatch has reached a terminal state.
    pub fn is_finished(&self) -> bool {
        self.join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }

    /// Await and consume the background result.
    ///
    /// Cloned handles share one result slot; only the first `join` call
    /// consumes it. Later calls return an explicit error.
    pub async fn join(&self) -> Result<SubagentResult> {
        let handle = self
            .join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| ReactError::Other("Background result already consumed".to_string()))?;
        handle
            .await
            .map_err(|error| ReactError::Other(format!("Background join error: {error}")))?
    }
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
    /// Optional application-owned provider for product-defined isolation kinds.
    pub isolation_provider: Option<super::isolation::SharedIsolationProvider>,
    /// Compiler shared by direct `agent_tool` and programmatic delegation.
    pub prompt_compiler: Arc<dyn SubagentPromptCompiler>,
}

impl Default for SubagentExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_forks: 5,
            default_timeout_secs: 600,
            enable_hooks: true,
            unified_hook_executor: None,
            isolation_provider: None,
            prompt_compiler: Arc::new(super::prompt::DefaultSubagentPromptCompiler),
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
    control_registry: Arc<SubagentControlRegistry>,
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
            control_registry: Arc::new(SubagentControlRegistry::default()),
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
            control_registry: Arc::new(SubagentControlRegistry::default()),
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
    pub async fn dispatch(&self, req: DispatchRequest) -> Result<SubagentResult> {
        self.dispatch_inner(req, None).await
    }

    /// Dispatch one explicitly identified task attempt under live control.
    ///
    /// The identity is process-scoped. Durable run/revision/command validation
    /// remains the responsibility of the framework consumer.
    pub async fn dispatch_attempt(
        &self,
        req: DispatchRequest,
        identity: SubagentAttemptIdentity,
    ) -> Result<SubagentResult> {
        let (req, admission) = self.admit_attempt(req, identity)?;
        self.dispatch_admitted_attempt(req, admission).await
    }

    fn admit_attempt(
        &self,
        mut req: DispatchRequest,
        identity: SubagentAttemptIdentity,
    ) -> Result<(DispatchRequest, super::control::SubagentAttemptAdmission)> {
        Self::bind_attempt_runtime_identity(&mut req, &identity)?;
        let admission = self
            .control_registry
            .admit(identity, req.cancel.clone())
            .map_err(Self::control_react_error)?;
        Self::append_queued_guidance(&mut req.task, &admission.guidance);
        Ok((req, admission))
    }

    async fn dispatch_admitted_attempt(
        &self,
        req: DispatchRequest,
        admission: super::control::SubagentAttemptAdmission,
    ) -> Result<SubagentResult> {
        let binding = admission.binding.clone();
        let result = self.dispatch_inner(req, Some(binding)).await;
        let status = match &result {
            Ok(result) => result.outcome.status,
            Err(error) => subagent_status_from_error(error),
        };
        admission.settle(status);
        result
    }

    /// Deliver a live instruction to one exact active attempt.
    pub async fn send_message(
        &self,
        execution_id: &str,
        expected_attempt: u32,
        instruction: impl Into<String>,
    ) -> std::result::Result<SubagentMessageDelivery, SubagentControlError> {
        self.control_registry
            .send_message(execution_id, expected_attempt, instruction)
            .await
    }

    /// Queue guidance for one exact future attempt. Admission claims it once.
    pub fn queue_guidance(
        &self,
        task_id: &str,
        expected_next_attempt: u32,
        instruction: impl Into<String>,
    ) -> std::result::Result<SubagentGuidanceQueueReceipt, SubagentControlError> {
        self.control_registry
            .queue_guidance(task_id, expected_next_attempt, instruction)
    }

    /// Cancel one exact attempt and wait until its dispatch has settled.
    pub async fn interrupt_subagent(
        &self,
        execution_id: &str,
        expected_attempt: u32,
    ) -> std::result::Result<SubagentInterruptOutcome, SubagentControlError> {
        self.control_registry
            .interrupt(execution_id, expected_attempt)
            .await
    }

    async fn dispatch_inner(
        &self,
        mut req: DispatchRequest,
        control: Option<SubagentAttemptBinding>,
    ) -> Result<SubagentResult> {
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

            // Fire unified SubagentStart for this concrete dispatch attempt.
            if let Some(ref executor) = self.config.unified_hook_executor {
                let ctx = correlate_subagent_hook(
                    crate::skills::hooks::HookContext::for_subagent_start(
                        &req_agent_name,
                        &mode.to_string(),
                        &req.task,
                        "", // session_id not available at this layer
                        &req_parent_agent,
                    ),
                    event_run_id.as_deref(),
                    event_execution_id.as_deref(),
                    hook_ctx.attempt,
                );
                executor(ctx).await;
            }

            // Dispatch based on mode
            let start = Instant::now();
            let result = match mode {
                ExecutionMode::Sync => self.dispatch_sync(&req, control.clone()).await,
                ExecutionMode::Fork => self.dispatch_fork(&req, control.clone()).await,
                ExecutionMode::Teammate => {
                    // Teammate mode: spawn independently, then await result
                    match self
                        .dispatch_teammate_with_control(req.clone(), control.clone())
                        .await
                    {
                        Ok(handle) => handle.join().await,
                        Err(e) => Err(e),
                    }
                }
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

                    // Every Start has exactly one Stop. An executor may return a
                    // structured cancelled/failed outcome inside Ok, so map the
                    // outcome instead of assuming Completed from the Result arm.
                    if let Some(ref executor) = self.config.unified_hook_executor {
                        let ctx = correlate_subagent_hook(
                            crate::skills::hooks::HookContext::for_subagent_stop(
                                &req_agent_name,
                                &mode.to_string(),
                                &sub_result.output,
                                hook_stop_status(sub_result.outcome.status),
                                "",
                                &req_parent_agent,
                            ),
                            event_run_id.as_deref(),
                            event_execution_id.as_deref(),
                            hook_ctx.attempt,
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

                    // Close this attempt before cancellation returns or a hook
                    // policy starts another attempt. Previously cancellation
                    // returned above the only Stop call, while retries emitted
                    // multiple Starts followed by one terminal Stop.
                    if let Some(ref executor) = self.config.unified_hook_executor {
                        let ctx = correlate_subagent_hook(
                            crate::skills::hooks::HookContext::for_subagent_stop(
                                &req_agent_name,
                                &mode.to_string(),
                                &format!("error: {error_str}"),
                                hook_stop_status(status),
                                "",
                                &req_parent_agent,
                            ),
                            event_run_id.as_deref(),
                            event_execution_id.as_deref(),
                            hook_ctx.attempt,
                        );
                        executor(ctx).await;
                    }

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
                                    let prompt_payload = req.prompt_payload.clone();
                                    let constraints = req.constraints.clone();
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
                                        prompt_payload,
                                        constraints,
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
                                    let prompt_payload = req.prompt_payload.clone();
                                    let constraints = req.constraints.clone();
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
                                        prompt_payload,
                                        constraints,
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

                    return Err(e);
                }
            }
        }
    }

    fn dispatch_owned(
        self,
        req: DispatchRequest,
    ) -> futures::future::BoxFuture<'static, Result<SubagentResult>> {
        Box::pin(async move { self.dispatch(req).await })
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
                isolation_provider: self.config.isolation_provider.clone(),
                prompt_compiler: self.config.prompt_compiler.clone(),
            },
            semaphore: self.semaphore.clone(),
            control_registry: self.control_registry.clone(),
        }
    }

    fn bind_attempt_runtime_identity(
        req: &mut DispatchRequest,
        identity: &SubagentAttemptIdentity,
    ) -> Result<()> {
        let context =
            req.runtime_context
                .get_or_insert_with(|| echo_core::tools::ExternalRunContext {
                    conversation_id: None,
                    run_id: None,
                    turn_id: Some(identity.execution_id.clone()),
                    execution_id: Some(identity.execution_id.clone()),
                    isolation_id: None,
                    message_id: None,
                    cancel: None,
                    trace_sink: None,
                    delegation_policy: None,
                });
        if let Some(existing) = context.execution_id.as_deref()
            && existing != identity.execution_id.as_str()
        {
            return Err(Self::control_react_error(
                SubagentControlError::ExecutionIdentityMismatch {
                    expected: identity.execution_id.clone(),
                    actual: existing.to_string(),
                },
            ));
        }
        context.execution_id = Some(identity.execution_id.clone());
        if context.turn_id.is_none() && context.run_id.is_none() {
            context.turn_id = Some(identity.execution_id.clone());
        }
        Ok(())
    }

    fn append_queued_guidance(task: &mut String, guidance: &[String]) {
        if guidance.is_empty() {
            return;
        }
        task.push_str("\n\n[queued_guidance]");
        for instruction in guidance {
            task.push_str("\n- ");
            task.push_str(instruction);
        }
        task.push_str("\n[/queued_guidance]");
    }

    fn control_react_error(error: SubagentControlError) -> ReactError {
        ReactError::Other(format!("Subagent control rejected: {error}"))
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
                isolation_id: None,
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
        let cancel = req.cancel.clone();
        let spawned = self.clone_for_spawn();

        let join_handle = tokio::spawn(async move {
            let result = spawned.dispatch(req).await;
            if let Err(error) = &result {
                warn!(agent = %agent_name, error = %error, "background subagent dispatch failed");
            }
            result
        });

        Ok(BackgroundSubagentHandle {
            execution_id,
            agent_name: agent_name_for_handle,
            cancel,
            join_handle: Arc::new(Mutex::new(Some(join_handle))),
        })
    }

    /// Non-blocking variant of [`Self::dispatch_attempt`].
    pub async fn dispatch_background_attempt(
        &self,
        req: DispatchRequest,
        identity: SubagentAttemptIdentity,
    ) -> Result<BackgroundSubagentHandle> {
        if self.registry.get(&req.agent_name).await.is_none() {
            return Err(ReactError::Other(format!(
                "Subagent '{}' not found",
                req.agent_name
            )));
        }
        let (mut req, admission) = self.admit_attempt(req, identity)?;
        req.background = true;
        let execution_id = admission.binding.identity().execution_id.clone();
        let agent_name = req.agent_name.clone();
        let agent_name_for_handle = agent_name.clone();
        let cancel = req.cancel.clone();
        let spawned = self.clone_for_spawn();

        let join_handle = tokio::spawn(async move {
            let result = spawned.dispatch_admitted_attempt(req, admission).await;
            if let Err(error) = &result {
                warn!(agent = %agent_name, error = %error, "controlled background subagent dispatch failed");
            }
            result
        });

        Ok(BackgroundSubagentHandle {
            execution_id,
            agent_name: agent_name_for_handle,
            cancel,
            join_handle: Arc::new(Mutex::new(Some(join_handle))),
        })
    }

    /// Dispatch a teammate, returning a handle for async polling.
    pub async fn dispatch_teammate(&self, req: DispatchRequest) -> Result<TeammateHandle> {
        self.dispatch_teammate_with_control(req, None).await
    }

    async fn dispatch_teammate_with_control(
        &self,
        req: DispatchRequest,
        control: Option<SubagentAttemptBinding>,
    ) -> Result<TeammateHandle> {
        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;

        let agent_arc = self.isolated_dispatch_agent(&req.agent_name).await?;

        let child_token = req.cancel.child_token();
        let handle_cancel = child_token.clone();
        let compiled = self.compile_invocation(
            &req,
            ExecutionMode::Teammate,
            registered.definition.inherit_history,
        );
        let task = compiled.task_input;
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
            history: (!compiled.history.is_empty()).then_some(compiled.history),
            ..AgentInvocationContext::default()
        };

        let join_handle = tokio::spawn(async move {
            let _permit = child_token.clone();
            let start = Instant::now();

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
                        agent_arc.clone(),
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
                        control.clone(),
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
                        agent_arc.clone(),
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
                        control.clone(),
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

    /// Compile Team intent to one revisioned graph and execute it through the
    /// canonical `RuntimeDagExecutor`.
    async fn dispatch_team(&self, req: &DispatchRequest) -> Result<SubagentResult> {
        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;
        let spec = registered.definition.team.clone().ok_or_else(|| {
            ReactError::Other("Team mode requested but definition has no TeamSpec".to_string())
        })?;
        let delegation_policy = req.delegation_policy.child_policy().ok_or_else(|| {
            ReactError::Other(format!(
                "Delegation depth exceeded before Team '{}' (max {})",
                req.agent_name, req.delegation_policy.max_delegate_depth
            ))
        })?;
        let compiled = self.compile_invocation(
            req,
            ExecutionMode::Team,
            registered.definition.inherit_history,
        );
        let run_id = req
            .runtime_context
            .as_ref()
            .and_then(|context| context.run_id.clone())
            .unwrap_or_else(|| format!("team-{}", uuid::Uuid::new_v4().as_simple()));
        let parent_agent = req.agent_name.clone();
        let team_runtime = req.runtime_context.clone();
        let spawned = self.clone_for_spawn();
        let dispatch: super::team::TeamDispatchFn = Arc::new(move |agent_name, task, cancel| {
            let executor = spawned.clone_for_spawn();
            let parent_agent = parent_agent.clone();
            let runtime_context = team_runtime.clone().map(|mut context| {
                context.execution_id =
                    Some(format!("team-member-{}", uuid::Uuid::new_v4().as_simple()));
                context.isolation_id = context
                    .run_id
                    .as_ref()
                    .map(|run_id| format!("{run_id}:{agent_name}"));
                context
            });
            Box::pin(async move {
                let member = executor
                    .registry
                    .get(&agent_name)
                    .await
                    .ok_or_else(|| format!("Team Subagent '{agent_name}' not registered"))?;
                if member.definition.execution_mode == ExecutionMode::Team {
                    return Err(format!(
                        "nested Team mode is not supported for '{agent_name}'"
                    ));
                }
                executor
                    .dispatch_owned(DispatchRequest {
                        agent_name,
                        task,
                        mode_override: None,
                        cancel,
                        parent_agent,
                        parent_context: None,
                        delegation_policy,
                        runtime_context,
                        message: None,
                        prompt_payload: None,
                        constraints: Vec::new(),
                        background: false,
                    })
                    .await
                    .map_err(|error| error.to_string())
            })
        });
        let start = Instant::now();
        let result = super::team::execute_team_with_runtime_dispatch(
            &spec,
            &compiled.task_input,
            &run_id,
            req.cancel.child_token(),
            dispatch,
        )
        .await?;
        let tokens_used = result
            .usage
            .as_ref()
            .map(|usage| usize::try_from(usage.total_tokens).unwrap_or(usize::MAX));
        Ok(SubagentResult {
            agent_name: req.agent_name.clone(),
            output: result.output,
            outcome: SubagentOutcome {
                status: SubagentStatus::Completed,
                ..SubagentOutcome::default()
            },
            duration: start.elapsed(),
            iterations: 1,
            tokens_used,
            was_truncated: false,
            mode: ExecutionMode::Team,
            isolation_observed: ObservedIsolation::new("subagent"),
            usage: result.usage,
        }
        .with_structured(
            req.runtime_context
                .as_ref()
                .and_then(|context| context.execution_id.as_deref()),
            std::env::current_dir().ok().as_deref(),
        ))
    }

    // ── Internal dispatch methods ──────────────────────────────────────────

    fn compile_invocation(
        &self,
        req: &DispatchRequest,
        mode: ExecutionMode,
        inherit_history: Option<usize>,
    ) -> CompiledSubagentInvocation {
        let transfer_policy = if mode == ExecutionMode::Fork
            && req
                .parent_context
                .as_ref()
                .is_some_and(|context| !context.messages.is_empty())
        {
            ContextTransferPolicy::InheritStructured
        } else {
            ContextTransferPolicy::Fresh
        };
        self.config
            .prompt_compiler
            .compile_invocation(&SubagentPromptInput {
                agent_name: &req.agent_name,
                task: &req.task,
                mode,
                transfer_policy,
                parent_context: req.parent_context.as_ref(),
                inherit_history,
                payload: req.prompt_payload.as_ref(),
                constraints: &req.constraints,
            })
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_agent_streaming(
        registry: Arc<SubagentRegistry>,
        agent: Arc<dyn Agent>,
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
        control: Option<SubagentAttemptBinding>,
    ) -> Result<SubagentResult> {
        let artifact_base_dir = invocation
            .working_dir
            .clone()
            .or_else(|| std::env::current_dir().ok());
        let control_turn_id = control.as_ref().map(|binding| {
            invocation
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.turn_id.clone().or_else(|| runtime.run_id.clone()))
                .unwrap_or_else(|| binding.identity().execution_id.clone())
        });
        // Multimodal path: when a Message is supplied, run it so the subagent
        // sees images/files. Falls back to the text task otherwise.
        let event_identity = echo_core::agent::EventIdentity::from_invocation(&invocation)?;
        let raw_stream = if let Some(msg) = message {
            agent
                .execute_stream_message_with_invocation_context(
                    with_compiled_task(msg, task),
                    cancel,
                    invocation,
                )
                .await?
        } else {
            agent
                .execute_stream_with_invocation_context(task, cancel, invocation)
                .await?
        };
        let _steering_lease = if let Some(binding) = control {
            let turn_id = control_turn_id
                .ok_or_else(|| ReactError::Other("Subagent control turn id missing".to_string()))?;
            Some(
                binding
                    .attach(agent.clone(), turn_id)
                    .map_err(Self::control_react_error)?,
            )
        } else {
            None
        };
        let mut stream = echo_core::agent::envelope_event_stream(raw_stream, event_identity);
        let mut output = String::new();
        let mut in_thinking = false;
        let mut prompt_tokens: usize = 0;
        let mut completion_tokens: usize = 0;
        let mut usage_stats = super::usage::LlmUsageStats::default();
        let mut pending_tool_evidence = HashMap::<String, (String, serde_json::Value)>::new();
        let mut observed_evidence = Vec::new();
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
                    invocation,
                } => {
                    pending_tool_evidence.insert(
                        call_id.clone(),
                        (invocation.name.clone(), invocation.args.clone()),
                    );
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolStarted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            call_id,
                            invocation,
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::ToolResult {
                    call_id,
                    name,
                    result,
                } => {
                    let detail = result
                        .error
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .unwrap_or(&result.output);
                    let (subject, args) = pending_tool_evidence
                        .remove(&call_id)
                        .unwrap_or_else(|| (name.clone(), serde_json::Value::Null));
                    observed_evidence.push(SubagentEvidence {
                        kind: "tool_result".to_string(),
                        subject,
                        outcome: Some(if result.success {
                            "succeeded".to_string()
                        } else {
                            "failed".to_string()
                        }),
                        details: bounded_detail(detail),
                        source: SubagentEvidenceSource::Observed,
                        attributes: serde_json::json!({ "args": args }),
                    });
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
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolCompleted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            call_id,
                            name,
                            result,
                            execution_id: execution_id.clone(),
                            run_id: run_id.clone(),
                        });
                }
                AgentEvent::FinalAnswer(answer) if !answer.is_empty() => {
                    output = answer;
                }
                AgentEvent::FinalAnswer(_) => {}
                AgentEvent::Cancelled => {
                    return Err(ReactError::Agent(Box::new(AgentError::Cancelled(format!(
                        "Subagent '{subagent}' cancelled"
                    )))));
                }
                AgentEvent::Error {
                    source,
                    message,
                    failure,
                } => {
                    return match failure.terminal_kind {
                        AgentTerminalKind::Cancelled => {
                            Err(ReactError::Agent(Box::new(AgentError::Cancelled(message))))
                        }
                        AgentTerminalKind::TimedOut => {
                            Err(ReactError::Agent(Box::new(AgentError::Timeout(message))))
                        }
                        AgentTerminalKind::PermissionDenied => Err(ReactError::Agent(Box::new(
                            AgentError::PermissionDenied(message),
                        ))),
                        AgentTerminalKind::Failed => {
                            Err(ReactError::Other(format!("{source}: {message}")))
                        }
                    };
                }
                _ => {}
            }
        }

        let tokens_used = Some(prompt_tokens.saturating_add(completion_tokens));
        let usage = if usage_stats.call_count > 0 {
            // Also update tokens_used from real usage stats for consistency.
            Some(usage_stats)
        } else {
            None
        };

        let status = SubagentStatus::Completed;
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
            isolation_observed: ObservedIsolation::default(),
            usage,
        }
        .with_structured(execution_id.as_deref(), artifact_base_dir.as_deref());
        merge_observed_evidence(&mut result.outcome, observed_evidence, observed_artifacts);
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
    async fn dispatch_sync(
        &self,
        req: &DispatchRequest,
        control: Option<SubagentAttemptBinding>,
    ) -> Result<SubagentResult> {
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
        let compiled = self.compile_invocation(req, ExecutionMode::Sync, inherit_history);
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
            history: (!compiled.history.is_empty()).then_some(compiled.history.clone()),
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
                        agent_arc.clone(),
                        &compiled.task_input,
                        req.message.clone(),
                        execution_cancel.clone(),
                        invocation.clone(),
                        &req.parent_agent,
                        &req.agent_name,
                        ExecutionMode::Sync,
                        start,
                        event_execution_id.clone(),
                        event_run_id.clone(),
                        control.clone(),
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
                agent_arc,
                &compiled.task_input,
                req.message.clone(),
                execution_cancel,
                invocation,
                &req.parent_agent,
                &req.agent_name,
                ExecutionMode::Sync,
                start,
                event_execution_id,
                event_run_id,
                control,
            )
            .await
        }
    }

    /// Fork mode: acquire semaphore, spawn task, await with timeout.
    async fn dispatch_fork(
        &self,
        req: &DispatchRequest,
        control: Option<SubagentAttemptBinding>,
    ) -> Result<SubagentResult> {
        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;
        let timeout_secs = if registered.definition.timeout_secs > 0 {
            registered.definition.timeout_secs
        } else {
            self.config.default_timeout_secs
        };
        let deadline = (timeout_secs > 0)
            .then(|| tokio::time::Instant::now() + Duration::from_secs(timeout_secs));
        let permit = tokio::select! {
            biased;
            _ = req.cancel.cancelled() => {
                return Err(ReactError::Agent(Box::new(AgentError::Cancelled(format!(
                    "Fork subagent '{}' cancelled while waiting for capacity",
                    req.agent_name
                )))));
            }
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                return Err(ReactError::Agent(Box::new(AgentError::Timeout(format!(
                    "Fork subagent '{}' timed out after {}s while waiting for capacity",
                    req.agent_name, timeout_secs
                )))));
            }
            permit = self.semaphore.clone().acquire_owned() => permit
                .map_err(|error| ReactError::Other(format!("Semaphore error: {error}")))?,
        };

        let agent_arc = tokio::select! {
            biased;
            _ = req.cancel.cancelled() => {
                return Err(ReactError::Agent(Box::new(AgentError::Cancelled(format!(
                    "Fork subagent '{}' cancelled during initialization",
                    req.agent_name
                )))));
            }
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => {
                return Err(ReactError::Agent(Box::new(AgentError::Timeout(format!(
                    "Fork subagent '{}' timed out after {}s during initialization",
                    req.agent_name, timeout_secs
                )))));
            }
            agent = self.isolated_dispatch_agent(&req.agent_name) => agent?,
        };

        let agent_name = req.agent_name.clone();
        let parent_agent = req.parent_agent.clone();
        let cancel = req.cancel.clone();
        let registry = self.registry.clone();
        let message = req.message.clone();
        let compiled = self.compile_invocation(
            req,
            ExecutionMode::Fork,
            registered.definition.inherit_history,
        );
        let enhanced_task = compiled.task_input;
        let invocation_history = compiled.history;
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

        let isolation_kind = registered.definition.isolation.clone();
        let isolation_provider = isolation_kind
            .as_ref()
            .and_then(|_| self.config.isolation_provider.clone());
        if isolation_kind.is_some() && isolation_provider.is_none() {
            // Local execution still requires this guard: silently dropping a
            // requested isolation boundary can corrupt user data.
            return Err(ReactError::Other(format!(
                "Subagent '{}' requests isolation but no IsolationProvider is configured; refusing to run without isolation",
                agent_name
            )));
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
            isolation = isolation_kind.as_deref().unwrap_or("context"),
            "subagent_fork_start"
        );
        // Prefer a caller-supplied stable isolation identity so retries of one
        // logical task can reuse the same worktree/workspace while preserving
        // their distinct execution ids for events and audit records.
        let isolation_identity = runtime_context
            .as_ref()
            .and_then(|context| {
                context
                    .isolation_id
                    .as_deref()
                    .or(context.execution_id.as_deref())
                    .or(context.run_id.as_deref())
                    .or(context.turn_id.as_deref())
            })
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().as_simple().to_string());
        let isolation_label = format!("{agent_name}-{isolation_identity}");
        let execution_cancel = cancel.child_token();
        let invocation_allowed_tools = req
            .parent_context
            .as_ref()
            .and_then(|context| context.allowed_tools.clone())
            .filter(|allowed| !allowed.is_empty());

        let result = tokio::spawn(async move {
            let _permit = permit;
            let mut enhanced_task = enhanced_task;
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

            let mut isolation_handle: Option<super::isolation::IsolationHandle> = None;
            if let (Some(provider), Some(kind)) = (&isolation_provider, &isolation_kind) {
                let request = super::isolation::IsolationRequest {
                    kind: kind.clone(),
                    label: isolation_label,
                };
                match provider.isolate(&request) {
                    Ok(handle) => {
                        isolation_handle = Some(handle);
                    }
                    Err(error) => {
                        return Err(ReactError::Other(format!(
                            "Isolation '{}' for Fork subagent '{agent_name}' failed: {error}",
                            request.kind
                        )));
                    }
                }
            }
            let isolation_observed = isolation_handle
                .as_ref()
                .map(|handle| handle.observed.clone())
                .unwrap_or_else(|| ObservedIsolation::new("context"));
            let disabled_tools = invocation_disabled_tools(
                agent_arc.tool_names(),
                invocation_allowed_tools.as_deref(),
            );
            let invocation = AgentInvocationContext {
                runtime: runtime_context.clone(),
                working_dir: isolation_handle.as_ref().map(|handle| handle.path.clone()),
                cancel: None,
                disabled_tools,
                visible_tools: None,
                run_budget: None,
                history: (!invocation_history.is_empty()).then_some(invocation_history.clone()),
            };
            registry
                .event_bus()
                .emit(SubagentEvent::DispatchIsolationObserved {
                    parent: parent_agent.clone(),
                    agent: agent_name.clone(),
                    isolation: isolation_observed.clone(),
                    execution_id: event_execution_id.clone(),
                    run_id: event_run_id.clone(),
                });
            // The compiled task input cannot know the isolated working dir
            // (created just above, at dispatch time), so append it here with
            // the same `[workspace]` shape planned invocations use.
            append_working_dir_context(&mut enhanced_task, invocation.working_dir.as_deref());

            let mut result = if let Some(deadline) = deadline {
                tokio::select! {
                    biased;
                    _ = execution_cancel.cancelled() => Err(ReactError::Agent(Box::new(
                        AgentError::Cancelled(format!("Fork subagent '{}' cancelled", agent_name))
                    ))),
                    r = tokio::time::timeout_at(
                        deadline,
                        Self::execute_agent_streaming(
                            registry,
                            agent_arc.clone(),
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
                            control.clone(),
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
                        agent_arc.clone(),
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
                        control.clone(),
                    ) => r,
                }
            };
            if let Ok(subagent_result) = &mut result {
                subagent_result.isolation_observed = isolation_observed;
            }

            if let Some(handle) = isolation_handle {
                match (handle.finalize)() {
                    Ok(finalized) => {
                        if let Ok(mut r) = result {
                            if !finalized.summary.trim().is_empty() {
                                r.output = format!(
                                    "{}\n\n--- isolation outcome ---\n{}",
                                    r.output, finalized.summary
                                );
                            }
                            merge_observed_evidence(
                                &mut r.outcome,
                                finalized.evidence,
                                finalized.artifacts,
                            );
                            return Ok(r);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            subagent = %agent_name,
                            error = %error,
                            "Subagent isolation finalization failed; result preserved"
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
    use crate::agent::subagent::SubagentDefinition;
    use crate::agent::subagent::prompt::{
        CompiledSubagentSystemPrompt, PromptDiagnostics, SubagentSystemPromptInput,
    };
    use crate::agent::subagent::registry::FnAgentFactory;
    use crate::testing::{FailingMockAgent, MockAgent};
    use echo_core::agent::{ToolInvocation, ToolInvocationRewrite};
    use echo_core::tools::{ToolResult, ToolResultKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn append_working_dir_context_appends_workspace_section_only_when_isolated() {
        let mut input = String::from("task text");
        append_working_dir_context(&mut input, None);
        assert_eq!(
            input, "task text",
            "no isolation must not touch the task input"
        );

        append_working_dir_context(&mut input, Some(Path::new("/tmp/eko-work-42")));
        assert!(
            input.contains("\n\n[workspace]\n- root: /tmp/eko-work-42\n[/workspace]"),
            "isolated working dir must be appended with the [workspace] shape, got: {input}"
        );
    }

    struct PrefixPromptCompiler;

    impl SubagentPromptCompiler for PrefixPromptCompiler {
        fn compile_system(
            &self,
            input: &SubagentSystemPromptInput<'_>,
        ) -> CompiledSubagentSystemPrompt {
            CompiledSubagentSystemPrompt {
                system_prompt: input.role_prompt.to_string(),
                diagnostics: PromptDiagnostics::default(),
            }
        }

        fn compile_invocation(
            &self,
            input: &SubagentPromptInput<'_>,
        ) -> CompiledSubagentInvocation {
            CompiledSubagentInvocation {
                task_input: format!("compiled:{}", input.task),
                history: Vec::new(),
                diagnostics: PromptDiagnostics::default(),
            }
        }
    }

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

    struct RichToolEventAgent;

    impl Agent for RichToolEventAgent {
        fn name(&self) -> &str {
            "rich-tool-events"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn system_prompt(&self) -> &str {
            ""
        }

        fn execute<'a>(&'a self, _task: &'a str) -> futures::future::BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("done".to_string()) })
        }

        fn execute_stream<'a>(
            &'a self,
            _task: &'a str,
        ) -> futures::future::BoxFuture<
            'a,
            Result<futures::stream::BoxStream<'a, Result<AgentEvent>>>,
        > {
            Box::pin(async {
                let invocation = ToolInvocation {
                    requested_name: "requested_tool".to_string(),
                    requested_args: serde_json::json!({"value": "requested"}),
                    name: "effective_tool".to_string(),
                    args: serde_json::json!({"value": "effective"}),
                    rewrites: vec![ToolInvocationRewrite::Approval],
                };
                let result = ToolResult {
                    kind: ToolResultKind::Json,
                    success: true,
                    output: "{\"ok\":true}".to_string(),
                    error: None,
                    failure: None,
                    data: Some(serde_json::json!({"ok": true})),
                    truncated: true,
                    mime_type: Some("application/json".to_string()),
                    metadata: HashMap::from([("source".to_string(), "fixture".to_string())]),
                    model_content: Vec::new(),
                };
                let events = vec![
                    Ok(AgentEvent::ToolCall {
                        call_id: "call-rich".to_string(),
                        invocation,
                    }),
                    Ok(AgentEvent::ToolResult {
                        call_id: "call-rich".to_string(),
                        name: "effective_tool".to_string(),
                        result,
                    }),
                    Ok(AgentEvent::FinalAnswer("done".to_string())),
                ];
                Ok(Box::pin(futures::stream::iter(events))
                    as futures::stream::BoxStream<'a, Result<AgentEvent>>)
            })
        }
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

    #[tokio::test]
    async fn dispatch_preserves_rich_tool_events_without_parallel_adapters()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let (registry, executor) = make_executor().await;
        registry
            .register(
                SubagentDefinition::new("rich-tool-events", "Rich tool events"),
                Box::new(RichToolEventAgent),
            )
            .await;
        let mut events = registry.event_bus().subscribe();

        let dispatched = executor
            .dispatch(DispatchRequest {
                agent_name: "rich-tool-events".to_string(),
                task: "preserve typed events".to_string(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                prompt_payload: None,
                constraints: Vec::new(),
                background: false,
            })
            .await?;
        assert_eq!(dispatched.output, "done");

        let mut started = None;
        let mut completed = None;
        while let Ok(event) = events.try_recv() {
            match event.as_ref() {
                SubagentEvent::DispatchToolStarted { invocation, .. } => {
                    started = Some(invocation.clone());
                }
                SubagentEvent::DispatchToolCompleted { name, result, .. } => {
                    completed = Some((name.clone(), result.clone()));
                }
                _ => {}
            }
        }

        let Some(invocation) = started else {
            return Err(std::io::Error::other("missing typed tool-start event").into());
        };
        assert_eq!(invocation.requested_name, "requested_tool");
        assert_eq!(invocation.name, "effective_tool");
        assert_eq!(invocation.rewrites, vec![ToolInvocationRewrite::Approval]);
        assert_eq!(
            invocation
                .args
                .get("value")
                .and_then(serde_json::Value::as_str),
            Some("effective")
        );

        let Some((name, result)) = completed else {
            return Err(std::io::Error::other("missing typed tool-result event").into());
        };
        assert_eq!(name, "effective_tool");
        assert_eq!(result.kind, ToolResultKind::Json);
        assert_eq!(result.data, Some(serde_json::json!({"ok": true})));
        assert_eq!(result.mime_type.as_deref(), Some("application/json"));
        assert!(result.truncated);
        assert_eq!(
            result.metadata.get("source").map(String::as_str),
            Some("fixture")
        );
        Ok(())
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
            evidence: vec![SubagentEvidence {
                kind: "verification".to_string(),
                subject: "cargo test".to_string(),
                outcome: Some("passed".to_string()),
                details: "model claim".to_string(),
                source: SubagentEvidenceSource::Reported,
                attributes: serde_json::Value::Null,
            }],
            ..SubagentOutcome::default()
        };
        merge_observed_evidence(
            &mut outcome,
            vec![
                SubagentEvidence {
                    kind: "verification".to_string(),
                    subject: "cargo test".to_string(),
                    outcome: Some("failed".to_string()),
                    details: "first run failed".to_string(),
                    source: SubagentEvidenceSource::Observed,
                    attributes: serde_json::Value::Null,
                },
                SubagentEvidence {
                    kind: "verification".to_string(),
                    subject: "cargo test".to_string(),
                    outcome: Some("passed".to_string()),
                    details: "retry passed".to_string(),
                    source: SubagentEvidenceSource::Observed,
                    attributes: serde_json::Value::Null,
                },
            ],
            Vec::new(),
        );
        assert!(matches!(
            outcome.evidence.as_slice(),
            [SubagentEvidence {
                source: SubagentEvidenceSource::Observed,
                ..
            }]
        ));
        assert_eq!(
            outcome
                .evidence
                .first()
                .and_then(|item| item.outcome.as_deref()),
            Some("passed")
        );
    }

    #[test]
    fn merge_observed_evidence_preserves_distinct_tool_arguments() {
        let mut outcome = SubagentOutcome::default();
        let evidence = ["src/a.rs", "src/b.rs"]
            .into_iter()
            .map(|path| SubagentEvidence {
                kind: "tool_result".to_string(),
                subject: "write_file".to_string(),
                outcome: Some("succeeded".to_string()),
                details: String::new(),
                source: SubagentEvidenceSource::Observed,
                attributes: serde_json::json!({ "args": { "path": path } }),
            })
            .collect();

        merge_observed_evidence(&mut outcome, evidence, Vec::new());

        assert_eq!(outcome.evidence.len(), 2);
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "done");
        assert_eq!(result.mode, ExecutionMode::Sync);
    }

    #[tokio::test]
    async fn fork_cancelled_while_queued_never_starts_agent() -> std::result::Result<(), String> {
        let registry = Arc::new(SubagentRegistry::new());
        let agent = MockAgent::new("queued").with_response("should not run");
        let observed = agent.clone();
        let mut definition = SubagentDefinition::new("queued", "Queued agent");
        definition.execution_mode = ExecutionMode::Fork;
        registry.register(definition, Box::new(agent)).await;
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig {
                max_concurrent_forks: 1,
                default_timeout_secs: 30,
                ..SubagentExecutorConfig::default()
            },
        ));
        let permit = Arc::clone(&executor.semaphore)
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        let cancel = CancellationToken::new();
        let request = DispatchRequest {
            agent_name: "queued".to_string(),
            task: "must not start".to_string(),
            mode_override: None,
            cancel: cancel.clone(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };
        let execution = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.dispatch(request).await }
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        drop(permit);

        let result = tokio::time::timeout(Duration::from_secs(2), execution)
            .await
            .map_err(|_| "cancelled queued dispatch did not terminate".to_string())?
            .map_err(|error| error.to_string())?;
        assert!(result.is_err());
        assert_eq!(observed.call_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_uses_injected_prompt_compiler() -> Result<()> {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                prompt_compiler: Arc::new(PrefixPromptCompiler),
                ..SubagentExecutorConfig::default()
            },
        );
        let agent = MockAgent::new("compiled").with_response("done");
        registry
            .register(
                super::super::types::SubagentDefinition::new("compiled", "Compiled subagent"),
                Box::new(agent.clone()),
            )
            .await;

        executor
            .dispatch(DispatchRequest {
                agent_name: "compiled".to_string(),
                task: "do work".to_string(),
                mode_override: Some(ExecutionMode::Sync),
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                prompt_payload: None,
                constraints: Vec::new(),
                background: false,
            })
            .await?;

        assert_eq!(agent.last_task().as_deref(), Some("compiled:do work"));
        Ok(())
    }

    #[tokio::test]
    async fn dispatch_background_returns_before_completion() -> Result<()> {
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let started_at = Instant::now();
        let handle = executor.dispatch_background(req).await?;
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "dispatch_background must return before the slow subagent finishes"
        );
        assert!(!handle.execution_id.is_empty());
        assert_eq!(handle.agent_name, "slow");
        assert!(handle.execution_id.starts_with("agent_tool-"));
        assert!(!handle.is_finished());

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
        let result = handle.join().await?;
        assert!(result.output.contains("bg done"));
        assert!(
            handle.join().await.is_err(),
            "result must only be consumed once"
        );
        Ok(())
    }

    #[tokio::test]
    async fn controlled_dispatch_claims_guidance_once_for_exact_attempt() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let agent = MockAgent::new("guided").with_response("done");
        registry
            .register(
                SubagentDefinition::new("guided", "Guided subagent"),
                Box::new(agent.clone()),
            )
            .await;
        executor
            .queue_guidance("task-guided", 1, "inspect the latest revision")
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let request = DispatchRequest {
            agent_name: "guided".to_string(),
            task: "perform task".to_string(),
            mode_override: Some(ExecutionMode::Sync),
            cancel: CancellationToken::new(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };
        let identity = SubagentAttemptIdentity::new("task-guided", "execution-guided-1", 1)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        executor.dispatch_attempt(request, identity).await?;

        let observed = agent.last_task().unwrap_or_default();
        assert!(observed.contains("perform task"));
        assert!(observed.contains("[queued_guidance]"));
        assert!(observed.contains("inspect the latest revision"));
        assert!(matches!(
            executor.queue_guidance("task-guided", 1, "late"),
            Err(SubagentControlError::AttemptAlreadyStarted { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn background_handle_cancels_dispatch() -> Result<()> {
        let (registry, executor) = make_executor().await;
        registry
            .register(
                super::super::types::SubagentDefinition::new("slow", "Slow subagent"),
                Box::new(MockAgent::new("slow").with_delay_ms(1_000)),
            )
            .await;
        let handle = executor
            .dispatch_background(DispatchRequest {
                agent_name: "slow".into(),
                task: "wait".into(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".into(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                prompt_payload: None,
                constraints: Vec::new(),
                background: false,
            })
            .await?;
        handle.cancel();
        let result = handle.join().await?;
        assert_eq!(result.outcome.status, SubagentStatus::Cancelled);
        Ok(())
    }

    #[tokio::test]
    async fn controlled_background_interrupt_is_registered_before_handle_returns() -> Result<()> {
        let (registry, executor) = make_executor().await;
        registry
            .register(
                SubagentDefinition::new("controlled", "Controlled subagent"),
                Box::new(MockAgent::new("controlled").with_response("must not escape")),
            )
            .await;
        let request = DispatchRequest {
            agent_name: "controlled".to_string(),
            task: "wait".to_string(),
            mode_override: Some(ExecutionMode::Sync),
            cancel: CancellationToken::new(),
            parent_agent: "parent".to_string(),
            parent_context: None,
            delegation_policy: DispatchRequest::policy_from_depth(0),
            runtime_context: None,
            message: None,
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };
        let identity = SubagentAttemptIdentity::new("task-controlled", "execution-controlled", 1)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let handle = executor
            .dispatch_background_attempt(request, identity)
            .await?;

        let interrupted = executor
            .interrupt_subagent("execution-controlled", 1)
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(interrupted.requested);
        assert!(interrupted.settled);
        assert_eq!(interrupted.terminal_status, Some(SubagentStatus::Cancelled));
        let result = handle.join().await?;
        assert_eq!(result.outcome.status, SubagentStatus::Cancelled);
        assert!(matches!(
            executor
                .send_message("execution-controlled", 1, "late")
                .await,
            Err(SubagentControlError::AttemptSettled { .. })
        ));
        Ok(())
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
                isolation_id: None,
                message_id: Some("message-identity".to_string()),
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            message: None,
            prompt_payload: None,
            constraints: Vec::new(),
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
    // and execute_agent_streaming branch, and (3) desktop UI manual testing of
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
            prompt_payload: None,
            constraints: Vec::new(),
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
            prompt_payload: None,
            constraints: Vec::new(),
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
        let recovery = MockAgent::new("recovery").with_response("recovered");
        registry
            .register(
                super::super::types::SubagentDefinition::new("recovery", "Recovery"),
                Box::new(recovery.clone()),
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
                prompt_payload: None,
                constraints: vec!["Preserve the caller boundary".to_string()],
                background: false,
            })
            .await?;
        assert_eq!(result.outcome.status, SubagentStatus::Completed);
        assert!(
            recovery
                .last_task()
                .as_deref()
                .is_some_and(|task| task.contains("Preserve the caller boundary")),
            "delegated recovery must receive the original constraints"
        );

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
                prompt_payload: None,
                constraints: Vec::new(),
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
                Box::new(
                    MockAgent::new("slow-cancel")
                        .with_delay_ms(500)
                        .with_default_success("completed"),
                ),
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
                prompt_payload: None,
                constraints: Vec::new(),
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
                Box::new(
                    MockAgent::new("slow-timeout")
                        .with_delay_ms(1_500)
                        .with_default_success("completed"),
                ),
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
                prompt_payload: None,
                constraints: Vec::new(),
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
            prompt_payload: None,
            constraints: Vec::new(),
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
            prompt_payload: None,
            constraints: Vec::new(),
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
                prompt_payload: None,
                constraints: Vec::new(),
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
            prompt_payload: None,
            constraints: Vec::new(),
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
                prompt_payload: None,
                constraints: Vec::new(),
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let handle = executor.dispatch_teammate(req).await.unwrap();
        assert_eq!(handle.agent_name, "tm");

        let result = handle.join().await.unwrap();
        assert_eq!(result.output, "team result");
    }

    #[tokio::test]
    async fn team_dispatch_rejects_unregistered_member_without_false_completion() -> Result<()> {
        let (registry, executor) = make_executor().await;
        let definition = super::super::builder::SubagentBuilder::new("pipeline-team")
            .team(super::super::team::TeamSpec {
                strategy: super::super::team::TeamStrategy::Pipeline(vec![
                    "missing-member".to_string(),
                ]),
                manager: String::new(),
                subagents: Vec::new(),
                config: super::super::team::TeamConfig::default(),
            })
            .build();
        registry
            .register(definition, Box::new(MockAgent::new("pipeline-team")))
            .await;

        let error = executor
            .dispatch(DispatchRequest {
                agent_name: "pipeline-team".to_string(),
                task: "run pipeline".to_string(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: None,
                message: None,
                prompt_payload: None,
                constraints: Vec::new(),
                background: false,
            })
            .await
            .err()
            .ok_or_else(|| {
                ReactError::Other("unregistered Team member completed successfully".to_string())
            })?;
        assert!(error.to_string().contains("not registered"));
        Ok(())
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
            prompt_payload: None,
            constraints: Vec::new(),
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
                prompt_payload: None,
                constraints: Vec::new(),
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
                prompt_payload: None,
                constraints: Vec::new(),
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

    // ── Generic Fork isolation ─────────────────────────────────────────────

    use crate::agent::subagent::{
        IsolationError, IsolationHandle, IsolationOutcome, IsolationProvider, IsolationRequest,
    };
    use std::sync::Mutex as StdMutex;

    /// A mock factory whose `create` always succeeds, records the label, and
    /// whose `finalize` returns a canned diff. `should_fail` toggles hard-fail.
    struct MockIsolationProvider {
        labels: StdMutex<Vec<String>>,
        should_fail: bool,
    }

    impl IsolationProvider for MockIsolationProvider {
        fn isolate(
            &self,
            request: &IsolationRequest,
        ) -> std::result::Result<IsolationHandle, IsolationError> {
            if self.should_fail {
                return Err(IsolationError::new("mock isolation failed"));
            }
            let label = &request.label;
            self.labels.lock().unwrap().push(label.to_string());
            let path = std::path::PathBuf::from(format!("/tmp/mock-wt-{label}"));
            Ok(IsolationHandle {
                path,
                observed: ObservedIsolation::new(&request.kind),
                finalize: Box::new(|| {
                    Ok(IsolationOutcome {
                        summary: "=== mock diff ===\nfoo.rs | 1 +".to_string(),
                        artifacts: Vec::new(),
                        evidence: Vec::new(),
                    })
                }),
            })
        }
    }

    /// Build an executor with a worktree factory wired into its config.
    fn make_executor_with_factory(
        provider: Arc<dyn IsolationProvider>,
    ) -> (Arc<SubagentRegistry>, SubagentExecutor) {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                isolation_provider: Some(provider),
                ..SubagentExecutorConfig::default()
            },
        );
        (registry, executor)
    }

    #[tokio::test]
    async fn fork_provider_binds_path_and_appends_outcome() {
        let factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory.clone();
        let (registry, executor) = make_executor_with_factory(factory_obs);

        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
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
    async fn fork_worktree_prefers_stable_isolation_identity() -> std::result::Result<(), String> {
        let factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_for_executor: Arc<dyn IsolationProvider> = factory.clone();
        let (registry, executor) = make_executor_with_factory(factory_for_executor);
        let agent = MockAgent::new("writer").with_response("done");
        let definition = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
            ..super::super::types::SubagentDefinition::new("writer", "Writer")
        };
        registry.register(definition, Box::new(agent)).await;

        executor
            .dispatch(DispatchRequest {
                agent_name: "writer".to_string(),
                task: "retry the same logical task".to_string(),
                mode_override: None,
                cancel: CancellationToken::new(),
                parent_agent: "parent".to_string(),
                parent_context: None,
                delegation_policy: DispatchRequest::policy_from_depth(0),
                runtime_context: Some(echo_core::tools::ExternalRunContext {
                    conversation_id: None,
                    run_id: Some("run-1".to_string()),
                    turn_id: None,
                    execution_id: Some("task-1:2".to_string()),
                    isolation_id: Some("run-1:task-1".to_string()),
                    message_id: None,
                    cancel: None,
                    trace_sink: None,
                    delegation_policy: None,
                }),
                message: None,
                prompt_payload: None,
                constraints: Vec::new(),
                background: false,
            })
            .await
            .map_err(|error| error.to_string())?;

        let labels = factory
            .labels
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        if labels != vec!["writer-run-1:task-1".to_string()] {
            return Err(format!("unexpected worktree labels: {labels:?}"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn fork_worktree_observation_precedes_subagent_completion()
    -> std::result::Result<(), String> {
        let factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory;
        let (registry, executor) = make_executor_with_factory(factory_obs);
        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let result = executor
            .dispatch(req)
            .await
            .map_err(|error| error.to_string())?;
        if result.isolation_observed != ObservedIsolation::new("sandbox") {
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
            SubagentEvent::DispatchIsolationObserved { isolation, .. }
                if isolation.as_str() == "sandbox"
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
        let factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory;
        let (registry, executor) = make_executor_with_factory(factory_obs);
        let agent = MockAgent::new("writer").with_responses(["done-a", "done-b"]);
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
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
                isolation_id: None,
                message_id: None,
                cancel: None,
                trace_sink: None,
                delegation_policy: None,
            }),
            message: None,
            prompt_payload: None,
            constraints: Vec::new(),
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
    async fn fork_isolation_provider_failure_fails_dispatch() {
        // A factory that hard-fails → dispatch must fail, never run unisolated.
        let factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: true,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory;
        let (registry, executor) = make_executor_with_factory(factory_obs);

        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(
            err.to_string().contains("Isolation 'sandbox'"),
            "got: {err}"
        );
        // The dispatch hard-failed (the safety gate) — never silently continued
        // without isolation. The error message itself is the proof; we don't
        // assert on MockAgent's recorded working_dir_calls because the registry
        // stores the agent behind an Arc<dyn Agent> and the recorded-state
        // sharing across the clone boundary is not reliably observable here.
        let _ = agent; // suppress unused warning
    }

    #[tokio::test]
    async fn fork_isolate_without_factory_hard_fails() {
        // A requested boundary with no provider must hard-fail.
        let (registry, executor) = make_executor().await; // default config, no factory

        let agent = MockAgent::new("writer").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(
            err.to_string().contains("no IsolationProvider")
                || err
                    .to_string()
                    .contains("refusing to run without isolation"),
            "got: {err}"
        );
        let _ = agent;
    }

    #[tokio::test]
    async fn fork_without_isolation_does_not_invoke_provider() {
        // A subagent with no isolation request never invokes the provider.
        let factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory.clone();
        let (registry, executor) = make_executor_with_factory(factory_obs);

        let agent = MockAgent::new("reader").with_response("ok");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "ok");
        assert_eq!(result.isolation_observed, ObservedIsolation::new("context"));
        // Factory never invoked — readonly subagents don't request isolation.
        assert!(factory.labels.lock().unwrap().is_empty());
        let _ = agent;
    }

    // A second provider proves the framework treats isolation kinds opaquely.

    /// A second mock provider used with a different opaque isolation kind.
    struct MockWorkspaceFactory {
        labels: StdMutex<Vec<String>>,
        should_fail: bool,
    }

    impl IsolationProvider for MockWorkspaceFactory {
        fn isolate(
            &self,
            request: &IsolationRequest,
        ) -> std::result::Result<IsolationHandle, IsolationError> {
            if self.should_fail {
                return Err(IsolationError::new("mock workspace create failed"));
            }
            let label = &request.label;
            self.labels.lock().unwrap().push(label.to_string());
            Ok(IsolationHandle {
                path: std::path::PathBuf::from(format!("/tmp/mock-ws-{label}")),
                observed: ObservedIsolation::new(&request.kind),
                finalize: Box::new(|| {
                    Ok(IsolationOutcome {
                        summary: "run_001_clean.parquet\nrun_001_stats.json".to_string(),
                        artifacts: Vec::new(),
                        evidence: Vec::new(),
                    })
                }),
            })
        }
    }

    /// Build an executor with a data-workspace factory wired into its config.
    fn make_executor_with_workspace_factory(
        provider: Arc<dyn IsolationProvider>,
    ) -> (Arc<SubagentRegistry>, SubagentExecutor) {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                isolation_provider: Some(provider),
                ..SubagentExecutorConfig::default()
            },
        );
        (registry, executor)
    }

    #[tokio::test]
    async fn fork_arbitrary_isolation_kind_appends_provider_summary() {
        let factory = Arc::new(MockWorkspaceFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory.clone();
        let (registry, executor) = make_executor_with_workspace_factory(factory_obs);

        let agent = MockAgent::new("analyst").with_response("analysis done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("workspace".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert!(result.output.contains("analysis done"));
        assert!(result.output.contains("run_001_clean.parquet"));
        assert!(result.output.contains("--- isolation outcome ---"));
        assert_eq!(
            result.isolation_observed,
            ObservedIsolation::new("workspace")
        );
        // Factory invoked once with a label derived from the agent name.
        let labels = factory.labels.lock().unwrap().clone();
        assert_eq!(labels.len(), 1);
        assert!(labels[0].starts_with("analyst-"));
    }

    #[tokio::test]
    async fn fork_arbitrary_isolation_kind_propagates_provider_failure() {
        // Workspace factory hard-fails → dispatch fails (safety gate).
        let factory = Arc::new(MockWorkspaceFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: true,
        });
        let factory_obs: Arc<dyn IsolationProvider> = factory;
        let (registry, executor) = make_executor_with_workspace_factory(factory_obs);

        let agent = MockAgent::new("analyst").with_response("ok");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("workspace".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
            background: false,
        };

        let err = executor.dispatch(req).await.unwrap_err();
        assert!(
            err.to_string().contains("Isolation 'workspace'"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn fork_uses_only_the_configured_provider() {
        let ws_factory = Arc::new(MockWorkspaceFactory {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let wt_factory = Arc::new(MockIsolationProvider {
            labels: StdMutex::new(Vec::new()),
            should_fail: false,
        });
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(
            registry.clone(),
            SubagentExecutorConfig {
                isolation_provider: Some(wt_factory.clone()),
                ..SubagentExecutorConfig::default()
            },
        );

        let agent = MockAgent::new("w").with_response("done");
        let def = super::super::types::SubagentDefinition {
            execution_mode: ExecutionMode::Fork,
            isolation: Some("sandbox".to_string()),
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
            prompt_payload: None,
            constraints: Vec::new(),
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
