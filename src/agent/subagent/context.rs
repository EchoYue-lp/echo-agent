//! Subagent context isolation and inheritance
//!
//! Defines what gets shared from a parent agent to its subagent,
//! and provides utilities for creating isolated execution contexts.

use echo_core::llm::ToolDefinition;
use echo_core::llm::types::{Message, Role};
use std::collections::HashMap;
use std::sync::Arc;

use crate::memory::store::Store;

use super::types::ExecutionMode;

// ── Context Inheritance ───────────────────────────────────────────────────────

/// Declares what the subagent inherits from its parent.
///
/// ## Fresh vs Fork inheritance (Claude/Cursor-aligned)
///
/// - **Fresh** ([`Self::fresh_default`]): no parent history / memory.
///   The current user-authored request is still carried as a scoped goal so
///   the isolated subagent can preserve user intent without inheriting the
///   conversation transcript. This is the product default for TaskRuntime and
///   `agent_tool` (omit mode).
/// - **Fork inheritance** ([`Self::fork_default`]): inherit filtered recent
///   history + memory. Parent system prompts are never transferred as user text.
///
/// `ExecutionMode::Fork` is orthogonal: it selects the concurrent dispatch path
/// (semaphore + worktree/workspace). A Fork *execution* can still use Fresh
/// *inheritance*.
///
/// Historical mode presets:
/// - **Sync**: no inheritance ([`Self::sync_default`] == fresh).
/// - **Fork mode preset**: inherits tools + recent history.
/// - **Teammate**: no inheritance; execution is controlled by its background handle.
#[derive(Debug, Clone)]
pub struct ContextInheritance {
    /// Inherit specific tools by name. `None` = inherit all.
    pub inherit_tools: Option<Vec<String>>,
    /// Inherit recent N messages from conversation history. `None` = don't inherit.
    pub inherit_history: Option<usize>,
    /// Inherit the parent's memory store reference.
    pub inherit_memory: bool,
    /// Inject custom key-value metadata into the subagent context.
    pub inject_metadata: HashMap<String, String>,
}

impl ContextInheritance {
    /// Sync mode default: nothing inherited (shared state via mutex).
    pub fn sync_default() -> Self {
        Self {
            inherit_tools: None,
            inherit_history: None,
            inherit_memory: false,
            inject_metadata: HashMap::new(),
        }
    }

    /// Claude/Cursor-aligned default: no parent conversation inheritance.
    ///
    /// Prefer this name in new call sites; [`Self::sync_default`] remains as
    /// the historical alias with identical fields.
    pub fn fresh_default() -> Self {
        Self::sync_default()
    }

    /// Fork mode default: inherit tools + recent 2 messages.
    ///
    /// Sprint 6b: lowered 10 → 2 (was over-inheriting, bloating Fork subagent
    /// context with stale turns). `SubagentDefinition.inherit_history` (e.g.
    /// from a `.md` frontmatter or `.inherit_history(n)`) is now honored at
    /// dispatch time by the configured prompt compiler and overrides this default.
    pub fn fork_default() -> Self {
        Self {
            inherit_tools: None,
            inherit_history: Some(2),
            inherit_memory: true,
            inject_metadata: HashMap::new(),
        }
    }

    /// Teammate mode default: nothing inherited.
    pub fn teammate_default() -> Self {
        Self {
            inherit_tools: None,
            inherit_history: None,
            inherit_memory: false,
            inject_metadata: HashMap::new(),
        }
    }

    /// Select the default inheritance for a given execution mode.
    pub fn for_mode(mode: &ExecutionMode) -> Self {
        match mode {
            ExecutionMode::Sync => Self::sync_default(),
            ExecutionMode::Fork => Self::fork_default(),
            ExecutionMode::Teammate => Self::teammate_default(),
            ExecutionMode::Team => Self::teammate_default(),
        }
    }
}

impl Default for ContextInheritance {
    fn default() -> Self {
        Self::sync_default()
    }
}

/// Snapshot of a parent agent's context for inheritance.
///
/// Extracted from the parent before spawning a subagent.
/// This is a read-only snapshot — the subagent gets its own copy.
#[derive(Clone)]
pub struct SubagentContext {
    /// Parent's tool definitions (filtered by `ContextInheritance::inherit_tools`).
    pub tool_definitions: Vec<ToolDefinition>,
    /// Parent's recent conversation messages (limited by `inherit_history`).
    pub messages: Vec<Message>,
    /// Parent's memory store (if shared).
    pub store: Option<Arc<dyn Store>>,

    // ── Scoped Context Fields (Step 6) ──────────────────────────────────────
    /// Parent's overall goal (for context)
    pub parent_goal: Option<String>,
    /// Allowed tools for this subagent (overrides inheritance)
    pub allowed_tools: Option<Vec<String>>,
}

impl std::fmt::Debug for SubagentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentContext")
            .field("tool_definitions", &self.tool_definitions)
            .field("messages", &self.messages)
            .field("store", &self.store.as_ref().map(|_| "Store { .. }"))
            .field("parent_goal", &self.parent_goal)
            .field("allowed_tools", &self.allowed_tools)
            .finish()
    }
}

impl SubagentContext {
    /// Create an empty context (no inheritance).
    pub fn empty() -> Self {
        Self {
            tool_definitions: Vec::new(),
            messages: Vec::new(),
            store: None,
            // Scoped context fields
            parent_goal: None,
            allowed_tools: None,
        }
    }

    /// Build a context by applying an inheritance spec to a parent's full context.
    pub fn from_parent(
        all_tools: &[ToolDefinition],
        all_messages: &[Message],
        store: Option<Arc<dyn Store>>,
        inheritance: &ContextInheritance,
    ) -> Self {
        let parent_goal = latest_user_request(all_messages);
        let filtered_tools = if let Some(allowed) = &inheritance.inherit_tools {
            // Specific tool list: only inherit named tools
            all_tools
                .iter()
                .filter(|t| allowed.iter().any(|a| a == &t.function.name))
                .cloned()
                .collect()
        } else if inheritance.inherit_tools.is_none()
            && inheritance.inherit_history.is_none()
            && !inheritance.inherit_memory
        {
            // No explicit tool list, but no other inheritance requested: don't inherit tools
            Vec::new()
        } else {
            // No explicit tool list + some inheritance context requested: inherit all tools
            all_tools.to_vec()
        };

        let messages = match inheritance.inherit_history {
            Some(0) => all_messages.to_vec(),
            Some(n) => {
                let start = all_messages.len().saturating_sub(n);
                all_messages.get(start..).unwrap_or_default().to_vec()
            }
            None => Vec::new(),
        };

        Self {
            tool_definitions: filtered_tools,
            messages,
            store: if inheritance.inherit_memory {
                store
            } else {
                None
            },
            // The active user request is scoped delegation context rather than
            // inherited conversation history, so Fresh subagents receive it.
            parent_goal,
            allowed_tools: None,
        }
    }

    /// Check if this context has any inheritable content.
    pub fn has_content(&self) -> bool {
        !self.tool_definitions.is_empty()
            || !self.messages.is_empty()
            || self.store.is_some()
            || self.parent_goal.is_some()
            || self.allowed_tools.is_some()
    }
}

fn latest_user_request(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != Role::User || crate::compression::is_context_projection_message(message)
        {
            return None;
        }

        let text = message.content.as_text()?;
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with("[runtime_context:") {
            return None;
        }
        Some(trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_default_no_inheritance() {
        let inh = ContextInheritance::sync_default();
        assert!(inh.inherit_history.is_none());
        assert!(!inh.inherit_memory);
    }

    #[test]
    fn fresh_default_is_alias_of_sync_default() {
        let a = ContextInheritance::fresh_default();
        let b = ContextInheritance::sync_default();
        assert_eq!(a.inherit_history, b.inherit_history);
        assert_eq!(a.inherit_memory, b.inherit_memory);
    }

    #[test]
    fn from_parent_fresh_keeps_only_current_user_request() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: echo_core::llm::types::FunctionSpec {
                name: "search".into(),
                description: "Search".into(),
                parameters: serde_json::json!({}),
            },
        }];
        let msgs = vec![Message::user("hello".to_string())];
        let ctx =
            SubagentContext::from_parent(&tools, &msgs, None, &ContextInheritance::fresh_default());
        assert!(ctx.messages.is_empty());
        assert!(ctx.tool_definitions.is_empty());
        assert!(ctx.store.is_none());
        assert_eq!(ctx.parent_goal.as_deref(), Some("hello"));
        assert!(ctx.has_content());
    }

    #[test]
    fn from_parent_ignores_runtime_context_and_projection_messages() {
        let mut projected = crate::compression::ContextManager::builder(4096).build();
        projected.push(Message::user("用户请求：核对并发问题 🧭".to_string()));
        projected.apply_projections(&[crate::compression::ContextProjection {
            marker: "test:turn-contract".to_string(),
            message: Some(Message::user("English runtime contract".to_string())),
        }]);
        projected.push(Message::user(
            "[runtime_context:task-runtime]\ninternal state".to_string(),
        ));

        let ctx = SubagentContext::from_parent(
            &[],
            projected.messages(),
            None,
            &ContextInheritance::fresh_default(),
        );

        assert_eq!(
            ctx.parent_goal.as_deref(),
            Some("用户请求：核对并发问题 🧭")
        );
        assert!(ctx.messages.is_empty());
        assert!(ctx.store.is_none());
    }

    #[test]
    fn from_parent_uses_latest_real_user_request() {
        let messages = vec![
            Message::user("first request".to_string()),
            Message::assistant("answer".to_string()),
            Message::user("最后一个真实请求".to_string()),
        ];

        let ctx = SubagentContext::from_parent(
            &[],
            &messages,
            None,
            &ContextInheritance::fresh_default(),
        );

        assert_eq!(ctx.parent_goal.as_deref(), Some("最后一个真实请求"));
    }

    #[test]
    fn test_fork_default_inherits() {
        let inh = ContextInheritance::fork_default();
        // Sprint 6b: fork default inherit_history lowered 10 → 2.
        assert_eq!(inh.inherit_history, Some(2));
        assert!(inh.inherit_memory);
    }

    #[test]
    fn test_teammate_default_no_inheritance() {
        let inh = ContextInheritance::teammate_default();
        assert!(inh.inherit_history.is_none());
    }

    #[test]
    fn test_for_mode() {
        assert!(
            ContextInheritance::for_mode(&ExecutionMode::Sync)
                .inherit_history
                .is_none()
        );
        assert_eq!(
            ContextInheritance::for_mode(&ExecutionMode::Fork).inherit_history,
            Some(2)
        );
        assert!(
            ContextInheritance::for_mode(&ExecutionMode::Teammate)
                .inherit_history
                .is_none()
        );
    }

    #[test]
    fn test_empty_context() {
        let ctx = SubagentContext::empty();
        assert!(!ctx.has_content());
    }

    #[test]
    fn test_from_parent_filters_tools() {
        let tools = vec![
            ToolDefinition {
                tool_type: "function".to_string(),
                function: echo_core::llm::types::FunctionSpec {
                    name: "search".into(),
                    description: "Search".into(),
                    parameters: serde_json::json!({}),
                },
            },
            ToolDefinition {
                tool_type: "function".to_string(),
                function: echo_core::llm::types::FunctionSpec {
                    name: "read".into(),
                    description: "Read".into(),
                    parameters: serde_json::json!({}),
                },
            },
        ];

        let inh = ContextInheritance {
            inherit_tools: Some(vec!["search".into()]),
            ..ContextInheritance::sync_default()
        };

        let ctx = SubagentContext::from_parent(&tools, &[], None, &inh);
        assert_eq!(ctx.tool_definitions.len(), 1);
        assert!(matches!(
            ctx.tool_definitions.first(),
            Some(tool) if tool.function.name == "search"
        ));
    }
}
