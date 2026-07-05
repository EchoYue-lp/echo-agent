//! Tool execution subsystem
//!
//! Centralized management of tool registration/execution, Skill, Hook, MCP,
//! SubAgent, Sandbox, intervention callbacks, and other components directly
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
/// Skill/Hook system, MCP connection management, SubAgent scheduling, sandbox
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
    /// Per-run disabled tool names. Tools in this set are hidden from the LLM
    /// (filtered out of the tool list sent to the model). Populated by the
    /// application layer (e.g. EKO hides task-management tools when in Chat
    /// interaction mode). Read fresh each iteration via [`tools_for_llm`].
    ///
    /// Unlike `ToolManager::unregister` (which mutates the shared registry and
    /// would affect other in-flight turns on pooled agents), this is a
    /// separate runtime flag — the tool stays registered and available to
    /// other turns; only the LLM tool list is filtered.
    pub(crate) disabled_tools: Arc<std::sync::RwLock<Option<std::collections::HashSet<String>>>>,
}

impl ToolExecutionSubsystem {
    /// Return a clone of the tool manager Arc (for StreamRunner construction).
    #[allow(dead_code)]
    pub(crate) fn tool_manager_arc(&self) -> Arc<ToolManager> {
        Arc::clone(&self.tool_manager)
    }

    /// Return the tool definitions to send to the LLM, with `disabled_tools`
    /// filtered out. This is the single chokepoint that honors per-run tool
    /// hiding — both LLM call sites (streaming + non-streaming) use this
    /// instead of `tool_manager.get_openai_tools()` directly.
    pub(crate) fn tools_for_llm(&self) -> Vec<crate::llm::types::ToolDefinition> {
        let tools = self.tool_manager.get_openai_tools();
        let disabled = self
            .disabled_tools
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        match disabled {
            Some(set) if !set.is_empty() => tools
                .into_iter()
                .filter(|t| !set.contains(&t.function.name))
                .collect(),
            _ => tools,
        }
    }

    /// Set the disabled tool set for the current run. Callers (e.g. drive_chat)
    /// pass `Some(set)` to hide tools, or `None` to clear. The set is read
    /// fresh on each LLM iteration via [`tools_for_llm`].
    pub(crate) fn set_disabled_tools(&self, names: Option<std::collections::HashSet<String>>) {
        if let Ok(mut guard) = self.disabled_tools.write() {
            *guard = names;
        }
    }

    #[cfg(feature = "mcp")]
    #[allow(dead_code)]
    pub(crate) fn mcp_manager_arc(&self) -> Option<Arc<McpManager>> {
        None // McpManager is not Arc-wrapped; use shared registry instead
    }

    #[cfg(feature = "subagent")]
    #[allow(dead_code)]
    pub(crate) fn subagent_registry(&self) -> Option<Arc<SubagentRegistry>> {
        Some(Arc::clone(&self.subagent_registry))
    }

    #[allow(dead_code)]
    pub(crate) fn progressive_skill_registry(&self) -> Option<SharedRegistry> {
        self.progressive_skill_registry.clone()
    }
}
