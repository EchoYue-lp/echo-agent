use std::sync::Arc;

use serde_json::Value;

use crate::error::ToolError;
use crate::human_loop::{HumanLoopProvider, HumanLoopRequest, HumanLoopResponse};
use crate::tools::{Tool, ToolParameters, ToolResult};

/// LLM 触发的人工介入工具。
///
/// 当 LLM 不确定用户意图、需要额外信息或需要用户确认时调用。
/// 通过注入的 [`HumanLoopProvider`] 以异步方式向用户请求输入，
/// 支持命令行、HTTP Webhook、WebSocket 等多种渠道。
pub struct HumanInLoop {
    provider: Arc<dyn HumanLoopProvider>,
}

impl HumanInLoop {
    pub fn new(provider: Arc<dyn HumanLoopProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl Tool for HumanInLoop {
    fn name(&self) -> &str {
        "human_in_loop"
    }

    fn description(&self) -> &str {
        "当你不确定用户意图、需要额外信息、或需要用户确认时使用此工具。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reasoning": {
                    "type": "string",
                    "description": "触发原因：为什么需要人工介入？意图不明确时由 LLM 给出；工具存在风险时由用户确认。"
                },
                "tool": {
                    "type": "string",
                    "description": "引起触发的工具名称（可选）"
                },
                "approval_type": {
                    "type": "string",
                    "description": "触发类型：LLM 主动触发时填 'LLM'，工具触发时填 'tool'"
                }
            },
            "required": ["reasoning", "approval_type"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let approval_type = parameters
            .get("approval_type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| ToolError::MissingParameter("approval_type".to_string()))?;

        let reasoning = parameters
            .get("reasoning")
            .and_then(|t| t.as_str())
            .ok_or_else(|| ToolError::MissingParameter("reasoning".to_string()))?;

        let tool = parameters
            .get("tool")
            .and_then(|t| t.as_str())
            .unwrap_or("无");

        let prompt = format!(
            "需要你给予帮助。\n触发类型：{approval_type}\n触发原因：{reasoning}\n触发工具：{tool}\n\n请直接回复你的意见或确认："
        );

        let req = HumanLoopRequest::input(prompt);
        let result_text = match self.provider.request(req).await? {
            HumanLoopResponse::Text(text) => text,
            HumanLoopResponse::Approved => "用户已确认".to_string(),
            HumanLoopResponse::Rejected { reason } => {
                format!(
                    "用户已拒绝{}",
                    reason.map(|r| format!("，原因：{r}")).unwrap_or_default()
                )
            }
            HumanLoopResponse::Timeout => "等待用户输入超时".to_string(),
        };

        Ok(ToolResult::success(format!(
            "用户回复（触发原因：{reasoning}，工具：{tool}）：{result_text}"
        )))
    }
}
