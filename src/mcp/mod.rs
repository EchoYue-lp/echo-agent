pub mod client;
pub mod server_config;
pub(crate) mod tool_adapter;
pub mod transport;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

pub use client::McpClient;
pub use server_config::{McpServerConfig, TransportConfig};
pub use tool_adapter::McpToolAdapter;
pub use types::{McpContent, McpTool, McpToolCallResult};

use crate::error::Result;
use crate::tools::Tool;

/// MCP 管理器
///
/// 统一管理多个 MCP 服务端的连接生命周期。
/// 典型用法：
/// ```
/// let mut manager = McpManager::new();
///
/// // 连接文件系统服务端，获取它提供的工具
/// let tools = manager.connect(McpServerConfig::stdio(
///     "filesystem",
///     "npx",
///     vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
/// )).await?;
///
/// // 将工具注册到 Agent
/// agent.register_tools(tools);
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
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
