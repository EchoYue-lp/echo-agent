//! Quickstart demo — minimal agent with tools in under 30 lines.
//!
//! ```sh
//! cargo run --example demo00_quickstart
//! ```

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
    let agent = agent! {
        model: "deepseek-chat",
        system_prompt: "You are a helpful math assistant. Use tools to calculate.",
        tools: [AddTool, SubtractTool, MultiplyTool, DivideTool],
    }?;

    let answer = agent.execute("What is 1337 * 42?").await?;
    println!("Answer: {answer}");
    Ok(())
}
