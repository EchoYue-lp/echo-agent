//! MCP (Model Context Protocol) façade.
//!
//! Re-exports the public API of `echo_integration::mcp`.
//! The authoritative implementation lives in `echo_integration`.

/// Direct re-exports from `echo_integration::mcp`.
pub mod integration {
    pub use echo_integration::mcp::*;
}

pub use echo_integration::mcp::{
    AGENT_PLUGIN_MCP_SCHEMA_V1, AgentPluginMcpLoad, LIST_MCP_RESOURCE_TEMPLATES_TOOL,
    LIST_MCP_RESOURCES_TOOL, MCP_RESOURCE_TOOL_NAMES, McpClient, McpConfigFile, McpManager,
    McpServer, McpServerConfig, McpServerEntry, McpTargetChange, McpTargetReceipt, McpToolAdapter,
    READ_MCP_RESOURCE_TOOL, TransportConfig, build_mcp_resource_tools,
};
pub use echo_integration::mcp::{
    McpContent, McpPrompt, McpPromptGetResult, McpResource, McpResourceReadResult,
    McpResourceTemplate, McpResourceTemplatesListResult, McpTool, McpToolCallResult,
    ServerCapabilities,
};
