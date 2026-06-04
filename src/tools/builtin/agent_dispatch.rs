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
    cancel: CancellationToken,
    /// Optional factory for building parent context at dispatch time.
    /// When set, subagents in Fork mode inherit conversation history,
    /// system prompt, and tools from the parent agent.
    parent_context_factory: Option<Arc<ParentContextFactory>>,
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
            cancel,
            parent_context_factory: None,
        }
    }

    /// Create with a parent context factory for context inheritance.
    pub fn with_parent_context(mut self, factory: Arc<ParentContextFactory>) -> Self {
        self.parent_context_factory = Some(factory);
        self
    }
}

impl Tool for AgentDispatchTool {
    fn name(&self) -> &str {
        "agent_tool"
    }

    fn description(&self) -> &str {
        "Dispatch a task to a specialized SubAgent for execution. As the orchestrator, prefer using this tool to delegate computation, data fetching, etc. to professional SubAgents rather than answering directly."
    }

    fn parameters(&self) -> Value {
        // NOTE: agent_names would require async, so we provide a generic description
        json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": "SubAgent name"
                },
                "task": {
                    "type": "string",
                    "description": "Specific task description to assign to the SubAgent, should include necessary context"
                },
                "mode": {
                    "type": "string",
                    "enum": ["sync", "fork", "teammate"],
                    "description": "Execution mode: sync - synchronous wait (default), fork - independent with inherited context, teammate - parallel collaboration"
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
        let cancel = self.cancel.clone();
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

            let req = crate::agent::subagent::DispatchRequest {
                agent_name: agent_name.to_string(),
                task: task.to_string(),
                mode_override,
                cancel,
                parent_agent: parent_agent.clone(),
                parent_context,
                delegate_depth: 0,
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
