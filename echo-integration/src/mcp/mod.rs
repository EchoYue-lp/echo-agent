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
pub mod resource_tool;
pub mod server;
pub mod server_config;
pub mod tool_adapter;
pub mod transport;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

pub use client::McpClient;
pub use config_loader::{
    AGENT_PLUGIN_MCP_SCHEMA_V1, AgentPluginMcpLoad, McpConfigFile, McpServerEntry,
};
pub use resource_tool::{
    LIST_MCP_RESOURCE_TEMPLATES_TOOL, LIST_MCP_RESOURCES_TOOL, MCP_RESOURCE_TOOL_NAMES,
    READ_MCP_RESOURCE_TOOL, build_mcp_resource_tools,
};
pub use server::McpServer;
pub use server_config::{McpServerConfig, TransportConfig};
pub use tool_adapter::McpToolAdapter;
pub use types::{
    McpContent, McpPrompt, McpPromptGetResult, McpResource, McpResourceReadResult,
    McpResourceTemplate, McpResourceTemplatesListResult, McpTool, McpToolCallResult,
    ServerCapabilities,
};

use echo_core::error::Result;
use echo_core::tools::Tool;

/// 多 MCP 服务端连接管理器
///
/// 按需连接服务端，获取工具列表后注册到 Agent：
/// ```rust,no_run
/// # async fn example() -> echo_core::error::Result<()> {
/// use echo_integration::mcp::{McpManager, McpServerConfig};
///
/// let mut manager = McpManager::new();
/// let tools = manager.connect(McpServerConfig::stdio(
///     "filesystem",
///     "npx",
///     vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
/// )).await?;
/// # Ok(())
/// # }
/// ```
pub struct McpManager {
    clients: HashMap<String, Arc<McpClient>>,
    configs: HashMap<String, McpServerConfig>,
}

/// Topology change performed by [`McpManager::reconcile_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTargetChange {
    Unchanged,
    Connected,
    Replaced,
    Disconnected,
    Absent,
}

/// Typed receipt for reconciling one named MCP target.
pub struct McpTargetReceipt {
    pub name: String,
    pub change: McpTargetChange,
    pub tools: Vec<Box<dyn Tool>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            configs: HashMap::new(),
        }
    }

    /// 连接到一个 MCP 服务端
    ///
    /// 返回该服务端提供的所有工具（已适配为框架 `Tool` trait），
    /// 可直接传递给 `ReactAgent::register_tools()`。
    ///
    /// 如果已存在同名连接，会先断开旧连接再建立新连接。
    pub async fn connect(&mut self, config: McpServerConfig) -> Result<Vec<Box<dyn Tool>>> {
        let name = config.name.clone();
        Ok(self.reconcile_target(&name, Some(config)).await?.tools)
    }

    /// Reconcile one named server against an optional desired configuration.
    ///
    /// An unchanged target keeps its live connection. A replacement is fully
    /// connected and initialized before the old client is swapped out, so a
    /// failed prepare preserves the last-known-good connection. `None` removes
    /// the target idempotently.
    pub async fn reconcile_target(
        &mut self,
        name: &str,
        desired: Option<McpServerConfig>,
    ) -> Result<McpTargetReceipt> {
        let Some(config) = desired else {
            let change = if self.disconnect(name).await {
                McpTargetChange::Disconnected
            } else {
                McpTargetChange::Absent
            };
            return Ok(McpTargetReceipt {
                name: name.to_string(),
                change,
                tools: Vec::new(),
            });
        };
        if config.name != name {
            return Err(echo_core::error::ReactError::Other(format!(
                "MCP reconcile target '{name}' does not match config name '{}'",
                config.name
            )));
        }
        if self.configs.get(name) == Some(&config)
            && let Some(client) = self.clients.get(name)
        {
            return Ok(McpTargetReceipt {
                name: name.to_string(),
                change: McpTargetChange::Unchanged,
                tools: Self::tools_for_client(name, client),
            });
        }

        let client = McpClient::new(config.clone()).await?;
        let tools = Self::tools_for_client(name, &client);
        let previous = self.clients.insert(name.to_string(), client);
        self.configs.insert(name.to_string(), config);
        let change = if let Some(previous) = previous {
            previous.close().await;
            McpTargetChange::Replaced
        } else {
            McpTargetChange::Connected
        };
        Ok(McpTargetReceipt {
            name: name.to_string(),
            change,
            tools,
        })
    }

    fn tools_for_client(name: &str, client: &Arc<McpClient>) -> Vec<Box<dyn Tool>> {
        client
            .tools()
            .iter()
            .map(|tool| {
                Box::new(McpToolAdapter::with_server_name(
                    Arc::clone(client),
                    tool.clone(),
                    name.to_string(),
                )) as Box<dyn Tool>
            })
            .collect()
    }

    /// 从配置文件连接多个服务端
    ///
    /// # 示例
    /// ```rust,no_run
    /// # async fn example() -> echo_core::error::Result<()> {
    /// use echo_integration::mcp::{McpManager, McpConfigFile};
    ///
    /// let mut manager = McpManager::new();
    /// let config = McpConfigFile::from_file("mcp.json")?;
    /// let all_tools = manager.connect_from_config(&config).await?;
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
            .iter()
            .flat_map(|(server_name, client)| {
                client.tools().iter().map(|tool| {
                    Box::new(McpToolAdapter::with_server_name(
                        client.clone(),
                        tool.clone(),
                        server_name.clone(),
                    )) as Box<dyn Tool>
                })
            })
            .collect()
    }

    /// 获取指定服务端的客户端引用
    pub fn get_client(&self, name: &str) -> Option<&Arc<McpClient>> {
        self.clients.get(name)
    }

    /// 获取所有已连接客户端的快照（用于 hook 执行器等场景）。
    pub fn get_clients(&self) -> HashMap<String, Arc<McpClient>> {
        self.clients.clone()
    }

    /// Build the canonical model-callable Resource tools for current connections.
    pub fn resource_tools(&self) -> Vec<Box<dyn Tool>> {
        build_mcp_resource_tools(self.get_clients())
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
        self.configs.remove(name);
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

#[cfg(test)]
mod tests {
    use futures::future::BoxFuture;

    use super::transport::McpTransport;
    use super::types::{
        JsonRpcNotification, JsonRpcNotificationReceiver, JsonRpcRequest, JsonRpcResponse,
    };
    use super::*;

    struct InertTransport;

    impl McpTransport for InertTransport {
        fn send(&self, _request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
            Box::pin(async {
                Err(echo_core::error::ReactError::Other(
                    "inert test transport cannot send".to_string(),
                ))
            })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }

        fn notification_rx(&self) -> Option<Arc<dyn JsonRpcNotificationReceiver>> {
            None
        }
    }

    #[test]
    fn resource_tool_projection_follows_manager_topology() {
        let mut manager = McpManager::new();
        assert!(manager.resource_tools().is_empty());

        manager.clients.insert(
            "context".to_string(),
            McpClient::with_test_transport("context", Arc::new(InertTransport)),
        );
        let mut names = manager
            .resource_tools()
            .into_iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                LIST_MCP_RESOURCE_TEMPLATES_TOOL.to_string(),
                LIST_MCP_RESOURCES_TOOL.to_string(),
                READ_MCP_RESOURCE_TOOL.to_string(),
            ]
        );

        manager.clients.remove("context");
        assert!(manager.resource_tools().is_empty());
    }

    #[tokio::test]
    async fn reconcile_keeps_unchanged_target_and_disconnects_idempotently() {
        let mut manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let client = McpClient::with_test_transport("context", Arc::new(InertTransport));
        manager
            .clients
            .insert("context".to_string(), Arc::clone(&client));
        manager
            .configs
            .insert("context".to_string(), config.clone());

        let unchanged = manager
            .reconcile_target("context", Some(config))
            .await
            .expect("unchanged reconcile");
        assert_eq!(unchanged.change, McpTargetChange::Unchanged);
        assert!(Arc::ptr_eq(
            manager.get_client("context").expect("client retained"),
            &client
        ));

        let disconnected = manager
            .reconcile_target("context", None)
            .await
            .expect("disconnect reconcile");
        assert_eq!(disconnected.change, McpTargetChange::Disconnected);
        assert!(manager.get_client("context").is_none());
        let absent = manager
            .reconcile_target("context", None)
            .await
            .expect("idempotent disconnect");
        assert_eq!(absent.change, McpTargetChange::Absent);
    }
}
