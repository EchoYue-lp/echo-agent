//! Tool execution subsystem
//!
//! Centralized management of tool registration/execution, Skill, Hook, MCP,
//! SubAgent, Sandbox, intervention callbacks, and other components directly
//! related to tool invocation.

#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentRegistry;
#[cfg(feature = "mcp")]
use crate::mcp::McpManager;
use crate::agent::InterventionCallback;
use crate::sandbox::SandboxManager;
use crate::skills::SkillRegistry;
use crate::skills::hooks::HookRegistry;
use crate::skills::registry::SharedRegistry;
#[cfg(feature = "tasks")]
use crate::tasks::TaskManager;
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
    #[cfg(feature = "tasks")]
    pub(crate) task_manager: Arc<TaskManager>,
    pub(crate) skill_registry: SkillRegistry,
    pub(crate) progressive_skill_registry: Option<SharedRegistry>,
    pub(crate) hook_registry: Arc<tokio::sync::RwLock<HookRegistry>>,
    #[cfg(feature = "mcp")]
    pub(crate) mcp_manager: McpManager,
    pub(crate) sandbox_manager: Option<Arc<SandboxManager>>,
    /// Intervention callbacks that can influence agent behavior before
    /// tool calls, LLM reasoning, and final answers.
    pub(crate) intervention_callbacks: Vec<Arc<dyn InterventionCallback>>,
}

impl ToolExecutionSubsystem {
    /// Return a clone of the tool manager Arc (for StreamRunner construction).
    #[allow(dead_code)]
    pub(crate) fn tool_manager_arc(&self) -> Arc<ToolManager> {
        Arc::clone(&self.tool_manager)
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

    #[cfg(feature = "tasks")]
    pub(crate) fn task_manager(&self) -> Option<Arc<TaskManager>> {
        Some(Arc::clone(&self.task_manager))
    }

    #[allow(dead_code)]
    pub(crate) fn progressive_skill_registry(&self) -> Option<SharedRegistry> {
        self.progressive_skill_registry.clone()
    }
}
