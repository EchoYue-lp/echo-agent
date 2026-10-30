use crate::agent::subagent::context::{ContextInheritance, SubagentContext};
use crate::agent::subagent::executor::SubagentExecutor;
use crate::agent::subagent::{ExecutionMode, SubagentOutcome};
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_core::agent::CancellationToken;
use echo_core::tools::{
    ExternalRunContext, NestedDelegationPolicy, ToolAccess, ToolContext, ToolRiskLevel,
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, info, warn};

fn serialize_parent_result(
    outcome: &SubagentOutcome,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string(outcome)
}

/// Factory for lazily building parent context at dispatch time.
///
/// Holds Arc references to the parent agent's shared subsystems so
/// that `SubagentContext` can be built with the latest messages and
/// tool definitions when a dispatch actually occurs.
pub struct ParentContextFactory {
    /// Parent's tool manager (to get current tool definitions).
    pub tool_manager: Arc<crate::tools::ToolManager>,
    /// Parent's context manager (to get recent messages).
    pub context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    /// Parent's memory store (for sharing with subagent).
    pub store: Option<Arc<dyn crate::memory::Store>>,
}

impl ParentContextFactory {
    /// Build a SubagentContext using an explicit inheritance policy.
    pub async fn build_with_inheritance(
        &self,
        inheritance: &ContextInheritance,
    ) -> SubagentContext {
        let ctx = self.context.lock().await;
        let messages = ctx.messages().to_vec();
        let tool_defs: Vec<_> = self
            .tool_manager
            .get_tool_definitions()
            .into_iter()
            .collect();
        SubagentContext::from_parent(&tool_defs, &messages, self.store.clone(), inheritance)
    }
}

pub struct AgentDispatchTool {
    executor: Arc<SubagentExecutor>,
    parent_agent: String,
    /// Shared, updatable handle to the parent run's cancel token (P1-11).
    ///
    /// The tool is constructed during agent build, before any run starts, so
    /// the parent's cancel token is not known yet. We hold an `Arc<Mutex<..>>`
    /// that the parent run updates when it begins; `execute` reads the latest
    /// value and derives a `child_token` so a subagent dispatched via this tool
    /// is cancelled when the parent run is. Previously this was a fixed
    /// `CancellationToken::new()` captured at build time — an inert token that
    /// could never fire, so LLM-initiated dispatches were detached from the
    /// parent's cancellation entirely.
    cancel: Arc<tokio::sync::Mutex<Option<CancellationToken>>>,
    /// Optional factory for building parent context at dispatch time.
    /// When set, explicit Fork mode can inherit filtered conversation history.
    parent_context_factory: Option<Arc<ParentContextFactory>>,
    /// Snapshot of available subagents exposed to the LLM through the tool schema.
    catalog: Arc<std::sync::RwLock<Vec<crate::agent::subagent::SubagentDefinition>>>,
    catalog_revision: Arc<AtomicU64>,
    /// 该实例只委派只读目标；不改变普通 dispatcher 的能力声明。
    read_only: bool,
}

impl AgentDispatchTool {
    pub fn new(
        executor: Arc<SubagentExecutor>,
        parent_agent: impl Into<String>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            catalog: executor.registry().executable_catalog_handle(),
            catalog_revision: executor.registry().catalog_revision_handle(),
            executor,
            parent_agent: parent_agent.into(),
            cancel: Arc::new(tokio::sync::Mutex::new(Some(cancel))),
            parent_context_factory: None,
            read_only: false,
        }
    }

    /// 为只读 Agent 装配的受限实例。普通 `new` 仍保留 Mutating 合同。
    pub fn read_only(
        executor: Arc<SubagentExecutor>,
        parent_agent: impl Into<String>,
        cancel: CancellationToken,
    ) -> Self {
        let mut tool = Self::new(executor, parent_agent, cancel);
        tool.read_only = true;
        tool
    }

    /// Create with a parent context factory for context inheritance.
    pub fn with_parent_context(mut self, factory: Arc<ParentContextFactory>) -> Self {
        self.parent_context_factory = Some(factory);
        self
    }

    fn delegation_policy_from_context(
        ctx: Option<&ToolContext>,
    ) -> std::result::Result<NestedDelegationPolicy, String> {
        match ctx.and_then(|ctx| ctx.delegation_policy) {
            Some(policy) => policy.child_policy().ok_or_else(|| {
                format!(
                    "Delegation depth exceeded (max {})",
                    policy.max_delegate_depth
                )
            }),
            None => Ok(crate::agent::subagent::DispatchRequest::policy_from_depth(
                0,
            )),
        }
    }

    /// Build [`ExternalRunContext`] for application UI identity pinning.
    ///
    /// Uses the formal run id when present and always preserves the chat turn id.
    /// `execution_id` has no `:` so the Tauri bridge uses the full string as
    /// `subagent_run_id` (avoids colliding parallel same-role dispatches).
    fn runtime_context_from_tool_ctx(ctx: Option<&ToolContext>) -> Option<ExternalRunContext> {
        let c = ctx?;
        if c.conversation_id.is_none()
            && c.run_id.is_none()
            && c.turn_id.is_none()
            && c.message_id.is_none()
            && c.execution_id.is_none()
            && c.cancel.is_none()
            && c.trace_sink.is_none()
            && c.delegation_policy.is_none()
            && c.resource_guards.is_empty()
        {
            return None;
        }
        Some(ExternalRunContext {
            conversation_id: c.conversation_id.clone(),
            run_id: c.run_id.clone(),
            turn_id: c.turn_id.clone(),
            execution_id: Some(format!("agent_tool-{}", uuid::Uuid::new_v4())),
            isolation_id: None,
            message_id: c
                .message_id
                .clone()
                .or_else(|| c.turn_id.clone())
                .or_else(|| c.run_id.clone()),
            cancel: c.cancel.clone(),
            trace_sink: c.trace_sink.clone(),
            delegation_policy: c.delegation_policy,
            resource_guards: c.resource_guards.clone(),
            // The child's lineage is stamped below from the invoking
            // context's own lineage — the parent's value must not leak.
            subagent_lineage: None,
            uplink: None,
        })
    }

    async fn child_cancel_token(
        cancel_handle: &Arc<tokio::sync::Mutex<Option<CancellationToken>>>,
        invocation_cancel: Option<&CancellationToken>,
    ) -> CancellationToken {
        if let Some(parent) = invocation_cancel {
            return parent.child_token();
        }

        cancel_handle
            .lock()
            .await
            .as_ref()
            .map(CancellationToken::child_token)
            .unwrap_or_else(CancellationToken::new)
    }

    fn dispatch_with_context(
        &self,
        parameters: ToolParameters,
        ctx: Option<&ToolContext>,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let executor = self.executor.clone();
        let parent_agent = self.parent_agent.clone();
        let cancel_handle = self.cancel.clone();
        let factory = self.parent_context_factory.clone();
        let mut runtime_context = Self::runtime_context_from_tool_ctx(ctx);
        let active_message = ctx.and_then(|context| context.active_message.clone());
        // Owned parent lineage values for stamping the child's identity
        // inside the async block (the ctx reference must not be captured).
        let parent_lineage = ctx.and_then(|context| context.subagent_lineage.clone());
        let parent_execution_id = ctx.and_then(|context| context.execution_id.clone());
        let parent_call_id = ctx.and_then(|context| context.call_id.clone());
        let effect_sink = ctx.and_then(|context| context.effect_sink.clone());
        let invocation_cancel = ctx.and_then(|context| context.cancel.clone());
        // `available` is the immutable invocation hard bound. First-turn
        // visibility and the shared manager's catalog do not grant descendants
        // permission to recover tools excluded by the invoking task.
        let read_only = self.read_only
            || ctx.and_then(|context| context.tool_access) == Some(ToolAccess::ReadOnly);
        let inherited_access = if read_only {
            Some(ToolAccess::ReadOnly)
        } else {
            ctx.and_then(|context| context.tool_access)
        };
        let inherited_tools = ctx
            .and_then(|context| context.tool_visibility.as_ref())
            .map(|visibility| visibility.available_names());
        if read_only && inherited_tools.is_none() {
            // 本地调用丢失执行快照时不能把缺失范围误当无限制权限。
            return Box::pin(async {
                Ok(ToolResult::error(
                    "Read-only delegation requires an invocation ToolContext with tool visibility",
                ))
            });
        }
        if inherited_tools
            .as_ref()
            .is_some_and(|tools| !tools.iter().any(|name| name == "agent_tool"))
        {
            return Box::pin(async {
                Ok(ToolResult::error(
                    "agent_tool is outside the inherited invocation tool scope",
                ))
            });
        }
        let delegation_policy = match Self::delegation_policy_from_context(ctx) {
            Ok(policy) => policy,
            Err(e) => return Box::pin(async move { Ok(ToolResult::error(e)) }),
        };

        Box::pin(async move {
            let agent_name = parameters
                .get("agent_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("agent_name".to_string()))?;

            // Stamp the child's lineage from the invoking context: the parent
            // path chains (`root/<parent>/<child>`), and the parent's own
            // execution id lets the default uplink steer the parent when the
            // child escalates. The parent's uplink is deliberately NOT
            // inherited — the executor installs a fresh default sink bound to
            // this child.
            if let Some(context) = runtime_context.as_mut() {
                let parent_path = parent_lineage
                    .as_ref()
                    .and_then(|lineage| lineage.agent_path.clone())
                    .unwrap_or_else(|| "root".to_string());
                let parent_event_id = parent_execution_id
                    .as_deref()
                    .zip(parent_call_id.as_deref())
                    .and_then(|(execution_id, call_id)| {
                        executor
                            .registry()
                            .event_bus()
                            .parent_event_id_for_tool(execution_id, call_id)
                    })
                    .map(|event_id| event_id.as_str().to_string());
                context.subagent_lineage = Some(echo_core::tools::SubagentLineage {
                    agent_name: Some(agent_name.to_string()),
                    execution_id: context.execution_id.clone(),
                    run_id: context.run_id.clone(),
                    parent_agent: Some(parent_agent.clone()),
                    parent_execution_id: parent_execution_id.clone(),
                    parent_event_id,
                    agent_path: Some(format!("{parent_path}/{agent_name}")),
                    task_id: None,
                    attempt: None,
                    plan_revision: None,
                });
            }

            let task = parameters
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task".to_string()))?;

            let mode_override = match parameters.get("mode") {
                None | Some(Value::Null) => None,
                Some(Value::String(mode)) => Some(match mode.as_str() {
                    "sync" => ExecutionMode::Sync,
                    "fork" => ExecutionMode::Fork,
                    "teammate" => ExecutionMode::Teammate,
                    "team" => ExecutionMode::Team,
                    _ => {
                        return Ok(ToolResult::error(format!(
                            "Invalid subagent execution mode '{mode}'"
                        )));
                    }
                }),
                Some(_) => {
                    return Ok(ToolResult::error(
                        "Subagent execution mode must be a string",
                    ));
                }
            };

            let param_background = parameters
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let constraints = parameters
                .get("constraints")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // Inheritance is independent of execution mode:
            // - omit / sync → fresh (no parent system/history/memory)
            // - fork → inherit a filtered parent slice
            // (applied below when building parent_context)

            // Execution mode: keep caller's choice, but force Fork when the
            // target role declares worktree/workspace isolation (those paths
            // only exist on dispatch_fork today).
            let registered = executor
                .registry()
                .resolve_registration(agent_name)
                .await
                .ok_or_else(|| ToolError::ExecutionFailed {
                    tool: "agent_tool".to_string(),
                    message: format!("Subagent '{agent_name}' is not registered"),
                })?;
            let mut exec_mode = mode_override
                .clone()
                .unwrap_or_else(|| registered.definition().execution_mode.clone());
            let def = registered.definition();
            let isolation_forces_fork = def.isolation.is_some();
            if isolation_forces_fork {
                exec_mode = ExecutionMode::Fork;
            }
            let role_is_background = def.is_background;
            let run_background = param_background || role_is_background;

            info!(
                target_agent = %agent_name,
                task = %task,
                mode = ?exec_mode,
                background = run_background,
                inherit_fork = exec_mode == ExecutionMode::Fork,
                delegate_depth = delegation_policy.delegate_depth,
                max_delegate_depth = delegation_policy.max_delegate_depth,
                "Dispatching task to subagent via SubagentExecutor"
            );

            // Build parent context if factory is available. Fork, teammate,
            // and team all use the same structured-history policy; the
            // compiler decides the final bounded message set. Fresh sync
            // dispatches intentionally carry only the scoped parent goal.
            let mut parent_context = if let Some(ref f) = factory {
                let ctx = if matches!(
                    exec_mode,
                    ExecutionMode::Fork | ExecutionMode::Teammate | ExecutionMode::Team
                ) {
                    let inheritance = ContextInheritance {
                        // Preserve the complete raw snapshot here. The
                        // configured prompt compiler performs the single
                        // provider-safe filter and applies the role limit.
                        inherit_history: Some(0),
                        ..ContextInheritance::fork_default()
                    };
                    f.build_with_inheritance(&inheritance).await
                } else {
                    f.build_with_inheritance(&ContextInheritance::fresh_default())
                        .await
                };
                if ctx.has_content() { Some(ctx) } else { None }
            } else {
                None
            };
            if let Some(inherited) = inherited_tools {
                // A factory is optional for standalone consumers. A supplied
                // hard scope is still known even without inherited history.
                let context = parent_context.get_or_insert_with(SubagentContext::empty);
                if let Err(error) = context.inherit_tool_scope(&inherited) {
                    return Ok(ToolResult::error(error));
                }
            }

            if let Some(inherited) = inherited_access {
                let context = parent_context.get_or_insert_with(SubagentContext::empty);
                if context.tool_access != Some(ToolAccess::ReadOnly) {
                    context.tool_access = Some(inherited);
                }
            }

            // ToolContext is invocation-scoped and therefore authoritative for
            // pooled agents. The shared handle remains a compatibility fallback
            // for callers that execute the tool without runtime context.
            let cancel =
                Self::child_cancel_token(&cancel_handle, invocation_cancel.as_deref()).await;

            let req = crate::agent::subagent::DispatchRequest {
                agent_name: agent_name.to_string(),
                task: task.to_string(),
                mode_override: if isolation_forces_fork {
                    Some(ExecutionMode::Fork)
                } else {
                    mode_override
                },
                cancel,
                parent_agent: parent_agent.clone(),
                parent_context,
                delegation_policy,
                runtime_context,
                message: active_message,
                prompt_payload: None,
                prompt_context: None,
                constraints,
                background: run_background,
            };

            if run_background {
                match executor
                    .dispatch_background_registered(req, registered)
                    .await
                {
                    Ok(handle) => {
                        let execution_id = handle.execution_id.clone();
                        let started_agent_name = handle.agent_name.clone();
                        info!(
                            target_agent = %started_agent_name,
                            execution_id = %execution_id,
                            "Background subagent started"
                        );
                        if let Some(sink) = effect_sink {
                            let task = task.to_string();
                            tokio::spawn(async move {
                                let outcome = match handle.join().await {
                                    Ok(result) => result.outcome.status.as_str().to_string(),
                                    Err(error) => {
                                        match crate::agent::subagent::subagent_status_from_error(
                                            &error,
                                        ) {
                                            Ok(status) => status.as_str().to_string(),
                                            Err(_) => return, // Observation is not a terminal effect.
                                        }
                                    }
                                };
                                sink(echo_core::tools::ToolEffect::SubagentRun {
                                    agent_name: started_agent_name,
                                    task,
                                    outcome,
                                })
                                .await;
                            });
                        }
                        Ok(ToolResult::success(
                            json!({
                                "status": "started",
                                "execution_id": execution_id,
                                "agent_name": agent_name,
                            })
                            .to_string(),
                        ))
                    }
                    Err(e) => {
                        warn!(target_agent = %agent_name, error = %e, "Background subagent start failed");
                        Ok(ToolResult::error(format!(
                            "Subagent '{}' background start failed: {}",
                            agent_name, e
                        )))
                    }
                }
            } else {
                match executor.dispatch_registered(req, registered).await {
                    Ok(result) => {
                        info!(target_agent = %agent_name, "Subagent completed successfully");
                        debug!(
                            target_agent = %agent_name,
                            summary = %result.outcome.summary,
                            output_chars = result.output.chars().count(),
                            "Subagent result"
                        );
                        let outcome = result.outcome.status.as_str().to_string();
                        Ok(match serialize_parent_result(&result.outcome) {
                            Ok(output) => ToolResult::success(output).with_effect(
                                echo_core::tools::ToolEffect::SubagentRun {
                                    agent_name: agent_name.to_string(),
                                    task: task.to_string(),
                                    outcome,
                                },
                            ),
                            Err(error) => ToolResult::error(format!(
                                "Subagent '{}' result serialization failed: {}",
                                agent_name, error
                            ))
                            .with_effect(
                                echo_core::tools::ToolEffect::SubagentRun {
                                    agent_name: agent_name.to_string(),
                                    task: task.to_string(),
                                    outcome,
                                },
                            ),
                        })
                    }
                    Err(e) => {
                        warn!(target_agent = %agent_name, error = %e, "Subagent execution failed");
                        Ok(ToolResult::error(format!(
                            "Subagent '{}' execution failed: {}",
                            agent_name, e
                        ))
                        .with_effect(
                            echo_core::tools::ToolEffect::SubagentRun {
                                agent_name: agent_name.to_string(),
                                task: task.to_string(),
                                outcome: crate::agent::subagent::subagent_status_from_error(&e)?
                                    .as_str()
                                    .to_string(),
                            },
                        ))
                    }
                }
            }
        })
    }
}

impl Tool for AgentDispatchTool {
    fn name(&self) -> &str {
        "agent_tool"
    }

    fn risk_level(&self) -> ToolRiskLevel {
        // 每次派发都建立真实子调用/回执；只读能力不代表可重放缓存结果。
        ToolRiskLevel::Standard
    }

    fn capabilities(&self) -> echo_core::tools::ToolCapabilities {
        echo_core::tools::ToolCapabilities {
            access: if self.read_only {
                ToolAccess::ReadOnly
            } else {
                ToolAccess::Mutating
            },
            risk: self.risk_level(),
            permissions: self.permissions(),
        }
    }

    fn description(&self) -> &str {
        "Dispatch tasks to specialized Subagents in an isolated context. \
         Default is fresh context (no parent conversation). Use mode=fork only when \
         the Subagent needs shared background from this session. Set background=true \
         to start the Subagent without blocking (returns started + execution_id; \
         completion arrives via events / chat note). One call delegates one bounded \
         task. When the host provides a formal task planner, use that planner for \
         coordinated, dependent, or parallel multi-task work. Synchronous completion returns a \
         JSON result with status, summary, artifacts, evidence, remaining_work, \
         and evidence. Use only agent_name values listed in the schema.\n\
         The Subagent's result is not visible to the user — summarize it in your reply to the user.\n\
         Tell the Subagent clearly whether to write code or only do research, and the expected result format.\n\
         Do not duplicate work the Subagent is already doing (same searches, edits, or checks).\n\
         To run multiple independent Subagents in parallel, send a single message with multiple agent_tool calls."
    }

    /// `agent_tool` dispatches a subagent that runs its own multi-step ReAct
    /// (latency far higher than typical file/shell tools). Exempt it from the
    /// parallel batch timeout so it doesn't dominate the batch budget and
    /// prematurely cancel peers; it has its own per-dispatch timeout instead
    /// (see `SubagentExecutor` default 600s).
    fn exempt_from_batch_timeout(&self) -> bool {
        true
    }

    fn parameters(&self) -> Value {
        let catalog = self
            .catalog
            .read()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| {
                        !self.read_only
                            || entry.access_mode
                                == crate::agent::subagent::SubagentAccessMode::ReadOnly
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let agent_names: Vec<String> = catalog.iter().map(|entry| entry.name.clone()).collect();
        let catalog_text = if catalog.is_empty() {
            "No Subagents are currently registered.".to_string()
        } else {
            let lines: Vec<String> = catalog
                .iter()
                .map(|entry| format!("{}: {}", entry.name, entry.description))
                .collect();
            format!("Available Subagents: {}", lines.join("; "))
        };

        let agent_name_schema = if agent_names.is_empty() {
            json!({
                "type": "string",
                "description": catalog_text
            })
        } else {
            json!({
                "type": "string",
                "enum": agent_names,
                "description": catalog_text
            })
        };

        json!({
            "type": "object",
            "properties": {
                "agent_name": agent_name_schema,
                "task": {
                    "type": "string",
                    "description": "Specific task description to assign to the Subagent. Write its natural-language prose in the user's current language, then include relevant paths, scope, constraints, and required result format."
                },
                "mode": {
                    "type": "string",
                    "enum": ["sync", "fork", "teammate", "team"],
                    "description": "Optional. Omit or \"sync\" = fresh context. \"fork\", \"teammate\", and \"team\" may receive only filtered recent user/final-assistant messages; parent system prompts, tool traffic, and reasoning are never transferred. Worktree/workspace isolation is automatic for roles that declare it. \"teammate\" returns a join/cancel handle. \"team\" executes the role's TeamSpec through the canonical revisioned task DAG."
                },
                "constraints": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional. Explicit constraints or boundary rules the Subagent must respect — scope limits, files/dirs to avoid, verification expectations, output format. Rendered into the task context even for fresh-context dispatches."
                },
                "background": {
                    "type": "boolean",
                    "description": "Optional. When true, start the Subagent without blocking this turn; returns {status:\"started\", execution_id, agent_name}. Also true when the target role declares is_background."
                }
            },
            "required": ["agent_name", "task"]
        })
    }

    fn schema_revision(&self) -> u64 {
        self.catalog_revision.load(Ordering::Acquire)
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        self.dispatch_with_context(parameters, None)
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, crate::error::Result<ToolResult>> {
        self.dispatch_with_context(parameters, Some(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{SubagentDefinition, SubagentExecutorConfig, SubagentRegistry};
    use crate::testing::{FailingMockAgent, MockAgent, MockAgentFailure};
    use echo_core::llm::types::{ContentPart, Message};
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn readonly_dispatch_uses_distinct_real_calls_instead_of_result_cache_replay()
    -> Result<(), String> {
        let registry = Arc::new(SubagentRegistry::new());
        let child = MockAgent::new("reader")
            .with_responses(["## Summary\nfirst child", "## Summary\nsecond child"]);
        let mut definition = SubagentDefinition::new("reader", "readonly child");
        definition.access_mode = crate::agent::subagent::SubagentAccessMode::ReadOnly;
        registry.register(definition, Box::new(child.clone())).await;
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig::default(),
        ));
        let manager = crate::tools::ToolManager::new();
        manager.register(Box::new(AgentDispatchTool::read_only(
            executor,
            "primary",
            CancellationToken::new(),
        )));
        let parameters = [
            ("agent_name".into(), json!("reader")),
            ("task".into(), json!("same requested task")),
        ]
        .into_iter()
        .collect::<ToolParameters>();
        let mut context = readonly_dispatch_context(&["agent_tool"]);
        context.run_id = Some("same-parent-run".to_string());
        context.turn_id = Some("same-parent-turn".to_string());
        context.message_id = Some("same-parent-message".to_string());
        context.execution_id = Some("same-parent-execution".to_string());
        for call_id in ["first-physical-request", "second-physical-request"] {
            context.call_id = Some(call_id.to_string());
            let result = manager
                .execute_tool_with_context("agent_tool", parameters.clone(), &context)
                .await
                .map_err(|error| error.to_string())?;
            assert!(result.success, "{}", result.output);
        }
        assert_eq!(
            child.invocation_contexts().len(),
            2,
            "a second independent dispatch replayed an old cached Subagent outcome"
        );
        Ok(())
    }

    fn readonly_dispatch_context(names: &[&str]) -> ToolContext {
        let names = names.iter().map(|name| (*name).to_string()).collect();
        ToolContext {
            tool_access: Some(ToolAccess::ReadOnly),
            tool_visibility: Some(Arc::new(echo_core::tools::ToolVisibilityState::new(
                names,
                std::collections::HashSet::new(),
            ))),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn readonly_dispatch_instance_requires_context_filters_catalog_and_keeps_ordinary_mode()
    -> Result<(), String> {
        let registry = Arc::new(SubagentRegistry::new());
        let reader = MockAgent::new("reader").with_response("## Summary\nread");
        let writer = MockAgent::new("writer").with_response("## Summary\nwrite");
        let mut definition = SubagentDefinition::new("reader", "read only");
        definition.access_mode = crate::agent::subagent::SubagentAccessMode::ReadOnly;
        registry
            .register(definition, Box::new(reader.clone()))
            .await;
        registry
            .register(
                SubagentDefinition::new("writer", "write"),
                Box::new(writer.clone()),
            )
            .await;
        let executor = Arc::new(SubagentExecutor::new(
            Arc::clone(&registry),
            SubagentExecutorConfig::default(),
        ));
        let restricted = AgentDispatchTool::read_only(
            Arc::clone(&executor),
            "primary",
            CancellationToken::new(),
        );
        let ordinary = AgentDispatchTool::new(executor, "primary", CancellationToken::new());
        assert_eq!(restricted.risk_level(), ToolRiskLevel::Standard);
        assert_eq!(restricted.capabilities().risk, ToolRiskLevel::Standard);
        assert_eq!(restricted.capabilities().access, ToolAccess::ReadOnly);
        assert_eq!(ordinary.capabilities().access, ToolAccess::Mutating);
        assert_eq!(
            restricted
                .parameters()
                .pointer("/properties/agent_name/enum"),
            Some(&json!(["reader"]))
        );
        let params = |name: &str| {
            [
                ("agent_name".into(), json!(name)),
                ("task".into(), json!("bounded read")),
            ]
            .into_iter()
            .collect::<ToolParameters>()
        };
        let missing = restricted
            .execute(params("reader"))
            .await
            .map_err(|error| error.to_string())?;
        assert!(!missing.success);
        for context in [
            ToolContext::default(),
            readonly_dispatch_context(&[]),
            readonly_dispatch_context(&["read_file"]),
        ] {
            let result = restricted
                .execute_with_context(params("reader"), &context)
                .await
                .map_err(|error| error.to_string())?;
            assert!(!result.success);
        }
        assert!(reader.invocation_contexts().is_empty());
        let context = readonly_dispatch_context(&["agent_tool", "read_file", "tool_search"]);
        let blocked = restricted
            .execute_with_context(params("writer"), &context)
            .await
            .map_err(|error| error.to_string())?;
        assert!(!blocked.success);
        assert!(writer.invocation_contexts().is_empty());
        let result = restricted
            .execute_with_context(params("reader"), &context)
            .await
            .map_err(|error| error.to_string())?;
        assert!(result.success, "{}", result.output);
        let calls = reader.invocation_contexts();
        let invocation = calls
            .first()
            .ok_or_else(|| "readonly child invocation missing".to_string())?;
        assert_eq!(invocation.tool_access, Some(ToolAccess::ReadOnly));
        assert_eq!(
            invocation.allowed_tools,
            Some(
                ["agent_tool", "read_file", "tool_search"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            )
        );
        let positive = ordinary
            .execute(params("writer"))
            .await
            .map_err(|error| error.to_string())?;
        assert!(positive.success, "{}", positive.output);
        assert_eq!(writer.invocation_contexts().len(), 1);
        // A per-call ReadOnly value also narrows a normal instance used directly.
        let blocked = ordinary
            .execute_with_context(params("writer"), &context)
            .await
            .map_err(|error| error.to_string())?;
        assert!(!blocked.success);
        assert_eq!(writer.invocation_contexts().len(), 1);
        Ok(())
    }

    struct ReadOnlyScopeProbe(ScopeProbeTool);
    impl Tool for ReadOnlyScopeProbe {
        fn name(&self) -> &str {
            self.0.name()
        }
        fn description(&self) -> &str {
            self.0.description()
        }
        fn parameters(&self) -> Value {
            self.0.parameters()
        }
        fn risk_level(&self) -> ToolRiskLevel {
            ToolRiskLevel::ReadOnly
        }
        fn execute(
            &self,
            parameters: ToolParameters,
        ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
            self.0.execute(parameters)
        }
    }

    #[tokio::test]
    async fn readonly_target_label_does_not_enable_writer_tools_or_tool_search_promotion()
    -> crate::error::Result<()> {
        let registry = Arc::new(SubagentRegistry::new());
        let writes = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let child = crate::agent::ReactAgentBuilder::new()
            .name("mistagged-reader")
            .model("mock-model")
            .llm_client(Arc::new(ScopeRequestClient {
                inner: crate::testing::MockLlmClient::new()
                    .then_tool_call("read", "read_file", "{}")
                    .then_tool_call("search", "tool_search", r#"{"query":"write_file"}"#)
                    .then_tool_call("write", "write_file", "{}")
                    .with_response("## Summary\n只读完成"),
                requests: Arc::clone(&requests),
            }))
            .tool(Box::new(ReadOnlyScopeProbe(ScopeProbeTool {
                name: "read_file",
                calls: Arc::clone(&reads),
            })))
            .tool(Box::new(ScopeProbeTool {
                name: "write_file",
                calls: Arc::clone(&writes),
            }))
            .build()?;
        let mut definition =
            SubagentDefinition::new("mistagged-reader", "declaration alone is insufficient");
        definition.access_mode = crate::agent::subagent::SubagentAccessMode::ReadOnly;
        registry.register(definition, Box::new(child)).await;
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig::default(),
        ));
        // 可写父也必须兑现目标的 ReadOnly 声明。
        let dispatch = AgentDispatchTool::new(executor, "primary", CancellationToken::new());
        let result = dispatch
            .execute_with_context(
                [
                    ("agent_name".into(), json!("mistagged-reader")),
                    ("task".into(), json!("只读检查")),
                ]
                .into_iter()
                .collect(),
                &ToolContext::default(),
            )
            .await?;
        assert!(result.success, "{}", result.output);
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let requests = requests
            .lock()
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        assert!(!requests.is_empty());
        assert!(
            requests
                .iter()
                .all(|names| !names.iter().any(|name| name == "write_file"))
        );
        Ok(())
    }

    struct ScopeRequestClient {
        inner: crate::testing::MockLlmClient,
        requests: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    impl ScopeRequestClient {
        fn record(&self, request: &crate::llm::ChatRequest) -> crate::error::Result<()> {
            let names = request
                .tools
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|tool| tool.function.name.clone())
                .collect();
            self.requests
                .lock()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?
                .push(names);
            Ok(())
        }
    }

    impl crate::llm::LlmClient for ScopeRequestClient {
        fn chat(
            &self,
            request: crate::llm::ChatRequest,
        ) -> BoxFuture<'_, crate::error::Result<crate::llm::ChatResponse>> {
            Box::pin(async move {
                self.record(&request)?;
                crate::llm::LlmClient::chat(&self.inner, request).await
            })
        }
        fn chat_stream(
            &self,
            request: crate::llm::ChatRequest,
        ) -> BoxFuture<
            '_,
            crate::error::Result<
                futures::stream::BoxStream<'static, crate::error::Result<crate::llm::ChatChunk>>,
            >,
        > {
            Box::pin(async move {
                self.record(&request)?;
                crate::llm::LlmClient::chat_stream(&self.inner, request).await
            })
        }
        fn model_name(&self) -> &str {
            "mock-model"
        }
    }

    struct ScopeProbeTool {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl Tool for ScopeProbeTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "tool scope test probe"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{}})
        }
        fn execute(
            &self,
            _parameters: ToolParameters,
        ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::success("probe called"))
            })
        }
    }

    #[derive(Clone, Debug)]
    struct DepthObservation {
        policy: Option<NestedDelegationPolicy>,
        available: Option<Vec<String>>,
        access: Option<ToolAccess>,
    }

    struct DepthProbeTool {
        inner: ScopeProbeTool,
        observations: Arc<std::sync::Mutex<Vec<DepthObservation>>>,
    }

    impl Tool for DepthProbeTool {
        fn name(&self) -> &str {
            "depth_probe"
        }
        fn description(&self) -> &str {
            "capture the actual tool invocation policy"
        }
        fn risk_level(&self) -> ToolRiskLevel {
            ToolRiskLevel::ReadOnly
        }
        fn parameters(&self) -> Value {
            self.inner.parameters()
        }
        fn execute(
            &self,
            parameters: ToolParameters,
        ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
            self.inner.execute(parameters)
        }
        fn execute_with_context<'a>(
            &'a self,
            parameters: ToolParameters,
            context: &'a ToolContext,
        ) -> BoxFuture<'a, crate::error::Result<ToolResult>> {
            Box::pin(async move {
                self.observations
                    .lock()
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))?
                    .push(DepthObservation {
                        policy: context.delegation_policy,
                        access: context.tool_access,
                        available: context
                            .tool_visibility
                            .as_ref()
                            .map(|visibility| visibility.available_names()),
                    });
                self.inner.execute(parameters).await
            })
        }
    }

    async fn assert_actual_depth_chain(
        mode: &'static str,
        maximum: u8,
        team: bool,
        readonly: bool,
    ) -> crate::error::Result<()> {
        assert_actual_depth_chain_with_background(mode, maximum, team, readonly, false).await
    }

    async fn assert_actual_depth_chain_with_background(
        mode: &'static str,
        maximum: u8,
        team: bool,
        readonly: bool,
        background: bool,
    ) -> crate::error::Result<()> {
        let registry = Arc::new(SubagentRegistry::new());
        let mut events = registry.event_bus().subscribe();
        let mut actors = Vec::new();
        // One extra registered actor makes an unintended dispatch observable;
        // it cannot be mistaken for a harmless unregistered-role rejection.
        for depth in 1u8..=5 {
            let name = format!("depth-{depth}");
            let next = format!("depth-{}", depth.saturating_add(1));
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let observations = Arc::new(std::sync::Mutex::new(Vec::new()));
            let client = Arc::new(ScopeRequestClient {
                inner: crate::testing::MockLlmClient::new()
                    .then_tool_call(format!("probe-{depth}"), "depth_probe", "{}")
                    .then_tool_call(
                        format!("search-{depth}"),
                        "tool_search",
                        r#"{"query":"agent_tool"}"#,
                    )
                    .then_tool_call(
                        format!("spawn-{depth}"),
                        "agent_tool",
                        json!({"agent_name":next,"task":"next bounded child","mode":mode})
                            .to_string(),
                    )
                    .with_response(format!("## Summary\nactor {depth} complete")),
                requests: Arc::clone(&requests),
            });
            let builder = crate::agent::ReactAgentBuilder::new()
                .name(&name)
                .model("mock-model")
                .enable_tools()
                .enable_subagent()
                .register_agent_dispatch_tool()
                .subagent_registry(Arc::clone(&registry))
                .llm_client(client)
                .tool(Box::new(DepthProbeTool {
                    inner: ScopeProbeTool {
                        name: "depth_probe",
                        calls: Arc::new(AtomicUsize::new(0)),
                    },
                    observations: Arc::clone(&observations),
                }));
            let agent = if readonly {
                builder.readonly_tools()
            } else {
                builder
            }
            .build()?;
            let mut definition = SubagentDefinition::new(&name, "bounded actor");
            if readonly {
                definition.access_mode = crate::agent::subagent::SubagentAccessMode::ReadOnly;
            }
            registry.register(definition, Box::new(agent)).await;
            actors.push((depth, name, requests, observations));
        }
        let target = if team {
            let mut definition = crate::agent::subagent::SubagentBuilder::new("depth-team")
                .team(crate::agent::subagent::TeamSpec {
                    strategy: crate::agent::subagent::TeamStrategy::Pipeline(vec![
                        "depth-2".into(),
                    ]),
                    manager: String::new(),
                    subagents: Vec::new(),
                    config: crate::agent::subagent::TeamConfig::default(),
                })
                .build();
            if readonly {
                definition.access_mode = crate::agent::subagent::SubagentAccessMode::ReadOnly;
            }
            registry
                .register(definition, Box::new(MockAgent::new("depth-team")))
                .await;
            "depth-team"
        } else {
            "depth-1"
        };
        let executor = Arc::new(SubagentExecutor::new(
            Arc::clone(&registry),
            SubagentExecutorConfig::default(),
        ));
        let cancellation = CancellationToken::new();
        let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel();
        let dispatch = if readonly {
            AgentDispatchTool::read_only(executor, "primary", cancellation.clone())
        } else {
            AgentDispatchTool::new(executor, "primary", cancellation.clone())
        };
        let result = dispatch
            .execute_with_context(
                [
                    ("agent_name".into(), json!(target)),
                    ("task".into(), json!("bounded chain")),
                    ("mode".into(), json!(if team { "team" } else { mode })),
                    ("background".into(), json!(background)),
                ]
                .into_iter()
                .collect(),
                &ToolContext {
                    // 沿原 background effect sink 等其原 handle.join()，不拿 started/事件当EOF。
                    effect_sink: background.then(|| {
                        let sender = effect_tx.clone();
                        let root_target = target.to_string();
                        Arc::new(move |effect| {
                            let sender = sender.clone();
                            let root_target = root_target.clone();
                            Box::pin(async move {
                                if matches!(&effect, echo_core::tools::ToolEffect::SubagentRun {
                                    agent_name, task, ..
                                } if agent_name == &root_target && task == "bounded chain")
                                {
                                    let _ = sender.send(effect);
                                }
                            })
                                as futures::future::BoxFuture<'static, ()>
                        }) as echo_core::tools::ToolEffectSinkFn
                    }),
                    tool_access: readonly.then_some(ToolAccess::ReadOnly),
                    tool_visibility: readonly.then(|| {
                        Arc::new(echo_core::tools::ToolVisibilityState::new(
                            ["agent_tool", "depth_probe", "tool_search"]
                                .into_iter()
                                .map(str::to_string)
                                .collect(),
                            std::collections::HashSet::new(),
                        ))
                    }),
                    delegation_policy: Some(NestedDelegationPolicy {
                        can_spawn_subagents: true,
                        delegate_depth: 0,
                        max_delegate_depth: maximum,
                    }),
                    ..Default::default()
                },
            )
            .await?;
        drop(effect_tx);
        assert_eq!(result.success, !team || maximum > 1);
        if background {
            // 复用本模块原 background_effect_settles_once 控制的3秒fixture界限。
            // 超时只取消原token，仍等同一原sink（其后已真实join），不遗留后台source。
            let (late, effect) =
                match tokio::time::timeout(std::time::Duration::from_secs(3), effect_rx.recv())
                    .await
                {
                    Ok(effect) => (false, effect),
                    Err(_) => {
                        cancellation.cancel();
                        (true, effect_rx.recv().await)
                    }
                };
            let effect = effect.ok_or_else(|| {
                crate::error::ReactError::Other(
                    "original background result sink closed without physical settlement".into(),
                )
            })?;
            assert!(
                !late,
                "original background settled only after its existing fixture deadline"
            );
            assert_eq!(
                effect,
                echo_core::tools::ToolEffect::SubagentRun {
                    agent_name: target.to_string(),
                    task: "bounded chain".into(),
                    outcome: "completed".into(),
                }
            );
            assert!(
                effect_rx.try_recv().is_err(),
                "original background result delivered twice"
            );
        }
        let first_concrete_depth = if team { 2 } else { 1 };
        for (depth, name, requests, observations) in &actors {
            let observed = observations
                .lock()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
            let requests = requests
                .lock()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
            if *depth < first_concrete_depth || *depth > maximum {
                assert!(observed.is_empty(), "unexpected actor {name} in {mode}");
                assert!(requests.is_empty());
                continue;
            }
            assert_eq!(observed.len(), 1);
            let actual = observed.first().ok_or_else(|| {
                crate::error::ReactError::Other("actual policy probe missing".into())
            })?;
            if readonly {
                assert_eq!(actual.access, Some(ToolAccess::ReadOnly));
            }
            assert_eq!(
                actual.policy,
                Some(NestedDelegationPolicy {
                    can_spawn_subagents: true,
                    delegate_depth: *depth,
                    max_delegate_depth: maximum
                }),
                "actor {name}, mode {mode}"
            );
            assert!(!requests.is_empty());
            let can_delegate = *depth < maximum;
            assert!(
                requests
                    .iter()
                    .all(|tools| tools.iter().any(|tool| tool == "agent_tool") == can_delegate),
                "model schema disagrees for {name}/{mode}"
            );
            if !can_delegate {
                let available = actual.available.as_ref().ok_or_else(|| {
                    crate::error::ReactError::Other("nondelegating availability missing".into())
                })?;
                assert!(!available.iter().any(|tool| tool == "agent_tool"));
            }
        }
        let mut started = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let crate::agent::subagent::SubagentEvent::DispatchStarted { agent, .. } =
                event.as_ref()
            {
                started.push(agent.clone());
            }
        }
        let expected = (first_concrete_depth..=maximum)
            .map(|depth| format!("depth-{depth}"))
            .collect::<Vec<_>>();
        for (_, name, _, _) in &actors {
            assert_eq!(
                started.iter().filter(|agent| *agent == name).count(),
                usize::from(expected.contains(name)),
                "forced stale call created an extra dispatch for {name}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn readonly_background_fork_keeps_actual_depth_and_scope_until_original_join()
    -> crate::error::Result<()> {
        for maximum in [1, 2, 3, 4] {
            assert_actual_depth_chain_with_background("fork", maximum, false, true, true).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn direct_dispatch_stamps_actual_child_policy_once() -> crate::error::Result<()> {
        for mode in ["sync", "fork", "teammate"] {
            for maximum in [1, 2, 3, 4] {
                assert_actual_depth_chain(mode, maximum, false, false).await?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn team_member_keeps_its_original_target_depth() -> crate::error::Result<()> {
        for maximum in [1, 2, 3, 4] {
            assert_actual_depth_chain("sync", maximum, true, false).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn readonly_model_dispatch_keeps_actual_depth_and_access_in_each_mode()
    -> crate::error::Result<()> {
        for maximum in [1, 2, 3, 4] {
            for mode in ["sync", "fork", "teammate"] {
                assert_actual_depth_chain(mode, maximum, false, true).await?;
            }
            assert_actual_depth_chain("sync", maximum, true, true).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn primary_explicit_policy_masks_model_search_and_forced_dispatch()
    -> crate::error::Result<()> {
        use futures::StreamExt;
        // 真实 primary Agent 的当前 value-scoped policy 与 hard scope；
        // 旧共享 setter 优先级两分支已退役，不移植为第二份 policy 权威。
        for (policy, hard_scope_only_dispatch) in [
            (Some(false), false),
            (None, false),
            (Some(true), false),
            (Some(false), true),
        ] {
            let registry = Arc::new(SubagentRegistry::new());
            let child = MockAgent::new("child").with_response("child complete");
            registry
                .register(
                    SubagentDefinition::new("child", "child"),
                    Box::new(child.clone()),
                )
                .await;
            let mut events = registry.event_bus().subscribe();
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let observations = Arc::new(std::sync::Mutex::new(Vec::new()));
            let agent = crate::agent::ReactAgentBuilder::new()
                .name("actual-primary")
                .model("mock-model")
                .enable_tools()
                .enable_subagent()
                .register_agent_dispatch_tool()
                .subagent_registry(registry)
                .llm_client(Arc::new(ScopeRequestClient {
                    inner: crate::testing::MockLlmClient::new()
                        .then_tool_call("primary-probe", "depth_probe", "{}")
                        .then_tool_call(
                            "primary-search",
                            "tool_search",
                            r#"{"query":"agent_tool"}"#,
                        )
                        .then_tool_call(
                            "primary-forced",
                            "agent_tool",
                            r#"{"agent_name":"child","task":"forced"}"#,
                        )
                        .with_response("## Summary\nprimary complete"),
                    requests: Arc::clone(&requests),
                }))
                .tool(Box::new(DepthProbeTool {
                    inner: ScopeProbeTool {
                        name: "depth_probe",
                        calls: Arc::new(AtomicUsize::new(0)),
                    },
                    observations: Arc::clone(&observations),
                }))
                .build()?;
            let stopped = NestedDelegationPolicy {
                can_spawn_subagents: false,
                delegate_depth: 0,
                max_delegate_depth: 3,
            };
            let actual_policy = policy.map(|enabled| {
                if enabled {
                    NestedDelegationPolicy {
                        can_spawn_subagents: true,
                        delegate_depth: 0,
                        max_delegate_depth: 1,
                    }
                } else {
                    stopped
                }
            });
            let mut stream = crate::agent::Agent::execute_stream_message_with_invocation_context(
                &agent,
                Message::user("primary bounded turn".into()),
                CancellationToken::new(),
                echo_core::agent::AgentInvocationContext {
                    runtime: Some(echo_core::tools::ExternalRunContext {
                        delegation_policy: actual_policy,
                        ..Default::default()
                    }),
                    allowed_tools: hard_scope_only_dispatch
                        .then(|| std::collections::HashSet::from(["agent_tool".into()])),
                    ..Default::default()
                },
            )
            .await?;
            while let Some(event) = stream.next().await {
                event?;
            }
            let can_delegate = actual_policy.is_none_or(|policy| policy.can_delegate());
            let requests = requests
                .lock()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
            assert!(!requests.is_empty());
            assert!(
                requests
                    .iter()
                    .all(|tools| tools.iter().any(|name| name == "agent_tool") == can_delegate)
            );
            if !can_delegate {
                assert!(
                    requests
                        .iter()
                        .all(|tools| !tools.iter().any(|name| name == "tool_search"))
                );
            }
            let observed = observations
                .lock()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
            if hard_scope_only_dispatch {
                assert!(observed.is_empty());
                assert!(requests.iter().all(Vec::is_empty));
            } else {
                assert_eq!(observed.len(), 1);
                assert_eq!(observed.first().and_then(|item| item.policy), actual_policy);
            }
            assert_eq!(child.invocation_contexts().len(), usize::from(can_delegate));
            let mut starts = 0usize;
            while let Ok(event) = events.try_recv() {
                if matches!(
                    event.as_ref(),
                    crate::agent::subagent::SubagentEvent::DispatchStarted { .. }
                ) {
                    starts = starts.saturating_add(1);
                }
            }
            assert_eq!(starts, usize::from(can_delegate));
        }
        Ok(())
    }

    struct QueuedScopeAgent {
        inner: crate::agent::ReactAgent,
        entered: Arc<tokio::sync::Notify>,
    }

    impl crate::agent::Agent for QueuedScopeAgent {
        fn name(&self) -> &str {
            crate::agent::Agent::name(&self.inner)
        }
        fn model_name(&self) -> &str {
            crate::agent::Agent::model_name(&self.inner)
        }
        fn system_prompt(&self) -> &str {
            crate::agent::Agent::system_prompt(&self.inner)
        }
        fn tool_names(&self) -> Vec<String> {
            crate::agent::Agent::tool_names(&self.inner)
        }
        fn tool_definitions(&self) -> Vec<echo_core::llm::ToolDefinition> {
            crate::agent::Agent::tool_definitions(&self.inner)
        }
        fn register_tool(&self, tool: Box<dyn Tool>) {
            crate::agent::Agent::register_tool(&self.inner, tool);
        }
        fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, crate::error::Result<String>> {
            crate::agent::Agent::execute(&self.inner, task)
        }
        fn execute_stream<'a>(
            &'a self,
            task: &'a str,
        ) -> BoxFuture<
            'a,
            crate::error::Result<
                futures::stream::BoxStream<'a, crate::error::Result<crate::agent::AgentEvent>>,
            >,
        > {
            crate::agent::Agent::execute_stream(&self.inner, task)
        }
        fn execute_stream_with_invocation_context<'a>(
            &'a self,
            task: &'a str,
            cancel: CancellationToken,
            invocation: echo_core::agent::AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            crate::error::Result<
                futures::stream::BoxStream<'a, crate::error::Result<crate::agent::AgentEvent>>,
            >,
        > {
            Box::pin(async move {
                self.entered.notify_one();
                crate::agent::Agent::execute_stream_with_invocation_context(
                    &self.inner,
                    task,
                    cancel,
                    invocation,
                )
                .await
            })
        }
        fn execute_stream_message_with_invocation_context<'a>(
            &'a self,
            message: Message,
            cancel: CancellationToken,
            invocation: echo_core::agent::AgentInvocationContext,
        ) -> BoxFuture<
            'a,
            crate::error::Result<
                futures::stream::BoxStream<'a, crate::error::Result<crate::agent::AgentEvent>>,
            >,
        > {
            Box::pin(async move {
                self.entered.notify_one();
                crate::agent::Agent::execute_stream_message_with_invocation_context(
                    &self.inner,
                    message,
                    cancel,
                    invocation,
                )
                .await
            })
        }
    }

    #[tokio::test]
    async fn queued_subagent_keeps_hard_scope_when_a_new_tool_is_registered()
    -> crate::error::Result<()> {
        use std::collections::HashSet;
        for mode in ["sync", "fork", "teammate", "team"] {
            let registry = Arc::new(SubagentRegistry::new());
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let writes = Arc::new(AtomicUsize::new(0));
            let entered = Arc::new(tokio::sync::Notify::new());
            let agent = crate::agent::ReactAgentBuilder::new()
                .name("queued-scope")
                .model("mock-model")
                .enable_tools()
                .llm_client(Arc::new(ScopeRequestClient {
                    inner: crate::testing::MockLlmClient::new()
                        .then_tool_call(
                            "search-late",
                            "tool_search",
                            r#"{"query":"late_scope_write"}"#,
                        )
                        .then_tool_call("write-late", "late_scope_write", "{}")
                        .with_response("## Summary\ncomplete"),
                    requests: Arc::clone(&requests),
                }))
                .tool(Box::new(ScopeProbeTool {
                    name: "read_file",
                    calls: Arc::new(AtomicUsize::new(0)),
                }))
                .build()?;
            let original_execution_gate = Arc::clone(&agent.execution_mutex).lock_owned().await;
            registry
                .register(
                    SubagentDefinition::new("queued-scope", "shared queued child"),
                    Box::new(QueuedScopeAgent {
                        inner: agent,
                        entered: Arc::clone(&entered),
                    }),
                )
                .await;
            let target = if mode == "team" {
                let definition = crate::agent::subagent::SubagentBuilder::new("queued-team")
                    .team(crate::agent::subagent::TeamSpec {
                        strategy: crate::agent::subagent::TeamStrategy::Pipeline(vec![
                            "queued-scope".into(),
                        ]),
                        manager: String::new(),
                        subagents: Vec::new(),
                        config: crate::agent::subagent::TeamConfig::default(),
                    })
                    .build();
                registry
                    .register(definition, Box::new(MockAgent::new("queued-team")))
                    .await;
                "queued-team"
            } else {
                "queued-scope"
            };
            let executor = Arc::new(SubagentExecutor::new(
                Arc::clone(&registry),
                SubagentExecutorConfig::default(),
            ));
            let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
            let context = ToolContext {
                tool_visibility: Some(Arc::new(echo_core::tools::ToolVisibilityState::new(
                    HashSet::from([
                        "read_file".into(),
                        "agent_tool".into(),
                        "tool_search".into(),
                    ]),
                    HashSet::from(["agent_tool".into()]),
                ))),
                ..Default::default()
            };
            let dispatch = tokio::spawn(async move {
                tool.execute_with_context(
                    [
                        ("agent_name".into(), json!(target)),
                        ("task".into(), json!("constrained queued turn")),
                        ("mode".into(), json!(mode)),
                    ]
                    .into_iter()
                    .collect(),
                    &context,
                )
                .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                .await
                .map_err(|error| {
                    crate::error::ReactError::Other(format!(
                        "scope never reached original queue: {error}"
                    ))
                })?;
            assert!(
                requests
                    .lock()
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))?
                    .is_empty()
            );
            let shared = registry
                .get_agent("queued-scope")
                .await
                .ok_or_else(|| crate::error::ReactError::Other("shared agent missing".into()))?;
            shared.register_tool(Box::new(ScopeProbeTool {
                name: "late_scope_write",
                calls: Arc::clone(&writes),
            }));
            drop(original_execution_gate);
            let result = dispatch
                .await
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))??;
            assert!(result.success);
            let observed = requests
                .lock()
                .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
            assert!(observed.len() >= 2);
            assert!(
                observed
                    .iter()
                    .all(|tools| !tools.iter().any(|name| name == "late_scope_write"))
            );
            assert_eq!(
                writes.load(Ordering::SeqCst),
                0,
                "late tool executed in {mode}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn inherited_available_scope_reaches_real_model_and_tool_pipeline()
    -> crate::error::Result<()> {
        use std::collections::HashSet;
        for mode in ["sync", "fork", "teammate", "team"] {
            for zero_role_matches in [false, true] {
                let registry = Arc::new(SubagentRegistry::new());
                let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
                let writes = Arc::new(AtomicUsize::new(0));
                let client = Arc::new(ScopeRequestClient {
                    inner: crate::testing::MockLlmClient::new()
                        .then_tool_call("outside", "write_file", "{}")
                        .with_response("## Summary\ncomplete"),
                    requests: Arc::clone(&requests),
                });
                let mut builder = crate::agent::ReactAgentBuilder::new()
                    .name("scoped-child")
                    .model("mock-model")
                    .enable_tools()
                    .llm_client(client)
                    .tool(Box::new(ScopeProbeTool {
                        name: "write_file",
                        calls: Arc::clone(&writes),
                    }));
                if !zero_role_matches {
                    builder = builder.tool(Box::new(ScopeProbeTool {
                        name: "read_file",
                        calls: Arc::new(AtomicUsize::new(0)),
                    }));
                }
                registry
                    .register(
                        SubagentDefinition::new("scoped-child", "scoped child"),
                        Box::new(builder.build()?),
                    )
                    .await;
                let target = if mode == "team" {
                    let definition = crate::agent::subagent::SubagentBuilder::new("scoped-team")
                        .team(crate::agent::subagent::TeamSpec {
                            strategy: crate::agent::subagent::TeamStrategy::Pipeline(vec![
                                "scoped-child".into(),
                            ]),
                            manager: String::new(),
                            subagents: Vec::new(),
                            config: crate::agent::subagent::TeamConfig::default(),
                        })
                        .build();
                    registry
                        .register(definition, Box::new(MockAgent::new("scoped-team")))
                        .await;
                    "scoped-team"
                } else {
                    "scoped-child"
                };
                let executor = Arc::new(SubagentExecutor::new(
                    registry,
                    SubagentExecutorConfig::default(),
                ));
                let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
                // Use an absent name for the zero-match case even when the
                // all-feature StandardToolPack registers read_file itself.
                let allowed_role_tool = if zero_role_matches {
                    "absent_scope_tool"
                } else {
                    "read_file"
                };
                let context = ToolContext {
                    tool_visibility: Some(Arc::new(
                        echo_core::tools::ToolVisibilityState::with_available(
                            HashSet::from([
                                allowed_role_tool.to_string(),
                                "agent_tool".to_string(),
                            ]),
                            HashSet::from([
                                allowed_role_tool.to_string(),
                                "agent_tool".to_string(),
                            ]),
                            HashSet::from(["agent_tool".to_string()]),
                        ),
                    )),
                    ..Default::default()
                };
                let params = [
                    ("agent_name".into(), json!(target)),
                    ("task".into(), json!("bounded task 中文 👋")),
                    ("mode".into(), json!(mode)),
                ]
                .into_iter()
                .collect();
                let result = tool.execute_with_context(params, &context).await?;
                assert!(result.success, "mode={mode}, result={result:?}");
                let observed = requests
                    .lock()
                    .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
                assert!(!observed.is_empty());
                assert!(
                    observed
                        .iter()
                        .all(|tools| !tools.iter().any(|name| name == "write_file"))
                );
                let first = observed.first().ok_or_else(|| {
                    crate::error::ReactError::Other("model request missing".into())
                })?;
                assert_eq!(
                    first.iter().any(|name| name == "read_file"),
                    !zero_role_matches
                );
                if zero_role_matches {
                    assert!(observed.iter().all(Vec::is_empty));
                }
                assert_eq!(
                    writes.load(Ordering::SeqCst),
                    0,
                    "stale tool call escaped in {mode}"
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn cached_dispatch_outside_parent_hard_scope_is_rejected() -> crate::error::Result<()> {
        let registry = Arc::new(SubagentRegistry::new());
        let child = MockAgent::new("child").with_response("must not execute");
        registry
            .register(
                SubagentDefinition::new("child", "child"),
                Box::new(child.clone()),
            )
            .await;
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig::default(),
        ));
        let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
        let context = ToolContext {
            tool_visibility: Some(Arc::new(echo_core::tools::ToolVisibilityState::new(
                std::collections::HashSet::from(["read_file".into()]),
                std::collections::HashSet::from(["read_file".into()]),
            ))),
            ..Default::default()
        };
        let result = tool
            .execute_with_context(
                [
                    ("agent_name".into(), json!("child")),
                    ("task".into(), json!("must reject")),
                ]
                .into_iter()
                .collect(),
                &context,
            )
            .await?;
        assert!(!result.success);
        assert!(child.invocation_contexts().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn nested_fresh_dispatch_keeps_scope_after_factory_and_new_role()
    -> crate::error::Result<()> {
        use std::collections::HashSet;
        let registry = Arc::new(SubagentRegistry::new());
        let leaf_requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writes = Arc::new(AtomicUsize::new(0));
        let leaf = crate::agent::ReactAgentBuilder::new()
            .name("leaf")
            .model("mock-model")
            .enable_tools()
            .llm_client(Arc::new(ScopeRequestClient {
                inner: crate::testing::MockLlmClient::new()
                    .then_tool_call("outside-leaf", "write_file", "{}")
                    .with_response("## Summary\nleaf complete"),
                requests: Arc::clone(&leaf_requests),
            }))
            .tool(Box::new(ScopeProbeTool {
                name: "read_file",
                calls: Arc::new(AtomicUsize::new(0)),
            }))
            .tool(Box::new(ScopeProbeTool {
                name: "write_file",
                calls: Arc::clone(&writes),
            }))
            .build()?;
        registry
            .register(
                SubagentDefinition::new("leaf", "new leaf role"),
                Box::new(leaf),
            )
            .await;
        let child = crate::agent::ReactAgentBuilder::new()
            .name("branch")
            .model("mock-model")
            .enable_tools()
            .enable_subagent()
            .register_agent_dispatch_tool()
            .subagent_registry(Arc::clone(&registry))
            .llm_client(Arc::new(
                crate::testing::MockLlmClient::new()
                    .then_tool_call(
                        "nested",
                        "agent_tool",
                        r#"{"agent_name":"leaf","task":"bounded leaf"}"#,
                    )
                    .with_response("## Summary\nbranch complete"),
            ))
            .tool(Box::new(ScopeProbeTool {
                name: "read_file",
                calls: Arc::new(AtomicUsize::new(0)),
            }))
            .tool(Box::new(ScopeProbeTool {
                name: "write_file",
                calls: Arc::clone(&writes),
            }))
            .build()?;
        registry
            .register(
                SubagentDefinition::new("branch", "delegating branch"),
                Box::new(child),
            )
            .await;
        let executor = Arc::new(SubagentExecutor::new(
            registry,
            SubagentExecutorConfig::default(),
        ));
        let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
        let context = ToolContext {
            tool_visibility: Some(Arc::new(
                echo_core::tools::ToolVisibilityState::with_available(
                    HashSet::from(["read_file".into(), "agent_tool".into()]),
                    HashSet::from(["read_file".into(), "agent_tool".into()]),
                    HashSet::from(["agent_tool".into()]),
                ),
            )),
            delegation_policy: Some(NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 0,
                max_delegate_depth: 4,
            }),
            ..Default::default()
        };
        let result = tool
            .execute_with_context(
                [
                    ("agent_name".into(), json!("branch")),
                    ("task".into(), json!("fresh nested scope")),
                ]
                .into_iter()
                .collect(),
                &context,
            )
            .await?;
        assert!(result.success);
        let observed = leaf_requests
            .lock()
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let first = observed
            .first()
            .ok_or_else(|| crate::error::ReactError::Other("leaf never reached model".into()))?;
        assert!(first.iter().any(|name| name == "read_file"));
        assert!(
            observed
                .iter()
                .all(|tools| !tools.iter().any(|name| name == "write_file"))
        );
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        Ok(())
    }

    struct BackgroundGuardDrop(Arc<AtomicUsize>);

    impl Drop for BackgroundGuardDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn synchronous_parent_result_preserves_structured_contract() -> Result<(), String> {
        let outcome = SubagentOutcome {
            contract_version: 2,
            status: crate::agent::subagent::SubagentStatus::TimedOut,
            summary: "partial result".to_string(),
            artifacts: Vec::new(),
            evidence: vec![crate::agent::subagent::SubagentEvidence {
                kind: "file_read".to_string(),
                subject: "src/lib.rs".to_string(),
                outcome: Some("succeeded".to_string()),
                details: String::new(),
                source: crate::agent::subagent::SubagentEvidenceSource::Observed,
                attributes: serde_json::Value::Null,
            }],
            verification: Vec::new(),
            remaining_work: vec!["finish verification".to_string()],
            touched_files: crate::agent::subagent::SubagentTouchedFiles::default(),
        };
        let serialized = serialize_parent_result(&outcome).map_err(|error| error.to_string())?;
        let decoded: SubagentOutcome =
            serde_json::from_str(&serialized).map_err(|error| error.to_string())?;

        assert_eq!(decoded, outcome);
        Ok(())
    }

    #[test]
    fn default_dispatch_policy_starts_at_root() {
        let policy = AgentDispatchTool::delegation_policy_from_context(None).unwrap_or_default();
        assert!(policy.can_spawn_subagents);
        assert_eq!(policy.delegate_depth, 0);
        assert_eq!(policy.max_delegate_depth, 3);
    }

    #[test]
    fn context_dispatch_policy_advances_to_child() {
        let ctx = ToolContext {
            delegation_policy: Some(NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 1,
                max_delegate_depth: 2,
            }),
            ..Default::default()
        };
        let policy =
            AgentDispatchTool::delegation_policy_from_context(Some(&ctx)).unwrap_or_default();
        assert_eq!(policy.delegate_depth, 2);
        assert!(!policy.can_delegate());
    }

    #[test]
    fn context_dispatch_policy_rejects_exhausted_depth() {
        let ctx = ToolContext {
            delegation_policy: Some(NestedDelegationPolicy {
                can_spawn_subagents: true,
                delegate_depth: 2,
                max_delegate_depth: 2,
            }),
            ..Default::default()
        };
        assert!(AgentDispatchTool::delegation_policy_from_context(Some(&ctx)).is_err());
    }

    #[test]
    fn runtime_context_none_without_tool_ctx() {
        assert!(AgentDispatchTool::runtime_context_from_tool_ctx(None).is_none());
    }

    #[test]
    fn runtime_context_none_without_run_id() {
        let ctx = ToolContext::default();
        assert!(AgentDispatchTool::runtime_context_from_tool_ctx(Some(&ctx)).is_none());
    }

    #[test]
    fn runtime_context_preserves_guards_without_run_or_turn_id() -> Result<(), String> {
        let ctx = ToolContext {
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                "anonymous-lease".to_string(),
            )],
            ..ToolContext::default()
        };
        let runtime = AgentDispatchTool::runtime_context_from_tool_ctx(Some(&ctx))
            .ok_or_else(|| "guard-only runtime context was dropped".to_string())?;

        assert!(runtime.run_id.is_none());
        assert!(runtime.turn_id.is_none());
        assert_eq!(runtime.resource_guards.len(), 1);
        Ok(())
    }

    #[test]
    fn runtime_context_preserves_conversation_only() -> Result<(), String> {
        let context = ToolContext {
            conversation_id: Some("conversation-only".to_string()),
            ..ToolContext::default()
        };
        let runtime = AgentDispatchTool::runtime_context_from_tool_ctx(Some(&context))
            .ok_or_else(|| "conversation-only ToolContext was dropped".to_string())?;
        assert_eq!(
            runtime.conversation_id.as_deref(),
            Some("conversation-only")
        );
        Ok(())
    }

    #[test]
    fn runtime_context_preserves_cancel_and_trace_only() -> Result<(), String> {
        let cancel = Arc::new(CancellationToken::new());
        let trace: echo_core::tools::TraceSinkFn = Arc::new(|_| {});
        let context = ToolContext {
            cancel: Some(Arc::clone(&cancel)),
            trace_sink: Some(Arc::clone(&trace)),
            ..ToolContext::default()
        };
        let runtime = AgentDispatchTool::runtime_context_from_tool_ctx(Some(&context))
            .ok_or_else(|| "cancel/trace-only ToolContext was dropped".to_string())?;
        assert!(
            runtime
                .cancel
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, &cancel))
        );
        assert!(
            runtime
                .trace_sink
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, &trace))
        );
        Ok(())
    }

    #[test]
    fn runtime_context_from_run_id_pins_message_and_execution_id() -> Result<(), String> {
        let ctx = ToolContext {
            run_id: Some("msg-key-1".to_string()),
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                "lease".to_string(),
            )],
            ..Default::default()
        };
        let rt = AgentDispatchTool::runtime_context_from_tool_ctx(Some(&ctx))
            .ok_or_else(|| "runtime_context missing when run_id is set".to_string())?;
        assert_eq!(rt.run_id.as_deref(), Some("msg-key-1"));
        assert_eq!(rt.message_id.as_deref(), Some("msg-key-1"));
        assert_eq!(rt.resource_guards.len(), 1);
        let exec_id = rt
            .execution_id
            .ok_or_else(|| "execution_id missing from runtime context".to_string())?;
        assert!(
            exec_id.starts_with("agent_tool-"),
            "execution_id should be agent_tool-{{uuid}}, got {exec_id}"
        );
        assert!(
            !exec_id.contains(':'),
            "execution_id must not contain ':' so bridge uses full id as subagent_run_id"
        );
        Ok(())
    }

    #[tokio::test]
    async fn invocation_cancel_token_overrides_stale_shared_handle() -> Result<(), String> {
        let stale_parent = CancellationToken::new();
        let invocation_parent = CancellationToken::new();
        let shared = Arc::new(tokio::sync::Mutex::new(Some(stale_parent.clone())));
        let ctx = ToolContext {
            cancel: Some(Arc::new(invocation_parent.clone())),
            ..Default::default()
        };

        let child = AgentDispatchTool::child_cancel_token(&shared, ctx.cancel.as_deref()).await;
        stale_parent.cancel();
        if child.is_cancelled() {
            return Err("stale shared token cancelled the invocation child".to_string());
        }

        invocation_parent.cancel();
        if !child.is_cancelled() {
            return Err("invocation cancellation did not reach the subagent child".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn anonymous_background_dispatch_retains_guard_until_subagent_settles()
    -> Result<(), String> {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = Arc::new(SubagentExecutor::new(
            Arc::clone(&registry),
            SubagentExecutorConfig::default(),
        ));
        let agent = MockAgent::new("background").with_response("## Summary\ncomplete");
        registry
            .register(
                SubagentDefinition::new("background", "Background role"),
                Box::new(agent.clone()),
            )
            .await;
        let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
        let drops = Arc::new(AtomicUsize::new(0));
        let context = ToolContext {
            resource_guards: vec![echo_core::tools::InvocationResourceGuard::new(
                BackgroundGuardDrop(Arc::clone(&drops)),
            )],
            ..ToolContext::default()
        };
        let parameters: ToolParameters = [
            (
                "agent_name".to_string(),
                Value::String("background".to_string()),
            ),
            ("task".to_string(), Value::String("continue".to_string())),
            ("background".to_string(), Value::Bool(true)),
        ]
        .into_iter()
        .collect();

        let started = tool
            .execute_with_context(parameters, &context)
            .await
            .map_err(|error| error.to_string())?;
        if !started.success {
            return Err(format!("background dispatch failed to start: {started:?}"));
        }
        drop(context);
        assert_eq!(drops.load(Ordering::SeqCst), 0);

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while agent.invocation_contexts().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "background invocation did not reach the subagent".to_string())?;
        let invocations = agent.take_invocation_contexts();
        let guard_count = invocations
            .first()
            .and_then(|invocation| invocation.runtime.as_ref())
            .map(|runtime| runtime.resource_guards.len());
        if guard_count != Some(1) {
            return Err(format!(
                "background subagent lost anonymous guards: {guard_count:?}"
            ));
        }
        drop(invocations);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while drops.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| "background guard outlived subagent settlement".to_string())?;
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[tokio::test]
    async fn background_effect_settles_once_after_start_for_success_failure_and_cancel()
    -> Result<(), String> {
        for expected in ["completed", "failed", "cancelled"] {
            let registry = Arc::new(SubagentRegistry::new());
            let executor = Arc::new(SubagentExecutor::new(
                Arc::clone(&registry),
                SubagentExecutorConfig::default(),
            ));
            if expected == "failed" {
                registry
                    .register(
                        SubagentDefinition::new("child", "Failure"),
                        Box::new(
                            FailingMockAgent::new("child", "failed execution")
                                .with_failure(MockAgentFailure::Subagent),
                        ),
                    )
                    .await;
            } else {
                registry
                    .register(
                        SubagentDefinition::new("child", "Success or cancellation"),
                        Box::new(
                            MockAgent::new("child")
                                .with_delay_ms(if expected == "cancelled" { 1_000 } else { 30 })
                                .with_response("## Summary\ncomplete"),
                        ),
                    )
                    .await;
            }
            let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
            let cancellation = Arc::new(CancellationToken::new());
            let (effect_tx, mut effect_rx) = tokio::sync::mpsc::unbounded_channel();
            let context = ToolContext {
                call_id: Some(format!("call-{expected}")),
                run_id: Some(format!("run-{expected}")),
                cancel: Some(Arc::clone(&cancellation)),
                effect_sink: Some(Arc::new(move |effect| {
                    let effect_tx = effect_tx.clone();
                    Box::pin(async move {
                        let _ = effect_tx.send(effect);
                    })
                })),
                ..ToolContext::default()
            };
            let parameters: ToolParameters = [
                ("agent_name".to_string(), Value::String("child".to_string())),
                ("task".to_string(), Value::String("task".to_string())),
                ("background".to_string(), Value::Bool(true)),
            ]
            .into_iter()
            .collect();

            let started = tool
                .execute_with_context(parameters, &context)
                .await
                .map_err(|error| error.to_string())?;
            if !started.success || !started.effects.is_empty() {
                return Err(format!("background launch is not an effect: {started:?}"));
            }
            if expected == "cancelled" {
                cancellation.cancel();
            }
            let effect = tokio::time::timeout(std::time::Duration::from_secs(3), effect_rx.recv())
                .await
                .map_err(|_| format!("{expected} effect did not settle"))?
                .ok_or_else(|| format!("{expected} effect sink disconnected"))?;
            if effect
                != (echo_core::tools::ToolEffect::SubagentRun {
                    agent_name: "child".to_string(),
                    task: "task".to_string(),
                    outcome: expected.to_string(),
                })
            {
                return Err(format!("wrong {expected} effect: {effect:?}"));
            }
            if effect_rx.try_recv().is_ok() {
                return Err(format!("duplicate {expected} terminal effect"));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn cached_schema_tracks_shared_registry_revision() -> Result<(), String> {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = Arc::new(SubagentExecutor::new(
            Arc::clone(&registry),
            SubagentExecutorConfig::default(),
        ));
        let manager = crate::tools::ToolManager::new();
        manager.register(Box::new(AgentDispatchTool::new(
            executor,
            "parent",
            CancellationToken::new(),
        )));

        let initial = manager.get_tool_definitions();
        let initial_schema = initial
            .iter()
            .find(|definition| definition.function.name == "agent_tool")
            .map(|definition| &definition.function.parameters);
        assert!(initial_schema.is_some());

        registry
            .register_definition(SubagentDefinition::new("researcher", "Pending role"))
            .await;
        let pending_schema = manager.get_tool_definitions();
        let pending = pending_schema
            .iter()
            .find(|definition| definition.function.name == "agent_tool")
            .map(|definition| &definition.function.parameters);
        assert!(pending.is_some_and(|parameters| {
            parameters.pointer("/properties/agent_name/enum").is_none()
                && parameters
                    .pointer("/properties/agent_name/description")
                    .and_then(Value::as_str)
                    .is_some_and(|description| !description.contains("researcher"))
        }));

        registry
            .register(
                SubagentDefinition::new("researcher", "Research role"),
                Box::new(MockAgent::new("researcher")),
            )
            .await;

        let refreshed = manager.get_tool_definitions();
        let names = refreshed
            .iter()
            .find(|definition| definition.function.name == "agent_tool")
            .and_then(|definition| {
                definition
                    .function
                    .parameters
                    .pointer("/properties/agent_name/enum")
            })
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(names, vec![Value::String("researcher".to_string())]);

        registry.remove("researcher").await;
        let removed_schema = manager.get_tool_definitions();
        let removed = removed_schema
            .iter()
            .find(|definition| definition.function.name == "agent_tool")
            .map(|definition| &definition.function.parameters);
        assert!(removed.is_some_and(|parameters| {
            parameters.pointer("/properties/agent_name/enum").is_none()
                && parameters
                    .pointer("/properties/agent_name/description")
                    .and_then(Value::as_str)
                    .is_some_and(|description| !description.contains("researcher"))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn active_message_attachments_reach_dispatched_subagent() -> Result<(), String> {
        let registry = Arc::new(SubagentRegistry::new());
        let executor = Arc::new(SubagentExecutor::new(
            Arc::clone(&registry),
            SubagentExecutorConfig::default(),
        ));
        let agent = MockAgent::new("reader").with_response("## Summary\nread");
        registry
            .register(
                SubagentDefinition::new("reader", "Reads attachments"),
                Box::new(agent.clone()),
            )
            .await;
        let tool = AgentDispatchTool::new(executor, "parent", CancellationToken::new());
        let message = Message::user_multimodal(vec![
            ContentPart::Text {
                text: "inspect this".to_string(),
            },
            ContentPart::File {
                name: "notes.txt".to_string(),
                content: "aGVsbG8=".to_string(),
            },
        ]);
        let ctx = ToolContext {
            active_message: Some(message),
            ..Default::default()
        };
        let parameters: ToolParameters = [
            (
                "agent_name".to_string(),
                Value::String("reader".to_string()),
            ),
            ("task".to_string(), Value::String("inspect".to_string())),
        ]
        .into_iter()
        .collect();
        let result = tool
            .execute_with_context(parameters, &ctx)
            .await
            .map_err(|error| error.to_string())?;
        if !result.success {
            return Err(format!("dispatch failed: {}", result.output));
        }
        let received = agent
            .last_message()
            .ok_or_else(|| "subagent did not receive active message".to_string())?;
        let has_file = received.content.parts().is_some_and(|parts| {
            parts
                .iter()
                .any(|part| matches!(part, ContentPart::File { name, .. } if name == "notes.txt"))
        });
        if !has_file {
            return Err("subagent message lost file attachment".to_string());
        }
        Ok(())
    }
}
