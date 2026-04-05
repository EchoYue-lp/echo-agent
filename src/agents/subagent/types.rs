//! Subagent core types — definitions, execution modes, and results

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

// ── Execution Mode ────────────────────────────────────────────────────────────

/// How a subagent executes relative to its parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Sync/delegate: parent blocks until subagent returns.
    /// Maps to the existing `AgentDispatchTool` behavior.
    Sync,
    /// Fork: inherits parent context (system prompt, tools, history),
    /// runs independently with optional timeout.
    Fork,
    /// Teammate: parallel independent agent with message-passing coordination.
    Teammate,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        Self::Sync
    }
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionMode::Sync => write!(f, "sync"),
            ExecutionMode::Fork => write!(f, "fork"),
            ExecutionMode::Teammate => write!(f, "teammate"),
        }
    }
}

// ── Subagent Kind ─────────────────────────────────────────────────────────────

/// How the subagent definition is sourced.
#[derive(Debug, Clone)]
pub enum SubagentKind {
    /// Hard-coded agent provided by the library or application.
    BuiltIn,
    /// Loaded from a `.md` definition file (similar to skills).
    Custom { path: PathBuf },
    /// Loaded from a plugin or remote registry.
    Plugin { source: String },
}

impl Default for SubagentKind {
    fn default() -> Self {
        Self::BuiltIn
    }
}

// ── Subagent Definition ───────────────────────────────────────────────────────

/// Declarative specification of a subagent.
///
/// Describes *what* the subagent looks like (model, tools, prompt, constraints)
/// without prescribing *how* to build it — that's the factory's job.
#[derive(Debug, Clone)]
pub struct SubagentDefinition {
    /// Unique name for discovery and dispatch.
    pub name: String,
    /// Human-readable description (exposed to the LLM in tool descriptions).
    pub description: String,
    /// How this agent is sourced.
    pub kind: SubagentKind,
    /// Default execution mode when dispatched.
    pub execution_mode: ExecutionMode,
    /// Model override (None = inherit from parent).
    pub model: Option<String>,
    /// System prompt override (None = inherit or auto-generate).
    pub system_prompt: Option<String>,
    /// Restrict available tools by name (None = inherit all from parent).
    pub tool_filter: Option<Vec<String>>,
    /// Max agent iterations (None = unlimited).
    pub max_iterations: Option<usize>,
    /// Token limit for this subagent (None = use default).
    pub token_limit: Option<usize>,
    /// How many recent messages to inherit from parent.
    /// `None` = don't inherit, `Some(0)` = inherit all, `Some(n)` = last n messages.
    pub inherit_history: Option<usize>,
    /// Whether to inherit parent memory store.
    pub inherit_memory: bool,
    /// Timeout in seconds (0 = no timeout).
    pub timeout_secs: u64,
    /// Whether this subagent can itself dispatch to further subagents.
    pub can_delegate: bool,
    /// Tags for discovery / filtering.
    pub tags: Vec<String>,
}

impl SubagentDefinition {
    /// Create a minimal Sync-mode built-in definition.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: SubagentKind::BuiltIn,
            execution_mode: ExecutionMode::Sync,
            model: None,
            system_prompt: None,
            tool_filter: None,
            max_iterations: None,
            token_limit: None,
            inherit_history: None,
            inherit_memory: false,
            timeout_secs: 0,
            can_delegate: false,
            tags: Vec::new(),
        }
    }

    /// Convenience: a Sync-mode definition for backward-compatible registration.
    pub fn simple_sync(name: impl Into<String>) -> Self {
        let name = name.into();
        Self::new(name.clone(), format!("Subagent {}", name))
    }
}

// ── Subagent Result ───────────────────────────────────────────────────────────

/// Result returned by a subagent execution.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// Agent name that produced this result.
    pub agent_name: String,
    /// Final output text.
    pub output: String,
    /// Execution duration.
    pub duration: Duration,
    /// Number of iterations used.
    pub iterations: usize,
    /// Token usage (if available).
    pub tokens_used: Option<usize>,
    /// Whether the output was truncated due to token limits.
    pub was_truncated: bool,
    /// Execution mode that was used.
    pub mode: ExecutionMode,
}

impl SubagentResult {
    pub fn sync_result(agent_name: &str, output: String, duration: Duration) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            output,
            duration,
            iterations: 1,
            tokens_used: None,
            was_truncated: false,
            mode: ExecutionMode::Sync,
        }
    }

    pub fn fork_result(
        agent_name: &str,
        output: String,
        duration: Duration,
        iterations: usize,
    ) -> Self {
        Self {
            agent_name: agent_name.to_string(),
            output,
            duration,
            iterations,
            tokens_used: None,
            was_truncated: false,
            mode: ExecutionMode::Fork,
        }
    }
}

// ── Registered Subagent (view type) ──────────────────────────────────────────

/// A read-only snapshot of a registered subagent.
#[derive(Debug, Clone)]
pub struct RegisteredSubagent {
    pub definition: SubagentDefinition,
    /// Whether the agent instance is currently available (factory or pre-built).
    pub has_instance: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_mode_default() {
        assert_eq!(ExecutionMode::default(), ExecutionMode::Sync);
    }

    #[test]
    fn test_execution_mode_display() {
        assert_eq!(ExecutionMode::Sync.to_string(), "sync");
        assert_eq!(ExecutionMode::Fork.to_string(), "fork");
        assert_eq!(ExecutionMode::Teammate.to_string(), "teammate");
    }

    #[test]
    fn test_subagent_definition_new() {
        let def = SubagentDefinition::new("researcher", "Researches topics");
        assert_eq!(def.name, "researcher");
        assert_eq!(def.execution_mode, ExecutionMode::Sync);
        assert!(matches!(def.kind, SubagentKind::BuiltIn));
        assert!(def.inherit_history.is_none());
    }

    #[test]
    fn test_simple_sync() {
        let def = SubagentDefinition::simple_sync("worker");
        assert_eq!(def.name, "worker");
        assert!(def.description.contains("worker"));
    }

    #[test]
    fn test_subagent_result_sync() {
        let result = SubagentResult::sync_result("a", "ok".into(), Duration::from_millis(100));
        assert_eq!(result.mode, ExecutionMode::Sync);
        assert_eq!(result.output, "ok");
    }
}
