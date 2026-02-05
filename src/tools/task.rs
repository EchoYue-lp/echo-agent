use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;

pub struct TodoTaskTool;

impl Tool for TodoTaskTool {
    fn name(&self) -> &'static str {
        "todo task"
    }

    fn description(&self) -> &'static str {
        "A task management tool"
    }

    fn parameters(&self) -> Value {
        todo!()
    }

    fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        todo!()
    }
}

pub struct TodoReadTool;

impl Tool for TodoReadTool {
    fn name(&self) -> &'static str {
        "todo read"
    }

    fn description(&self) -> &'static str {
        "Reads a todo task"
    }

    fn parameters(&self) -> Value {
        todo!()
    }

    fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        todo!()
    }
}

pub struct TodoWriteTool;

impl Tool for TodoWriteTool {
    fn name(&self) -> &'static str {
        "todo write"
    }

    fn description(&self) -> &'static str {
        "Writes a todo task"
    }

    fn parameters(&self) -> Value {
        todo!()
    }

    fn execute(&self, parameters: ToolParameters) -> crate::error::Result<ToolResult> {
        todo!()
    }
}
