//! Tool execution subsystem
//!
//! Centralized management of tool registration/execution, Skill, Hook, MCP,
//! SubAgent, Sandbox, and other components directly related to tool invocation.

#[cfg(feature = "subagent")]
use crate::agent::subagent::SubagentRegistry;
#[cfg(feature = "mcp")]
use crate::mcp::McpManager;
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
/// Skill/Hook system, MCP connection management, SubAgent scheduling, and sandbox
/// environment.
pub(crate) struct ToolExecutionSubsystem {
    pub(crate) tool_manager: ToolManager,
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
}
