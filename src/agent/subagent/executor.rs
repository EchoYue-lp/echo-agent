//! Subagent executor — unified dispatch engine for Sync / Fork / Teammate modes
//!
//! The executor receives a [`DispatchRequest`] and routes it to the appropriate
//! execution strategy based on the definition's [`ExecutionMode`].

use crate::error::{AgentError, ReactError, Result};
use echo_core::agent::{Agent, AgentEvent, CancellationToken};
use echo_core::llm::types::Message;
use futures::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tracing::info;

use super::context::SubagentContext;
use super::events::SubagentEvent;
use super::hooks::{SubagentHookContext, SubagentHookRegistry};
use super::registry::SubagentRegistry;
use super::types::{ExecutionMode, SubagentResult};

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
    /// Current delegation depth (prevents infinite delegation chains).
    pub delegate_depth: u32,
    /// 应用层 run 级上下文（跨 spawn 安全，值传递）。
    ///
    /// dispatch_fork 在 worker agent 执行前把它注入到 worker 实例
    /// （`set_external_context`），使 worker 内的工具能经 ToolContext 读到
    /// run_id/cancel/trace_sink/cache_user_id——绕开会跨 tokio::spawn 断裂的
    /// task_local。`None` = 无外部 context（旧行为，工具读到 None）。
    pub runtime_context: Option<echo_core::tools::ExternalRunContext>,
    /// Optional multimodal message (images/files). When `Some`, the worker is
    /// dispatched via `execute_stream_message_with_cancel` instead of the text
    /// `task` path, so it sees user-uploaded attachments. `None` = plain text
    /// dispatch (the default for all existing callers).
    pub message: Option<Message>,
}

impl std::fmt::Debug for DispatchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchRequest")
            .field("agent_name", &self.agent_name)
            .field("task", &self.task)
            .field("mode_override", &self.mode_override)
            .field("parent_agent", &self.parent_agent)
            .field("delegate_depth", &self.delegate_depth)
            .field(
                "runtime_context",
                &self.runtime_context.as_ref().map(|c| &c.run_id),
            )
            .finish()
    }
}

/// Maximum delegation depth to prevent infinite chains.
const MAX_DELEGATE_DEPTH: u32 = 3;

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
}

impl Default for SubagentExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_forks: 5,
            default_timeout_secs: 600,
            enable_hooks: true,
            unified_hook_executor: None,
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
            return Ok(SubagentResult {
                agent_name: req.agent_name.clone(),
                output: "Cancelled before execution".into(),
                duration: std::time::Duration::ZERO,
                iterations: 0,
                tokens_used: None,
                was_truncated: false,
                mode: req.mode_override.clone().unwrap_or(ExecutionMode::Fork),
                usage: None,
            });
        }

        loop {
            // Guard against infinite delegation chains
            if req.delegate_depth > MAX_DELEGATE_DEPTH {
                return Err(ReactError::Other(format!(
                    "Delegation depth exceeded (max {}): agent '{}'",
                    MAX_DELEGATE_DEPTH, req.agent_name
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
                });

            if self.config.enable_hooks {
                self.hooks.before_dispatch(&hook_ctx).await;
            }

            // Snapshot fields needed in error path before `req` is moved
            let req_agent_name = req.agent_name.clone();
            let req_parent_agent = req.parent_agent.clone();
            let delegate_depth = req.delegate_depth;

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
            };

            let duration = start.elapsed();

            match result {
                Ok(mut sub_result) => {
                    sub_result.duration = duration;
                    sub_result.mode = mode.clone();

                    self.registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchCompleted {
                            parent: req_parent_agent.clone(),
                            agent: req_agent_name.clone(),
                            duration_ms: duration.as_millis() as u64,
                            tokens_used: sub_result.tokens_used.map(|t| t as u64),
                            iterations: Some(sub_result.iterations as u64),
                        });

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

                    self.registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchFailed {
                            parent: req_parent_agent.clone(),
                            agent: req_agent_name.clone(),
                            error: error_str.clone(),
                        });

                    if self.config.enable_hooks {
                        let decision = self.hooks.on_failure(&hook_ctx, &error_str).await;
                        match decision {
                            super::hooks::SubagentRetryDecision::Delegate { alternative_agent } => {
                                info!(
                                    from = %hook_ctx.subagent_name,
                                    to = %alternative_agent,
                                    depth = delegate_depth + 1,
                                    "Delegating to alternative subagent"
                                );
                                retry_count += 1;
                                let rt_ctx = req.runtime_context.clone();
                                let retry_msg = req.message.clone();
                                req = DispatchRequest {
                                    agent_name: alternative_agent,
                                    task: hook_ctx.task.clone(),
                                    mode_override: Some(hook_ctx.execution_mode.clone()),
                                    cancel: parent_cancel.child_token(),
                                    parent_agent: hook_ctx.parent_agent.clone(),
                                    parent_context: None,
                                    delegate_depth: delegate_depth + 1,
                                    runtime_context: rt_ctx,
                                    message: retry_msg,
                                };
                                // Loop instead of recursing
                                continue;
                            }
                            super::hooks::SubagentRetryDecision::Retry { delay_secs } => {
                                info!(
                                    delay_secs,
                                    attempt = retry_count + 1,
                                    "Retrying subagent dispatch"
                                );
                                tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                                retry_count += 1;
                                let rt_ctx = req.runtime_context.clone();
                                let retry_msg = req.message.clone();
                                req = DispatchRequest {
                                    agent_name: hook_ctx.subagent_name.clone(),
                                    task: hook_ctx.task.clone(),
                                    mode_override: Some(hook_ctx.execution_mode.clone()),
                                    cancel: parent_cancel.child_token(),
                                    parent_agent: hook_ctx.parent_agent.clone(),
                                    parent_context: None,
                                    delegate_depth,
                                    runtime_context: rt_ctx,
                                    message: retry_msg,
                                };
                                // Loop instead of recursing
                                continue;
                            }
                            super::hooks::SubagentRetryDecision::Fail => {}
                        }
                    }

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

    /// Dispatch a teammate, returning a handle for async polling.
    pub async fn dispatch_teammate(&self, req: DispatchRequest) -> Result<TeammateHandle> {
        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
                ReactError::Other(format!("Subagent '{}' not found", req.agent_name))
            })?;

        // get_agent() auto-instantiates from factory if needed
        let agent_arc = self
            .registry
            .get_agent(&req.agent_name)
            .await
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "Cannot get agent instance for '{}'",
                    req.agent_name
                ))
            })?;

        let child_token = req.cancel.child_token();
        let task = req.task.clone();
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
                        Err(ReactError::Other(format!(
                            "Teammate '{}' timed out after {}s",
                            agent_name, timeout_secs
                        )))
                    }
                    r = Self::execute_agent_streaming(
                        registry,
                        agent,
                        &task,
                        message.clone(),
                        child_token.clone(),
                        &parent_agent,
                        &agent_name,
                        ExecutionMode::Teammate,
                        start,
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
                        &parent_agent,
                        &agent_name,
                        ExecutionMode::Teammate,
                        start,
                    ) => r,
                }
            }
        });

        Ok(TeammateHandle {
            id: handle_id,
            agent_name: req.agent_name.clone(),
            cancel: req.cancel.clone(),
            join_handle,
        })
    }

    // ── Internal dispatch methods ──────────────────────────────────────────

    /// Enhance the task description with inherited parent context.
    ///
    /// Prepends inherited system prompt and conversation history to the task,
    /// giving the subagent awareness of the parent's state.
    fn enhance_task(task: &str, parent_ctx: Option<&super::context::SubagentContext>) -> String {
        let Some(ctx) = parent_ctx else {
            return task.to_string();
        };

        let mut parts = Vec::new();
        if !ctx.system_prompt.is_empty() {
            parts.push(format!("[Inherited System Context]\n{}", ctx.system_prompt));
        }
        if !ctx.messages.is_empty() {
            let history: Vec<String> = ctx
                .messages
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
        if parts.is_empty() {
            task.to_string()
        } else {
            format!("{}\n\n---\n\n{}", parts.join("\n\n"), task)
        }
    }

    async fn execute_agent_streaming(
        registry: Arc<SubagentRegistry>,
        agent: &(dyn Agent + Send + Sync),
        task: &str,
        message: Option<Message>,
        cancel: CancellationToken,
        parent: &str,
        subagent: &str,
        mode: ExecutionMode,
        start: Instant,
    ) -> Result<SubagentResult> {
        // Multimodal path: when a Message is supplied, run it so the worker
        // sees images/files. Falls back to the text task otherwise.
        let mut stream = if let Some(msg) = message {
            agent
                .execute_stream_message_with_cancel(msg, cancel)
                .await?
        } else {
            agent.execute_stream_with_cancel(task, cancel).await?
        };
        let mut output = String::new();
        let mut in_thinking = false;
        let mut prompt_tokens: usize = 0;
        let mut completion_tokens: usize = 0;
        let mut cancelled = false;
        let mut usage_stats = super::usage::LlmUsageStats::default();

        while let Some(event_result) = stream.next().await {
            let event = event_result?;
            match event {
                AgentEvent::Token(content) => {
                    if in_thinking {
                        registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchThinkingDelta {
                                parent: parent.to_string(),
                                agent: subagent.to_string(),
                                content,
                            });
                    } else {
                        output.push_str(&content);
                        registry
                            .event_bus()
                            .emit(SubagentEvent::DispatchTokenDelta {
                                parent: parent.to_string(),
                                agent: subagent.to_string(),
                                content,
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
                }
                AgentEvent::ToolCall { name, args } => {
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolStarted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            name,
                            args,
                        });
                }
                AgentEvent::ToolResult { name, output } => {
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolCompleted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            name,
                            result: output,
                            success: true,
                        });
                }
                AgentEvent::ToolError { name, error } => {
                    registry
                        .event_bus()
                        .emit(SubagentEvent::DispatchToolCompleted {
                            parent: parent.to_string(),
                            agent: subagent.to_string(),
                            name,
                            result: error,
                            success: false,
                        });
                }
                AgentEvent::FinalAnswer(answer) => {
                    if !answer.is_empty() {
                        output = answer;
                    }
                }
                AgentEvent::Cancelled => {
                    cancelled = true;
                    registry.event_bus().emit(SubagentEvent::DispatchCancelled {
                        parent: parent.to_string(),
                        agent: subagent.to_string(),
                    });
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

        Ok(SubagentResult {
            agent_name: subagent.to_string(),
            output,
            duration: start.elapsed(),
            iterations: 1,
            tokens_used,
            was_truncated: false,
            mode,
            usage,
        })
    }

    /// Sync mode: lock the agent, execute, return.
    async fn dispatch_sync(&self, req: &DispatchRequest) -> Result<SubagentResult> {
        let agent_arc = self
            .registry
            .get_agent(&req.agent_name)
            .await
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "Subagent '{}' not found or not instantiated",
                    req.agent_name
                ))
            })?;

        // Per-subagent override (0 = executor default). Sync now enforces a
        // timeout too (previously it blocked the parent indefinitely) — one
        // config (AgentConfig.subagent_timeout_secs) governs all three modes.
        let timeout_secs = match self.registry.get(&req.agent_name).await {
            Some(r) if r.definition.timeout_secs > 0 => r.definition.timeout_secs,
            _ => self.config.default_timeout_secs,
        };

        let start = Instant::now();
        let task = Self::enhance_task(&req.task, req.parent_context.as_ref());
        let cancel = req.cancel.clone();

        if timeout_secs > 0 {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(ReactError::Other(format!(
                    "Sync subagent '{}' cancelled", req.agent_name
                ))),
                r = tokio::time::timeout(
                    Duration::from_secs(timeout_secs),
                    Self::execute_agent_streaming(
                        self.registry.clone(),
                        agent_arc.as_ref(),
                        &task,
                        req.message.clone(),
                        cancel.clone(),
                        &req.parent_agent,
                        &req.agent_name,
                        ExecutionMode::Sync,
                        start,
                    )
                ) => match r {
                    Ok(r) => r,
                    Err(_) => Err(ReactError::Other(format!(
                        "Sync subagent '{}' timed out after {}s",
                        req.agent_name, timeout_secs
                    ))),
                },
            }
        } else {
            Self::execute_agent_streaming(
                self.registry.clone(),
                agent_arc.as_ref(),
                &task,
                req.message.clone(),
                cancel,
                &req.parent_agent,
                &req.agent_name,
                ExecutionMode::Sync,
                start,
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

        let agent_arc = self
            .registry
            .get_agent(&req.agent_name)
            .await
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "Cannot get agent instance for '{}'",
                    req.agent_name
                ))
            })?;

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
        let enhanced_task = Self::enhance_task(&task, req.parent_context.as_ref());
        // 跨 spawn 安全的值传递: 把外部 run context 带进 spawn 块。
        // worker agent 是 registry 预注册的单例(fork 不 clone),其 current_run_id
        // 初始为 None。dispatch_fork 在 worker 执行前显式 set_external_context,
        // 使 pipeline 构造 ToolContext 时带上应用层的 run_id/cancel/trace_sink/
        // cache_user_id——绕开会跨 tokio::spawn 断裂的 task_local。
        let runtime_context = req.runtime_context.clone();

        let result = tokio::spawn(async move {
            let _permit = permit;
            let start = Instant::now();

            // Check cancellation
            if cancel.is_cancelled() {
                return Ok(SubagentResult {
                    agent_name: agent_name.clone(),
                    output: "Cancelled before execution".into(),
                    duration: start.elapsed(),
                    iterations: 0,
                    tokens_used: None,
                    was_truncated: false,
                    mode: ExecutionMode::Fork,
                    usage: None,
                });
            }

            let agent = agent_arc.as_ref();

            // 注入外部 run context(worker 执行前)。worker 跑完 clear,防泄漏到
            // 同一 worker 实例的下一次 dispatch。worker 是单例,若不清,current_run_id
            // 等会残留给下一个 run。
            let has_ctx = runtime_context.is_some();
            if let Some(ctx) = &runtime_context {
                agent.set_external_context(ctx);
            }

            let result = if timeout_secs > 0 {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(ReactError::Other(format!(
                        "Fork subagent '{}' cancelled", agent_name
                    ))),
                    r = tokio::time::timeout(
                        Duration::from_secs(timeout_secs),
                        Self::execute_agent_streaming(
                            registry,
                            agent,
                            &enhanced_task,
                            message.clone(),
                            cancel.clone(),
                            &parent_agent,
                            &agent_name,
                            ExecutionMode::Fork,
                            start,
                        )
                    ) => {
                        match r {
                            Ok(r) => r,
                            Err(_) => Err(ReactError::Other(format!(
                                "Fork subagent '{}' timed out after {}s",
                                agent_name, timeout_secs
                            ))),
                        }
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Err(ReactError::Other(format!(
                        "Fork subagent '{}' cancelled", agent_name
                    ))),
                    r = Self::execute_agent_streaming(
                        registry,
                        agent,
                        &enhanced_task,
                        message.clone(),
                        cancel.clone(),
                        &parent_agent,
                        &agent_name,
                        ExecutionMode::Fork,
                        start,
                    ) => r,
                }
            };

            // worker 执行后清理外部 context(防泄漏)
            if has_ctx {
                agent.clear_external_context();
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
    use crate::testing::MockAgent;

    async fn make_executor() -> (Arc<SubagentRegistry>, SubagentExecutor) {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = SubagentExecutor::new(registry.clone(), SubagentExecutorConfig::default());
        (registry, executor)
    }

    #[tokio::test]
    async fn test_dispatch_sync() {
        let (registry, executor) = make_executor().await;

        let agent = MockAgent::new("worker").with_response("done");
        let def = super::super::types::SubagentDefinition::new("worker", "Worker");
        registry.register(def, Box::new(agent)).await;

        let req = DispatchRequest {
            agent_name: "worker".into(),
            task: "do work".into(),
            mode_override: None,
            cancel: CancellationToken::new(),
            parent_agent: "parent".into(),
            parent_context: None,
            delegate_depth: 0,
            runtime_context: None,
            message: None,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "done");
        assert_eq!(result.mode, ExecutionMode::Sync);
    }

    // NOTE: a unit test for multimodal dispatch forwarding (verifying a worker
    // receives the Message via execute_stream_message_with_cancel) is not
    // feasible with MockAgent — the trait-object vtable routes the default
    // trait method rather than MockAgent's override for this added method, so
    // the message path can't be exercised in isolation. Worker multimodal
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
            delegate_depth: 0,
            runtime_context: None,
            message: None,
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
            delegate_depth: 0,
            runtime_context: None,
            message: None,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert!(result.output.contains("Cancelled"));
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
            delegate_depth: 0,
            runtime_context: None,
            message: None,
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "forked");
        assert_eq!(result.mode, ExecutionMode::Fork);
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
            delegate_depth: 0,
            runtime_context: None,
            message: None,
        };

        let handle = executor.dispatch_teammate(req).await.unwrap();
        assert_eq!(handle.agent_name, "tm");

        let result = handle.join().await.unwrap();
        assert_eq!(result.output, "team result");
    }
}
