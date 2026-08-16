//! Tool execution subsystem
//!
//! Centralized management of tool registration/execution, Skill, Hook, MCP,
//! Subagent, Sandbox, intervention callbacks, and other components directly
//! related to tool invocation.

use crate::agent::InterventionCallback;
#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentRegistry;
#[cfg(feature = "mcp")]
use crate::mcp::McpManager;
use crate::sandbox::SandboxManager;
use crate::skills::SkillRegistry;
use crate::skills::hooks::HookRegistry;
use crate::skills::registry::SharedRegistry;
use crate::tools::ToolManager;
use std::sync::Arc;

/// Tool execution subsystem
///
/// Aggregates all components directly related to tool invocation: tool registry,
/// Skill/Hook system, MCP connection management, Subagent scheduling, sandbox
/// environment, and intervention callbacks.
pub(crate) struct ToolExecutionSubsystem {
    /// Tool registry (Arc for sharing with StreamRunner).
    pub(crate) tool_manager: Arc<ToolManager>,
    #[cfg(feature = "subagent")]
    pub(crate) subagent_registry: Arc<SubagentRegistry>,
    /// Shared subagent executor (with hook configuration) — reused by
    /// `delegate_task()` and `delegate_to_agent()` instead of creating
    /// throwaway executors.
    #[cfg(feature = "subagent")]
    pub(crate) subagent_executor: Arc<crate::agent::subagent::SubagentExecutor>,
    pub(crate) skill_registry: SkillRegistry,
    pub(crate) progressive_skill_registry: Option<SharedRegistry>,
    pub(crate) hook_registry: Arc<tokio::sync::RwLock<HookRegistry>>,
    #[cfg(feature = "mcp")]
    pub(crate) mcp_manager: McpManager,
    pub(crate) sandbox_manager: Option<Arc<SandboxManager>>,
    /// Intervention callbacks that can influence agent behavior before
    /// tool calls, LLM reasoning, and final answers.
    pub(crate) intervention_callbacks: Vec<Arc<dyn InterventionCallback>>,
    /// Agent-level default disabled tool names. Tools in this set are hidden from the LLM
    /// (filtered out of the tool list sent to the model). Populated by the
    /// application layer for durable agent-wide policy. Invocation-specific
    /// exclusions belong in `AgentInvocationContext::disabled_tools`. The
    /// current value is cloned into each run snapshot and never read again by
    /// that run.
    ///
    /// Unlike `ToolManager::unregister` (which mutates the shared registry and
    /// would affect other in-flight turns on pooled agents), this is a
    /// separate runtime flag — the tool stays registered and available to
    /// other agents; only the LLM tool list is filtered.
    pub(crate) disabled_tools: Arc<std::sync::RwLock<Option<std::collections::HashSet<String>>>>,
}

impl ToolExecutionSubsystem {
    /// Set agent-level default disabled tools for subsequent runs.
    ///
    /// Existing snapshots are immutable and are not affected.
    pub(crate) fn set_disabled_tools(&self, names: Option<std::collections::HashSet<String>>) {
        if let Ok(mut guard) = self.disabled_tools.write() {
            *guard = names;
        }
    }
}
