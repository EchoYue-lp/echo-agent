use crate::error::ToolError;
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;

pub struct HumanInLoop;

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
                    "description": "人工批准触发原因，即为什么会触发 human_in_loop。如果是因为用户意图模糊引起，需要 LLM 给出；如果是因为工具侵权，由 用户 给出；"
                },
                 "tool": {
                    "type": "string",
                    "description": "引起触发的工具名称"
                }
                ,
                 "approval_type": {
                    "type": "string",
                    "description": "引起触发的原因类型：LLM 触发时返回: LLM ，tool 触发时返回: tool"
                }
            },
            "required": ["reasoning","approval_type"]
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

        // 返回给用户的信息
        let approval_info = format!(
            "未正确理解你的意图，需要你给予帮助。触发类型：{} \n触发原因：{} \n触发工具：{}\n请你确认你的需求，请你直接回答：",
            approval_type, reasoning, tool
        );
        println!("{}", approval_info);

        // 从控制台获取用户输入
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");
        let input = input.trim();
        if input == "y" || input == "Y" || input == "yes" {
            // 返回给 LLM 的信息
            let result = format!("根据 {} 的要求，我已批准 {} 工具的运行。", reasoning, tool);
            println!("{}", result);
            Ok(ToolResult::success(result))
        } else if input == "n" || input == "N" || input == "no" {
            // 返回给 LLM 的信息
            let result = format!("根据 {} 的要求，我已否决 {} 工具的运行。", reasoning, tool);
            println!("{}", result);
            Ok(ToolResult::success(result))
        } else {
            let result = format!("根据 {} 的要求，我的回复是： {} ", reasoning, input);
            println!("{}", result);
            Ok(ToolResult::success(result))
        }
    }
}
