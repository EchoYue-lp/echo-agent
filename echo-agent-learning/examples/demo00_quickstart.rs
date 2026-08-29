//! Quickstart demo — minimal agent with tools in under 30 lines.
//!
//! ```sh
//! ECHO_AGENT_PROVIDER=openai \
//! ECHO_AGENT_BASE_URL=https://api.openai.com/v1 \
//! ECHO_AGENT_API_PROTOCOL=chat_completions \
//! ECHO_AGENT_MODEL=gpt-5.5 \
//! ECHO_AGENT_API_KEY=sk-... \
//! cargo run -p echo-agent-learning --example demo00_quickstart --locked
//! ```

mod support;

use echo_agent::prelude::*;
use echo_agent::{agent, tool};

#[tool(name = "add", description = "Add two numbers")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tool(name = "subtract", description = "Subtract b from a")]
async fn subtract(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a - b)))
}

#[tool(name = "multiply", description = "Multiply two numbers")]
async fn multiply(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a * b)))
}

#[tool(name = "divide", description = "Divide a by b")]
async fn divide(a: f64, b: f64) -> Result<ToolResult> {
    if b == 0.0 {
        return Ok(ToolResult::error("Division by zero".to_string()));
    }
    Ok(ToolResult::success(format!("{}", a / b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    require_api_key("demo00_quickstart")?;
    let llm_config = support::llm_config(None)?;
    let agent = agent! {
        llm_config: llm_config,
        system_prompt: "You are a helpful math assistant. Use tools to calculate.",
        tools: [AddTool, SubtractTool, MultiplyTool, DivideTool],
    }?;

    let answer = agent.execute("What is 1337 * 42?").await?;
    println!("Answer: {answer}");
    Ok(())
}

fn require_api_key(example: &str) -> Result<()> {
    std::env::var("ECHO_AGENT_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(drop)
        .ok_or_else(|| {
            echo_agent::error::ConfigError::MissingConfig(
                example.to_string(),
                "ECHO_AGENT_API_KEY".to_string(),
            )
            .into()
        })
}
