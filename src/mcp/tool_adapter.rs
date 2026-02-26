use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::mcp::client::McpClient;
use crate::mcp::types::McpTool;
use crate::tools::{Tool, ToolParameters, ToolResult};

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

#[async_trait]
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

    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        // 将 HashMap<String, Value> 序列化为 JSON Object 传递给 MCP
        let args = serde_json::to_value(&parameters)?;
        let result = self.client.call_tool(&self.tool.name, args).await?;

        let text = McpClient::content_to_text(&result.content);

        if result.is_error {
            Ok(ToolResult::error(text))
        } else {
            Ok(ToolResult::success(text))
        }
    }
}
