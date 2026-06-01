//! Intervention callback interface for influencing agent behavior
//!
//! Unlike `AgentCallback` (which is observational), `InterventionCallback`
//! can *influence* agent behavior: block tool calls, inject context,
//! redirect execution, or cancel operations.
//!
//! A `CallbackBridge` adapter converts plain `AgentCallback` instances
//! into neutral `InterventionCallback` implementations for backward
//! compatibility.

use crate::llm::types::Message;
use futures::future::BoxFuture;
use serde_json::Value;

/// Result of an intervention check, indicating how the agent should proceed.
#[derive(Debug, Clone, Default)]
pub struct InterventionResult {
    /// Whether to block the current action.
    pub block: bool,
    /// Reason for blocking (shown to user / logged).
    pub block_reason: Option<String>,
    /// Context to inject into the agent's next LLM call.
    pub injected_context: Option<String>,
    /// Redirect to a different tool name (replaces the original tool call).
    pub redirect_to: Option<String>,
    /// Whether to cancel the entire agent execution.
    pub cancel: bool,
    /// Modified tool arguments (replaces original args if present).
    pub modified_args: Option<Value>,
}

impl InterventionResult {
    /// Allow the action to proceed normally.
    pub fn allow() -> Self {
        Self::default()
    }

    /// Block the action with a reason.
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            block: true,
            block_reason: Some(reason.into()),
            ..Default::default()
        }
    }

    /// Inject additional context into the agent's reasoning.
    pub fn inject(context: impl Into<String>) -> Self {
        Self {
            injected_context: Some(context.into()),
            ..Default::default()
        }
    }

    /// Cancel the entire agent execution.
    pub fn cancel() -> Self {
        Self {
            cancel: true,
            ..Default::default()
        }
    }

    /// Modify the tool arguments before execution.
    pub fn modify_args(args: Value) -> Self {
        Self {
            modified_args: Some(args),
            ..Default::default()
        }
    }
}

/// Trait for intervention callbacks that can influence agent behavior.
///
/// All methods have default implementations that return `InterventionResult::allow()`,
/// so implementors only need to override the methods they care about.
pub trait InterventionCallback: Send + Sync {
    /// Inspect (and potentially block/modify) a tool call before execution.
    fn on_tool_call<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, InterventionResult> {
        Box::pin(async { InterventionResult::allow() })
    }

    /// Inspect (and potentially inject context or cancel) before LLM reasoning.
    fn on_think_start<'a>(
        &'a self,
        _agent: &'a str,
        _messages: &'a [Message],
    ) -> BoxFuture<'a, InterventionResult> {
        Box::pin(async { InterventionResult::allow() })
    }

    /// Inspect (and potentially block) the agent's final answer.
    fn on_final_answer<'a>(
        &'a self,
        _agent: &'a str,
        _answer: &'a str,
    ) -> BoxFuture<'a, InterventionResult> {
        Box::pin(async { InterventionResult::allow() })
    }
}

/// Bridge adapter that wraps an `AgentCallback` as a neutral `InterventionCallback`.
///
/// All intervention checks return `InterventionResult::allow()`, preserving
/// the observational-only behavior of `AgentCallback` while satisfying the
/// `InterventionCallback` interface.
///
/// Note: This struct is defined here but references `AgentCallback` which is
/// in the facade crate's `agent` module. The bridge is re-exported from the
/// facade, where it has access to `AgentCallback`. This file provides the
/// trait-level definition; the concrete `CallbackBridge` impl is in the facade.
pub struct CallbackBridge;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intervention_result_allow() {
        let result = InterventionResult::allow();
        assert!(!result.block);
        assert!(!result.cancel);
        assert!(result.block_reason.is_none());
        assert!(result.injected_context.is_none());
    }

    #[test]
    fn test_intervention_result_block() {
        let result = InterventionResult::block("dangerous operation");
        assert!(result.block);
        assert_eq!(result.block_reason.as_deref(), Some("dangerous operation"));
        assert!(!result.cancel);
    }

    #[test]
    fn test_intervention_result_inject() {
        let result = InterventionResult::inject("additional context");
        assert!(result.injected_context.is_some());
        assert!(!result.block);
    }

    #[test]
    fn test_intervention_result_cancel() {
        let result = InterventionResult::cancel();
        assert!(result.cancel);
        assert!(!result.block);
    }
}