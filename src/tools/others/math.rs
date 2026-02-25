use crate::error;
use crate::error::ToolError;
use crate::prelude::{Tool, ToolParameters, ToolResult};
use serde_json::Value;

/// 从参数 map 中提取 a、b 两个 f64 操作数，统一处理缺失参数错误
fn extract_operands(parameters: &ToolParameters) -> error::Result<(f64, f64)> {
    let a = parameters
        .get("a")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ToolError::MissingParameter("a".to_string()))?;
    let b = parameters
        .get("b")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ToolError::MissingParameter("b".to_string()))?;
    Ok((a, b))
}

pub struct AddTool;

#[async_trait::async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> &str {
        "两数相加，参数：a - 第一个加数，b - 第二个加数"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "第一个数" },
                "b": { "type": "number", "description": "第二个数" }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> error::Result<ToolResult> {
        let (a, b) = extract_operands(&parameters)?;
        Ok(ToolResult::success(format!("{} + {} = {}", a, b, a + b)))
    }
}

pub struct SubtractTool;

#[async_trait::async_trait]
impl Tool for SubtractTool {
    fn name(&self) -> &str {
        "subtract"
    }

    fn description(&self) -> &str {
        "两数相减，参数：a - 被减数，b - 减数"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "被减数" },
                "b": { "type": "number", "description": "减数" }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> error::Result<ToolResult> {
        let (a, b) = extract_operands(&parameters)?;
        Ok(ToolResult::success(format!("{} - {} = {}", a, b, a - b)))
    }
}

pub struct MultiplyTool;

#[async_trait::async_trait]
impl Tool for MultiplyTool {
    fn name(&self) -> &str {
        "multiply"
    }

    fn description(&self) -> &str {
        "两数相乘，参数：a - 第一个乘数，b - 第二个乘数"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "第一个乘数" },
                "b": { "type": "number", "description": "第二个乘数" }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> error::Result<ToolResult> {
        let (a, b) = extract_operands(&parameters)?;
        Ok(ToolResult::success(format!("{} * {} = {}", a, b, a * b)))
    }
}

pub struct DivideTool;

#[async_trait::async_trait]
impl Tool for DivideTool {
    fn name(&self) -> &str {
        "divide"
    }

    fn description(&self) -> &str {
        "两数相除，参数：a - 被除数，b - 除数"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "被除数" },
                "b": { "type": "number", "description": "除数" }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, parameters: ToolParameters) -> error::Result<ToolResult> {
        let (a, b) = extract_operands(&parameters)?;
        if b == 0.0 {
            return Err(ToolError::ExecutionFailed {
                tool: "divide".to_string(),
                message: "除数不能为 0".to_string(),
            }
            .into());
        }
        Ok(ToolResult::success(format!("{} / {} = {}", a, b, a / b)))
    }
}
