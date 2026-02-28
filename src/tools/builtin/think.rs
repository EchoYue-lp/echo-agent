/// Think 工具（可选注册，非默认行为）。
///
/// # 背景
///
/// 框架已切换到**（CoT via 系统提示）**：
/// `ReactAgent` 在 `enable_tool=true` 时自动将 CoT 引导语追加到系统提示，
/// 让 LLM 以文本 content 形式输出推理过程，天然兼容流式（Token 事件）。
///
/// 本工具**不再默认注册**，仅供以下场景手动 opt-in：
/// - 需要在对话历史中以结构化工具调用记录每次推理
/// - 非流式任务，且希望推理以 tool_result 形式存入上下文
///
/// # 用法
///
/// ```rust,no_run
/// use echo_agent::tools::builtin::think::ThinkTool;
///
/// agent.add_tool(Box::new(ThinkTool));
/// ```
///
/// > **注意**：在 `execute_stream` 流式路径中，注册本工具后模型推理内容将写入
/// > 工具调用参数而非 `content` 字段，导致推理阶段无 `AgentEvent::Token` 事件。
/// > 流式场景请依赖 CoT 系统提示（默认行为），无需注册本工具。
use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;
use tracing::info;

pub struct ThinkTool;

#[async_trait::async_trait]
impl Tool for ThinkTool {
    fn name(&self) -> &str {
        "think"
    }

    fn description(&self) -> &str {
        "在采取行动前，使用此工具记录推理和分析过程。参数：reasoning - 你对问题的分析和计划。"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "reasoning": {
                    "type": "string",
                    "description": "你的思考过程：分析问题、制定计划、推理步骤"
                }
            },
            "required": ["reasoning"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        let reasoning = parameters
            .get("reasoning")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("reasoning".to_string()))?;

        info!("Think: {}", reasoning);

        // 将推理内容回显到上下文，让下一轮 LLM 调用能看到完整推理记录
        Ok(ToolResult::success(reasoning.to_string()))
    }
}
