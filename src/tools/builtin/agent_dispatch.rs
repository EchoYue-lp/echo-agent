use crate::agent::subagent::ExecutionMode;
use crate::agent::subagent::context::{ContextInheritance, SubagentContext};
use crate::agent::subagent::executor::SubagentExecutor;
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_core::agent::CancellationToken;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Factory for lazily building parent context at dispatch time.
///
/// Holds Arc references to the parent agent's shared subsystems so
/// that `SubagentContext` can be built with the latest messages and
/// tool definitions when a dispatch actually occurs.
pub struct ParentContextFactory {
    /// Parent's system prompt.
    pub system_prompt: String,
    /// Parent's tool manager (to get current tool definitions).
    pub tool_manager: Arc<crate::tools::ToolManager>,
    /// Parent's context manager (to get recent messages).
    pub context: Arc<tokio::sync::Mutex<crate::compression::ContextManager>>,
    /// Parent's memory store (for sharing with subagent).
    pub store: Option<Arc<dyn crate::memory::Store>>,
}

impl ParentContextFactory {
    /// Build a SubagentContext using the specified inheritance policy.
    pub async fn build(&self, mode: &ExecutionMode) -> SubagentContext {
        let inheritance = ContextInheritance::for_mode(mode);
        let ctx = self.context.lock().await;
        let messages = ctx.messages().to_vec();
        let tool_defs: Vec<_> = self
            .tool_manager
            .get_tool_definitions()
            .into_iter()
            .filter(|d| d.function.name != "final_answer")
            .collect();
        SubagentContext::from_parent(
            &self.system_prompt,
            &tool_defs,
            &messages,
            self.store.clone(),
            &inheritance,
        )
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
    /// When set, subagents in Fork mode inherit conversation history,
    /// system prompt, and tools from the parent agent.
    parent_context_factory: Option<Arc<ParentContextFactory>>,
    /// Snapshot of available subagents exposed to the LLM through the tool schema.
    catalog: Arc<std::sync::RwLock<Vec<SubagentCatalogEntry>>>,
}

/// Compact subagent metadata exposed in `agent_tool` parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentCatalogEntry {
    pub name: String,
    pub description: String,
}

impl AgentDispatchTool {
    pub fn new(
        executor: Arc<SubagentExecutor>,
        parent_agent: impl Into<String>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            executor,
            parent_agent: parent_agent.into(),
            cancel: Arc::new(tokio::sync::Mutex::new(Some(cancel))),
            parent_context_factory: None,
            catalog: Arc::new(std::sync::RwLock::new(Vec::new())),
        }
    }

    /// Create with a parent context factory for context inheritance.
    pub fn with_parent_context(mut self, factory: Arc<ParentContextFactory>) -> Self {
        self.parent_context_factory = Some(factory);
        self
    }

    /// Shared updatable cancel handle for the parent run (P1-11).
    ///
    /// Returns a clone of the inner `Arc<Mutex<..>>`; the caller (the parent
    /// agent) writes the active run's token into it each time a run starts, so
    /// dispatches issued by this tool inherit cancellation.
    pub fn cancel_handle(&self) -> Arc<tokio::sync::Mutex<Option<CancellationToken>>> {
        self.cancel.clone()
    }

    /// Shared catalog handle. The parent agent updates this when subagents are
    /// registered so cached tool definitions can expose concrete worker names.
    pub fn catalog_handle(&self) -> Arc<std::sync::RwLock<Vec<SubagentCatalogEntry>>> {
        self.catalog.clone()
    }
}

impl Tool for AgentDispatchTool {
    fn name(&self) -> &str {
        "agent_tool"
    }

    fn description(&self) -> &str {
        "Dispatch tasks to specialized SubAgents. For complex read-only investigation, architecture review, or validation planning, prefer issuing multiple agent_tool calls in the same assistant turn so independent SubAgents run in parallel. Use only agent_name values listed in the schema."
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
            "No SubAgents are currently registered.".to_string()
        } else {
            let lines: Vec<String> = catalog
                .iter()
                .map(|entry| format!("{}: {}", entry.name, entry.description))
                .collect();
            format!("Available SubAgents: {}", lines.join("; "))
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
                    "description": "Specific task description to assign to the SubAgent. Include relevant paths, scope, constraints, and what result format you need."
                },
                "mode": {
                    "type": "string",
                    "enum": ["sync", "fork", "teammate", "team"],
                    "description": "Execution mode: sync - synchronous wait (default), fork - independent with inherited context, teammate - parallel independent agent, team - multi-agent ManagerWorker (plan→fan-out→synthesize, requires the named subagent to have a TeamSpec)"
                }
            },
            "required": ["agent_name", "task"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let executor = self.executor.clone();
        let parent_agent = self.parent_agent.clone();
        let cancel_handle = self.cancel.clone();
        let factory = self.parent_context_factory.clone();

        Box::pin(async move {
            let agent_name = parameters
                .get("agent_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("agent_name".to_string()))?;

            let task = parameters
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("task".to_string()))?;

            let mode_override =
                parameters
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .and_then(|m| match m {
                        "sync" => Some(ExecutionMode::Sync),
                        "fork" => Some(ExecutionMode::Fork),
                        "teammate" => Some(ExecutionMode::Teammate),
                        "team" => Some(ExecutionMode::Team),
                        _ => None,
                    });

            info!(
                target_agent = %agent_name,
                task = %task,
                mode = ?mode_override,
                "Dispatching task to subagent via SubagentExecutor"
            );

            // Build parent context if factory is available
            let parent_context = if let Some(ref f) = factory {
                let effective_mode = mode_override.clone().unwrap_or(ExecutionMode::Sync);
                let ctx = f.build(&effective_mode).await;
                if ctx.has_content() { Some(ctx) } else { None }
            } else {
                None
            };

            // Derive the subagent's cancel token from the parent run's current
            // token (P1-11). If the parent run hasn't started (no token set),
            // fall back to a fresh token so dispatch still works standalone.
            let cancel = cancel_handle
                .lock()
                .await
                .as_ref()
                .map(|t| t.child_token())
                .unwrap_or_else(CancellationToken::new);

            let req = crate::agent::subagent::DispatchRequest {
                agent_name: agent_name.to_string(),
                task: task.to_string(),
                mode_override,
                cancel,
                parent_agent: parent_agent.clone(),
                parent_context,
                delegate_depth: 0,
                runtime_context: None,
                message: None,
            };

            match executor.dispatch(req).await {
                Ok(result) => {
                    info!(target_agent = %agent_name, "Subagent completed successfully");
                    debug!(target_agent = %agent_name, output = %result.output, "Subagent result");
                    Ok(ToolResult::success(result.output))
                }
                Err(e) => {
                    warn!(target_agent = %agent_name, error = %e, "Subagent execution failed");
                    Ok(ToolResult::error(format!(
                        "SubAgent '{}' execution failed: {}",
                        agent_name, e
                    )))
                }
            }
        })
    }
}
