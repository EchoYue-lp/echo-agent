//! MCP（Model Context Protocol）客户端
//!
//! 完整实现 MCP 协议，支持：
//! - **Tools**: 工具发现与调用
//! - **Resources**: 资源列表与读取
//! - **Prompts**: 提示词列表与获取
//!
//! 支持的传输层：
//! - **STDIO**: 本地子进程通信
//! - **HTTP**: Streamable HTTP（基本）
//! - **StreamableHttp**: Streamable HTTP（完整，支持会话管理）
//! - **SSE**: 旧版 HTTP+SSE
//!
//! 通过 [`McpManager`] 统一管理多个服务端连接。

pub mod client;
pub mod config_loader;
pub mod server_config;
pub(crate) mod tool_adapter;
pub mod transport;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

pub use client::McpClient;
pub use config_loader::{McpConfigFile, McpServerEntry};
pub use server_config::{McpServerConfig, TransportConfig};
pub use tool_adapter::McpToolAdapter;
pub use types::{
    McpContent, McpPrompt, McpPromptGetResult, McpResource, McpResourceReadResult, McpTool,
    McpToolCallResult, ServerCapabilities,
};

use crate::error::Result;
use crate::tools::Tool;

/// 多 MCP 服务端连接管理器
///
/// 按需连接服务端，获取工具列表后注册到 Agent：
/// ```rust,no_run
/// # async fn example() -> echo_agent::error::Result<()> {
/// use echo_agent::prelude::*;
///
/// let mut manager = McpManager::new();
/// let tools = manager.connect(McpServerConfig::stdio(
///     "filesystem",
///     "npx",
///     vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
/// )).await?;
///
/// // 将工具注册到 Agent
/// let mut agent = ReactAgent::new(AgentConfig::minimal("qwen3-max", "你是一个助手"));
/// agent.add_tools(tools);
/// # Ok(())
/// # }
/// ```
pub struct McpManager {
    clients: HashMap<String, Arc<McpClient>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// 连接到一个 MCP 服务端
    ///
    /// 返回该服务端提供的所有工具（已适配为框架 `Tool` trait），
    /// 可直接传递给 `ReactAgent::register_tools()`。
    pub async fn connect(&mut self, config: McpServerConfig) -> Result<Vec<Box<dyn Tool>>> {
        let name = config.name.clone();
        let client = McpClient::new(config).await?;

        let tools = client
            .tools()
            .iter()
            .map(|tool| {
                Box::new(McpToolAdapter::new(client.clone(), tool.clone())) as Box<dyn Tool>
            })
            .collect::<Vec<_>>();

        self.clients.insert(name, client);
        Ok(tools)
    }

    /// 从配置文件连接多个服务端
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn example() -> echo_agent::error::Result<()> {
    /// use echo_agent::mcp::{McpManager, McpConfigFile};
    /// use echo_agent::prelude::*;
    ///
    /// let mut manager = McpManager::new();
    /// let config = McpConfigFile::from_file("mcp.json")?;
    /// let all_tools = manager.connect_from_config(&config).await?;
    ///
    /// // 将工具注册到 Agent
    /// let mut agent = ReactAgent::new(AgentConfig::minimal("qwen3-max", "你是一个助手"));
    /// agent.add_tools(all_tools);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect_from_config(
        &mut self,
        config: &McpConfigFile,
    ) -> Result<Vec<Box<dyn Tool>>> {
        let configs = config.to_server_configs()?;
        let mut all_tools = Vec::new();
        for cfg in configs {
            let tools = self.connect(cfg).await?;
            all_tools.extend(tools);
        }
        Ok(all_tools)
    }

    /// 获取所有已连接服务端的全部工具
    pub fn get_all_tools(&self) -> Vec<Box<dyn Tool>> {
        self.clients
            .values()
            .flat_map(|client| {
                client.tools().iter().map(|tool| {
                    Box::new(McpToolAdapter::new(client.clone(), tool.clone())) as Box<dyn Tool>
                })
            })
            .collect()
    }

    /// 获取指定服务端的客户端引用
    pub fn get_client(&self, name: &str) -> Option<&Arc<McpClient>> {
        self.clients.get(name)
    }

    /// 列出所有已连接的服务端名称
    pub fn server_names(&self) -> Vec<&str> {
        self.clients.keys().map(|s| s.as_str()).collect()
    }

    /// 关闭所有服务端连接
    pub async fn close_all(&self) {
        for (name, client) in &self.clients {
            tracing::info!("MCP: 关闭服务端 '{}'", name);
            client.close().await;
        }
    }

    /// 断开指定服务端连接
    ///
    /// 关闭连接并从管理器中移除。成功返回 true，服务端不存在返回 false。
    pub async fn disconnect(&mut self, name: &str) -> bool {
        if let Some(client) = self.clients.remove(name) {
            tracing::info!("MCP: 断开服务端 '{}'", name);
            client.close().await;
            true
        } else {
            false
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
