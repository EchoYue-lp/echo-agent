//! MCP (Model Context Protocol) façade.
//!
//! Re-exports the public API of `echo_integration::mcp`.
//! The authoritative implementation lives in `echo_integration`.

/// Direct re-exports from `echo_integration::mcp`.
pub mod integration {
    pub use echo_integration::mcp::*;
}

pub use echo_integration::mcp::{
    AGENT_PLUGIN_MCP_SCHEMA_V1, AgentPluginMcpLoad, McpClient, McpConfigFile, McpManager,
    McpServer, McpServerConfig, McpServerEntry, McpToolAdapter, TransportConfig,
};
pub use echo_integration::mcp::{
    McpContent, McpPrompt, McpPromptGetResult, McpResource, McpResourceReadResult, McpTool,
    McpToolCallResult, ServerCapabilities,
};
