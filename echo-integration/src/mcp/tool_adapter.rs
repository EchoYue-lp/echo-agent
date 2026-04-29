use std::sync::Arc;

use futures::future::BoxFuture;

use super::client::McpClient;
use super::types::McpTool;
use echo_core::error::Result;
use echo_core::tools::{Tool, ToolParameters, ToolResult};

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

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let args = serde_json::Value::Object(parameters.into_iter().collect());
            let result = self.client.call_tool(&self.tool.name, args).await?;

            let mut text = McpClient::content_to_text(&result.content);

            // 将结构化输出拼入文本（MCP 规范字段）
            if let Some(ref structured) = result.structured_content {
                let structured_str = serde_json::to_string_pretty(structured).unwrap_or_default();
                text = format!("{text}\n\n结构化数据:\n{structured_str}");
            }

            // 将 MCP 服务端返回的所有非标准扩展字段也拼入文本，
            // 确保任意 MCP 服务端的额外字段都不会被静默丢弃
            if !result.extra.is_empty() {
                let extra_str = serde_json::to_string_pretty(&result.extra).unwrap_or_default();
                text = format!("{text}\n\n附加字段:\n{extra_str}");
            }

            if result.is_error {
                Ok(ToolResult::error(text))
            } else {
                Ok(ToolResult::success(text))
            }
        })
    }
}
