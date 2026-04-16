//! MCP façade
//!
//! 此模块只重导出 `echo_integration::mcp` 的公共 API。
//! 权威实现位于 `echo_integration`；如需直接依赖拆分后的 crate，
//! 可使用 [`crate::workspace::integration::mcp`]。

/// Direct re-exports from `echo_integration::mcp`.
pub mod integration {
    pub use echo_integration::mcp::*;
}

pub use echo_integration::mcp::{
    McpClient, McpConfigFile, McpManager, McpServer, McpServerConfig, McpServerEntry,
    McpToolAdapter, TransportConfig,
};
pub use echo_integration::mcp::{
    McpContent, McpPrompt, McpPromptGetResult, McpResource, McpResourceReadResult, McpTool,
    McpToolCallResult, ServerCapabilities,
};
