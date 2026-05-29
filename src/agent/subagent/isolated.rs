//! Isolated sub-agent execution — independent context, messages, and budget.
//!
//! Unlike the current sub-agent system which shares the parent's context,
//! an isolated sub-agent gets its own system prompt and runs independently.
//! The agent's `execute()` method creates a fresh context automatically.

use crate::agent::Agent;
use crate::error::Result;

/// Configuration for an isolated sub-agent run.
#[derive(Debug, Clone)]
pub struct IsolatedSubAgentConfig {
    /// System prompt for the sub-agent.
    pub system_prompt: String,
    /// Maximum iterations for the ReAct loop.
    pub max_iterations: usize,
    /// Token budget for the sub-agent's context (triggers compression when exceeded).
    pub token_budget: usize,
    /// Maximum tool calls before force-stop (0 = unlimited).
    pub tool_call_limit: usize,
    /// Timeout in seconds for the entire sub-agent run.
    pub timeout_secs: u64,
}

impl Default for IsolatedSubAgentConfig {
    fn default() -> Self {
        Self {
            system_prompt:
                "You are a helpful sub-agent. Complete the task and report only the result.".into(),
            max_iterations: 5,
            token_budget: 16_000,
            tool_call_limit: 20,
            timeout_secs: 120,
        }
    }
}

/// Result of an isolated sub-agent execution.
pub struct IsolatedSubAgentResult {
    /// Final output from the sub-agent.
    pub output: String,
    /// Whether execution completed successfully.
    pub success: bool,
}

/// Runs a task with an isolated sub-agent with budget enforcement.
pub async fn run_isolated(
    parent: &crate::prelude::ReactAgent,
    task: &str,
    config: &IsolatedSubAgentConfig,
) -> Result<IsolatedSubAgentResult> {
    let sub = crate::prelude::ReactAgentBuilder::new()
        .model(parent.model_name())
        .system_prompt(&config.system_prompt)
        .max_iterations(config.max_iterations)
        .token_limit(config.token_budget)
        .enable_tools()
        .build()?;

    // Enforce timeout
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(config.timeout_secs),
        sub.execute(task),
    )
    .await;

    match result {
        Ok(Ok(output)) => Ok(IsolatedSubAgentResult {
            output,
            success: true,
        }),
        Ok(Err(e)) => Ok(IsolatedSubAgentResult {
            output: format!("Sub-agent error: {e}"),
            success: false,
        }),
        Err(_) => Ok(IsolatedSubAgentResult {
            output: format!("Sub-agent timed out after {}s", config.timeout_secs),
            success: false,
        }),
    }
}
