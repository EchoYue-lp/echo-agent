use crate::agent::subagent::context::{ContextInheritance, SubagentContext};
use crate::agent::subagent::executor::SubagentExecutor;
use crate::agent::subagent::{ExecutionMode, SubagentOutcome};
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_core::agent::CancellationToken;
use echo_core::tools::{ExternalRunContext, NestedDelegationPolicy, ToolContext};
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
            .filter(|d| d.function.name != "final_answer")
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
        }
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
        if c.run_id.is_none() && c.turn_id.is_none() {
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
        let runtime_context = Self::runtime_context_from_tool_ctx(ctx);
        let active_message = ctx.and_then(|context| context.active_message.clone());
        let invocation_cancel = ctx.and_then(|context| context.cancel.clone());
        let delegation_policy = match Self::delegation_policy_from_context(ctx) {
            Ok(policy) => policy,
            Err(e) => return Box::pin(async move { Ok(ToolResult::error(e)) }),
        };

        Box::pin(async move {
            let agent_name = parameters
                .get("agent_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("agent_name".to_string()))?;

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
            let registered = executor.registry().get(agent_name).await.ok_or_else(|| {
                ToolError::ExecutionFailed {
                    tool: "agent_tool".to_string(),
                    message: format!("Subagent '{agent_name}' is not registered"),
                }
            })?;
            let mut exec_mode = mode_override
                .clone()
                .unwrap_or_else(|| registered.definition.execution_mode.clone());
            let def = &registered.definition;
            let isolation_forces_fork = def.isolate_worktree || def.isolate_workspace;
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

            // Build parent context if factory is available.
            // mode=fork → structured history inheritance; otherwise fresh.
            let parent_context = if let Some(ref f) = factory {
                let ctx = if exec_mode == ExecutionMode::Fork {
                    let inheritance = ContextInheritance {
                        inherit_history: def.inherit_history.or(Some(2)),
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
                constraints,
                background: run_background,
            };

            if run_background {
                match executor.dispatch_background(req).await {
                    Ok(handle) => {
                        info!(
                            target_agent = %handle.agent_name,
                            execution_id = %handle.execution_id,
                            "Background subagent started"
                        );
                        Ok(ToolResult::success(
                            json!({
                                "status": "started",
                                "execution_id": handle.execution_id,
                                "agent_name": handle.agent_name,
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
                match executor.dispatch(req).await {
                    Ok(result) => {
                        info!(target_agent = %agent_name, "Subagent completed successfully");
                        debug!(
                            target_agent = %agent_name,
                            summary = %result.outcome.summary,
                            output_chars = result.output.chars().count(),
                            "Subagent result"
                        );
                        Ok(serialize_parent_result(&result.outcome)
                            .map(ToolResult::success)
                            .unwrap_or_else(|error| {
                                ToolResult::error(format!(
                                    "Subagent '{}' result serialization failed: {}",
                                    agent_name, error
                                ))
                            }))
                    }
                    Err(e) => {
                        warn!(target_agent = %agent_name, error = %e, "Subagent execution failed");
                        Ok(ToolResult::error(format!(
                            "Subagent '{}' execution failed: {}",
                            agent_name, e
                        )))
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

    fn description(&self) -> &str {
        "Dispatch tasks to specialized Subagents in an isolated context. \
         Default is fresh context (no parent conversation). Use mode=fork only when \
         the Subagent needs shared background from this session. Set background=true \
         to start the Subagent without blocking (returns started + execution_id; \
         completion arrives via events / chat note). One call delegates one bounded \
         task. When the host provides a formal task planner, use that planner for \
         coordinated, dependent, or parallel multi-task work. Synchronous completion returns a \
         JSON result with status, summary, artifacts, verification, remaining_work, \
         and touched_files. Use only agent_name values listed in the schema.\n\
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
            .map(|entries| entries.clone())
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
                    "description": "Optional. Omit or \"sync\" = fresh context (recommended; no parent system/history). \"fork\" = inherit parent system prompt + recent messages. Worktree/workspace isolation is automatic for roles that declare it, independent of this field. \"teammate\" = independent background Subagent with a join/cancel handle. \"team\" = execute the role's TeamSpec through the canonical revisioned task DAG."
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
    use crate::testing::MockAgent;
    use echo_core::llm::types::{ContentPart, Message};

    #[test]
    fn synchronous_parent_result_preserves_structured_contract() -> Result<(), String> {
        let outcome = SubagentOutcome {
            contract_version: 1,
            status: crate::agent::subagent::SubagentStatus::TimedOut,
            summary: "partial result".to_string(),
            artifacts: Vec::new(),
            verification: Vec::new(),
            remaining_work: vec!["finish verification".to_string()],
            touched_files: crate::agent::subagent::SubagentTouchedFiles {
                read: vec!["src/lib.rs".to_string()],
                written: Vec::new(),
            },
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
    fn runtime_context_from_run_id_pins_message_and_execution_id() -> Result<(), String> {
        let ctx = ToolContext {
            run_id: Some("msg-key-1".to_string()),
            ..Default::default()
        };
        let rt = AgentDispatchTool::runtime_context_from_tool_ctx(Some(&ctx))
            .ok_or_else(|| "runtime_context missing when run_id is set".to_string())?;
        assert_eq!(rt.run_id.as_deref(), Some("msg-key-1"));
        assert_eq!(rt.message_id.as_deref(), Some("msg-key-1"));
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
