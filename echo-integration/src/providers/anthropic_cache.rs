//! Anthropic cache breakpoint helper.
//!
//! This module extracts the breakpoint strategy from
//! [`super::anthropic::AnthropicClient::convert_request`] into a standalone,
//! testable helper. It maps a [`PromptCacheLayout`] to a concrete plan that
//! the Anthropic provider can map to protocol-level `cache_control` breakpoints.
//!
//! # Breakpoint allocation (max 4, Anthropic API limit)
//!
//! | # | Target                 | Effect                                |
//! |---|------------------------|---------------------------------------|
//! | 1 | SystemLastBlock        | System prompt cached across turns     |
//! | 2 | ToolsLastTool          | Tool definitions cached               |
//! | 3 | HistoryIndex(~75%)     | Deep history cached (long convos)     |
//! | 4 | HistoryLastStable      | Latest stable message cached next turn|
//!
//! Runtime context messages (tagged `[runtime_context:`) are always excluded
//! from breakpoints — they change every turn and would invalidate the cache.

use echo_core::llm::cache::layout::{BreakpointTarget, PromptCacheLayout};

/// A resolved cache breakpoint plan for the Anthropic Messages API.
///
/// Produced from a [`PromptCacheLayout`] read-only view; the
/// [`AnthropicClient`](super::anthropic::AnthropicClient) maps this to
/// `cache_control: { type: "ephemeral" }` fields in the request body.
#[derive(Debug, Clone)]
pub struct AnthropicCachePlan {
    /// Breakpoint targets derived from the layout (max 4).
    pub breakpoints: Vec<BreakpointTarget>,
    /// Whether a system-level breakpoint is recommended.
    pub has_system_breakpoint: bool,
    /// Whether a tools-level breakpoint is recommended.
    pub has_tool_breakpoint: bool,
}

impl AnthropicCachePlan {
    /// Build a plan from a read-only layout view.
    ///
    /// This is the unified entry point that replaces the ad-hoc logic
    /// previously in `convert_request` and `apply_conversation_cache_breakpoints`.
    pub fn from_layout(layout: &PromptCacheLayout<'_>) -> Self {
        let mut breakpoints = Vec::with_capacity(4);

        let has_system = !layout.system.is_empty() || !layout.canonical.is_empty();
        let has_tools = !layout.tools.is_empty();
        let has_history = !layout.history.is_empty();

        if has_system {
            breakpoints.push(BreakpointTarget::SystemLastBlock);
        }
        if has_tools {
            breakpoints.push(BreakpointTarget::ToolsLastTool);
        }
        if has_history && layout.history.len() >= 4 {
            // 75% depth for long conversations
            let idx = (layout.history.len() - 1) * 3 / 4;
            breakpoints.push(BreakpointTarget::HistoryIndex(idx));
        }
        if has_history {
            breakpoints.push(BreakpointTarget::HistoryLastStable);
        }

        // Anthropic hard limit: max 4 breakpoints total
        breakpoints.truncate(4);

        let has_system_breakpoint = breakpoints
            .iter()
            .any(|b| matches!(b, BreakpointTarget::SystemLastBlock));
        let has_tool_breakpoint = breakpoints
            .iter()
            .any(|b| matches!(b, BreakpointTarget::ToolsLastTool));

        Self {
            breakpoints,
            has_system_breakpoint,
            has_tool_breakpoint,
        }
    }

    /// Build a plan from an existing `cache_hints` (or fall back to the
    /// hardcoded strategy when hints are absent — backward compat).
    pub fn from_layout_or_default(layout: Option<&PromptCacheLayout<'_>>) -> Self {
        match layout {
            Some(l) => Self::from_layout(l),
            None => Self::default_plan(),
        }
    }

    /// Default hardcoded plan used when no layout is available (backward compat).
    fn default_plan() -> Self {
        Self {
            breakpoints: vec![],
            has_system_breakpoint: true,
            has_tool_breakpoint: true,
        }
    }

    /// Whether a given [`BreakpointTarget`] is present in this plan.
    pub fn has(&self, target: BreakpointTarget) -> bool {
        self.breakpoints.iter().any(|b| *b == target)
    }

    /// Conversation-history breakpoints as message indices (relative to the
    /// history segment). Callers must add the segment base offset.
    pub fn history_indices(&self) -> Vec<usize> {
        self.breakpoints
            .iter()
            .filter_map(|bp| match bp {
                BreakpointTarget::HistoryIndex(i) => Some(*i),
                _ => None,
            })
            .collect()
    }

    /// Whether the plan includes a `HistoryLastStable` breakpoint.
    pub fn has_history_last_stable(&self) -> bool {
        self.breakpoints
            .iter()
            .any(|b| matches!(b, BreakpointTarget::HistoryLastStable))
    }

    /// Total number of conversation-history breakpoints (HistoryIndex + HistoryLastStable).
    pub fn history_breakpoint_count(&self) -> usize {
        self.breakpoints
            .iter()
            .filter(|b| {
                matches!(
                    b,
                    BreakpointTarget::HistoryIndex(_) | BreakpointTarget::HistoryLastStable
                )
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::llm::types::Message;

    fn sys(t: &str) -> Message {
        Message::system(t.to_string())
    }
    fn user(t: &str) -> Message {
        Message::user(t.to_string())
    }
    fn rt(t: &str) -> Message {
        Message::user(format!("[runtime_context:{t}]"))
    }

    #[test]
    fn plan_skips_runtime_context_in_breakpoints() {
        let msgs = vec![
            sys("You are Echo Agent"),
            user("h1"),
            user("h2"),
            user("h3"),
            user("h4"),
            rt("turn\ncwd: /tmp"),
        ];
        let tools = vec![];
        let layout = PromptCacheLayout::from_messages(&msgs, &tools);
        let plan = AnthropicCachePlan::from_layout(&layout);
        // runtime_context should not appear in breakpoints
        assert!(!plan
            .breakpoints
            .iter()
            .any(|b| matches!(b, BreakpointTarget::HistoryIndex(i) if *i >= layout.history.len())));
        assert!(plan.has_history_last_stable());
        assert!(!plan.has(BreakpointTarget::ToolsLastTool));
    }

    #[test]
    fn plan_truncates_to_four_breakpoints() {
        // 10 history messages → eligible for full breakpoint set
        let msgs: Vec<Message> = (0..10).map(|i| user(&format!("h{i}"))).collect();
        let tools = vec![];
        let layout = PromptCacheLayout::from_messages(&msgs, &tools);
        let plan = AnthropicCachePlan::from_layout(&layout);
        assert!(plan.breakpoints.len() <= 4);
    }

    #[test]
    fn no_history_breakpoints_for_short_conversation() {
        let msgs = vec![sys("S"), user("only one")];
        let tools = vec![];
        let layout = PromptCacheLayout::from_messages(&msgs, &tools);
        let plan = AnthropicCachePlan::from_layout(&layout);
        // only 1 history message → no history breakpoints
        assert!(!plan
            .breakpoints
            .iter()
            .any(|b| matches!(b, BreakpointTarget::HistoryIndex(_))));
        // HistoryLastStable should still be present (non-empty history)
        assert!(plan.has_history_last_stable());
    }

    #[test]
    fn system_and_tools_breakpoints_when_present() {
        let msgs = vec![sys("S")];
        let tools = vec![echo_core::llm::types::ToolDefinition {
            tool_type: "function".to_string(),
            function: echo_core::llm::types::FunctionSpec {
                name: "test_tool".to_string(),
                description: "A test tool".to_string(),
                parameters: serde_json::json!({}),
            },
        }];
        let layout = PromptCacheLayout::from_messages(&msgs, &tools);
        let plan = AnthropicCachePlan::from_layout(&layout);
        assert!(plan.has_system_breakpoint);
        assert!(plan.has_tool_breakpoint);
    }

    #[test]
    fn from_layout_or_default_falls_back_when_none() {
        let plan = AnthropicCachePlan::from_layout_or_default(None);
        assert!(plan.breakpoints.is_empty());
        assert!(plan.has_system_breakpoint);
        assert!(plan.has_tool_breakpoint);
    }
}
