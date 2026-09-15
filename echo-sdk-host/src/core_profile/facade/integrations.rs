//! Integration family adapters: MCP, A2A, LSP, topology (plan 07, todo 5).
//!
//! Each family resource-izes the framework's own client/manager and calls
//! its real verbs; connection semantics, protocol behavior and failures
//! stay with the framework integrations.
//!
//! Channels are bound when both the framework channel feature and the
//! extension bridge are compiled. Telemetry has its own process-scoped
//! adapter in `facade::telemetry`; it is intentionally not represented by a
//! manager resource in this module.

#[cfg(feature = "framework-lsp")]
use echo_agent::lsp::LspClient as _;
use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::handle::WireHandle;
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::{WirePath, WireU64, WireValue};
#[cfg(feature = "framework-a2a")]
use futures::StreamExt as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::handles::HandleRegistry;
use super::super::wire;

const METHOD: &str = "_echo_agent/integration/op";

/// One MCP manager resource. Connect takes `&mut` and awaits, so the
/// lock must stay Send across the await (tokio Mutex).
pub(crate) struct McpManagerRecord {
    pub manager: tokio::sync::Mutex<echo_agent::mcp::McpManager>,
}

/// One A2A client resource.
#[cfg(feature = "framework-a2a")]
pub(crate) struct A2aClientRecord {
    pub client: echo_agent::a2a::A2AClient,
}

/// One LSP manager resource; status_all awaits under the lock.
#[cfg(feature = "framework-lsp")]
pub(crate) struct LspManagerRecord {
    pub manager: tokio::sync::Mutex<echo_agent::lsp::LspManager>,
}

/// One topology tracker resource.
#[cfg(feature = "framework-topology")]
pub(crate) struct TopologyRecord {
    pub tracker: echo_agent::topology::TopologyTracker,
}

#[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
pub(crate) struct ChannelManagerRecord {
    pub manager: tokio::sync::Mutex<echo_agent::channels::ChannelManager>,
}

/// All integration family resources of one Host connection.
pub(crate) struct IntegrationResources {
    #[cfg(feature = "framework-mcp")]
    pub mcp: Mutex<HashMap<String, Arc<McpManagerRecord>>>,
    #[cfg(feature = "framework-mcp")]
    pub mcp_clients: Mutex<HashMap<String, Arc<echo_agent::mcp::McpClient>>>,
    #[cfg(feature = "framework-mcp")]
    pub mcp_tools: Mutex<HashMap<String, Arc<dyn echo_agent::tools::Tool>>>,
    #[cfg(feature = "framework-a2a")]
    pub a2a: Mutex<HashMap<String, Arc<A2aClientRecord>>>,
    #[cfg(feature = "framework-lsp")]
    pub lsp: Mutex<HashMap<String, Arc<LspManagerRecord>>>,
    #[cfg(feature = "framework-lsp")]
    pub lsp_clients:
        Mutex<HashMap<String, Arc<tokio::sync::RwLock<echo_agent::lsp::StdioLspClient>>>>,
    #[cfg(feature = "framework-topology")]
    pub topology: Mutex<HashMap<String, Arc<TopologyRecord>>>,
    #[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
    pub channels: Mutex<HashMap<String, Arc<ChannelManagerRecord>>>,
}

impl IntegrationResources {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "framework-mcp")]
            mcp: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-mcp")]
            mcp_clients: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-mcp")]
            mcp_tools: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-a2a")]
            a2a: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-lsp")]
            lsp: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-lsp")]
            lsp_clients: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-topology")]
            topology: Mutex::new(HashMap::new()),
            #[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// Close every integration resource (connection teardown, bounded by
    /// the caller's shutdown timeout). MCP managers await `close_all` so
    /// connected child processes are taken down, not just dropped.
    pub async fn close_all(&self) {
        #[cfg(feature = "framework-mcp")]
        {
            let managers =
                std::mem::take(&mut *self.mcp.lock().unwrap_or_else(|error| error.into_inner()));
            for (_, record) in managers {
                record.manager.lock().await.close_all().await;
            }
            let clients = std::mem::take(
                &mut *self
                    .mcp_clients
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            );
            for (_, client) in clients {
                client.close().await;
            }
            self.mcp_tools
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        #[cfg(feature = "framework-a2a")]
        {
            self.a2a
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        #[cfg(feature = "framework-lsp")]
        {
            let managers =
                std::mem::take(&mut *self.lsp.lock().unwrap_or_else(|error| error.into_inner()));
            for (_, record) in managers {
                record.manager.lock().await.shutdown_all().await;
            }
            self.lsp_clients
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        #[cfg(feature = "framework-topology")]
        {
            self.topology
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
        #[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
        {
            let managers = std::mem::take(
                &mut *self
                    .channels
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
            );
            for (_, record) in managers {
                let _ = record.manager.lock().await.stop_all().await;
            }
        }
    }
}

fn invalid(message: impl Into<String>) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::InvalidValue,
        message,
        Retryability::Never,
        METHOD,
    )
}

fn framework(message: impl std::fmt::Display) -> EchoSdkError {
    wire::sdk_error(
        ExtensionErrorCode::FrameworkError,
        wire::bounded_framework_message(&message.to_string()),
        Retryability::Never,
        METHOD,
    )
}

/// Owner-checked MCP manager lookup.
fn mcp_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<McpManagerRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    let record = resources
        .mcp
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown MCP manager {}", resource.id)))?;
    Ok(record)
}

#[cfg(feature = "framework-mcp")]
fn mcp_client_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    client: &WireHandle,
    owner: &str,
) -> Result<Arc<echo_agent::mcp::McpClient>, EchoSdkError> {
    let record = super::owned_resource(handles, client, owner, METHOD)?;
    if record.resource_type != "mcp.client" {
        return Err(invalid("facade resource is not an MCP client"));
    }
    resources
        .mcp_clients
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&client.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown MCP client resource {}", client.id)))
}

#[cfg(feature = "framework-mcp")]
pub(crate) fn register_mcp_client(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    client: Arc<echo_agent::mcp::McpClient>,
    owner: &str,
    max_resources: usize,
) -> Result<WireHandle, EchoSdkError> {
    let (handle, _) = handles.register_facade_resource(
        max_resources,
        "mcp",
        "mcp.client",
        Some(owner),
        METHOD,
    )?;
    resources
        .mcp_clients
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(handle.id.clone(), client);
    Ok(handle)
}

#[cfg(feature = "framework-mcp")]
fn mcp_tool_snapshot(
    tool: &dyn echo_agent::tools::Tool,
    handle: &WireHandle,
    client: Option<&WireHandle>,
) -> Result<WireValue, EchoSdkError> {
    let parameters = WireValue::from_json(tool.parameters())
        .map_err(|error| invalid(format!("MCP tool schema is not wire-safe: {error}")))?;
    let mut fields = vec![
        echo_sdk_protocol::scalar::WireField {
            name: "tool".to_string(),
            value: WireValue::Handle(handle.clone()),
        },
        echo_sdk_protocol::scalar::WireField {
            name: "name".to_string(),
            value: WireValue::String(tool.name().to_string()),
        },
        echo_sdk_protocol::scalar::WireField {
            name: "description".to_string(),
            value: WireValue::String(tool.description().to_string()),
        },
        echo_sdk_protocol::scalar::WireField {
            name: "parameters".to_string(),
            value: parameters,
        },
        echo_sdk_protocol::scalar::WireField {
            name: "schema_revision".to_string(),
            value: WireValue::U64(WireU64::from_u64(tool.schema_revision())),
        },
    ];
    if let Some(client) = client {
        fields.push(echo_sdk_protocol::scalar::WireField {
            name: "client".to_string(),
            value: WireValue::Handle(client.clone()),
        });
    }
    Ok(WireValue::Record {
        type_id: "echo_sdk_protocol::McpToolReference".to_string(),
        fields,
    })
}

#[cfg(feature = "framework-mcp")]
fn register_mcp_tools(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    tools: Vec<Box<dyn echo_agent::tools::Tool>>,
    owner: &str,
    max_resources: usize,
    client: Option<&WireHandle>,
) -> Result<Vec<WireValue>, EchoSdkError> {
    let mut registered = Vec::new();
    let result = (|| {
        let mut snapshots = Vec::with_capacity(tools.len());
        for tool in tools {
            let (handle, _) = handles.register_facade_resource(
                max_resources,
                "mcp",
                "mcp.tool",
                Some(owner),
                METHOD,
            )?;
            let tool: Arc<dyn echo_agent::tools::Tool> = Arc::from(tool);
            let snapshot = mcp_tool_snapshot(tool.as_ref(), &handle, client)?;
            resources
                .mcp_tools
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(handle.id.clone(), tool);
            registered.push(handle);
            snapshots.push(snapshot);
        }
        Ok(snapshots)
    })();
    if result.is_err() {
        let mut tools = resources
            .mcp_tools
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for handle in registered {
            tools.remove(&handle.id);
            let _ = handles.close_facade_resource(&handle, METHOD);
        }
    }
    result
}

#[cfg(feature = "framework-mcp")]
fn mcp_tool_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    tool: &WireHandle,
    owner: &str,
) -> Result<Arc<dyn echo_agent::tools::Tool>, EchoSdkError> {
    let record = super::owned_resource(handles, tool, owner, METHOD)?;
    if record.resource_type != "mcp.tool" {
        return Err(invalid("facade resource is not an executable MCP tool"));
    }
    resources
        .mcp_tools
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&tool.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown MCP tool resource {}", tool.id)))
}

#[cfg(feature = "framework-a2a")]
/// Owner-checked A2A client lookup.
fn a2a_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<A2aClientRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    let record = resources
        .a2a
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown A2A client {}", resource.id)))?;
    Ok(record)
}

#[cfg(feature = "framework-lsp")]
/// Owner-checked LSP manager lookup.
fn lsp_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<LspManagerRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    let record = resources
        .lsp
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown LSP manager {}", resource.id)))?;
    Ok(record)
}

#[cfg(feature = "framework-lsp")]
fn lsp_client_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<tokio::sync::RwLock<echo_agent::lsp::StdioLspClient>>, EchoSdkError> {
    let record = super::owned_resource(handles, resource, owner, METHOD)?;
    if record.family != "lsp" || record.resource_type != "lsp.client" {
        return Err(invalid("resource does not address an LSP client"));
    }
    resources
        .lsp_clients
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown LSP client {}", resource.id)))
}

#[cfg(feature = "framework-topology")]
/// Owner-checked topology tracker lookup.
fn topology_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<TopologyRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    let record = resources
        .topology
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown topology tracker {}", resource.id)))?;
    Ok(record)
}

#[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
fn channel_of(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    resource: &WireHandle,
    owner: &str,
) -> Result<Arc<ChannelManagerRecord>, EchoSdkError> {
    super::owned_resource(handles, resource, owner, METHOD)?;
    resources
        .channels
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&resource.id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown channel manager {}", resource.id)))
}

fn json_of(value: &WireValue, position: usize) -> Result<serde_json::Value, EchoSdkError> {
    value.clone().into_json().map_err(|error| {
        invalid(format!(
            "argument {position} is not a lossless wire value: {error}"
        ))
    })
}

fn string_at(
    arguments: &[serde_json::Value],
    position: usize,
    what: &str,
) -> Result<String, EchoSdkError> {
    arguments
        .get(position)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("operation requires {what} at argument {position}")))
}

#[cfg(feature = "framework-mcp")]
fn string_map(
    value: Option<&serde_json::Value>,
    field: &str,
) -> Result<HashMap<String, String>, EchoSdkError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(HashMap::new()),
        Some(serde_json::Value::Object(entries)) => entries
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| invalid(format!("MCP {field}.{key} must be a string")))
            })
            .collect(),
        Some(_) => Err(invalid(format!("MCP {field} must be an object"))),
    }
}

#[cfg(feature = "framework-mcp")]
fn mcp_server_config_at(
    arguments: &[serde_json::Value],
    position: usize,
) -> Result<echo_agent::mcp::McpServerConfig, EchoSdkError> {
    let object = arguments
        .get(position)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            invalid(format!(
                "MCP config at argument {position} must be an object"
            ))
        })?;
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid("MCP config requires a non-empty name"))?
        .to_string();
    let transport = object
        .get("transport")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("MCP config requires a transport object"))?;
    let kind = transport
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("MCP transport requires kind"))?;
    let transport = match kind {
        "stdio" => {
            let command = transport
                .get("command")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid("MCP stdio transport requires command"))?
                .to_string();
            let args = match transport.get("args") {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(serde_json::Value::Array(values)) => values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .ok_or_else(|| invalid("MCP stdio args must contain strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => return Err(invalid("MCP stdio args must be an array")),
            };
            let env = string_map(transport.get("env"), "transport.env")?
                .into_iter()
                .collect();
            let cwd = transport
                .get("cwd")
                .filter(|value| !value.is_null())
                .map(|value| {
                    serde_json::from_value::<WirePath>(value.clone())
                        .map_err(|error| invalid(format!("MCP stdio cwd is invalid: {error}")))
                        .and_then(|path| wire::path_from_wire(&path).map_err(invalid))
                })
                .transpose()?;
            echo_agent::mcp::TransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            }
        }
        "http" | "sse" => {
            let base_url = transport
                .get("base_url")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid("MCP HTTP/SSE transport requires base_url"))?
                .to_string();
            let headers = string_map(transport.get("headers"), "transport.headers")?;
            if kind == "http" {
                echo_agent::mcp::TransportConfig::Http { base_url, headers }
            } else {
                echo_agent::mcp::TransportConfig::Sse { base_url, headers }
            }
        }
        other => {
            return Err(invalid(format!(
                "unknown MCP transport {other}; expected stdio|http|sse"
            )));
        }
    };
    Ok(echo_agent::mcp::McpServerConfig { name, transport })
}

#[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
fn handle_at(
    arguments: &[serde_json::Value],
    position: usize,
    what: &str,
) -> Result<WireHandle, EchoSdkError> {
    arguments
        .get(position)
        .cloned()
        .ok_or_else(|| invalid(format!("operation requires {what} at argument {position}")))
        .and_then(|value| {
            serde_json::from_value(value).map_err(|error| {
                invalid(format!("{what} at argument {position} is invalid: {error}"))
            })
        })
}

/// Dispatch one integration family operation.
pub(crate) async fn dispatch(
    handles: &HandleRegistry,
    family: &str,
    resources: &IntegrationResources,
    _streams: &super::stream::FacadeStreamRuntime,
    owner: &str,
    request: &FeatureOperationRequest,
    max_resources: usize,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(error.to_string()))
    };
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(value, position))
        .collect::<Result<_, _>>()?;
    match family {
        #[cfg(feature = "framework-mcp")]
        "mcp" => match request.operation.as_str() {
            "mcp.client.open_exact" => {
                let config = mcp_server_config_at(&arguments, 0)?;
                let client = echo_agent::mcp::McpClient::new(config)
                    .await
                    .map_err(framework)?;
                let handle = register_mcp_client(handles, resources, client, owner, max_resources)?;
                Ok(WireValue::Handle(handle))
            }
            "mcp.manager.open" | "mcp.manager.open_exact" => {
                let (resource, _record) = handles.register_facade_resource(
                    max_resources,
                    "mcp",
                    "mcp.manager",
                    Some(owner),
                    METHOD,
                )?;
                resources
                    .mcp
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        resource.id.clone(),
                        Arc::new(McpManagerRecord {
                            manager: tokio::sync::Mutex::new(echo_agent::mcp::McpManager::new()),
                        }),
                    );
                if request.operation == "mcp.manager.open_exact" {
                    Ok(WireValue::Handle(resource))
                } else {
                    wire(serde_json::json!({"resource": resource}))
                }
            }
            "mcp.server.connect" | "mcp.server.connect_exact" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let config = if request.operation == "mcp.server.connect_exact" {
                    mcp_server_config_at(&arguments, 1)?
                } else {
                    let name = string_at(&arguments, 1, "a server name")?;
                    let mode = string_at(&arguments, 2, "a transport mode (stdio|http)")?;
                    match mode.as_str() {
                        "stdio" => {
                            let command = string_at(&arguments, 3, "a command")?;
                            let args: Vec<String> = arguments
                                .get(4)
                                .and_then(serde_json::Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|item| item.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default();
                            echo_agent::mcp::McpServerConfig::stdio(&name, &command, args)
                        }
                        "http" => {
                            let base_url = string_at(&arguments, 3, "a base URL")?;
                            echo_agent::mcp::McpServerConfig::http(&name, &base_url)
                        }
                        other => {
                            return Err(invalid(format!(
                                "unknown MCP transport {other}; expected stdio|http"
                            )));
                        }
                    }
                };
                let name = config.name.clone();
                let record = mcp_of(handles, resources, &resource, owner)?;
                let mut manager = record.manager.lock().await;
                let tools = manager.connect(config).await.map_err(framework)?;
                let client = manager
                    .get_client(&name)
                    .ok_or_else(|| framework("MCP client was not retained"))?;
                let client_handle = match register_mcp_client(
                    handles,
                    resources,
                    client.clone(),
                    owner,
                    max_resources,
                ) {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = manager.disconnect(&name).await;
                        return Err(error);
                    }
                };
                if request.operation == "mcp.server.connect_exact" {
                    match register_mcp_tools(
                        handles,
                        resources,
                        tools,
                        owner,
                        max_resources,
                        Some(&client_handle),
                    ) {
                        Ok(tools) => Ok(WireValue::List(tools)),
                        Err(error) => {
                            resources
                                .mcp_clients
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .remove(&client_handle.id);
                            let _ = handles.close_facade_resource(&client_handle, METHOD);
                            let _ = manager.disconnect(&name).await;
                            Err(error)
                        }
                    }
                } else {
                    wire(serde_json::json!({
                        "tools": tools.len(),
                        "client": client_handle,
                        "server_name": client.server_name(),
                    }))
                }
            }
            "mcp.server.list" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                wire(serde_json::json!({"servers": manager.server_names()}))
            }
            "mcp.manager.connect_from_config" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let config: echo_agent::mcp::McpConfigFile = arguments
                    .get(1)
                    .cloned()
                    .ok_or_else(|| invalid("McpManager::connect_from_config requires a config"))
                    .and_then(|value| {
                        serde_json::from_value(value).map_err(|error| {
                            invalid(format!("MCP config file is malformed: {error}"))
                        })
                    })?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                let tools = record
                    .manager
                    .lock()
                    .await
                    .connect_from_config(&config)
                    .await
                    .map_err(framework)?;
                Ok(WireValue::List(register_mcp_tools(
                    handles,
                    resources,
                    tools,
                    owner,
                    max_resources,
                    None,
                )?))
            }
            "mcp.manager.get_client" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let name = string_at(&arguments, 1, "a server name")?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                let Some(client) = manager.get_client(&name) else {
                    return Ok(WireValue::Variant {
                        type_id: "core::option::Option<McpClient>".to_string(),
                        variant: "none".to_string(),
                        fields: Vec::new(),
                    });
                };
                let client = register_mcp_client(handles, resources, client, owner, max_resources)?;
                Ok(WireValue::Variant {
                    type_id: "core::option::Option<McpClient>".to_string(),
                    variant: "some".to_string(),
                    fields: vec![echo_sdk_protocol::scalar::WireField {
                        name: "client".to_string(),
                        value: WireValue::Handle(client),
                    }],
                })
            }
            "mcp.manager.get_clients" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                let clients = record.manager.lock().await.get_clients();
                let mut entries = Vec::with_capacity(clients.len());
                let mut registered: Vec<WireHandle> = Vec::with_capacity(clients.len());
                for (name, client) in clients {
                    let client =
                        match register_mcp_client(handles, resources, client, owner, max_resources)
                        {
                            Ok(client) => client,
                            Err(error) => {
                                let mut clients = resources
                                    .mcp_clients
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner());
                                for registered in registered {
                                    clients.remove(&registered.id);
                                    let _ = handles.close_facade_resource(&registered, METHOD);
                                }
                                return Err(error);
                            }
                        };
                    registered.push(client.clone());
                    entries.push(echo_sdk_protocol::scalar::WireMapEntry {
                        key: WireValue::String(name),
                        value: WireValue::Handle(client),
                    });
                }
                Ok(WireValue::Map(entries))
            }
            "mcp.manager.server_names" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                Ok(WireValue::List(
                    record
                        .manager
                        .lock()
                        .await
                        .server_names()
                        .into_iter()
                        .map(WireValue::String)
                        .collect(),
                ))
            }
            "mcp.manager.get_all_tools" | "mcp.manager.resource_tools" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                let tools = if request.operation == "mcp.manager.get_all_tools" {
                    manager.get_all_tools()
                } else {
                    manager.resource_tools()
                };
                Ok(WireValue::List(register_mcp_tools(
                    handles,
                    resources,
                    tools,
                    owner,
                    max_resources,
                    None,
                )?))
            }
            "mcp.manager.reconcile_target" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let name = string_at(&arguments, 1, "a server name")?;
                let desired = match arguments.get(2) {
                    None | Some(serde_json::Value::Null) => None,
                    Some(value) => Some(mcp_server_config_at(std::slice::from_ref(value), 0)?),
                };
                let record = mcp_of(handles, resources, &resource, owner)?;
                let receipt = record
                    .manager
                    .lock()
                    .await
                    .reconcile_target(&name, desired)
                    .await
                    .map_err(framework)?;
                let change = match receipt.change {
                    echo_agent::mcp::McpTargetChange::Connected => "connected",
                    echo_agent::mcp::McpTargetChange::Replaced => "replaced",
                    echo_agent::mcp::McpTargetChange::Unchanged => "unchanged",
                    echo_agent::mcp::McpTargetChange::Disconnected => "disconnected",
                    echo_agent::mcp::McpTargetChange::Absent => "absent",
                };
                Ok(WireValue::Record {
                    type_id: "echo_integration::mcp::McpTargetReceipt".to_string(),
                    fields: vec![
                        echo_sdk_protocol::scalar::WireField {
                            name: "name".to_string(),
                            value: WireValue::String(receipt.name),
                        },
                        echo_sdk_protocol::scalar::WireField {
                            name: "change".to_string(),
                            value: WireValue::String(change.to_string()),
                        },
                        echo_sdk_protocol::scalar::WireField {
                            name: "tools".to_string(),
                            value: WireValue::List(register_mcp_tools(
                                handles,
                                resources,
                                receipt.tools,
                                owner,
                                max_resources,
                                None,
                            )?),
                        },
                    ],
                })
            }
            "mcp.manager.close_all" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                record.manager.lock().await.close_all().await;
                wire(serde_json::Value::Null)
            }
            "mcp.server.disconnect" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let name = string_at(&arguments, 1, "a server name")?;
                let record = mcp_of(handles, resources, &resource, owner)?;
                let mut manager = record.manager.lock().await;
                let disconnected = manager.disconnect(&name).await;
                if disconnected {
                    let client_ids = {
                        let clients = resources
                            .mcp_clients
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        clients
                            .iter()
                            .map(|(id, client)| (id.clone(), client.server_name().to_string()))
                            .collect::<Vec<_>>()
                    };
                    let client_ids = client_ids
                        .into_iter()
                        .filter(|(id, client_name)| {
                            client_name == &name
                                && handles
                                    .facade_resource(
                                        &WireHandle {
                                            id: id.clone(),
                                            generation: WireU64::from_u64(handles.generation()),
                                            kind: echo_sdk_protocol::handle::HandleKind::FacadeResource,
                                        },
                                        METHOD,
                                    )
                                    .ok()
                                    .and_then(|record| record.owner_session.clone())
                                    .as_deref()
                                    == Some(owner)
                        })
                        .map(|(id, _)| id)
                        .collect::<Vec<_>>();
                    for id in client_ids {
                        resources
                            .mcp_clients
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .remove(&id);
                        let handle = WireHandle {
                            id,
                            generation: WireU64::from_u64(handles.generation()),
                            kind: echo_sdk_protocol::handle::HandleKind::FacadeResource,
                        };
                        let _ = handles.close_facade_resource(&handle, METHOD);
                    }
                }
                wire(serde_json::json!({"disconnected": disconnected}))
            }
            "mcp.tool.execute" => {
                let tool_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP tool resource", METHOD)?;
                let parameters = arguments
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let parameters = serde_json::from_value(parameters).map_err(|error| {
                    invalid(format!("MCP tool parameters are malformed: {error}"))
                })?;
                let tool = mcp_tool_of(handles, resources, &tool_handle, owner)?;
                let result = tool.execute(parameters).await.map_err(framework)?;
                wire(serde_json::to_value(result).map_err(framework)?)
            }
            "echo_agent::mcp::McpClient::call_tool" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let name = string_at(&arguments, 1, "a MCP tool name")?;
                let tool_arguments = arguments
                    .get(2)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                let result = client
                    .call_tool(&name, tool_arguments)
                    .await
                    .map_err(framework)?;
                wire(serde_json::to_value(result).map_err(|error| invalid(error.to_string()))?)
            }
            "echo_agent::mcp::McpClient::get_prompt" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let name = string_at(&arguments, 1, "an MCP prompt name")?;
                let prompt_arguments = arguments
                    .get(2)
                    .cloned()
                    .map(serde_json::from_value::<std::collections::HashMap<String, String>>)
                    .transpose()
                    .map_err(|error| invalid(format!("prompt arguments are malformed: {error}")))?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                let result = client
                    .get_prompt(&name, prompt_arguments)
                    .await
                    .map_err(framework)?;
                wire(serde_json::to_value(result).map_err(|error| invalid(error.to_string()))?)
            }
            "echo_agent::mcp::McpClient::list_resource_templates" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                let result = client.list_resource_templates().await.map_err(framework)?;
                wire(serde_json::to_value(result).map_err(|error| invalid(error.to_string()))?)
            }
            "echo_agent::mcp::McpClient::list_resources" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                let result = client.list_resources().await.map_err(framework)?;
                wire(serde_json::to_value(result).map_err(|error| invalid(error.to_string()))?)
            }
            "echo_agent::mcp::McpClient::read_resource" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let uri = string_at(&arguments, 1, "a resource URI")?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                let result = client.read_resource(&uri).await.map_err(framework)?;
                wire(serde_json::to_value(result).map_err(|error| invalid(error.to_string()))?)
            }
            "echo_agent::mcp::McpClient::ping" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                client.ping().await.map_err(framework)?;
                wire(serde_json::Value::Null)
            }
            "echo_agent::mcp::McpClient::server_name" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(serde_json::json!(client.server_name()))
            }
            "echo_agent::mcp::McpClient::protocol_version" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(serde_json::json!(client.protocol_version()))
            }
            "echo_agent::mcp::McpClient::server_capabilities" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(
                    serde_json::to_value(client.server_capabilities())
                        .map_err(|error| invalid(error.to_string()))?,
                )
            }
            "echo_agent::mcp::McpClient::supports_resources" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(serde_json::json!(client.supports_resources()))
            }
            "echo_agent::mcp::McpClient::supports_prompts" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(serde_json::json!(client.supports_prompts()))
            }
            "echo_agent::mcp::McpClient::tools" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(
                    serde_json::to_value(client.tools())
                        .map_err(|error| invalid(error.to_string()))?,
                )
            }
            "echo_agent::mcp::McpClient::resources" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(
                    serde_json::to_value(client.resources())
                        .map_err(|error| invalid(error.to_string()))?,
                )
            }
            "echo_agent::mcp::McpClient::prompts" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                wire(
                    serde_json::to_value(client.prompts())
                        .map_err(|error| invalid(error.to_string()))?,
                )
            }
            "echo_agent::mcp::McpClient::close" => {
                let client_handle =
                    super::resource_handle_at(&arguments, 0, "an MCP client resource", METHOD)?;
                let client = mcp_client_of(handles, resources, &client_handle, owner)?;
                client.close().await;
                resources
                    .mcp_clients
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&client_handle.id);
                let _ = handles.close_facade_resource(&client_handle, METHOD);
                wire(serde_json::Value::Null)
            }
            other => Err(invalid(format!(
                "unknown mcp operation {other}; the family surface is closed"
            ))),
        },
        #[cfg(feature = "framework-a2a")]
        "a2a" => match request.operation.as_str() {
            "a2a.client.open" => {
                let (resource, _record) = handles.register_facade_resource(
                    max_resources,
                    "a2a",
                    "a2a.client",
                    Some(owner),
                    METHOD,
                )?;
                resources
                    .a2a
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        resource.id.clone(),
                        Arc::new(A2aClientRecord {
                            client: echo_agent::a2a::A2AClient::new(),
                        }),
                    );
                wire(serde_json::json!({"resource": resource}))
            }
            "a2a.discover" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an A2A client resource", METHOD)?;
                let base_url = string_at(&arguments, 1, "a base URL")?;
                let record = a2a_of(handles, resources, &resource, owner)?;
                let card = record.client.discover(&base_url).await.map_err(framework)?;
                wire(serde_json::to_value(&card).unwrap_or_else(|_| serde_json::Value::Null))
            }
            "a2a.task.send" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an A2A client resource", METHOD)?;
                let agent_url = string_at(&arguments, 1, "an agent URL")?;
                let message = string_at(&arguments, 2, "a message")?;
                let record = a2a_of(handles, resources, &resource, owner)?;
                let session_id = match arguments.get(3) {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
                        Some(value.clone())
                    }
                    _ => return Err(invalid("A2A task session id must be a string or null")),
                };
                let task = record
                    .client
                    .send_task_with_session(&agent_url, &message, session_id)
                    .await
                    .map_err(framework)?;
                wire(serde_json::to_value(&task).unwrap_or(serde_json::Value::Null))
            }
            "a2a.task.get" | "a2a.task.cancel" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an A2A client resource", METHOD)?;
                let agent_url = string_at(&arguments, 1, "an agent URL")?;
                let task_id = string_at(&arguments, 2, "a task id")?;
                let record = a2a_of(handles, resources, &resource, owner)?;
                let task = if request.operation == "a2a.task.cancel" {
                    record
                        .client
                        .cancel_task(&agent_url, &task_id)
                        .await
                        .map_err(framework)?
                } else {
                    record
                        .client
                        .get_task(&agent_url, &task_id)
                        .await
                        .map_err(framework)?
                };
                wire(serde_json::to_value(&task).unwrap_or(serde_json::Value::Null))
            }
            "a2a.task.stream.open" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an A2A client resource", METHOD)?;
                let agent_url = string_at(&arguments, 1, "an agent URL")?;
                let message = string_at(&arguments, 2, "a message")?;
                let session_id = match arguments.get(3) {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(value)) if !value.trim().is_empty() => {
                        Some(value.clone())
                    }
                    _ => return Err(invalid("A2A stream session id must be a string or null")),
                };
                let record = a2a_of(handles, resources, &resource, owner)?;
                let mut source = record
                    .client
                    .send_task_streaming_with_session(&agent_url, &message, session_id)
                    .await
                    .map_err(framework)?;
                let producer = _streams.open(handles, &resource, owner, &request.operation)?;
                let sender = producer.sender.clone();
                let cancel = producer.cancel.clone();
                let background = tokio::spawn(async move {
                    loop {
                        let item = tokio::select! {
                            () = cancel.cancelled() => break,
                            item = source.next() => item,
                        };
                        let Some(event) = item else { break };
                        let item = serde_json::to_value(event)
                            .map_err(|error| {
                                wire::sdk_error(
                                    ExtensionErrorCode::FrameworkError,
                                    format!("A2A stream event serialization failed: {error}"),
                                    Retryability::Never,
                                    "a2a.task.stream.open",
                                )
                            })
                            .and_then(|value| {
                                WireValue::from_json(value).map_err(|error| {
                                    wire::sdk_error(
                                        ExtensionErrorCode::FrameworkError,
                                        format!("A2A stream event projection failed: {error}"),
                                        Retryability::Never,
                                        "a2a.task.stream.open",
                                    )
                                })
                            });
                        tokio::select! {
                            () = cancel.cancelled() => break,
                            result = sender.send(item) => {
                                if result.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
                _streams.attach_background(&producer, background);
                Ok(WireValue::Handle(producer.handle))
            }
            other => Err(invalid(format!(
                "unknown a2a operation {other}; the family surface is closed"
            ))),
        },
        #[cfg(feature = "framework-lsp")]
        "lsp" => match request.operation.as_str() {
            "lsp.manager.open" | "lsp.manager.open_exact" => {
                let (resource, _record) = handles.register_facade_resource(
                    max_resources,
                    "lsp",
                    "lsp.manager",
                    Some(owner),
                    METHOD,
                )?;
                resources
                    .lsp
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        resource.id.clone(),
                        Arc::new(LspManagerRecord {
                            manager: tokio::sync::Mutex::new(echo_agent::lsp::LspManager::new()),
                        }),
                    );
                if request.operation == "lsp.manager.open_exact" {
                    Ok(WireValue::Handle(resource))
                } else {
                    wire(serde_json::json!({"resource": resource}))
                }
            }
            "lsp.manager.load_config" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP manager resource", METHOD)?;
                let config_file: echo_agent::lsp::LspConfigFile = arguments
                    .get(1)
                    .cloned()
                    .ok_or_else(|| invalid("lsp.manager.load_config requires a config"))
                    .and_then(|value| {
                        serde_json::from_value(value)
                            .map_err(|error| invalid(format!("LSP config is malformed: {error}")))
                    })?;
                let config = echo_agent::lsp::LspConfig {
                    servers: config_file.languages,
                };
                let record = lsp_of(handles, resources, &resource, owner)?;
                record.manager.lock().await.load_config(&config);
                wire(serde_json::Value::Null)
            }
            "lsp.manager.set_project_root" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP manager resource", METHOD)?;
                let path = arguments
                    .get(1)
                    .cloned()
                    .ok_or_else(|| invalid("lsp.manager.set_project_root requires a path"))
                    .and_then(|value| {
                        serde_json::from_value::<WirePath>(value).map_err(|error| {
                            invalid(format!("LSP project root is invalid: {error}"))
                        })
                    })
                    .and_then(|path| wire::path_from_wire(&path).map_err(invalid))?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                record.manager.lock().await.set_project_root(&path);
                wire(serde_json::Value::Null)
            }
            "lsp.manager.start_server"
            | "lsp.manager.stop_server"
            | "lsp.manager.restart_server" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP manager resource", METHOD)?;
                let language = string_at(&arguments, 1, "a language")?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                let mut manager = record.manager.lock().await;
                match request.operation.as_str() {
                    "lsp.manager.start_server" => manager.start_server(&language).await,
                    "lsp.manager.stop_server" => manager.stop_server(&language).await,
                    _ => manager.restart_server(&language).await,
                }
                .map_err(framework)?;
                wire(serde_json::Value::Null)
            }
            "lsp.manager.shutdown_all" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP manager resource", METHOD)?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                record.manager.lock().await.shutdown_all().await;
                wire(serde_json::Value::Null)
            }
            "lsp.manager.get_client" | "lsp.manager.get_client_for_file" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP manager resource", METHOD)?;
                let selector = string_at(
                    &arguments,
                    1,
                    if request.operation == "lsp.manager.get_client" {
                        "a language"
                    } else {
                        "a file path"
                    },
                )?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                let selected = if request.operation == "lsp.manager.get_client" {
                    manager.get_client(&selector).map(|client| (None, client))
                } else {
                    manager
                        .get_client_for_file(&selector)
                        .await
                        .map(|(language, client)| (Some(language), client))
                };
                let Some((language, client)) = selected else {
                    return Ok(WireValue::Variant {
                        type_id: "core::option::Option<LspClient>".to_string(),
                        variant: "none".to_string(),
                        fields: Vec::new(),
                    });
                };
                let (client_handle, _) = handles.register_facade_resource(
                    max_resources,
                    "lsp",
                    "lsp.client",
                    Some(owner),
                    METHOD,
                )?;
                resources
                    .lsp_clients
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(client_handle.id.clone(), client);
                let mut fields = vec![echo_sdk_protocol::scalar::WireField {
                    name: "client".to_string(),
                    value: WireValue::Handle(client_handle),
                }];
                if let Some(language) = language {
                    fields.push(echo_sdk_protocol::scalar::WireField {
                        name: "language".to_string(),
                        value: WireValue::String(language),
                    });
                }
                Ok(WireValue::Variant {
                    type_id: "core::option::Option<LspClient>".to_string(),
                    variant: "some".to_string(),
                    fields,
                })
            }
            "lsp.client.language"
            | "lsp.client.is_running"
            | "lsp.client.is_initialized"
            | "lsp.client.status" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP client resource", METHOD)?;
                let client = lsp_client_of(handles, resources, &resource, owner)?;
                let client = client.read().await;
                match request.operation.as_str() {
                    "lsp.client.language" => wire(serde_json::json!(client.language())),
                    "lsp.client.is_running" => wire(serde_json::json!(client.is_running())),
                    "lsp.client.is_initialized" => wire(serde_json::json!(client.is_initialized())),
                    _ => wire(serde_json::to_value(client.status()).map_err(framework)?),
                }
            }
            "lsp.client.initialize" | "lsp.client.shutdown" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP client resource", METHOD)?;
                let client = lsp_client_of(handles, resources, &resource, owner)?;
                let mut client = client.write().await;
                if request.operation == "lsp.client.initialize" {
                    let root_uri = string_at(&arguments, 1, "a root URI")?;
                    client.initialize(&root_uri).await.map_err(framework)?;
                } else {
                    client.shutdown().await.map_err(framework)?;
                }
                wire(serde_json::Value::Null)
            }
            "lsp.client.diagnostics"
            | "lsp.client.goto_definition"
            | "lsp.client.find_references"
            | "lsp.client.hover"
            | "lsp.client.completion"
            | "lsp.client.did_open"
            | "lsp.client.did_change"
            | "lsp.client.did_save"
            | "lsp.client.did_close" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP client resource", METHOD)?;
                let uri = string_at(&arguments, 1, "a document URI")?;
                let client = lsp_client_of(handles, resources, &resource, owner)?;
                let client = client.read().await;
                let value = match request.operation.as_str() {
                    "lsp.client.diagnostics" => {
                        serde_json::to_value(client.diagnostics(&uri).await.map_err(framework)?)
                    }
                    "lsp.client.goto_definition" | "lsp.client.find_references" => {
                        let position: echo_agent::lsp::Position = arguments
                            .get(2)
                            .cloned()
                            .ok_or_else(|| invalid("LSP operation requires a position"))
                            .and_then(|value| {
                                serde_json::from_value(value).map_err(|error| {
                                    invalid(format!("LSP position is malformed: {error}"))
                                })
                            })?;
                        if request.operation == "lsp.client.goto_definition" {
                            serde_json::to_value(
                                client
                                    .goto_definition(&uri, position)
                                    .await
                                    .map_err(framework)?,
                            )
                        } else {
                            serde_json::to_value(
                                client
                                    .find_references(&uri, position)
                                    .await
                                    .map_err(framework)?,
                            )
                        }
                    }
                    "lsp.client.hover" | "lsp.client.completion" => {
                        let position: echo_agent::lsp::Position = arguments
                            .get(2)
                            .cloned()
                            .ok_or_else(|| invalid("LSP operation requires a position"))
                            .and_then(|value| {
                                serde_json::from_value(value).map_err(|error| {
                                    invalid(format!("LSP position is malformed: {error}"))
                                })
                            })?;
                        if request.operation == "lsp.client.hover" {
                            serde_json::to_value(
                                client.hover(&uri, position).await.map_err(framework)?,
                            )
                        } else {
                            serde_json::to_value(
                                client.completion(&uri, position).await.map_err(framework)?,
                            )
                        }
                    }
                    "lsp.client.did_open" => {
                        let language_id = string_at(&arguments, 2, "a language id")?;
                        let text = arguments
                            .get(3)
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| invalid("LSP did_open requires document text"))?;
                        client
                            .did_open(&uri, &language_id, text)
                            .await
                            .map_err(framework)?;
                        Ok(serde_json::Value::Null)
                    }
                    "lsp.client.did_change" => {
                        let changes: Vec<echo_agent::lsp::TextChange> = arguments
                            .get(2)
                            .cloned()
                            .ok_or_else(|| invalid("LSP did_change requires changes"))
                            .and_then(|value| {
                                serde_json::from_value(value).map_err(|error| {
                                    invalid(format!("LSP changes are malformed: {error}"))
                                })
                            })?;
                        client.did_change(&uri, changes).await.map_err(framework)?;
                        Ok(serde_json::Value::Null)
                    }
                    "lsp.client.did_save" => {
                        client.did_save(&uri).await.map_err(framework)?;
                        Ok(serde_json::Value::Null)
                    }
                    _ => {
                        client.did_close(&uri).await.map_err(framework)?;
                        Ok(serde_json::Value::Null)
                    }
                }
                .map_err(framework)?;
                wire(value)
            }
            "lsp.server.status" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                let statuses: Vec<serde_json::Value> = manager
                    .status_all()
                    .await
                    .into_iter()
                    .map(|status| serde_json::to_value(&status).unwrap_or_default())
                    .collect();
                wire(serde_json::json!({"servers": statuses}))
            }
            "lsp.language.list" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an MCP manager resource", METHOD)?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                wire(serde_json::json!({"configured": manager.configured_languages()}))
            }
            "lsp.language.configured" | "lsp.server.running" | "lsp.server.status_all" => {
                let resource =
                    super::resource_handle_at(&arguments, 0, "an LSP manager resource", METHOD)?;
                let record = lsp_of(handles, resources, &resource, owner)?;
                let manager = record.manager.lock().await;
                let value = match request.operation.as_str() {
                    "lsp.language.configured" => {
                        serde_json::to_value(manager.configured_languages())
                    }
                    "lsp.server.running" => serde_json::to_value(manager.running_servers()),
                    _ => serde_json::to_value(manager.status_all().await),
                }
                .map_err(framework)?;
                wire(value)
            }
            other => Err(invalid(format!(
                "unknown lsp operation {other}; the family surface is closed"
            ))),
        },
        #[cfg(feature = "framework-topology")]
        "topology" => match request.operation.as_str() {
            "topology.tracker.open" | "topology.tracker.open_exact" => {
                let (resource, _record) = handles.register_facade_resource(
                    max_resources,
                    "topology",
                    "topology.tracker",
                    Some(owner),
                    METHOD,
                )?;
                resources
                    .topology
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        resource.id.clone(),
                        Arc::new(TopologyRecord {
                            tracker: echo_agent::topology::TopologyTracker::new(),
                        }),
                    );
                if request.operation == "topology.tracker.open_exact" {
                    Ok(WireValue::Handle(resource))
                } else {
                    wire(serde_json::json!({"resource": resource}))
                }
            }
            "topology.node.add" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let node_id = string_at(&arguments, 1, "a node id")?;
                let node_type = string_at(&arguments, 2, "a node type")?;
                let node_type = match node_type.as_str() {
                    "agent" => echo_agent::topology::NodeType::Agent,
                    "orchestrator" => echo_agent::topology::NodeType::Orchestrator,
                    "subagent" => echo_agent::topology::NodeType::Subagent,
                    "planner" => echo_agent::topology::NodeType::Planner,
                    "external" => echo_agent::topology::NodeType::External,
                    "tool" => echo_agent::topology::NodeType::Tool,
                    other => {
                        return Err(invalid(format!(
                            "unknown topology node type {other}; expected agent|orchestrator|subagent|planner|external|tool"
                        )));
                    }
                };
                let record = topology_of(handles, resources, &resource, owner)?;
                record
                    .tracker
                    .add_node(echo_agent::topology::TopologyNode::new(node_id, node_type));
                wire(serde_json::json!({"ok": true}))
            }
            "topology.node.add_exact" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let node: echo_agent::topology::TopologyNode = arguments
                    .get(1)
                    .cloned()
                    .ok_or_else(|| invalid("topology.node.add_exact requires a node"))
                    .and_then(|value| {
                        serde_json::from_value(value).map_err(|error| {
                            invalid(format!("topology node is malformed: {error}"))
                        })
                    })?;
                let record = topology_of(handles, resources, &resource, owner)?;
                record.tracker.add_node(node);
                wire(serde_json::Value::Null)
            }
            "topology.call.record" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let from = string_at(&arguments, 1, "a from node id")?;
                let to = string_at(&arguments, 2, "a to node id")?;
                let label = string_at(&arguments, 3, "a call label")?;
                let record = topology_of(handles, resources, &resource, owner)?;
                record.tracker.record_call(&from, &to, &label);
                wire(serde_json::json!({"ok": true}))
            }
            "topology.call.record_with_duration" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let from = string_at(&arguments, 1, "a from node id")?;
                let to = string_at(&arguments, 2, "a to node id")?;
                let label = string_at(&arguments, 3, "a call label")?;
                let duration_ms = arguments
                    .get(4)
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
                    })
                    .ok_or_else(|| invalid("topology call duration must be a canonical u64"))?;
                let record = topology_of(handles, resources, &resource, owner)?;
                record
                    .tracker
                    .record_call_with_duration(&from, &to, &label, duration_ms);
                wire(serde_json::Value::Null)
            }
            "topology.clear" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let record = topology_of(handles, resources, &resource, owner)?;
                record.tracker.clear();
                wire(serde_json::Value::Null)
            }
            "topology.to_mermaid" | "topology.to_json" | "topology.to_dot" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let record = topology_of(handles, resources, &resource, owner)?;
                let value = match request.operation.as_str() {
                    "topology.to_mermaid" => record.tracker.to_mermaid(),
                    "topology.to_dot" => record.tracker.to_dot(),
                    _ => record
                        .tracker
                        .to_json()
                        .map_err(|error| framework(error.to_string()))?,
                };
                wire(serde_json::Value::String(value))
            }
            "topology.snapshot" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let record = topology_of(handles, resources, &resource, owner)?;
                wire(serde_json::json!({
                    "nodes": record.tracker.nodes(),
                    "edges": record.tracker.edges(),
                    "stats": serde_json::to_value(record.tracker.stats()).unwrap_or_default(),
                }))
            }
            "topology.nodes" | "topology.edges" | "topology.stats" => {
                let resource = super::resource_handle_at(
                    &arguments,
                    0,
                    "a topology tracker resource",
                    METHOD,
                )?;
                let record = topology_of(handles, resources, &resource, owner)?;
                let value = match request.operation.as_str() {
                    "topology.nodes" => serde_json::to_value(record.tracker.nodes()),
                    "topology.edges" => serde_json::to_value(record.tracker.edges()),
                    _ => serde_json::to_value(record.tracker.stats()),
                }
                .map_err(framework)?;
                wire(value)
            }
            other => Err(invalid(format!(
                "unknown topology operation {other}; the family surface is closed"
            ))),
        },
        _ => Err(invalid(format!(
            "integration family {family} is not available in this Host build"
        ))),
    }
}

/// Dispatch the channel family through the reverse extension bridge. This is
/// separate from the framework-owned integrations dispatcher because channel
/// plugins and handlers require the connection-scoped bridge authority.
#[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
pub(crate) async fn dispatch_channels(
    handles: &HandleRegistry,
    resources: &IntegrationResources,
    bridge: &Arc<crate::core_profile::extension_bridge::ExtensionBridge>,
    owner: &str,
    request: &FeatureOperationRequest,
    max_resources: usize,
) -> Result<WireValue, EchoSdkError> {
    let wire = |value: serde_json::Value| {
        WireValue::from_json(value).map_err(|error| invalid(error.to_string()))
    };
    let arguments: Vec<serde_json::Value> = request
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| json_of(value, position))
        .collect::<Result<_, _>>()?;
    match request.operation.as_str() {
        "channels.manager.open" => {
            let (resource, _record) = handles.register_facade_resource(
                max_resources,
                "channels",
                "channels.manager",
                Some(owner),
                METHOD,
            )?;
            resources
                .channels
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(
                    resource.id.clone(),
                    Arc::new(ChannelManagerRecord {
                        manager: tokio::sync::Mutex::new(
                            echo_agent::channels::ChannelManager::new(),
                        ),
                    }),
                );
            wire(serde_json::json!({"resource": resource}))
        }
        "channels.plugin.register" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let plugin = handle_at(&arguments, 1, "a ChannelPlugin extension handle")?;
            let handler = handle_at(&arguments, 2, "a MessageHandler extension handle")?;
            let plugin_record = handles.extension(&plugin)?;
            let descriptor = match &plugin_record.descriptor {
                echo_sdk_protocol::methods::ExtensionDescriptor::ChannelPlugin(value) => {
                    value.clone()
                }
                _ => return Err(invalid("extension handle is not a ChannelPlugin")),
            };
            let handler_record = handles.extension(&handler)?;
            if handler_record.kind
                != echo_sdk_protocol::methods::ExtensionKind::ChannelMessageHandler
                || handler_record.implementation_id != descriptor.handler_id
            {
                return Err(invalid(
                    "MessageHandler extension does not match the ChannelPlugin descriptor",
                ));
            }
            let record = channel_of(handles, resources, &resource, owner)?;
            let proxy = crate::core_profile::extension_bridge::ExtensionChannelPluginProxy::new(
                bridge.clone(),
                plugin,
                descriptor,
            );
            record
                .manager
                .lock()
                .await
                .register(Box::new(proxy))
                .map_err(framework)?;
            wire(serde_json::json!({"registered": true}))
        }
        "channels.manager.start" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let handler = handle_at(&arguments, 1, "a MessageHandler extension handle")?;
            let record = channel_of(handles, resources, &resource, owner)?;
            let bridge = bridge.clone();
            let results = record
                .manager
                .lock()
                .await
                .start_all(move |_| {
                    Arc::new(
                        crate::core_profile::extension_bridge::ExtensionChannelMessageHandlerProxy::new(
                            bridge.clone(),
                            handler.clone(),
                        ),
                    ) as Arc<dyn echo_agent::channels::MessageHandler>
                })
                .await;
            let failures = results
                .iter()
                .filter(|result| result.result.is_err())
                .count();
            wire(serde_json::json!({"channels": results.len(), "failures": failures}))
        }
        "channels.manager.stop" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let channel_id = string_at(&arguments, 1, "a channel id")?;
            let record = channel_of(handles, resources, &resource, owner)?;
            record
                .manager
                .lock()
                .await
                .stop(&channel_id)
                .await
                .map_err(framework)?;
            wire(serde_json::json!({"stopped": true}))
        }
        "channels.manager.stop_exact" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let channel_id = string_at(&arguments, 1, "a channel id")?;
            let record = channel_of(handles, resources, &resource, owner)?;
            record
                .manager
                .lock()
                .await
                .stop(&channel_id)
                .await
                .map_err(framework)?;
            Ok(WireValue::Null)
        }
        "channels.manager.stop_all" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let record = channel_of(handles, resources, &resource, owner)?;
            record
                .manager
                .lock()
                .await
                .stop_all()
                .await
                .map_err(framework)?;
            Ok(WireValue::Null)
        }
        "channels.manager.health" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let channel_id = string_at(&arguments, 1, "a channel id")?;
            let record = channel_of(handles, resources, &resource, owner)?;
            let manager = record.manager.lock().await;
            let channel = manager
                .get(&channel_id)
                .ok_or_else(|| invalid(format!("channel {channel_id} is not registered")))?;
            channel.health_check().await.map_err(framework)?;
            wire(serde_json::json!({"healthy": true}))
        }
        "channels.manager.list" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let record = channel_of(handles, resources, &resource, owner)?;
            let manager = record.manager.lock().await;
            wire(serde_json::json!({"channels": manager.channel_ids()}))
        }
        "channels.manager.ids" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let record = channel_of(handles, resources, &resource, owner)?;
            let manager = record.manager.lock().await;
            wire(serde_json::to_value(manager.channel_ids()).map_err(framework)?)
        }
        "channels.plugin.send" => {
            let resource =
                super::resource_handle_at(&arguments, 0, "a channel manager resource", METHOD)?;
            let channel_id = string_at(&arguments, 1, "a channel id")?;
            let message: echo_sdk_protocol::methods::ChannelOutboundMessageWire =
                serde_json::from_value(
                    arguments.get(2).cloned().ok_or_else(|| {
                        invalid("channels.plugin.send requires an outbound message")
                    })?,
                )
                .map_err(|error| invalid(format!("outbound message is malformed: {error}")))?;
            let record = channel_of(handles, resources, &resource, owner)?;
            record
                .manager
                .lock()
                .await
                .get(&channel_id)
                .ok_or_else(|| invalid(format!("channel {channel_id} is not registered")))?
                .send(
                    crate::core_profile::extension_bridge::channel_outbound_from_wire(message)
                        .map_err(framework)?,
                )
                .await
                .map_err(framework)?;
            wire(serde_json::json!({"sent": true}))
        }
        other => Err(invalid(format!(
            "unknown channels operation {other}; the family surface is closed"
        ))),
    }
}

/// Close every integration resource owned by one session. MCP managers are
/// explicitly awaited so stdio child processes cannot outlive an explicit
/// session close; connection teardown uses the same close authority.
pub(crate) async fn close_session_resources(resources: &IntegrationResources, closed: &[String]) {
    #[cfg(feature = "framework-mcp")]
    {
        let managers = {
            let mut map = resources
                .mcp
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let ids = map
                .keys()
                .filter(|id| closed.iter().any(|closed_id| closed_id == *id))
                .cloned()
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| map.remove(&id))
                .collect::<Vec<_>>()
        };
        for record in managers {
            record.manager.lock().await.close_all().await;
        }
        let clients = {
            let mut clients = resources
                .mcp_clients
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            closed
                .iter()
                .filter_map(|id| clients.remove(id))
                .collect::<Vec<_>>()
        };
        for client in clients {
            client.close().await;
        }
        resources
            .mcp_tools
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
    }
    #[cfg(feature = "framework-a2a")]
    resources
        .a2a
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
    #[cfg(feature = "framework-lsp")]
    {
        let managers = {
            let mut managers = resources
                .lsp
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            closed
                .iter()
                .filter_map(|id| managers.remove(id))
                .collect::<Vec<_>>()
        };
        for manager in managers {
            manager.manager.lock().await.shutdown_all().await;
        }
        resources
            .lsp_clients
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
    }
    #[cfg(feature = "framework-topology")]
    resources
        .topology
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|id, _| !closed.iter().any(|closed_id| closed_id == id));
    #[cfg(all(feature = "framework-channels", feature = "sdk-extension-bridge"))]
    {
        let managers = {
            let mut map = resources
                .channels
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            closed
                .iter()
                .filter_map(|id| map.remove(id))
                .collect::<Vec<_>>()
        };
        for record in managers {
            let _ = record.manager.lock().await.stop_all().await;
        }
    }
}

#[cfg(all(test, feature = "framework-mcp"))]
mod tests {
    use super::*;

    #[test]
    fn exact_mcp_config_preserves_every_transport_field() -> Result<(), Box<dyn std::error::Error>>
    {
        let cwd = tempfile::tempdir()?;
        let cwd_wire = serde_json::to_value(WirePath::Utf8 {
            path: cwd.path().display().to_string(),
        })?;
        let stdio = vec![serde_json::json!({
            "name": "stdio-server",
            "transport": {
                "kind": "stdio",
                "command": "server-bin",
                "args": ["--flag"],
                "env": {"TOKEN": "secret-ref"},
                "cwd": cwd_wire,
            }
        })];
        let parsed = mcp_server_config_at(&stdio, 0).map_err(|error| error.message)?;
        let echo_agent::mcp::TransportConfig::Stdio {
            command,
            args,
            env,
            cwd: parsed_cwd,
        } = parsed.transport
        else {
            return Err("stdio config changed transport".into());
        };
        assert_eq!(command, "server-bin");
        assert_eq!(args, vec!["--flag"]);
        assert_eq!(env, vec![("TOKEN".to_string(), "secret-ref".to_string())]);
        assert_eq!(parsed_cwd.as_deref(), Some(cwd.path()));

        for kind in ["http", "sse"] {
            let config = vec![serde_json::json!({
                "name": format!("{kind}-server"),
                "transport": {
                    "kind": kind,
                    "base_url": "https://example.test/mcp",
                    "headers": {"Authorization": "secret-ref"},
                }
            })];
            let parsed = mcp_server_config_at(&config, 0).map_err(|error| error.message)?;
            let (base_url, headers) = match parsed.transport {
                echo_agent::mcp::TransportConfig::Http { base_url, headers } if kind == "http" => {
                    (base_url, headers)
                }
                echo_agent::mcp::TransportConfig::Sse { base_url, headers } if kind == "sse" => {
                    (base_url, headers)
                }
                _ => return Err(format!("{kind} config changed transport").into()),
            };
            assert_eq!(base_url, "https://example.test/mcp");
            assert_eq!(
                headers.get("Authorization").map(String::as_str),
                Some("secret-ref")
            );
        }
        Ok(())
    }
}
