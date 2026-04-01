//! MCP 协议客户端
//!
//! 实现 Model Context Protocol (MCP)，支持通过 stdio、SSE、HTTP 三种传输方式
//! 连接 MCP 服务器，将远端工具自动注册为 Agent 可用工具。

pub use echo_mcp::{
    McpClient, McpConfigFile, McpManager, McpServer, McpServerConfig, McpServerEntry,
    McpToolAdapter, TransportConfig,
};
pub use echo_mcp::{
    McpContent, McpPrompt, McpPromptGetResult, McpResource, McpResourceReadResult, McpTool,
    McpToolCallResult, ServerCapabilities,
};
