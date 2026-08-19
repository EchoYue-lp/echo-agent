//! A real echo-agent tool that runs locally without an LLM or API key.

use echo_agent::prelude::{Result, Tool, ToolResult};
use echo_agent::tool;
use serde_json::json;
use std::collections::HashMap;

#[tool(name = "greet", description = "Greet a contributor")]
async fn greet(name: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("你好，{name}")))
}

pub async fn run_greet_tool(name: &str) -> Result<ToolResult> {
    let parameters = HashMap::from([("name".to_string(), json!(name))]);
    GreetTool.execute(parameters).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn generated_tool_runs_without_an_llm() -> Result<()> {
        let result = run_greet_tool("小明").await?;
        assert!(result.success);
        assert_eq!(result.output, "你好，小明");
        Ok(())
    }
}
