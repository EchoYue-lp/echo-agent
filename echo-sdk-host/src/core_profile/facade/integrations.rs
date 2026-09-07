//! Integration family adapters: MCP, A2A, LSP, topology (plan 07, todo 5).
//!
//! Each family resource-izes the framework's own client/manager and calls
//! its real verbs; connection semantics, protocol behavior and failures
//! stay with the framework integrations.
//!
//! Two catalog families stay deliberately unbound:
//! - `channels`: the service is constructed from a host-language
//!   `MessageHandler` factory closure — exactly the ExtensionBridge's
//!   consumer-trait obligation, not a data-driven remote surface.
//! - `telemetry`: the global metrics expose OTLP export, not a readable
//!   snapshot; there is nothing honest to return over RPC.
//!
//! Their methods keep the official method-not-found.

use echo_sdk_protocol::error::{EchoSdkError, ExtensionErrorCode, Retryability};
use echo_sdk_protocol::methods::FeatureOperationRequest;
use echo_sdk_protocol::scalar::WireValue;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::wire;

const METHOD: &str = "_echo_agent/integration/op";
/// Upper bound on integration resources per family per connection.
const MAX_RESOURCES: usize = 32;

/// One MCP manager resource. Connect takes `&mut` and awaits, so the
/// lock must stay Send across the await (tokio Mutex).
pub(crate) struct McpManagerRecord {
    pub manager: tokio::sync::Mutex<echo_agent::mcp::McpManager>,
    pub owner: String,
}

/// One A2A client resource.
pub(crate) struct A2aClientRecord {
    pub client: echo_agent::a2a::A2AClient,
    pub owner: String,
}

/// One LSP manager resource; status_all awaits under the lock.
pub(crate) struct LspManagerRecord {
    pub manager: tokio::sync::Mutex<echo_agent::lsp::LspManager>,
    pub owner: String,
}

/// One topology tracker resource.
pub(crate) struct TopologyRecord {
    pub tracker: echo_agent::topology::TopologyTracker,
    pub owner: String,
}

/// All integration family resources of one Host connection.
pub(crate) struct IntegrationResources {
    #[cfg(feature = "framework-mcp")]
    pub mcp: Mutex<HashMap<String, Arc<McpManagerRecord>>>,
    #[cfg(feature = "framework-a2a")]
    pub a2a: Mutex<HashMap<String, Arc<A2aClientRecord>>>,
    #[cfg(feature = "framework-lsp")]
    pub lsp: Mutex<HashMap<String, Arc<LspManagerRecord>>>,
    #[cfg(feature = "framework-topology")]
    pub topology: Mutex<HashMap<String, Arc<TopologyRecord>>>,
}

impl IntegrationResources {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "framework-mcp")]
            mcp: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-a2a")]
            a2a: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-lsp")]
            lsp: Mutex::new(HashMap::new()),
            #[cfg(feature = "framework-topology")]
            topology: Mutex::new(HashMap::new()),
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
    resources: &IntegrationResources,
    id: &str,
    owner: &str,
) -> Result<Arc<McpManagerRecord>, EchoSdkError> {
    let record = resources
        .mcp
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown MCP manager {id}")))?;
    if record.owner != owner {
        return Err(invalid("MCP manager belongs to another session"));
    }
    Ok(record)
}

/// Owner-checked A2A client lookup.
fn a2a_of(
    resources: &IntegrationResources,
    id: &str,
    owner: &str,
) -> Result<Arc<A2aClientRecord>, EchoSdkError> {
    let record = resources
        .a2a
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown A2A client {id}")))?;
    if record.owner != owner {
        return Err(invalid("A2A client belongs to another session"));
    }
    Ok(record)
}

/// Owner-checked LSP manager lookup.
fn lsp_of(
    resources: &IntegrationResources,
    id: &str,
    owner: &str,
) -> Result<Arc<LspManagerRecord>, EchoSdkError> {
    let record = resources
        .lsp
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown LSP manager {id}")))?;
    if record.owner != owner {
        return Err(invalid("LSP manager belongs to another session"));
    }
    Ok(record)
}

/// Owner-checked topology tracker lookup.
fn topology_of(
    resources: &IntegrationResources,
    id: &str,
    owner: &str,
) -> Result<Arc<TopologyRecord>, EchoSdkError> {
    let record = resources
        .topology
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(id)
        .cloned()
        .ok_or_else(|| invalid(format!("unknown topology tracker {id}")))?;
    if record.owner != owner {
        return Err(invalid("topology tracker belongs to another session"));
    }
    Ok(record)
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

/// Dispatch one integration family operation.
pub(crate) async fn dispatch(
    family: &str,
    resources: &IntegrationResources,
    owner: &str,
    request: &FeatureOperationRequest,
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
            "mcp.manager.open" => {
                let id = format!("mcp-{}", uuid::Uuid::new_v4());
                let mut map = resources
                    .mcp
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if map.len() >= MAX_RESOURCES {
                    return Err(invalid("MCP manager resource limit reached"));
                }
                map.insert(
                    id.clone(),
                    Arc::new(McpManagerRecord {
                        manager: tokio::sync::Mutex::new(echo_agent::mcp::McpManager::new()),
                        owner: owner.to_string(),
                    }),
                );
                wire(serde_json::json!({"manager_id": id}))
            }
            "mcp.server.connect" => {
                let manager_id = string_at(&arguments, 0, "a manager id")?;
                let name = string_at(&arguments, 1, "a server name")?;
                let mode = string_at(&arguments, 2, "a transport mode (stdio|http)")?;
                let config = match mode.as_str() {
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
                };
                let record = mcp_of(resources, &manager_id, owner)?;
                let mut manager = record.manager.lock().await;
                let tools = manager.connect(config).await.map_err(framework)?;
                wire(serde_json::json!({"tools": tools.len()}))
            }
            "mcp.server.list" => {
                let manager_id = string_at(&arguments, 0, "a manager id")?;
                let record = mcp_of(resources, &manager_id, owner)?;
                let manager = record.manager.lock().await;
                wire(serde_json::json!({"servers": manager.server_names()}))
            }
            "mcp.server.disconnect" => {
                let manager_id = string_at(&arguments, 0, "a manager id")?;
                let name = string_at(&arguments, 1, "a server name")?;
                let record = mcp_of(resources, &manager_id, owner)?;
                let mut manager = record.manager.lock().await;
                let disconnected = manager.disconnect(&name).await;
                wire(serde_json::json!({"disconnected": disconnected}))
            }
            other => Err(invalid(format!(
                "unknown mcp operation {other}; the family surface is closed"
            ))),
        },
        #[cfg(feature = "framework-a2a")]
        "a2a" => match request.operation.as_str() {
            "a2a.client.open" => {
                let id = format!("a2a-{}", uuid::Uuid::new_v4());
                let mut map = resources
                    .a2a
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if map.len() >= MAX_RESOURCES {
                    return Err(invalid("A2A client resource limit reached"));
                }
                map.insert(
                    id.clone(),
                    Arc::new(A2aClientRecord {
                        client: echo_agent::a2a::A2AClient::new(),
                        owner: owner.to_string(),
                    }),
                );
                wire(serde_json::json!({"client_id": id}))
            }
            "a2a.discover" => {
                let client_id = string_at(&arguments, 0, "a client id")?;
                let base_url = string_at(&arguments, 1, "a base URL")?;
                let record = a2a_of(resources, &client_id, owner)?;
                let card = record.client.discover(&base_url).await.map_err(framework)?;
                wire(serde_json::to_value(&card).unwrap_or_else(|_| serde_json::Value::Null))
            }
            "a2a.task.send" => {
                let client_id = string_at(&arguments, 0, "a client id")?;
                let agent_url = string_at(&arguments, 1, "an agent URL")?;
                let message = string_at(&arguments, 2, "a message")?;
                let record = a2a_of(resources, &client_id, owner)?;
                let task = record
                    .client
                    .send_task(&agent_url, &message)
                    .await
                    .map_err(framework)?;
                wire(serde_json::to_value(&task).unwrap_or(serde_json::Value::Null))
            }
            other => Err(invalid(format!(
                "unknown a2a operation {other}; the family surface is closed"
            ))),
        },
        #[cfg(feature = "framework-lsp")]
        "lsp" => match request.operation.as_str() {
            "lsp.manager.open" => {
                let id = format!("lsp-{}", uuid::Uuid::new_v4());
                let mut map = resources
                    .lsp
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if map.len() >= MAX_RESOURCES {
                    return Err(invalid("LSP manager resource limit reached"));
                }
                map.insert(
                    id.clone(),
                    Arc::new(LspManagerRecord {
                        manager: tokio::sync::Mutex::new(echo_agent::lsp::LspManager::new()),
                        owner: owner.to_string(),
                    }),
                );
                wire(serde_json::json!({"manager_id": id}))
            }
            "lsp.server.status" => {
                let manager_id = string_at(&arguments, 0, "a manager id")?;
                let record = lsp_of(resources, &manager_id, owner)?;
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
                let manager_id = string_at(&arguments, 0, "a manager id")?;
                let record = lsp_of(resources, &manager_id, owner)?;
                let manager = record.manager.lock().await;
                wire(serde_json::json!({"configured": manager.configured_languages()}))
            }
            other => Err(invalid(format!(
                "unknown lsp operation {other}; the family surface is closed"
            ))),
        },
        #[cfg(feature = "framework-topology")]
        "topology" => match request.operation.as_str() {
            "topology.tracker.open" => {
                let id = format!("topo-{}", uuid::Uuid::new_v4());
                let mut map = resources
                    .topology
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if map.len() >= MAX_RESOURCES {
                    return Err(invalid("topology tracker resource limit reached"));
                }
                map.insert(
                    id.clone(),
                    Arc::new(TopologyRecord {
                        tracker: echo_agent::topology::TopologyTracker::new(),
                        owner: owner.to_string(),
                    }),
                );
                wire(serde_json::json!({"tracker_id": id}))
            }
            "topology.node.add" => {
                let tracker_id = string_at(&arguments, 0, "a tracker id")?;
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
                let record = topology_of(resources, &tracker_id, owner)?;
                record
                    .tracker
                    .add_node(echo_agent::topology::TopologyNode::new(node_id, node_type));
                wire(serde_json::json!({"ok": true}))
            }
            "topology.call.record" => {
                let tracker_id = string_at(&arguments, 0, "a tracker id")?;
                let from = string_at(&arguments, 1, "a from node id")?;
                let to = string_at(&arguments, 2, "a to node id")?;
                let label = string_at(&arguments, 3, "a call label")?;
                let record = topology_of(resources, &tracker_id, owner)?;
                record.tracker.record_call(&from, &to, &label);
                wire(serde_json::json!({"ok": true}))
            }
            "topology.snapshot" => {
                let tracker_id = string_at(&arguments, 0, "a tracker id")?;
                let record = topology_of(resources, &tracker_id, owner)?;
                wire(serde_json::json!({
                    "nodes": record.tracker.nodes(),
                    "edges": record.tracker.edges(),
                    "stats": serde_json::to_value(record.tracker.stats()).unwrap_or_default(),
                }))
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

/// Drop every integration resource owned by one session (session close).
pub(crate) fn drop_session_resources(resources: &IntegrationResources, owner: &str) {
    #[cfg(feature = "framework-mcp")]
    resources
        .mcp
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|_, record| record.owner != owner);
    #[cfg(feature = "framework-a2a")]
    resources
        .a2a
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|_, record| record.owner != owner);
    #[cfg(feature = "framework-lsp")]
    resources
        .lsp
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|_, record| record.owner != owner);
    #[cfg(feature = "framework-topology")]
    resources
        .topology
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .retain(|_, record| record.owner != owner);
}
