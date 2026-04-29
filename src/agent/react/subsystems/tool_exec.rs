//! 工具执行子系统
//!
//! 集中管理工具注册/执行、Skill、Hook、MCP、SubAgent、Sandbox 等与工具调用
//! 直接相关的组件。

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

/// 工具执行子系统
///
/// 聚合所有与工具调用直接相关的组件：工具注册表、Skill/Hook 系统、
/// MCP 连接管理、SubAgent 调度、沙箱环境。
pub(crate) struct ToolExecutionSubsystem {
    pub(crate) tool_manager: ToolManager,
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
