use std::sync::Arc;

use futures::future::BoxFuture;

use super::client::McpClient;
use super::types::McpTool;
use echo_core::error::Result;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolResultKind, ToolRiskLevel};

/// 将 MCP 工具适配为框架的 `Tool` trait
///
/// 使 MCP 服务端提供的工具可以无缝注册到 `ToolManager`，
/// 由 ReAct Agent 像使用内置工具一样调用。
pub struct McpToolAdapter {
    client: Arc<McpClient>,
    tool: McpTool,
}

impl McpToolAdapter {
    pub fn new(client: Arc<McpClient>, tool: McpTool) -> Self {
        Self { client, tool }
    }
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.tool.name
    }

    fn description(&self) -> &str {
        self.tool.description.as_deref().unwrap_or("")
    }

    fn parameters(&self) -> serde_json::Value {
        self.tool.input_schema.clone()
    }

    /// Map the MCP server's `annotations.read_only_hint` / `destructive_hint`
    /// to the framework's [`ToolRiskLevel`] so the permission/approval system
    /// treats MCP tools with the same risk gating as built-in tools.
    fn risk_level(&self) -> ToolRiskLevel {
        if let Some(ref ann) = self.tool.annotations {
            if ann.destructive_hint == Some(true) {
                return ToolRiskLevel::Dangerous;
            }
            if ann.read_only_hint == Some(true) {
                return ToolRiskLevel::ReadOnly;
            }
        }
        ToolRiskLevel::Standard
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let args = serde_json::Value::Object(parameters.into_iter().collect());
            let result = self.client.call_tool(&self.tool.name, args).await?;

            let text = McpClient::content_to_text(&result.content);

            // Preserve the MCP spec's `structuredContent` as structured `data`
            // (callers can render it directly instead of re-parsing the text),
            // and map `isError` to the structured-error kind. The text view
            // remains a human-readable concatenation for the LLM.
            if result.is_error {
                let mut tr = ToolResult::error(text);
                if let Some(structured) = result.structured_content {
                    tr = tr.with_data(structured);
                    tr.kind = ToolResultKind::StructuredError {
                        error_code: "mcp_is_error".to_string(),
                    };
                }
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

            Ok(tr)
        })
    }
}
