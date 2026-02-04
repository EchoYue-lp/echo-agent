use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;

pub struct AddTool;

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
                "a": {
                    "type": "number",
                    "description": "第一个数"
                },
                "b": {
                    "type": "number",
                    "description": "第二个数"
                }
            },
            "required": ["a", "b"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {

        println!("--------------------------> add called <--------------------------");

        let a = parameters
            .get("a")
            .and_then(|a| a.as_str())
            .ok_or_else(|| ToolError::MissingParameter("a".to_string()))?;
        let b = parameters
            .get("b")
            .and_then(|b| b.as_str())
            .ok_or_else(|| ToolError::MissingParameter("b".to_string()))?;

        let a_val = a.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "a".to_string(),
            message: format!("'{}' 不是有效的数字", a),
        })?;
        let b_val = b.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "b".to_string(),
            message: format!("'{}' 不是有效的数字", b),
        })?;

        Ok(ToolResult::success(format!(
            "{} + {} = {}",
            a,
            b,
            a_val + b_val
        )))
    }
}

pub struct SubtractTool;

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
                "a": {
                    "type": "number",
                    "description": "被减数"
                },
                "b": {
                    "type": "number",
                    "description": "减数"
                }
            },
            "required": ["a", "b"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        println!("--------------------------> subtract called <--------------------------");

        let a = parameters
            .get("a")
            .and_then(|a| a.as_str())
            .ok_or_else(|| ToolError::MissingParameter("a".to_string()))?;
        let b = parameters
            .get("b")
            .and_then(|b| b.as_str())
            .ok_or_else(|| ToolError::MissingParameter("b".to_string()))?;

        let a_val = a.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "a".to_string(),
            message: format!("'{}' 不是有效的数字", a),
        })?;
        let b_val = b.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "b".to_string(),
            message: format!("'{}' 不是有效的数字", b),
        })?;

        Ok(ToolResult::success(format!(
            "{} - {} = {}",
            a,
            b,
            a_val - b_val
        )))
    }
}

pub struct MultiplyTool;

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
                "a": {
                    "type": "number",
                    "description": "第一个乘数"
                },
                "b": {
                    "type": "number",
                    "description": "第二个乘数"
                }
            },
            "required": ["a", "b"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        println!("--------------------------> multiply called <--------------------------");
        let a = parameters
            .get("a")
            .and_then(|a| a.as_str())
            .ok_or_else(|| ToolError::MissingParameter("a".to_string()))?;
        let b = parameters
            .get("b")
            .and_then(|b| b.as_str())
            .ok_or_else(|| ToolError::MissingParameter("b".to_string()))?;

        let a_val = a.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "a".to_string(),
            message: format!("'{}' 不是有效的数字", a),
        })?;
        let b_val = b.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "b".to_string(),
            message: format!("'{}' 不是有效的数字", b),
        })?;

        Ok(ToolResult::success(format!(
            "{} * {} = {}",
            a,
            b,
            a_val * b_val
        )))
    }
}

pub struct DivideTool;

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
                "a": {
                    "type": "number",
                    "description": "被除数"
                },
                "b": {
                    "type": "number",
                    "description": "除数"
                }
            },
            "required": ["a", "b"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> Result<ToolResult> {
        println!("--------------------------> divide called <--------------------------");
        let a = parameters
            .get("a")
            .and_then(|a| a.as_str())
            .ok_or_else(|| ToolError::MissingParameter("a".to_string()))?;
        let b = parameters
            .get("b")
            .and_then(|b| b.as_str())
            .ok_or_else(|| ToolError::MissingParameter("b".to_string()))?;

        let a_val = a.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "a".to_string(),
            message: format!("'{}' 不是有效的数字", a),
        })?;
        let b_val = b.parse::<f64>().map_err(|_| ToolError::InvalidParameter {
            name: "b".to_string(),
            message: format!("'{}' 不是有效的数字", b),
        })?;

        if b_val == 0.0 {
            return Err(ToolError::ExecutionFailed {
                tool: "divide".to_string(),
                message: "除数不能为 0".to_string(),
            }
            .into());
        }

        Ok(ToolResult::success(format!(
            "{} / {} = {}",
            a,
            b,
            a_val / b_val
        )))
    }
}
