use std::sync::Arc;

use futures::future::BoxFuture;

use super::client::McpClient;
use super::types::{McpContent, McpTool, McpToolCallResult};
use echo_core::error::Result;
use echo_core::tools::{
    Tool, ToolCapabilities, ToolFailure, ToolFailureCategory, ToolParameters, ToolResult,
    ToolResultKind, ToolRiskLevel, ToolSideEffect, permission::ToolPermission,
};

/// 将 MCP 工具适配为框架的 `Tool` trait
///
/// 使 MCP 服务端提供的工具可以无缝注册到 `ToolManager`，
/// 由 ReAct Agent 像使用内置工具一样调用。
pub struct McpToolAdapter {
    client: Arc<McpClient>,
    tool: McpTool,
    server_name: Option<String>,
    exposed_name: String,
    local_capabilities: ToolCapabilities,
}

impl McpToolAdapter {
    pub fn new(client: Arc<McpClient>, tool: McpTool) -> Self {
        let exposed_name = tool.name.clone();
        Self {
            client,
            tool,
            server_name: None,
            exposed_name,
            local_capabilities: Self::conservative_capabilities(),
        }
    }

    pub fn with_server_name(
        client: Arc<McpClient>,
        tool: McpTool,
        server_name: impl Into<String>,
    ) -> Self {
        let server_name = server_name.into();
        let exposed_name = Self::exposed_name_for(&server_name, &tool.name);
        Self {
            client,
            tool,
            server_name: Some(server_name),
            exposed_name,
            local_capabilities: Self::conservative_capabilities(),
        }
    }

    /// Override the conservative MCP capability classification with a decision
    /// owned by the embedding application.
    ///
    /// MCP server annotations are advisory protocol metadata and must not be
    /// converted directly into this value. Applications may downgrade a tool
    /// to read-only only after establishing that fact from local configuration
    /// or another trusted policy source.
    pub fn with_local_capabilities(mut self, capabilities: ToolCapabilities) -> Self {
        self.local_capabilities = capabilities;
        self
    }

    /// Stable tool name used when a server's tools are registered on an agent.
    pub fn exposed_name_for(server_name: &str, tool_name: &str) -> String {
        format!(
            "mcp__{}__{}",
            sanitize_tool_name_part(server_name),
            sanitize_tool_name_part(tool_name)
        )
    }

    fn attach_result_metadata(&self, result: &mut ToolResult, result_type: &str) {
        result
            .metadata
            .insert("tool_source".to_string(), "mcp".to_string());
        result
            .metadata
            .insert("mcp_tool".to_string(), self.tool.name.clone());
        result
            .metadata
            .insert("result_type".to_string(), result_type.to_string());
        if let Some(server_name) = &self.server_name {
            result
                .metadata
                .insert("mcp_server".to_string(), server_name.clone());
        }
    }

    fn conservative_capabilities() -> ToolCapabilities {
        ToolCapabilities::mutating(ToolRiskLevel::Standard, vec![ToolPermission::Write])
    }

    fn reported_failure(&self) -> ToolFailure {
        if self.local_capabilities.is_read_only() {
            ToolFailure::new(ToolFailureCategory::Permanent)
        } else {
            ToolFailure::new(ToolFailureCategory::PartialSideEffect)
                .with_side_effect(ToolSideEffect::Possible)
        }
    }
}

fn sanitize_tool_name_part(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

fn mcp_result_type(result: &McpToolCallResult) -> &'static str {
    if result.structured_content.is_some() {
        return "json";
    }
    if result
        .content
        .iter()
        .any(|content| matches!(content, McpContent::Image { .. }))
    {
        return "image";
    }
    if result
        .content
        .iter()
        .any(|content| matches!(content, McpContent::Audio { .. }))
    {
        return "audio";
    }
    if result
        .content
        .iter()
        .any(|content| matches!(content, McpContent::Resource { .. }))
    {
        return "resource";
    }
    "text"
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        self.tool.description.as_deref().unwrap_or("")
    }

    fn parameters(&self) -> serde_json::Value {
        self.tool.input_schema.clone()
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        self.local_capabilities.permissions.clone()
    }

    fn risk_level(&self) -> ToolRiskLevel {
        self.local_capabilities.risk
    }

    fn capabilities(&self) -> ToolCapabilities {
        self.local_capabilities.clone()
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let args = serde_json::Value::Object(parameters.into_iter().collect());
            let result = match self.client.call_tool(&self.tool.name, args).await {
                Ok(result) => result,
                Err(error) => {
                    let may_have_side_effects = !self.local_capabilities.is_read_only();
                    let mut result = ToolResult::error(error.to_string())
                        .with_failure(ToolFailure::from_error(&error, may_have_side_effects));
                    self.attach_result_metadata(&mut result, "protocol_error");
                    return Ok(result);
                }
            };
            let result_type = mcp_result_type(&result);

            let text = McpClient::content_to_text(&result.content);

            // Preserve the MCP spec's `structuredContent` as structured `data`
            // (callers can render it directly instead of re-parsing the text),
            // and map `isError` to the structured-error kind. The text view
            // remains a human-readable concatenation for the LLM.
            if result.is_error {
                let failure = self.reported_failure();
                let mut tr = ToolResult::failure(failure.category, text).with_failure(failure);
                if let Some(structured) = result.structured_content {
                    tr = tr.with_data(structured);
                    tr.kind = ToolResultKind::StructuredError {
                        error_code: "mcp_is_error".to_string(),
                    };
                }
                self.attach_result_metadata(&mut tr, result_type);
                return Ok(tr);
            }

            let mut tr = if let Some(structured) = result.structured_content {
                // success_json sets kind = Json and data; keep the text view too.
                let mut t = ToolResult::success_json(structured);
                t.output = text;
                t
            } else {
                ToolResult::success(text)
            };

            // Surface non-standard extension fields in the text view so no
            // MCP server extra data is silently dropped.
            if !result.extra.is_empty() {
                let extra_str = serde_json::to_string_pretty(&result.extra).unwrap_or_default();
                tr.output.push_str(&format!("\n\n附加字段:\n{extra_str}"));
            }

            self.attach_result_metadata(&mut tr, result_type);

            Ok(tr)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::ToolAccess;

    struct InertTransport;

    struct ToolErrorTransport;

    impl super::super::transport::McpTransport for InertTransport {
        fn send(
            &self,
            _request: super::super::types::JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            Box::pin(async {
                Err(echo_core::error::ReactError::Other(
                    "inert test transport cannot send".to_string(),
                ))
            })
        }

        fn notify(
            &self,
            _notification: super::super::types::JsonRpcNotification,
        ) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    impl super::super::transport::McpTransport for ToolErrorTransport {
        fn send(
            &self,
            request: super::super::types::JsonRpcRequest,
        ) -> BoxFuture<'_, Result<super::super::types::JsonRpcResponse>> {
            Box::pin(async move {
                Ok(super::super::types::JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: request.id,
                    result: Some(serde_json::json!({
                        "content": [{ "type": "text", "text": "remote failure" }],
                        "isError": true
                    })),
                    error: None,
                })
            })
        }

        fn notify(
            &self,
            _notification: super::super::types::JsonRpcNotification,
        ) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }

        fn notification_rx(
            &self,
        ) -> Option<Arc<dyn super::super::types::JsonRpcNotificationReceiver>> {
            None
        }
    }

    fn adapter_with_annotations(
        annotations: Option<super::super::types::ToolAnnotations>,
    ) -> McpToolAdapter {
        adapter_with_transport(annotations, Arc::new(InertTransport))
    }

    fn adapter_with_transport(
        annotations: Option<super::super::types::ToolAnnotations>,
        transport: Arc<dyn super::super::transport::McpTransport>,
    ) -> McpToolAdapter {
        let client = McpClient::with_test_transport("test", transport);
        McpToolAdapter::new(
            client,
            McpTool {
                name: "remote_tool".to_string(),
                title: None,
                description: None,
                input_schema: serde_json::json!({ "type": "object" }),
                output_schema: None,
                icons: Vec::new(),
                annotations,
                execution: None,
                meta: None,
            },
        )
    }

    fn result(
        content: Vec<McpContent>,
        structured_content: Option<serde_json::Value>,
    ) -> McpToolCallResult {
        McpToolCallResult {
            content,
            is_error: false,
            structured_content,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn result_type_prefers_structured_content() {
        let value = result(
            vec![McpContent::Image {
                data: "frame".to_string(),
                mime_type: "image/png".to_string(),
            }],
            Some(serde_json::json!({ "ok": true })),
        );
        assert_eq!(mcp_result_type(&value), "json");
    }

    #[test]
    fn result_type_reports_rich_content() {
        let value = result(
            vec![McpContent::Resource {
                resource: super::super::types::McpResourceLink {
                    uri: "file:///tmp/report".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    name: Some("report".to_string()),
                },
            }],
            None,
        );
        assert_eq!(mcp_result_type(&value), "resource");
    }

    #[test]
    fn qualified_names_are_stable_and_tool_safe() {
        assert_eq!(
            sanitize_tool_name_part("GitHub Issues/v2"),
            "GitHub_Issues_v2"
        );
        assert_eq!(sanitize_tool_name_part(""), "unnamed");
    }

    #[test]
    fn server_annotations_do_not_authorize_read_only_access() {
        let adapter = adapter_with_annotations(Some(super::super::types::ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: Some(true),
            open_world_hint: Some(false),
        }));

        let capabilities = adapter.capabilities();
        assert_eq!(capabilities.access, ToolAccess::Mutating);
        assert_eq!(capabilities.risk, ToolRiskLevel::Standard);
        assert_eq!(
            capabilities.permissions,
            vec![echo_core::tools::permission::ToolPermission::Write]
        );
    }

    #[tokio::test]
    async fn local_capabilities_control_failure_side_effect_settlement() -> Result<()> {
        let default_adapter =
            adapter_with_annotations(Some(super::super::types::ToolAnnotations {
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                idempotent_hint: Some(true),
                open_world_hint: Some(false),
            }));
        let default_result = default_adapter.execute(ToolParameters::new()).await?;
        let Some(default_failure) = default_result.failure else {
            return Err(echo_core::error::ReactError::Other(
                "default MCP protocol failure did not include failure metadata".to_string(),
            ));
        };
        assert_eq!(default_failure.side_effect, ToolSideEffect::Possible);

        let read_only_adapter =
            adapter_with_annotations(Some(super::super::types::ToolAnnotations {
                read_only_hint: Some(false),
                destructive_hint: Some(true),
                idempotent_hint: Some(false),
                open_world_hint: Some(true),
            }))
            .with_local_capabilities(ToolCapabilities::read_only(vec![ToolPermission::Read]));
        let capabilities = read_only_adapter.capabilities();
        assert_eq!(capabilities.access, ToolAccess::ReadOnly);
        assert_eq!(capabilities.risk, ToolRiskLevel::ReadOnly);
        assert_eq!(capabilities.permissions, vec![ToolPermission::Read]);

        let read_only_result = read_only_adapter.execute(ToolParameters::new()).await?;
        let Some(read_only_failure) = read_only_result.failure else {
            return Err(echo_core::error::ReactError::Other(
                "read-only MCP protocol failure did not include failure metadata".to_string(),
            ));
        };
        assert_eq!(read_only_failure.side_effect, ToolSideEffect::None);
        Ok(())
    }

    #[tokio::test]
    async fn local_capabilities_control_reported_tool_failure_settlement() -> Result<()> {
        let default_adapter = adapter_with_transport(None, Arc::new(ToolErrorTransport));
        let default_result = default_adapter.execute(ToolParameters::new()).await?;
        let Some(default_failure) = default_result.failure else {
            return Err(echo_core::error::ReactError::Other(
                "default MCP tool error did not include failure metadata".to_string(),
            ));
        };
        assert_eq!(
            default_failure.category,
            ToolFailureCategory::PartialSideEffect
        );
        assert_eq!(default_failure.side_effect, ToolSideEffect::Possible);

        let read_only_adapter = adapter_with_transport(None, Arc::new(ToolErrorTransport))
            .with_local_capabilities(ToolCapabilities::read_only(vec![ToolPermission::Read]));
        let read_only_result = read_only_adapter.execute(ToolParameters::new()).await?;
        let Some(read_only_failure) = read_only_result.failure else {
            return Err(echo_core::error::ReactError::Other(
                "read-only MCP tool error did not include failure metadata".to_string(),
            ));
        };
        assert_eq!(read_only_failure.category, ToolFailureCategory::Permanent);
        assert_eq!(read_only_failure.side_effect, ToolSideEffect::None);
        Ok(())
    }
}
