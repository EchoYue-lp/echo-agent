//! Subagent executor — unified dispatch engine for Sync / Fork / Teammate modes
//!
//! The executor receives a [`DispatchRequest`] and routes it to the appropriate
//! execution strategy based on the definition's [`ExecutionMode`].

use crate::error::{ReactError, Result};
use echo_core::agent::CancellationToken;
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct SubagentExecutorConfig {
    /// Maximum concurrent Fork dispatches.
    pub max_concurrent_forks: usize,
    /// Default timeout for Fork/Teammate dispatches (seconds). 0 = no timeout.
    pub default_timeout_secs: u64,
    /// Enable hooks.
    pub enable_hooks: bool,
}

impl Default for SubagentExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_forks: 5,
            default_timeout_secs: 300,
            enable_hooks: true,
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
    pub async fn dispatch(&self, req: DispatchRequest) -> Result<SubagentResult> {
        // Guard against infinite delegation chains
        if req.delegate_depth > MAX_DELEGATE_DEPTH {
            return Err(ReactError::Other(format!(
                "Delegation depth exceeded (max {}): agent '{}'",
                MAX_DELEGATE_DEPTH, req.agent_name
            )));
        }

        // Look up definition
        let registered =
            self.registry.get(&req.agent_name).await.ok_or_else(|| {
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
            attempt: 1,
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

        // Dispatch based on mode
        let start = Instant::now();
        let result = match mode {
            ExecutionMode::Sync => self.dispatch_sync(&req).await,
            ExecutionMode::Fork => self.dispatch_fork(&req).await,
            ExecutionMode::Teammate => {
                // Teammate mode: spawn independently, then await result
                match self.dispatch_teammate(req).await {
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
                        parent: req_parent_agent,
                        agent: req_agent_name,
                        duration_ms: duration.as_millis() as u64,
                    });

                if self.config.enable_hooks {
                    self.hooks.after_dispatch(&hook_ctx, &sub_result).await;
                }

                Ok(sub_result)
            }
            Err(e) => {
                let error_str = e.to_string();

                self.registry
                    .event_bus()
                    .emit(SubagentEvent::DispatchFailed {
                        parent: req_parent_agent,
                        agent: req_agent_name,
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
                            let new_req = DispatchRequest {
                                agent_name: alternative_agent,
                                task: hook_ctx.task.clone(),
                                mode_override: Some(hook_ctx.execution_mode.clone()),
                                cancel: CancellationToken::new(),
                                parent_agent: hook_ctx.parent_agent.clone(),
                                parent_context: None,
                                delegate_depth: delegate_depth + 1,
                            };
                            return Box::pin(self.dispatch(new_req)).await;
                        }
                        super::hooks::SubagentRetryDecision::Retry { delay_secs } => {
                            info!(delay_secs, "Retrying subagent dispatch");
                            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                            let retry_req = DispatchRequest {
                                agent_name: hook_ctx.subagent_name.clone(),
                                task: hook_ctx.task.clone(),
                                mode_override: Some(hook_ctx.execution_mode.clone()),
                                cancel: CancellationToken::new(),
                                parent_agent: hook_ctx.parent_agent.clone(),
                                parent_context: None,
                                delegate_depth,
                            };
                            return Box::pin(self.dispatch(retry_req)).await;
                        }
                        super::hooks::SubagentRetryDecision::Fail => {}
                    }
                }

                Err(e)
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
        let timeout_secs = if registered.definition.timeout_secs > 0 {
            registered.definition.timeout_secs
        } else {
            self.config.default_timeout_secs
        };

        let handle_id = format!("tm_{}", uuid::Uuid::new_v4().as_simple());

        let join_handle = tokio::spawn(async move {
            let _permit = child_token.clone();
            let start = Instant::now();

            let mut agent = agent_arc.lock().await;

            // Check cancellation
            if child_token.is_cancelled() {
                return Ok(SubagentResult {
                    agent_name: agent_name.clone(),
                    output: "Cancelled before execution".into(),
                    duration: start.elapsed(),
                    iterations: 0,
                    tokens_used: None,
                    was_truncated: false,
                    mode: ExecutionMode::Teammate,
                });
            }

            let result = if timeout_secs > 0 {
                match tokio::time::timeout(Duration::from_secs(timeout_secs), agent.execute(&task))
                    .await
                {
                    Ok(r) => r,
                    Err(_) => Err(ReactError::Other(format!(
                        "Teammate '{}' timed out after {}s",
                        agent_name, timeout_secs
                    ))),
                }
            } else {
                agent.execute(&task).await
            };

            match result {
                Ok(output) => Ok(SubagentResult {
                    agent_name,
                    output,
                    duration: start.elapsed(),
                    iterations: 1,
                    tokens_used: None,
                    was_truncated: false,
                    mode: ExecutionMode::Teammate,
                }),
                Err(e) => Err(e),
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
                .filter_map(|m| m.content.as_ref().map(|c| format!("[{}] {}", m.role, c)))
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

        let start = Instant::now();
        let mut agent = agent_arc.lock().await;

        // Check cancellation
        if req.cancel.is_cancelled() {
            return Ok(SubagentResult {
                agent_name: req.agent_name.clone(),
                output: "Cancelled before execution".into(),
                duration: start.elapsed(),
                iterations: 0,
                tokens_used: None,
                was_truncated: false,
                mode: ExecutionMode::Sync,
            });
        }

        let output = agent
            .execute(&Self::enhance_task(&req.task, req.parent_context.as_ref()))
            .await?;

        Ok(SubagentResult {
            agent_name: req.agent_name.clone(),
            output,
            duration: start.elapsed(),
            iterations: 1,
            tokens_used: None,
            was_truncated: false,
            mode: ExecutionMode::Sync,
        })
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
        let cancel = req.cancel.clone();
        let enhanced_task = Self::enhance_task(&task, req.parent_context.as_ref());

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
                });
            }

            let mut agent = agent_arc.lock().await;

            let result = if timeout_secs > 0 {
                match tokio::time::timeout(
                    Duration::from_secs(timeout_secs),
                    agent.execute(&enhanced_task),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(ReactError::Other(format!(
                        "Fork subagent '{}' timed out after {}s",
                        agent_name, timeout_secs
                    ))),
                }
            } else {
                agent.execute(&enhanced_task).await
            };

            result.map(|output| SubagentResult {
                agent_name,
                output,
                duration: start.elapsed(),
                iterations: 1,
                tokens_used: None,
                was_truncated: false,
                mode: ExecutionMode::Fork,
            })
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
        };

        let result = executor.dispatch(req).await.unwrap();
        assert_eq!(result.output, "done");
        assert_eq!(result.mode, ExecutionMode::Sync);
    }

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
        };

        let handle = executor.dispatch_teammate(req).await.unwrap();
        assert_eq!(handle.agent_name, "tm");

        let result = handle.join().await.unwrap();
        assert_eq!(result.output, "team result");
    }
}
