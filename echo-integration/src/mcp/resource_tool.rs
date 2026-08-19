use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::future::{BoxFuture, join_all};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{McpClient, McpResource, McpResourceTemplate};
use echo_core::error::Result;
use echo_core::tools::{Tool, ToolFailureCategory, ToolParameters, ToolResult, ToolRiskLevel};

pub const LIST_MCP_RESOURCES_TOOL: &str = "list_mcp_resources";
pub const LIST_MCP_RESOURCE_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
pub const READ_MCP_RESOURCE_TOOL: &str = "read_mcp_resource";
pub const MCP_RESOURCE_TOOL_NAMES: [&str; 3] = [
    LIST_MCP_RESOURCES_TOOL,
    LIST_MCP_RESOURCE_TEMPLATES_TOOL,
    READ_MCP_RESOURCE_TOOL,
];

const PAGE_SIZE: usize = 50;

#[derive(Clone)]
struct McpResourceDirectory {
    clients: Arc<BTreeMap<String, Arc<McpClient>>>,
}

impl McpResourceDirectory {
    fn new(clients: HashMap<String, Arc<McpClient>>) -> Self {
        let clients = clients
            .into_iter()
            .filter(|(_, client)| client.supports_resources())
            .collect::<BTreeMap<_, _>>();
        Self {
            clients: Arc::new(clients),
        }
    }

    fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    fn select(
        &self,
        server: Option<&str>,
    ) -> std::result::Result<Vec<(String, Arc<McpClient>)>, String> {
        match server {
            Some(server) => self
                .clients
                .get(server)
                .cloned()
                .map(|client| vec![(server.to_string(), client)])
                .ok_or_else(|| format!("MCP resource server '{server}' is not connected")),
            None => Ok(self
                .clients
                .iter()
                .map(|(name, client)| (name.clone(), Arc::clone(client)))
                .collect()),
        }
    }
}

/// Build the three canonical, cross-server MCP Resource tools.
///
/// The returned tools share an immutable snapshot of the manager's connected
/// clients. Callers replace the tools whenever the connection topology changes.
pub fn build_mcp_resource_tools(clients: HashMap<String, Arc<McpClient>>) -> Vec<Box<dyn Tool>> {
    let directory = McpResourceDirectory::new(clients);
    if directory.is_empty() {
        return Vec::new();
    }
    vec![
        Box::new(ListMcpResourcesTool {
            directory: directory.clone(),
        }),
        Box::new(ListMcpResourceTemplatesTool {
            directory: directory.clone(),
        }),
        Box::new(ReadMcpResourceTool { directory }),
    ]
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PageCursor {
    offset: usize,
}

fn encode_cursor(offset: usize) -> std::result::Result<String, String> {
    serde_json::to_vec(&PageCursor { offset })
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|error| format!("failed to encode MCP resource cursor: {error}"))
}

fn decode_cursor(cursor: Option<&str>) -> std::result::Result<usize, String> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|error| format!("invalid MCP resource cursor: {error}"))?;
    serde_json::from_slice::<PageCursor>(&bytes)
        .map(|cursor| cursor.offset)
        .map_err(|error| format!("invalid MCP resource cursor payload: {error}"))
}

fn optional_string(
    parameters: &ToolParameters,
    name: &str,
) -> std::result::Result<Option<String>, String> {
    let Some(value) = parameters.get(name) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("parameter '{name}' must be a string"))?
        .trim();
    if value.is_empty() {
        return Err(format!("parameter '{name}' must not be empty"));
    }
    Ok(Some(value.to_string()))
}

fn required_string(parameters: &ToolParameters, name: &str) -> std::result::Result<String, String> {
    optional_string(parameters, name)?.ok_or_else(|| format!("missing required parameter '{name}'"))
}

#[derive(Serialize)]
struct ResourceEntry {
    server: String,
    #[serde(flatten)]
    resource: McpResource,
}

#[derive(Serialize)]
struct ResourceTemplateEntry {
    server: String,
    #[serde(flatten)]
    template: McpResourceTemplate,
}

#[derive(Serialize)]
struct ServerError {
    server: String,
    message: String,
}

fn paginated_result<T: Serialize>(
    key: &str,
    items: Vec<T>,
    errors: Vec<ServerError>,
    offset: usize,
) -> std::result::Result<ToolResult, String> {
    let total = items.len();
    if offset > total {
        return Err(format!(
            "MCP resource cursor offset {offset} exceeds result count {total}"
        ));
    }
    let page = items
        .into_iter()
        .skip(offset)
        .take(PAGE_SIZE)
        .collect::<Vec<_>>();
    let returned = page.len();
    let end = offset.saturating_add(returned);
    let next_cursor = if end < total {
        Some(encode_cursor(end)?)
    } else {
        None
    };

    let mut payload = Map::new();
    payload.insert(
        key.to_string(),
        serde_json::to_value(page)
            .map_err(|error| format!("failed to serialize MCP resources: {error}"))?,
    );
    payload.insert(
        "next_cursor".to_string(),
        next_cursor
            .as_ref()
            .map_or(Value::Null, |cursor| Value::String(cursor.clone())),
    );
    if !errors.is_empty() {
        payload.insert(
            "errors".to_string(),
            serde_json::to_value(errors)
                .map_err(|error| format!("failed to serialize MCP resource errors: {error}"))?,
        );
    }

    let mut result = ToolResult::success_json(Value::Object(payload))
        .with_truncated(next_cursor.is_some())
        .with_meta("tool_source", "mcp")
        .with_meta("page.total", total.to_string())
        .with_meta("page.returned", returned.to_string())
        .with_meta("page.truncated", next_cursor.is_some().to_string());
    if let Some(cursor) = next_cursor {
        result = result.with_meta("page.next_cursor", cursor);
    }
    Ok(result)
}

fn list_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {
                "type": "string",
                "description": "MCP server name. Omit to list entries from every connected resource server."
            },
            "cursor": {
                "type": "string",
                "description": "Opaque cursor from a previous call; omit for the first page."
            }
        },
        "additionalProperties": false
    })
}

pub struct ListMcpResourcesTool {
    directory: McpResourceDirectory,
}

impl Tool for ListMcpResourcesTool {
    fn name(&self) -> &str {
        LIST_MCP_RESOURCES_TOOL
    }

    fn description(&self) -> &str {
        "List resources provided by connected MCP servers. Prefer these application-provided resources over web search when they can supply the needed context."
    }

    fn parameters(&self) -> Value {
        list_parameters()
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let server = match optional_string(&parameters, "server") {
                Ok(server) => server,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let cursor = match optional_string(&parameters, "cursor")
                .and_then(|cursor| decode_cursor(cursor.as_deref()))
            {
                Ok(cursor) => cursor,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let clients = match self.directory.select(server.as_deref()) {
                Ok(clients) => clients,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let targeted = server.is_some();
            let requests = clients.into_iter().map(|(name, client)| async move {
                let result = client.list_resources().await;
                (name, result)
            });
            let mut resources = Vec::new();
            let mut errors = Vec::new();
            for (name, response) in join_all(requests).await {
                match response {
                    Ok(entries) => {
                        resources.extend(entries.into_iter().map(|resource| ResourceEntry {
                            server: name.clone(),
                            resource,
                        }))
                    }
                    Err(error) if targeted => {
                        return Ok(ToolResult::failure(
                            ToolFailureCategory::Unavailable,
                            format!("MCP server '{name}' resources/list failed: {error}"),
                        ));
                    }
                    Err(error) => errors.push(ServerError {
                        server: name,
                        message: error.to_string(),
                    }),
                }
            }
            resources.sort_by(|left, right| {
                left.server
                    .cmp(&right.server)
                    .then_with(|| left.resource.uri.cmp(&right.resource.uri))
            });
            Ok(paginated_result("resources", resources, errors, cursor)
                .unwrap_or_else(ToolResult::invalid_arguments))
        })
    }
}

pub struct ListMcpResourceTemplatesTool {
    directory: McpResourceDirectory,
}

impl Tool for ListMcpResourceTemplatesTool {
    fn name(&self) -> &str {
        LIST_MCP_RESOURCE_TEMPLATES_TOOL
    }

    fn description(&self) -> &str {
        "List parameterized resource templates provided by connected MCP servers. Prefer these application-provided templates over web search when appropriate."
    }

    fn parameters(&self) -> Value {
        list_parameters()
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let server = match optional_string(&parameters, "server") {
                Ok(server) => server,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let cursor = match optional_string(&parameters, "cursor")
                .and_then(|cursor| decode_cursor(cursor.as_deref()))
            {
                Ok(cursor) => cursor,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let clients = match self.directory.select(server.as_deref()) {
                Ok(clients) => clients,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let targeted = server.is_some();
            let requests = clients.into_iter().map(|(name, client)| async move {
                let result = client.list_resource_templates().await;
                (name, result)
            });
            let mut templates = Vec::new();
            let mut errors = Vec::new();
            for (name, response) in join_all(requests).await {
                match response {
                    Ok(entries) => templates.extend(entries.into_iter().map(|template| {
                        ResourceTemplateEntry {
                            server: name.clone(),
                            template,
                        }
                    })),
                    Err(error) if targeted => {
                        return Ok(ToolResult::failure(
                            ToolFailureCategory::Unavailable,
                            format!("MCP server '{name}' resources/templates/list failed: {error}"),
                        ));
                    }
                    Err(error) => errors.push(ServerError {
                        server: name,
                        message: error.to_string(),
                    }),
                }
            }
            templates.sort_by(|left, right| {
                left.server
                    .cmp(&right.server)
                    .then_with(|| left.template.uri_template.cmp(&right.template.uri_template))
            });
            Ok(
                paginated_result("resource_templates", templates, errors, cursor)
                    .unwrap_or_else(ToolResult::invalid_arguments),
            )
        })
    }
}

pub struct ReadMcpResourceTool {
    directory: McpResourceDirectory,
}

impl Tool for ReadMcpResourceTool {
    fn name(&self) -> &str {
        READ_MCP_RESOURCE_TOOL
    }

    fn description(&self) -> &str {
        "Read a specific resource from a connected MCP server using its exact server name and resource URI."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server name exactly as returned by list_mcp_resources."
                },
                "uri": {
                    "type": "string",
                    "description": "Resource URI exactly as returned by list_mcp_resources."
                }
            },
            "required": ["server", "uri"],
            "additionalProperties": false
        })
    }

    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::ReadOnly
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let server = match required_string(&parameters, "server") {
                Ok(server) => server,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let uri = match required_string(&parameters, "uri") {
                Ok(uri) => uri,
                Err(error) => return Ok(ToolResult::invalid_arguments(error)),
            };
            let client = match self.directory.clients.get(&server) {
                Some(client) => Arc::clone(client),
                None => {
                    return Ok(ToolResult::invalid_arguments(format!(
                        "MCP resource server '{server}' is not connected"
                    )));
                }
            };
            let resource = match client.read_resource(&uri).await {
                Ok(resource) => resource,
                Err(error) => {
                    return Ok(ToolResult::failure(
                        ToolFailureCategory::Unavailable,
                        format!("MCP server '{server}' resources/read failed: {error}"),
                    ));
                }
            };
            let payload = match serde_json::to_value(resource) {
                Ok(Value::Object(mut payload)) => {
                    payload.insert("server".to_string(), Value::String(server.clone()));
                    payload.insert("uri".to_string(), Value::String(uri.clone()));
                    Value::Object(payload)
                }
                Ok(payload) => payload,
                Err(error) => {
                    return Ok(ToolResult::error(format!(
                        "failed to serialize MCP resource: {error}"
                    )));
                }
            };
            Ok(ToolResult::success_json(payload)
                .with_meta("tool_source", "mcp")
                .with_meta("mcp_server", server)
                .with_meta("mcp_resource_uri", uri))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::transport::McpTransport;
    use crate::mcp::types::{
        JsonRpcError, JsonRpcNotification, JsonRpcNotificationReceiver, JsonRpcRequest,
        JsonRpcResponse,
    };

    struct ResourceTransport;

    struct FailingResourceTransport;

    impl McpTransport for ResourceTransport {
        fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
            Box::pin(async move {
                let result = match request.method.as_str() {
                    "resources/list" => {
                        let cursor = request
                            .params
                            .as_ref()
                            .and_then(|params| params.get("cursor"))
                            .and_then(Value::as_str);
                        if cursor == Some("resources-2") {
                            json!({
                                "resources": [{
                                    "uri": "schema://orders",
                                    "name": "orders",
                                    "title": "Orders",
                                    "mimeType": "application/schema+json",
                                    "icons": []
                                }]
                            })
                        } else {
                            json!({
                                "resources": [{
                                    "uri": "file:///project/说明.txt",
                                    "name": "说明.txt",
                                    "description": "UTF-8 resource",
                                    "mimeType": "text/plain",
                                    "icons": []
                                }],
                                "nextCursor": "resources-2"
                            })
                        }
                    }
                    "resources/templates/list" => {
                        let cursor = request
                            .params
                            .as_ref()
                            .and_then(|params| params.get("cursor"))
                            .and_then(Value::as_str);
                        if cursor == Some("templates-2") {
                            json!({
                                "resourceTemplates": [{
                                    "uriTemplate": "schema://{table}",
                                    "name": "database schema",
                                    "icons": []
                                }]
                            })
                        } else {
                            json!({
                                "resourceTemplates": [{
                                    "uriTemplate": "file:///{path}",
                                    "name": "project file",
                                    "title": "Project File",
                                    "icons": []
                                }],
                                "nextCursor": "templates-2"
                            })
                        }
                    }
                    "resources/read" => json!({
                        "contents": [
                            {
                                "uri": "file:///project/说明.txt",
                                "mimeType": "text/plain",
                                "text": "你好，MCP",
                                "audience": ["assistant"]
                            },
                            {
                                "uri": "file:///project/pixel.png",
                                "mimeType": "image/png",
                                "blob": "iVBORw0KGgo="
                            }
                        ]
                    }),
                    method => {
                        return Ok(JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: request.id,
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32601,
                                message: format!("unsupported method '{method}'"),
                                data: None,
                            }),
                        });
                    }
                };
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(result),
                    error: None,
                })
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

    impl McpTransport for FailingResourceTransport {
        fn send(&self, request: JsonRpcRequest) -> BoxFuture<'_, Result<JsonRpcResponse>> {
            Box::pin(async move {
                Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: "resource backend unavailable".to_string(),
                        data: None,
                    }),
                })
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

    fn resource_tools() -> Vec<Box<dyn Tool>> {
        let client = McpClient::with_test_transport("context", Arc::new(ResourceTransport));
        build_mcp_resource_tools(HashMap::from([("context".to_string(), client)]))
    }

    async fn execute_named(
        tools: &[Box<dyn Tool>],
        name: &str,
        parameters: ToolParameters,
    ) -> std::result::Result<ToolResult, String> {
        let tool = tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| format!("missing tool '{name}'"))?;
        tool.execute(parameters)
            .await
            .map_err(|error| error.to_string())
    }

    #[test]
    fn cursor_round_trip_and_validation() -> std::result::Result<(), String> {
        let cursor = encode_cursor(51)?;
        assert_eq!(decode_cursor(Some(&cursor))?, 51);
        assert!(decode_cursor(Some("not-a-cursor")).is_err());
        Ok(())
    }

    #[test]
    fn pagination_is_bounded_and_reports_continuation() -> std::result::Result<(), String> {
        let items = (0..51).collect::<Vec<_>>();
        let result = paginated_result("resources", items, Vec::new(), 0)?;
        assert!(result.success);
        assert!(result.truncated);
        assert_eq!(
            result.metadata.get("page.returned").map(String::as_str),
            Some("50")
        );
        let next = result
            .metadata
            .get("page.next_cursor")
            .ok_or_else(|| "missing next cursor".to_string())?;
        assert_eq!(decode_cursor(Some(next))?, 50);
        Ok(())
    }

    #[test]
    fn empty_directory_registers_no_tools() {
        assert!(build_mcp_resource_tools(HashMap::new()).is_empty());
    }

    #[tokio::test]
    async fn canonical_tools_list_templates_and_read_contents() -> std::result::Result<(), String> {
        let tools = resource_tools();
        let mut names = tools
            .iter()
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
        assert!(
            tools
                .iter()
                .all(|tool| tool.risk_level() == ToolRiskLevel::ReadOnly)
        );

        let resources =
            execute_named(&tools, LIST_MCP_RESOURCES_TOOL, ToolParameters::new()).await?;
        assert!(resources.success);
        let resource_count = resources
            .data
            .as_ref()
            .and_then(|data| data.get("resources"))
            .and_then(Value::as_array)
            .map(Vec::len);
        assert_eq!(resource_count, Some(2));
        assert!(resources.output.contains("说明.txt"));

        let templates = execute_named(
            &tools,
            LIST_MCP_RESOURCE_TEMPLATES_TOOL,
            ToolParameters::new(),
        )
        .await?;
        let template_count = templates
            .data
            .as_ref()
            .and_then(|data| data.get("resource_templates"))
            .and_then(Value::as_array)
            .map(Vec::len);
        assert_eq!(template_count, Some(2));

        let resource = execute_named(
            &tools,
            READ_MCP_RESOURCE_TOOL,
            ToolParameters::from([
                ("server".to_string(), Value::String("context".to_string())),
                (
                    "uri".to_string(),
                    Value::String("file:///project/说明.txt".to_string()),
                ),
            ]),
        )
        .await?;
        assert!(resource.success);
        assert!(resource.output.contains("你好，MCP"));
        assert!(resource.output.contains("iVBORw0KGgo="));
        assert!(resource.output.contains("audience"));
        assert_eq!(
            resource.metadata.get("mcp_server").map(String::as_str),
            Some("context")
        );
        Ok(())
    }

    #[tokio::test]
    async fn unknown_server_is_an_argument_error() -> std::result::Result<(), String> {
        let tools = resource_tools();
        let result = execute_named(
            &tools,
            LIST_MCP_RESOURCES_TOOL,
            ToolParameters::from([("server".to_string(), Value::String("missing".to_string()))]),
        )
        .await?;
        assert!(!result.success);
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.category),
            Some(ToolFailureCategory::InvalidArguments)
        );
        Ok(())
    }

    #[tokio::test]
    async fn all_server_listing_keeps_successes_and_targeted_failure_is_explicit()
    -> std::result::Result<(), String> {
        let healthy = McpClient::with_test_transport("healthy", Arc::new(ResourceTransport));
        let failing = McpClient::with_test_transport("failing", Arc::new(FailingResourceTransport));
        let tools = build_mcp_resource_tools(HashMap::from([
            ("healthy".to_string(), healthy),
            ("failing".to_string(), failing),
        ]));

        let aggregate =
            execute_named(&tools, LIST_MCP_RESOURCES_TOOL, ToolParameters::new()).await?;
        assert!(aggregate.success);
        assert_eq!(
            aggregate
                .data
                .as_ref()
                .and_then(|data| data.get("resources"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            aggregate
                .data
                .as_ref()
                .and_then(|data| data.get("errors"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        let targeted = execute_named(
            &tools,
            LIST_MCP_RESOURCES_TOOL,
            ToolParameters::from([("server".to_string(), Value::String("failing".to_string()))]),
        )
        .await?;
        assert!(!targeted.success);
        assert_eq!(
            targeted.failure.as_ref().map(|failure| failure.category),
            Some(ToolFailureCategory::Unavailable)
        );
        Ok(())
    }
}
