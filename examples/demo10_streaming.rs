//! demo10_streaming.rs —— 流式输出综合演示

use echo_agent::agent::{Agent, AgentEvent};
use echo_agent::llm::stream_chat;
use echo_agent::llm::types::Message;
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};
use futures::StreamExt;
use reqwest::Client;
use std::io::Write;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo10_streaming=info".into()),
        )
        .init();

    print_banner();

    // Part 1: 原始 LLM 层流式调用
    println!("{}", "─".repeat(60));
    println!("Part 1: LLM 原始流式调用（stream_chat）\n");
    demo_raw_stream().await?;

    // Part 2: Agent 流式执行 —— 纯文本响应
    println!("\n{}", "─".repeat(60));
    println!("Part 2: Agent 流式执行 —— 纯文本响应\n");
    demo_agent_text_stream().await?;

    // Part 3: Agent 流式执行 —— 工具调用 ReAct 循环
    println!("\n{}", "─".repeat(60));
    println!("Part 3: Agent 流式执行 —— 工具调用 ReAct 循环\n");
    demo_agent_tool_stream().await?;

    println!("\n{}", "═".repeat(60));
    println!("  demo10 完成");
    println!("{}", "═".repeat(60));

    Ok(())
}

async fn demo_raw_stream() -> echo_agent::error::Result<()> {
    let client = Arc::new(Client::new());
    let messages = vec![
        Message::system("你是一个助手，请用中文简洁作答。".to_string()),
        Message::user("用三句话解释什么是流式输出。".to_string()),
    ];

    let mut stream = Box::pin(
        stream_chat(
            client,
            "qwen3-max",
            messages,
            Some(0.7),
            Some(512),
            None,
            None,
            None,
        )
        .await?,
    );

    print!("  🤖 LLM: ");
    std::io::stdout().flush().ok();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        if let Some(choice) = chunk.choices.first() {
            if let Some(content) = &choice.delta.content {
                print!("{}", content);
                std::io::stdout().flush().ok();
            }
        }
    }
    println!();
    Ok(())
}

async fn demo_agent_text_stream() -> echo_agent::error::Result<()> {
    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("stream_text_agent")
        .system_prompt("你是一个知识渊博的助手，用中文简洁回答问题。")
        .max_iterations(3)
        .build()?;

    println!("  任务: 列举三个 Rust 语言最显著的特点\n");

    let mut event_stream = agent
        .execute_stream("列举三个 Rust 语言最显著的特点，每点一句话。")
        .await?;

    print!("  🤖 Agent: ");
    std::io::stdout().flush().ok();

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::Token(token) => {
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::FinalAnswer(_) => println!(),
            _ => {}
        }
    }

    Ok(())
}

async fn demo_agent_tool_stream() -> echo_agent::error::Result<()> {
    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("stream_math_agent")
        .system_prompt("你是一个计算助手，必须通过工具完成所有计算。")
        .enable_tools()
        .max_iterations(10)
        .build()?;

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(DivideTool));

    let task = "计算：(15 + 27) × 4 - (100 / 5)";
    println!("  任务: {task}\n");

    let mut event_stream = agent.execute_stream(task).await?;

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::Token(token) => {
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args } => {
                println!("\n  🔧 工具调用: {name}({:?})", args);
            }
            AgentEvent::ToolResult { name, output } => {
                println!("  📤 工具结果: [{name}] → {}", truncate(&output, 60));
            }
            AgentEvent::FinalAnswer(answer) => {
                println!("\n  ✅ 最终答案: {}", truncate(&answer, 80));
            }
            AgentEvent::Cancelled => {
                println!("\n  ⚠️ 执行已取消");
            }
        }
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{out}…")
    } else {
        out
    }
}

fn print_banner() {
    println!("{}", "═".repeat(60));
    println!("      Echo Agent × 流式输出综合演示 (demo10)");
    println!("{}", "═".repeat(60));
}
