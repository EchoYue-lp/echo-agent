//! A deterministic end-to-end composition of a custom tool, the ReAct loop,
//! and a public-facade LLM client. Replace the mock client with a configured
//! provider when adapting this example to an application.

use std::sync::Arc;

use echo_agent::prelude::*;
use echo_agent::testing::MockLlmClient;
use echo_agent::tool;

#[tool(name = "add", description = "Add two integers")]
async fn add(a: i64, b: i64) -> Result<ToolResult> {
    Ok(ToolResult::success((a.saturating_add(b)).to_string()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let llm = Arc::new(
        MockLlmClient::new()
            .then_tool_call("call-1", "add", r#"{"a": 40, "b": 2}"#)
            .with_response("The calculation is complete: 42."),
    );
    let agent = ReactAgentBuilder::new()
        .llm_client(llm)
        .system_prompt("You are a concise assistant. Use the add tool for arithmetic.")
        .tool(Box::new(AddTool))
        .build()?;

    let answer = agent.execute("What is 40 + 2?").await?;
    println!("Answer: {answer}");
    Ok(())
}
