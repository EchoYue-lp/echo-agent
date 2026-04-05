//! Subagent context isolation and inheritance
//!
//! Defines what gets shared from a parent agent to its subagent,
//! and provides utilities for creating isolated execution contexts.

use echo_core::llm::ToolDefinition;
use echo_core::llm::types::Message;
use std::collections::HashMap;
use std::sync::Arc;

use crate::memory::store::Store;

use super::types::ExecutionMode;

// ── Context Inheritance ───────────────────────────────────────────────────────

/// Declares what the subagent inherits from its parent.
///
/// Each execution mode has a default preset:
/// - **Sync**: no inheritance (shares state via the mutex).
/// - **Fork**: inherits system prompt + tools + recent history.
/// - **Teammate**: no inheritance (communicates via mailbox).
#[derive(Debug, Clone)]
pub struct ContextInheritance {
    /// Inherit the parent's system prompt (prepended to subagent's own).
    pub inherit_system_prompt: bool,
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
            inherit_system_prompt: false,
            inherit_tools: None,
            inherit_history: None,
            inherit_memory: false,
            inject_metadata: HashMap::new(),
        }
    }

    /// Fork mode default: inherit prompt + tools + recent 10 messages.
    pub fn fork_default() -> Self {
        Self {
            inherit_system_prompt: true,
            inherit_tools: None,
            inherit_history: Some(10),
            inherit_memory: true,
            inject_metadata: HashMap::new(),
        }
    }

    /// Teammate mode default: nothing inherited, communication via mailbox.
    pub fn teammate_default() -> Self {
        Self {
            inherit_system_prompt: false,
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
        }
    }
}

impl Default for ContextInheritance {
    fn default() -> Self {
        Self::sync_default()
    }
}

// ── Subagent Context ──────────────────────────────────────────────────────────

/// Snapshot of a parent agent's context for inheritance.
///
/// Extracted from the parent before spawning a subagent.
/// This is a read-only snapshot — the subagent gets its own copy.
#[derive(Clone)]
pub struct SubagentContext {
    /// Parent's system prompt.
    pub system_prompt: String,
    /// Parent's tool definitions (filtered by `ContextInheritance::inherit_tools`).
    pub tool_definitions: Vec<ToolDefinition>,
    /// Parent's recent conversation messages (limited by `inherit_history`).
    pub messages: Vec<Message>,
    /// Parent's memory store (if shared).
    pub store: Option<Arc<dyn Store>>,
}

impl std::fmt::Debug for SubagentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentContext")
            .field("system_prompt", &self.system_prompt)
            .field("tool_definitions", &self.tool_definitions)
            .field("messages", &self.messages)
            .field("store", &self.store.as_ref().map(|_| "Store { .. }"))
            .finish()
    }
}

impl SubagentContext {
    /// Create an empty context (no inheritance).
    pub fn empty() -> Self {
        Self {
            system_prompt: String::new(),
            tool_definitions: Vec::new(),
            messages: Vec::new(),
            store: None,
        }
    }

    /// Build a context by applying an inheritance spec to a parent's full context.
    pub fn from_parent(
        system_prompt: &str,
        all_tools: &[ToolDefinition],
        all_messages: &[Message],
        store: Option<Arc<dyn Store>>,
        inheritance: &ContextInheritance,
    ) -> Self {
        let filtered_tools = if inheritance.inherit_tools.is_some() {
            // Specific tool list: only inherit named tools
            let allowed = inheritance.inherit_tools.as_ref().unwrap();
            all_tools
                .iter()
                .filter(|t| allowed.iter().any(|a| a == &t.function.name))
                .cloned()
                .collect()
        } else if inheritance.inherit_tools.is_none()
            && !inheritance.inherit_system_prompt
            && inheritance.inherit_history.is_none()
        {
            // No explicit tool list, but no other inheritance requested: don't inherit tools
            Vec::new()
        } else {
            // No explicit tool list + some inheritance context requested: inherit all tools
            all_tools.to_vec()
        };

        let messages = match inheritance.inherit_history {
            Some(n) => {
                let start = all_messages.len().saturating_sub(n);
                all_messages[start..].to_vec()
            }
            None => Vec::new(),
        };

        Self {
            system_prompt: if inheritance.inherit_system_prompt {
                system_prompt.to_string()
            } else {
                String::new()
            },
            tool_definitions: filtered_tools,
            messages,
            store: if inheritance.inherit_memory {
                store
            } else {
                None
            },
        }
    }

    /// Check if this context has any inheritable content.
    pub fn has_content(&self) -> bool {
        !self.system_prompt.is_empty()
            || !self.tool_definitions.is_empty()
            || !self.messages.is_empty()
            || self.store.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_default_no_inheritance() {
        let inh = ContextInheritance::sync_default();
        assert!(!inh.inherit_system_prompt);
        assert!(inh.inherit_history.is_none());
        assert!(!inh.inherit_memory);
    }

    #[test]
    fn test_fork_default_inherits() {
        let inh = ContextInheritance::fork_default();
        assert!(inh.inherit_system_prompt);
        assert_eq!(inh.inherit_history, Some(10));
        assert!(inh.inherit_memory);
    }

    #[test]
    fn test_teammate_default_no_inheritance() {
        let inh = ContextInheritance::teammate_default();
        assert!(!inh.inherit_system_prompt);
        assert!(inh.inherit_history.is_none());
    }

    #[test]
    fn test_for_mode() {
        assert!(!ContextInheritance::for_mode(&ExecutionMode::Sync).inherit_system_prompt);
        assert!(ContextInheritance::for_mode(&ExecutionMode::Fork).inherit_system_prompt);
        assert!(!ContextInheritance::for_mode(&ExecutionMode::Teammate).inherit_system_prompt);
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
            inherit_system_prompt: true,
            ..ContextInheritance::sync_default()
        };

        let ctx = SubagentContext::from_parent("prompt", &tools, &[], None, &inh);
        assert_eq!(ctx.tool_definitions.len(), 1);
        assert_eq!(ctx.tool_definitions[0].function.name, "search");
    }
}
