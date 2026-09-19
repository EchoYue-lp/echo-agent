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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

pub use client::{
    McpClient, McpClientCleanupOwner, McpClientPreparationError, McpClientPreparationResult,
};
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

use echo_core::error::{McpError, ReactError, Result};
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
    state: Mutex<McpManagerState>,
    close_gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct McpManagerState {
    clients: HashMap<String, Arc<McpClient>>,
    configs: HashMap<String, McpServerConfig>,
    cleanup_debt: Vec<(String, Arc<McpClient>)>,
    prepared: HashMap<u64, (String, Arc<McpClient>)>,
    closing_debt: HashSet<String>,
    next_prepared_ticket: u64,
    close_callers: usize,
}

struct CloseAllOwner<'a> {
    manager: &'a McpManager,
    armed: bool,
}

impl CloseAllOwner<'_> {
    fn complete(&mut self, state: &mut McpManagerState) {
        state.close_callers = state.close_callers.saturating_sub(1);
        self.armed = false;
    }
}

impl Drop for CloseAllOwner<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut state = self.manager.state();
            state.close_callers = state.close_callers.saturating_sub(1);
        }
    }
}

struct PreparedClientOwner<'a> {
    manager: &'a McpManager,
    name: String,
    client: Arc<McpClient>,
    ticket: u64,
    armed: bool,
}

impl PreparedClientOwner<'_> {
    fn is_registered(&self) -> bool {
        self.manager.state().prepared.contains_key(&self.ticket)
    }

    fn disarm(&mut self) {
        self.manager.state().prepared.remove(&self.ticket);
        self.armed = false;
    }

    async fn close(&mut self) -> Result<()> {
        self.client.close().await?;
        self.disarm();
        Ok(())
    }
}

impl Drop for PreparedClientOwner<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut state = self.manager.state();
            if state.prepared.remove(&self.ticket).is_some() {
                McpManager::retain_cleanup_debt(&mut state, &self.name, &self.client);
            }
        }
    }
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
            state: Mutex::new(McpManagerState::default()),
            close_gate: tokio::sync::Mutex::new(()),
        }
    }

    fn state(&self) -> MutexGuard<'_, McpManagerState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
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
            let change = if self.disconnect(name).await? {
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
        {
            let _close_guard = self.close_gate.lock().await;
            self.settle_active_close_debt(name).await?;
        }
        let unchanged = {
            let state = self.state();
            if state.configs.get(name) == Some(&config)
                && let Some(client) = state.clients.get(name)
            {
                Some(Arc::clone(client))
            } else {
                None
            }
        };
        if let Some(client) = unchanged {
            self.settle_same_name_cleanup_debt(name).await?;
            return Ok(McpTargetReceipt {
                name: name.to_string(),
                change: McpTargetChange::Unchanged,
                tools: Self::tools_for_client(name, &client),
            });
        }

        let client = match McpClient::new(config.clone()).await {
            Ok(client) => client,
            Err(error) => return Err(self.retain_preparation_failure(name, error)),
        };
        self.install_prepared_target(name, config, client).await
    }

    /// Publish an already-prepared client as the named target.
    ///
    /// Public so adapters that prepare targets through custom transports
    /// (tests, SDK bridges) share the exact replacement/cleanup admission
    /// contract with [`Self::reconcile_target`].
    pub fn install_prepared_target<'a>(
        &'a self,
        name: &'a str,
        config: McpServerConfig,
        client: Arc<McpClient>,
    ) -> impl std::future::Future<Output = Result<McpTargetReceipt>> + Send + 'a {
        // This owner must exist before the future's first poll. An embedding
        // adapter may cancel an already prepared target without polling us.
        let admission = {
            let mut state = self.state();
            if let Some(authority) = Self::client_authority(&state, &client) {
                Err(ReactError::Other(format!(
                    "MCP prepared target '{name}' aliases a client already owned by {authority}"
                )))
            } else {
                let mut ticket = state.next_prepared_ticket;
                while state.prepared.contains_key(&ticket) {
                    ticket = ticket.wrapping_add(1);
                }
                state.next_prepared_ticket = ticket.wrapping_add(1);
                state
                    .prepared
                    .insert(ticket, (name.to_string(), Arc::clone(&client)));
                Ok(ticket)
            }
        };
        let prepared_owner = admission.map(|ticket| PreparedClientOwner {
            manager: self,
            name: name.to_string(),
            client: Arc::clone(&client),
            ticket,
            armed: true,
        });
        async move {
            let mut prepared_owner = match prepared_owner {
                Ok(owner) => owner,
                Err(error) => return Err(error),
            };
            let _close_guard = self.close_gate.lock().await;
            if !prepared_owner.is_registered() {
                return Err(ReactError::Other(format!(
                    "MCP prepared target '{name}' was closed before installation"
                )));
            }
            if self.state().close_callers > 0 {
                let cleanup = prepared_owner.close().await.err();
                let cleanup_detail = cleanup
                    .map(|error| format!("; prepared client cleanup failed: {error}"))
                    .unwrap_or_default();
                return Err(ReactError::Other(format!(
                    "MCP prepared target '{name}' was rejected while close_all fenced admission{cleanup_detail}"
                )));
            }
            if config.name != name || client.server_name() != name {
                let error = ReactError::Other("MCP prepared target identity mismatch".into());
                return Err(Self::reject_prepared(&mut prepared_owner, error).await);
            }
            if let Err(error) = self.settle_active_close_debt(name).await {
                return Err(Self::reject_prepared(&mut prepared_owner, error).await);
            }
            if let Err(error) = self.settle_same_name_cleanup_debt(name).await {
                return Err(Self::reject_prepared(&mut prepared_owner, error).await);
            }
            if self.same_name_prepared_conflict(name, prepared_owner.ticket) {
                let error = ReactError::Other(format!(
                    "MCP target '{name}' has another unsettled prepared client"
                ));
                return Err(Self::reject_prepared(&mut prepared_owner, error).await);
            }
            let tools = Self::tools_for_client(name, &client);
            let previous = {
                let mut state = self.state();
                state.configs.remove(name);
                let previous = state.clients.remove(name);
                state.closing_debt.remove(name);
                if let Some(previous) = &previous {
                    Self::retain_cleanup_debt(&mut state, name, previous);
                }
                previous
            };
            let change = if let Some(previous) = previous {
                if let Err(error) = previous.close().await {
                    let prepared_cleanup = prepared_owner.close().await;
                    let prepared_detail = prepared_cleanup
                        .err()
                        .map(|cleanup| format!("; prepared replacement cleanup failed: {cleanup}"))
                        .unwrap_or_default();
                    return Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                        format!(
                            "MCP target '{name}' was withdrawn, but the previous transport did not settle: {error}{prepared_detail}"
                        ),
                    ))));
                }
                Self::remove_cleanup_debt(&mut self.state(), &previous);
                McpTargetChange::Replaced
            } else {
                McpTargetChange::Connected
            };
            let published = {
                let mut state = self.state();
                if Self::has_same_name_prepared_conflict(&state, name, prepared_owner.ticket)
                    || state
                        .cleanup_debt
                        .iter()
                        .any(|(debt_name, _)| debt_name == name)
                {
                    false
                } else {
                    state.prepared.remove(&prepared_owner.ticket);
                    state.clients.insert(name.to_string(), Arc::clone(&client));
                    state.configs.insert(name.to_string(), config);
                    true
                }
            };
            if !published {
                let error = ReactError::Other(format!(
                    "MCP target '{name}' gained unsettled cleanup debt before publication"
                ));
                return Err(Self::reject_prepared(&mut prepared_owner, error).await);
            }
            prepared_owner.disarm();
            Ok(McpTargetReceipt {
                name: name.to_string(),
                change,
                tools,
            })
        }
    }

    fn has_same_name_prepared_conflict(
        state: &McpManagerState,
        name: &str,
        current_ticket: u64,
    ) -> bool {
        state.prepared.iter().any(|(ticket, (candidate_name, _))| {
            *ticket != current_ticket && candidate_name == name
        })
    }

    fn same_name_prepared_conflict(&self, name: &str, current_ticket: u64) -> bool {
        Self::has_same_name_prepared_conflict(&self.state(), name, current_ticket)
    }

    fn client_authority(state: &McpManagerState, client: &Arc<McpClient>) -> Option<String> {
        if let Some((name, _)) = state
            .clients
            .iter()
            .find(|(_, existing)| Arc::ptr_eq(existing, client))
        {
            return Some(format!("active target '{name}'"));
        }
        if let Some((_, (name, _))) = state
            .prepared
            .iter()
            .find(|(_, (_, existing))| Arc::ptr_eq(existing, client))
        {
            return Some(format!("prepared target '{name}'"));
        }
        state
            .cleanup_debt
            .iter()
            .find(|(_, existing)| Arc::ptr_eq(existing, client))
            .map(|(name, _)| format!("cleanup debt for target '{name}'"))
    }

    /// Retry withdrawn clients for one name without closing its active target.
    /// A newer owner may already have published a different client under that name.
    pub async fn retry_cleanup_debt(&self, name: &str) -> Result<()> {
        let _close_guard = self.close_gate.lock().await;
        self.settle_same_name_cleanup_debt(name).await
    }

    /// Settle every cleanup debt owed by `name` before a new target publishes.
    ///
    /// Returns Ok only when no same-name debt remains (either none existed or
    /// every retry close succeeded). Errors aggregate but never publish.
    async fn settle_same_name_cleanup_debt(&self, name: &str) -> Result<()> {
        let debt_clients = {
            let state = self.state();
            state
                .cleanup_debt
                .iter()
                .filter(|(debt_name, _)| debt_name == name)
                .map(|(_, client)| Arc::clone(client))
                .collect::<Vec<_>>()
        };
        let mut failures = Vec::new();
        for client in debt_clients {
            match client.close().await {
                Ok(()) => Self::remove_cleanup_debt(&mut self.state(), &client),
                Err(error) => {
                    failures.push(error.to_string());
                    Self::retain_cleanup_debt(&mut self.state(), name, &client);
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!(
                    "MCP target '{name}' replacement blocked: previous cleanup debt did not settle: {}",
                    failures.join("; ")
                ),
            ))))
        }
    }

    async fn settle_active_close_debt(&self, name: &str) -> Result<()> {
        let client = {
            let state = self.state();
            if !state.closing_debt.contains(name) {
                return Ok(());
            }
            state.clients.get(name).cloned()
        };
        let Some(client) = client else {
            self.state().closing_debt.remove(name);
            return Ok(());
        };
        match client.close().await {
            Ok(()) => {
                let mut state = self.state();
                if state
                    .clients
                    .get(name)
                    .is_some_and(|active| Arc::ptr_eq(active, &client))
                {
                    state.clients.remove(name);
                    state.configs.remove(name);
                }
                state.closing_debt.remove(name);
                Ok(())
            }
            Err(error) => Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!("MCP target '{name}' close debt did not settle: {error}"),
            )))),
        }
    }

    async fn reject_prepared(
        prepared_owner: &mut PreparedClientOwner<'_>,
        rejection: ReactError,
    ) -> ReactError {
        match prepared_owner.close().await {
            Ok(()) => rejection,
            Err(cleanup_error) => ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                "{rejection}; prepared client cleanup failed and remains retryable: {cleanup_error}"
            )))),
        }
    }

    fn retain_cleanup_debt(state: &mut McpManagerState, name: &str, client: &Arc<McpClient>) {
        if !state
            .cleanup_debt
            .iter()
            .any(|(_, existing)| Arc::ptr_eq(existing, client))
        {
            state
                .cleanup_debt
                .push((name.to_string(), Arc::clone(client)));
        }
    }

    fn retain_preparation_failure(
        &self,
        name: &str,
        error: McpClientPreparationError,
    ) -> ReactError {
        let (initialization_error, cleanup_debt) = error.into_parts();
        let Some((cleanup_error, cleanup_owner)) = cleanup_debt else {
            return initialization_error;
        };
        Self::retain_cleanup_debt(&mut self.state(), name, &cleanup_owner.into_client());
        ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
            "MCP target '{name}' preparation failed: {initialization_error}; transport cleanup remains retryable after: {cleanup_error}"
        ))))
    }

    fn remove_cleanup_debt(state: &mut McpManagerState, client: &Arc<McpClient>) {
        state
            .cleanup_debt
            .retain(|(_, existing)| !Arc::ptr_eq(existing, client));
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
        self.state()
            .clients
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
    pub fn get_client(&self, name: &str) -> Option<Arc<McpClient>> {
        self.state().clients.get(name).cloned()
    }

    /// 获取所有已连接客户端的快照（用于 hook 执行器等场景）。
    pub fn get_clients(&self) -> HashMap<String, Arc<McpClient>> {
        self.state().clients.clone()
    }

    /// Build the canonical model-callable Resource tools for current connections.
    pub fn resource_tools(&self) -> Vec<Box<dyn Tool>> {
        build_mcp_resource_tools(self.get_clients())
    }

    /// 列出所有已连接的服务端名称
    pub fn server_names(&self) -> Vec<String> {
        self.state().clients.keys().cloned().collect()
    }

    #[cfg(test)]
    fn cleanup_debt_count(&self) -> usize {
        self.state().cleanup_debt.len()
    }

    #[cfg(test)]
    fn prepared_count(&self) -> usize {
        self.state().prepared.len()
    }

    /// 关闭所有服务端连接
    pub fn close_all(&self) -> impl std::future::Future<Output = Result<()>> + Send + '_ {
        {
            let mut state = self.state();
            state.close_callers = state.close_callers.saturating_add(1);
        }
        let mut close_owner = CloseAllOwner {
            manager: self,
            armed: true,
        };
        async move {
            let _close_guard = self.close_gate.lock().await;
            loop {
                let clients = {
                    let mut state = self.state();
                    let prepared = std::mem::take(&mut state.prepared);
                    for (_, (name, client)) in prepared {
                        Self::retain_cleanup_debt(&mut state, &name, &client);
                    }
                    let mut clients = state
                        .clients
                        .iter()
                        .map(|(name, client)| (name.clone(), Arc::clone(client)))
                        .collect::<Vec<_>>();
                    for (name, client) in &state.cleanup_debt {
                        if !clients
                            .iter()
                            .any(|(_, existing)| Arc::ptr_eq(existing, client))
                        {
                            clients.push((name.clone(), Arc::clone(client)));
                        }
                    }
                    if clients.is_empty() {
                        state.configs.clear();
                        state.closing_debt.clear();
                        close_owner.complete(&mut state);
                        None
                    } else {
                        Some(clients)
                    }
                };
                let Some(clients) = clients else {
                    return Ok(());
                };
                let mut failures = Vec::new();
                for (name, client) in clients {
                    tracing::info!("MCP: 关闭服务端 '{}'", name);
                    {
                        let mut state = self.state();
                        if state
                            .clients
                            .get(&name)
                            .is_some_and(|active| Arc::ptr_eq(active, &client))
                        {
                            // Mark the published target unhealthy before the
                            // fallible await. Cancellation must not let a
                            // partially fenced transport report Unchanged.
                            state.closing_debt.insert(name.clone());
                        }
                    }
                    let close_result = client.close().await;
                    let mut state = self.state();
                    let active = state
                        .clients
                        .get(&name)
                        .is_some_and(|active| Arc::ptr_eq(active, &client));
                    Self::remove_cleanup_debt(&mut state, &client);
                    match close_result {
                        Ok(()) => {
                            if active {
                                state.clients.remove(&name);
                                state.configs.remove(&name);
                            }
                            state.closing_debt.remove(&name);
                        }
                        Err(error) => {
                            failures.push(format!("{name}: {error}"));
                            if active {
                                state.closing_debt.insert(name.clone());
                            } else {
                                Self::retain_cleanup_debt(&mut state, &name, &client);
                            }
                        }
                    }
                }
                if !failures.is_empty() {
                    let error = ReactError::Mcp(Box::new(McpError::ConnectionFailed(format!(
                        "MCP close_all failed to settle {} transport(s): {}",
                        failures.len(),
                        failures.join("; ")
                    ))));
                    close_owner.complete(&mut self.state());
                    return Err(error);
                }
            }
        }
    }

    /// 断开指定服务端连接
    ///
    /// 关闭连接并从管理器中移除。成功返回 true，服务端不存在返回 false。
    pub async fn disconnect(&mut self, name: &str) -> Result<bool> {
        let clients = {
            let mut state = self.state();
            state.configs.remove(name);
            let mut clients = state
                .clients
                .get(name)
                .into_iter()
                .map(|client| (name.to_string(), Arc::clone(client)))
                .collect::<Vec<_>>();
            for (debt_name, client) in &state.cleanup_debt {
                if debt_name == name
                    && !clients
                        .iter()
                        .any(|(_, existing)| Arc::ptr_eq(existing, client))
                {
                    clients.push((debt_name.clone(), Arc::clone(client)));
                }
            }
            clients
        };
        if clients.is_empty() {
            return Ok(false);
        }
        let mut failures = Vec::new();
        for (debt_name, client) in clients {
            tracing::info!("MCP: 断开服务端 '{}'", name);
            let close_result = client.close().await;
            let mut state = self.state();
            if state
                .clients
                .get(name)
                .is_some_and(|active| Arc::ptr_eq(active, &client))
            {
                state.clients.remove(name);
                state.closing_debt.remove(name);
            }
            Self::remove_cleanup_debt(&mut state, &client);
            if let Err(error) = close_result {
                failures.push(error.to_string());
                Self::retain_cleanup_debt(&mut state, &debt_name, &client);
            }
        }
        if failures.is_empty() {
            Ok(true)
        } else {
            Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                format!(
                    "MCP target '{name}' cleanup did not settle: {}",
                    failures.join("; ")
                ),
            ))))
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::transport::McpTransport;
    use super::types::{
        JsonRpcNotification, JsonRpcNotificationReceiver, JsonRpcRequest, JsonRpcResponse,
    };
    use super::*;

    struct InertTransport;

    struct RecordingCloseTransport {
        close_count: Arc<AtomicUsize>,
        failures_remaining: Arc<AtomicUsize>,
    }

    struct BlockingCloseTransport {
        close_count: Arc<AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

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

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn notification_rx(&self) -> Option<Arc<dyn JsonRpcNotificationReceiver>> {
            None
        }
    }

    impl McpTransport for RecordingCloseTransport {
        fn send(&self, _request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
            Box::pin(async {
                Err(ReactError::Other(
                    "recording test transport cannot send".to_string(),
                ))
            })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            self.close_count.fetch_add(1, Ordering::AcqRel);
            let fail = self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok();
            Box::pin(async move {
                if fail {
                    Err(ReactError::Mcp(Box::new(McpError::ConnectionFailed(
                        "injected close failure".to_string(),
                    ))))
                } else {
                    Ok(())
                }
            })
        }

        fn notification_rx(&self) -> Option<Arc<dyn JsonRpcNotificationReceiver>> {
            None
        }
    }

    impl McpTransport for BlockingCloseTransport {
        fn send(&self, _request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
            Box::pin(async { Err(ReactError::Other("blocked test transport".to_string())) })
        }

        fn notify(&self, _notification: JsonRpcNotification) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, Result<()>> {
            self.close_count.fetch_add(1, Ordering::AcqRel);
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            Box::pin(async move {
                started.notify_waiters();
                release.notified().await;
                Ok(())
            })
        }

        fn notification_rx(&self) -> Option<Arc<dyn JsonRpcNotificationReceiver>> {
            None
        }
    }

    #[test]
    fn resource_tool_projection_follows_manager_topology() {
        let manager = McpManager::new();
        assert!(manager.resource_tools().is_empty());

        manager.state().clients.insert(
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

        manager.state().clients.remove("context");
        assert!(manager.resource_tools().is_empty());
    }

    #[tokio::test]
    async fn reconcile_keeps_unchanged_target_and_disconnects_idempotently() -> Result<()> {
        let mut manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let client = McpClient::with_test_transport("context", Arc::new(InertTransport));
        {
            let mut state = manager.state();
            state
                .clients
                .insert("context".to_string(), Arc::clone(&client));
            state.configs.insert("context".to_string(), config.clone());
        }

        let unchanged = manager.reconcile_target("context", Some(config)).await?;
        assert_eq!(unchanged.change, McpTargetChange::Unchanged);
        let retained = manager
            .get_client("context")
            .ok_or_else(|| ReactError::Other("unchanged client was not retained".to_string()))?;
        assert!(Arc::ptr_eq(&retained, &client));

        let disconnected = manager.reconcile_target("context", None).await?;
        assert_eq!(disconnected.change, McpTargetChange::Disconnected);
        assert!(manager.get_client("context").is_none());
        let absent = manager.reconcile_target("context", None).await?;
        assert_eq!(absent.change, McpTargetChange::Absent);
        Ok(())
    }

    #[tokio::test]
    async fn unchanged_reconcile_settles_same_name_prepared_cleanup_debt() -> Result<()> {
        let mut manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let active = McpClient::with_test_transport("context", Arc::new(InertTransport));
        {
            let mut state = manager.state();
            state
                .clients
                .insert("context".to_string(), Arc::clone(&active));
            state.configs.insert("context".to_string(), config.clone());
        }
        let debt_close_count = Arc::new(AtomicUsize::new(0));
        let prepared = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&debt_close_count),
                failures_remaining: Arc::new(AtomicUsize::new(1)),
            }),
        );
        let unpolled = manager.install_prepared_target("context", config.clone(), prepared);
        drop(unpolled);
        assert_eq!(manager.cleanup_debt_count(), 1);

        assert!(
            manager
                .reconcile_target("context", Some(config.clone()))
                .await
                .is_err()
        );
        let retained = manager
            .get_client("context")
            .ok_or_else(|| ReactError::Other("active target was withdrawn".to_string()))?;
        assert!(Arc::ptr_eq(&retained, &active));
        assert_eq!(manager.cleanup_debt_count(), 1);

        let receipt = manager.reconcile_target("context", Some(config)).await?;
        assert_eq!(receipt.change, McpTargetChange::Unchanged);
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(debt_close_count.load(Ordering::Acquire), 2);
        manager.close_all().await?;
        Ok(())
    }

    #[tokio::test]
    async fn retry_cleanup_debt_never_closes_same_name_active_client() -> Result<()> {
        let manager = McpManager::new();
        let active_close_count = Arc::new(AtomicUsize::new(0));
        let active = McpClient::with_test_transport(
            "shared",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&active_close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let debt_close_count = Arc::new(AtomicUsize::new(0));
        let debt = McpClient::with_test_transport(
            "shared",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&debt_close_count),
                failures_remaining: Arc::new(AtomicUsize::new(1)),
            }),
        );
        {
            let mut state = manager.state();
            state
                .clients
                .insert("shared".to_string(), Arc::clone(&active));
            state.cleanup_debt.push(("shared".to_string(), debt));
        }

        assert!(manager.retry_cleanup_debt("shared").await.is_err());
        assert_eq!(manager.cleanup_debt_count(), 1);
        assert_eq!(debt_close_count.load(Ordering::Acquire), 1);
        assert_eq!(active_close_count.load(Ordering::Acquire), 0);
        assert!(
            manager
                .get_client("shared")
                .is_some_and(|client| Arc::ptr_eq(&client, &active))
        );

        manager.retry_cleanup_debt("shared").await?;
        manager.retry_cleanup_debt("shared").await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(debt_close_count.load(Ordering::Acquire), 2);
        assert_eq!(active_close_count.load(Ordering::Acquire), 0);
        assert!(
            manager
                .get_client("shared")
                .is_some_and(|client| Arc::ptr_eq(&client, &active))
        );

        manager.close_all().await?;
        assert_eq!(active_close_count.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn prepared_install_rejects_active_client_alias_without_closing_it() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let client = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .state()
            .clients
            .insert("context".to_string(), Arc::clone(&client));

        assert!(
            manager
                .install_prepared_target("context", config, Arc::clone(&client))
                .await
                .is_err()
        );
        let active = manager
            .get_client("context")
            .ok_or_else(|| ReactError::Other("active alias was removed".to_string()))?;
        assert!(Arc::ptr_eq(&active, &client));
        assert_eq!(manager.prepared_count(), 0);
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 0);

        manager.close_all().await?;
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn prepared_install_rejects_cross_name_alias_without_closing_it() -> Result<()> {
        let manager = McpManager::new();
        let close_count = Arc::new(AtomicUsize::new(0));
        let client = McpClient::with_test_transport(
            "source",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );
        manager
            .state()
            .clients
            .insert("source".to_string(), Arc::clone(&client));
        let config = McpServerConfig::stdio("destination", "test-command", Vec::<String>::new());

        assert!(
            manager
                .install_prepared_target("destination", config, Arc::clone(&client))
                .await
                .is_err()
        );
        let active = manager
            .get_client("source")
            .ok_or_else(|| ReactError::Other("cross-name alias was removed".to_string()))?;
        assert!(Arc::ptr_eq(&active, &client));
        assert!(manager.get_client("destination").is_none());
        assert_eq!(manager.prepared_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 0);

        manager.close_all().await?;
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn prepared_rejection_reports_failed_cleanup_and_retains_retry_debt() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let mismatched = McpClient::with_test_transport(
            "different-name",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(1)),
            }),
        );

        let error = manager
            .install_prepared_target("context", config, mismatched)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("identity mismatch was accepted".to_string()))?;
        assert!(error.to_string().contains("cleanup failed"));
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        assert_eq!(manager.cleanup_debt_count(), 1);

        manager.close_all().await?;
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn second_prepared_install_cannot_claim_or_close_the_first_client() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let client = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let first = manager.install_prepared_target("context", config.clone(), Arc::clone(&client));
        assert_eq!(manager.prepared_count(), 1);

        assert!(
            manager
                .install_prepared_target("context", config, Arc::clone(&client))
                .await
                .is_err()
        );
        assert_eq!(manager.prepared_count(), 1);
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 0);

        drop(first);
        assert_eq!(manager.prepared_count(), 0);
        assert_eq!(manager.cleanup_debt_count(), 1);
        manager.close_all().await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn close_all_drains_topology_before_same_config_reconcile() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let original = McpClient::with_test_transport("context", Arc::new(InertTransport));
        {
            let mut state = manager.state();
            state
                .clients
                .insert("context".to_string(), Arc::clone(&original));
            state.configs.insert("context".to_string(), config.clone());
        }

        manager.close_all().await?;
        assert!(manager.get_client("context").is_none());
        assert!(manager.server_names().is_empty());
        assert!(manager.state().configs.is_empty());

        let replacement = McpClient::with_test_transport("context", Arc::new(InertTransport));
        let receipt = manager
            .install_prepared_target("context", config, Arc::clone(&replacement))
            .await?;
        assert_eq!(receipt.change, McpTargetChange::Connected);
        let connected = manager
            .get_client("context")
            .ok_or_else(|| ReactError::Other("replacement client was not published".to_string()))?;
        assert!(Arc::ptr_eq(&connected, &replacement));
        Ok(())
    }

    #[tokio::test]
    async fn close_all_attempts_every_client_before_aggregating_failures() -> Result<()> {
        let manager = McpManager::new();
        let failed_count = Arc::new(AtomicUsize::new(0));
        let healthy_count = Arc::new(AtomicUsize::new(0));
        {
            let mut state = manager.state();
            state.clients.insert(
                "failed".to_string(),
                McpClient::with_test_transport(
                    "failed",
                    Arc::new(RecordingCloseTransport {
                        close_count: Arc::clone(&failed_count),
                        failures_remaining: Arc::new(AtomicUsize::new(1)),
                    }),
                ),
            );
            state.clients.insert(
                "healthy".to_string(),
                McpClient::with_test_transport(
                    "healthy",
                    Arc::new(RecordingCloseTransport {
                        close_count: Arc::clone(&healthy_count),
                        failures_remaining: Arc::new(AtomicUsize::new(0)),
                    }),
                ),
            );
        }

        let result = manager.close_all().await;

        assert!(result.is_err());
        assert_eq!(failed_count.load(Ordering::Acquire), 1);
        assert_eq!(healthy_count.load(Ordering::Acquire), 1);
        assert_eq!(manager.server_names(), vec!["failed".to_string()]);
        assert_eq!(manager.cleanup_debt_count(), 0);
        manager.close_all().await?;
        assert_eq!(failed_count.load(Ordering::Acquire), 2);
        assert_eq!(healthy_count.load(Ordering::Acquire), 1);
        assert!(manager.server_names().is_empty());
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn same_config_reconcile_does_not_reuse_failed_close_all_target() -> Result<()> {
        let mut manager = McpManager::new();
        let config = McpServerConfig::stdio(
            "context",
            "echo-command-that-must-not-exist",
            Vec::<String>::new(),
        );
        let close_count = Arc::new(AtomicUsize::new(0));
        {
            let mut state = manager.state();
            state.clients.insert(
                "context".to_string(),
                McpClient::with_test_transport(
                    "context",
                    Arc::new(RecordingCloseTransport {
                        close_count: Arc::clone(&close_count),
                        failures_remaining: Arc::new(AtomicUsize::new(1)),
                    }),
                ),
            );
            state.configs.insert("context".to_string(), config.clone());
        }

        assert!(manager.close_all().await.is_err());
        assert!(manager.state().closing_debt.contains("context"));
        assert_eq!(close_count.load(Ordering::Acquire), 1);

        // Reconciliation first settles the fenced active transport. It then
        // attempts a fresh connection, which this deliberately invalid command
        // makes observable as an error rather than a false Unchanged receipt.
        assert!(
            manager
                .reconcile_target("context", Some(config))
                .await
                .is_err()
        );
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        assert!(manager.get_client("context").is_none());
        assert!(!manager.state().closing_debt.contains("context"));
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_close_all_marks_active_target_unhealthy_before_await() -> Result<()> {
        let manager = Arc::new(tokio::sync::Mutex::new(McpManager::new()));
        let config = McpServerConfig::stdio(
            "context",
            "echo-command-that-must-not-exist",
            Vec::<String>::new(),
        );
        let close_count = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        {
            let manager = manager.lock().await;
            let mut state = manager.state();
            state.clients.insert(
                "context".to_string(),
                McpClient::with_test_transport(
                    "context",
                    Arc::new(BlockingCloseTransport {
                        close_count: Arc::clone(&close_count),
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                    }),
                ),
            );
            state.configs.insert("context".to_string(), config.clone());
        }

        let close_started = started.notified();
        let close = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                let manager = manager.lock().await;
                manager.close_all().await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), close_started)
            .await
            .map_err(|_| ReactError::Other("close_all did not start".to_string()))?;
        close.abort();
        let join_error = close
            .await
            .err()
            .ok_or_else(|| ReactError::Other("cancelled close_all completed".to_string()))?;
        assert!(join_error.is_cancelled());

        let mut manager = manager.lock().await;
        assert!(manager.state().closing_debt.contains("context"));
        release.notify_one();
        assert!(
            manager
                .reconcile_target("context", Some(config))
                .await
                .is_err()
        );
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        assert!(manager.get_client("context").is_none());
        assert!(!manager.state().closing_debt.contains("context"));
        Ok(())
    }

    #[tokio::test]
    async fn manager_retains_failed_construction_cleanup_for_retry() -> Result<()> {
        let manager = McpManager::new();
        let close_count = Arc::new(AtomicUsize::new(0));
        let transport: Arc<dyn McpTransport> = Arc::new(RecordingCloseTransport {
            close_count: Arc::clone(&close_count),
            failures_remaining: Arc::new(AtomicUsize::new(1)),
        });
        let preparation = McpClient::from_transport("context", transport)
            .map_err(|error| ReactError::Other(error.to_string()))?;
        let preparation_error = preparation
            .await
            .err()
            .ok_or_else(|| ReactError::Other("failed construction was accepted".to_string()))?;
        let error = manager.retain_preparation_failure("context", preparation_error);

        assert!(error.to_string().contains("cleanup remains retryable"));
        assert_eq!(manager.cleanup_debt_count(), 1);
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        manager.close_all().await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn disconnect_retains_failed_client_for_retry() -> Result<()> {
        let mut manager = McpManager::new();
        let close_count = Arc::new(AtomicUsize::new(0));
        manager.state().clients.insert(
            "context".to_string(),
            McpClient::with_test_transport(
                "context",
                Arc::new(RecordingCloseTransport {
                    close_count: Arc::clone(&close_count),
                    failures_remaining: Arc::new(AtomicUsize::new(1)),
                }),
            ),
        );

        assert!(manager.disconnect("context").await.is_err());
        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.cleanup_debt_count(), 1);
        assert!(manager.disconnect("context").await?);
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn replacement_close_failure_does_not_publish_new_target() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let old_close_count = Arc::new(AtomicUsize::new(0));
        manager.state().clients.insert(
            "context".to_string(),
            McpClient::with_test_transport(
                "context",
                Arc::new(RecordingCloseTransport {
                    close_count: Arc::clone(&old_close_count),
                    failures_remaining: Arc::new(AtomicUsize::new(1)),
                }),
            ),
        );
        let replacement_close_count = Arc::new(AtomicUsize::new(0));
        let replacement = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&replacement_close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );

        assert!(
            manager
                .install_prepared_target("context", config, replacement)
                .await
                .is_err()
        );
        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.cleanup_debt_count(), 1);
        assert_eq!(old_close_count.load(Ordering::Acquire), 1);
        assert_eq!(replacement_close_count.load(Ordering::Acquire), 1);
        manager.close_all().await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn replacement_retry_does_not_publish_while_same_name_debt_remains() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        // The old target's close fails twice: once for the first replacement
        // attempt and once for the retry's debt settlement, before it finally
        // settles on the second retry.
        let old_close_count = Arc::new(AtomicUsize::new(0));
        manager.state().clients.insert(
            "context".to_string(),
            McpClient::with_test_transport(
                "context",
                Arc::new(RecordingCloseTransport {
                    close_count: Arc::clone(&old_close_count),
                    failures_remaining: Arc::new(AtomicUsize::new(2)),
                }),
            ),
        );

        // First replacement attempt: old close fails, target withdrawn and
        // both the failed previous client and the prepared client remain as
        // same-name cleanup debt.
        let first_close_count = Arc::new(AtomicUsize::new(0));
        let first = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&first_close_count),
                failures_remaining: Arc::new(AtomicUsize::new(1)),
            }),
        );
        assert!(
            manager
                .install_prepared_target("context", config.clone(), first)
                .await
                .is_err()
        );
        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.cleanup_debt_count(), 2);

        // A retry replacement must not publish while that debt remains. The
        // retry's admission first settles the debt: the old client's close
        // fails again (second failure) while the prepared client's close
        // succeeds, so admission still fails without publishing.
        let retry = McpClient::with_test_transport("context", Arc::new(InertTransport));
        assert!(
            manager
                .install_prepared_target("context", config.clone(), retry)
                .await
                .is_err()
        );
        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.cleanup_debt_count(), 1);

        // Retry until the debt settles; only then may a replacement publish.
        let settled = McpClient::with_test_transport("context", Arc::new(InertTransport));
        let receipt = manager
            .install_prepared_target("context", config, settled)
            .await?;
        assert_eq!(receipt.change, McpTargetChange::Connected);
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(old_close_count.load(Ordering::Acquire), 3);
        assert_eq!(first_close_count.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_close_all_keeps_client_owned_for_retry() -> Result<()> {
        let manager = Arc::new(McpManager::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        manager.state().clients.insert(
            "context".to_string(),
            McpClient::with_test_transport(
                "context",
                Arc::new(BlockingCloseTransport {
                    close_count: Arc::clone(&close_count),
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                }),
            ),
        );
        let started_wait = started.notified();
        let close_task = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close_all().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started_wait)
            .await
            .map_err(|_| ReactError::Other("close did not start".to_string()))?;
        close_task.abort();
        let _ = close_task.await;

        assert!(manager.get_client("context").is_some());
        let second_started = started.notified();
        let retry = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close_all().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), second_started)
            .await
            .map_err(|_| ReactError::Other("retry close did not start".to_string()))?;
        release.notify_waiters();
        retry
            .await
            .map_err(|error| ReactError::Other(format!("retry join failed: {error}")))??;
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        assert!(manager.get_client("context").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_prepared_replacement_retains_both_clients_for_retry() -> Result<()> {
        let manager = Arc::new(McpManager::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        {
            let mut state = manager.state();
            state.clients.insert(
                "context".to_string(),
                McpClient::with_test_transport(
                    "context",
                    Arc::new(BlockingCloseTransport {
                        close_count: Arc::clone(&close_count),
                        started: Arc::clone(&started),
                        release: Arc::clone(&release),
                    }),
                ),
            );
            state.configs.insert("context".to_string(), config.clone());
        }
        let prepared = McpClient::with_test_transport("context", Arc::new(InertTransport));
        let first_started = started.notified();
        let install = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move {
                manager
                    .install_prepared_target("context", config, prepared)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), first_started)
            .await
            .map_err(|_| ReactError::Other("replacement close did not start".to_string()))?;
        install.abort();
        let _join_result = install.await;

        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.cleanup_debt_count(), 2);
        let retry_started = started.notified();
        let retry = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close_all().await }
        });
        tokio::time::timeout(Duration::from_secs(1), retry_started)
            .await
            .map_err(|_| ReactError::Other("replacement retry did not start".to_string()))?;
        release.notify_waiters();
        retry
            .await
            .map_err(|error| ReactError::Other(format!("retry join failed: {error}")))??;
        assert_eq!(close_count.load(Ordering::Acquire), 2);
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn unpolled_prepared_installation_retains_the_client_for_cleanup() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let prepared = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let installation = manager.install_prepared_target("context", config, prepared);
        drop(installation);

        assert_eq!(manager.cleanup_debt_count(), 1);
        manager.close_all().await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn close_all_settles_a_prepared_client_before_its_future_is_polled() -> Result<()> {
        let manager = McpManager::new();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let prepared = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );
        let installation = manager.install_prepared_target("context", config, prepared);
        assert_eq!(manager.prepared_count(), 1);
        manager.close_all().await?;
        assert_eq!(manager.prepared_count(), 0);
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert_eq!(close_count.load(Ordering::Acquire), 1);

        assert!(installation.await.is_err());
        assert_eq!(manager.cleanup_debt_count(), 0);
        assert!(manager.get_client("context").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn created_close_all_future_fences_prepared_publication_before_poll() -> Result<()> {
        let manager = McpManager::new();
        let close = manager.close_all();
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let prepared = McpClient::with_test_transport(
            "context",
            Arc::new(RecordingCloseTransport {
                close_count: Arc::clone(&close_count),
                failures_remaining: Arc::new(AtomicUsize::new(0)),
            }),
        );

        let installation = manager.install_prepared_target("context", config, prepared);
        assert!(installation.await.is_err());
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.prepared_count(), 0);

        close.await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn prepared_client_drop_during_replacement_blocks_publication() -> Result<()> {
        let manager = Arc::new(McpManager::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let config = McpServerConfig::stdio("context", "test-command", Vec::<String>::new());
        manager.state().clients.insert(
            "context".to_string(),
            McpClient::with_test_transport(
                "context",
                Arc::new(BlockingCloseTransport {
                    close_count: Arc::clone(&close_count),
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                }),
            ),
        );
        let replacement = McpClient::with_test_transport("context", Arc::new(InertTransport));
        let closing = started.notified();
        let install = tokio::spawn({
            let manager = Arc::clone(&manager);
            let config = config.clone();
            async move {
                manager
                    .install_prepared_target("context", config, replacement)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), closing)
            .await
            .map_err(|_| ReactError::Other("previous close did not start".to_string()))?;

        let cancelled = McpClient::with_test_transport("context", Arc::new(InertTransport));
        let unpolled = manager.install_prepared_target("context", config, cancelled);
        drop(unpolled);
        assert_eq!(manager.cleanup_debt_count(), 2);
        release.notify_one();
        assert!(
            install
                .await
                .map_err(|error| ReactError::Other(error.to_string()))?
                .is_err()
        );
        assert!(manager.get_client("context").is_none());
        assert_eq!(manager.cleanup_debt_count(), 1);
        manager.close_all().await?;
        assert_eq!(manager.cleanup_debt_count(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_close_all_waits_for_the_active_settlement() -> Result<()> {
        let manager = Arc::new(McpManager::new());
        let close_count = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        manager.state().clients.insert(
            "context".to_string(),
            McpClient::with_test_transport(
                "context",
                Arc::new(BlockingCloseTransport {
                    close_count: Arc::clone(&close_count),
                    started: Arc::clone(&started),
                    release: Arc::clone(&release),
                }),
            ),
        );
        let started_wait = started.notified();
        let first = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close_all().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started_wait)
            .await
            .map_err(|_| ReactError::Other("first close did not start".to_string()))?;
        let second = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.close_all().await }
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished());

        release.notify_waiters();
        first
            .await
            .map_err(|error| ReactError::Other(format!("first close join failed: {error}")))??;
        second
            .await
            .map_err(|error| ReactError::Other(format!("second close join failed: {error}")))??;
        assert_eq!(close_count.load(Ordering::Acquire), 1);
        Ok(())
    }
}
