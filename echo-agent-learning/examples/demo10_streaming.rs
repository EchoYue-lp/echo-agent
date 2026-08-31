//! demo10_streaming.rs —— 流式输出综合演示
//!
//! 展示 Agent 的流式输出能力：
//! 1. execute_stream() —— 实时获取 Token、工具调用、最终答案
//! 2. chat_stream() —— 多轮对话流式输出
//! 3. AgentEvent 事件类型处理
//!
//! 运行方式：
//! ```bash
//! cargo run -p echo-agent-learning --example demo10_streaming
//! ```
mod support;

use echo_agent::agent::{Agent, AgentEvent};
use echo_agent::llm::types::Message;
use echo_agent::prelude::*;
use echo_agent::tool;
use futures::StreamExt;
use std::io::Write;
use std::time::Duration;

#[tool(name = "add", description = "两数相加")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} + {} = {}", a, b, a + b)))
}

#[tool(name = "subtract", description = "两数相减")]
async fn subtract(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} - {} = {}", a, b, a - b)))
}

#[tool(name = "multiply", description = "两数相乘")]
async fn multiply(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} * {} = {}", a, b, a * b)))
}

#[tool(name = "divide", description = "两数相除")]
async fn divide(a: f64, b: f64) -> Result<ToolResult> {
    if b == 0.0 {
        return Ok(ToolResult::error("除数不能为 0".to_string()));
    }
    Ok(ToolResult::success(format!("{} / {} = {}", a, b, a / b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

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
    let llm_config = support::llm_config(None)?.with_timeouts(
        LlmTimeouts::default()
            .with_first_chunk_timeout(Duration::from_secs(30))
            .with_idle_timeout(Duration::from_secs(60))
            .without_overall_timeout(),
    );
    let client = llm_config.build_client()?;
    let messages = vec![
        Message::system("你是一个助手，请用中文简洁作答。".to_string()),
        Message::user("用三句话解释什么是流式输出。".to_string()),
    ];

    let mut stream = client
        .chat_stream(ChatRequest {
            messages,
            temperature: Some(0.7),
            max_tokens: Some(512),
            ..Default::default()
        })
        .await?;

    print!("  🤖 LLM: ");
    std::io::stdout().flush().ok();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        if let Some(content) = &chunk.delta.content {
            print!("{}", content);
            std::io::stdout().flush().ok();
        }
    }
    println!();
    Ok(())
}

async fn demo_agent_text_stream() -> echo_agent::error::Result<()> {
    // 使用 AgentBuilder 创建 Agent
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("stream_text_agent")
        .system_prompt("你是一个知识渊博的助手，用中文简洁回答问题。")
        .max_iterations(3)
        .build()?;

    println!("  任务: 列举三个 Rust 语言最显著的特点\n");

    let mut event_stream = agent
        .execute_stream("列举三个 Rust 语言最显著的特点，每点一句话。")
        .await?;
    let mut final_answer = String::new();

    print!("  🤖 Agent: ");
    std::io::stdout().flush().ok();

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::Token(token) => {
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::FinalAnswer(answer) => {
                final_answer = answer;
                println!();
            }
            AgentEvent::ToolResult { name, result, .. } if !result.success => {
                return Err(echo_agent::error::ReactError::Other(format!(
                    "demo10 验收失败：文本流式执行中工具 `{name}` 出错: {}",
                    result.error.unwrap_or(result.output)
                )));
            }
            _ => {}
        }
    }

    if final_answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo10 验收失败：文本流式执行未产生最终答案".to_string(),
        ));
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
    let mut final_answer = String::new();

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::ThinkStart => {
                println!("  💭 开始推理...");
            }
            AgentEvent::ThinkEnd {
                prompt_tokens,
                completion_tokens,
            } => {
                println!(
                    "  💭 推理结束 (prompt: {prompt_tokens}, completion: {completion_tokens})"
                );
            }
            AgentEvent::MemoryRecalled { count } if count > 0 => {
                println!("  🧠 召回 {count} 条记忆");
            }
            AgentEvent::Token(token) => {
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { invocation, .. } => {
                println!(
                    "\n  🔧 工具调用: {}({:?})",
                    invocation.name, invocation.args
                );
            }
            AgentEvent::ToolResult { name, result, .. } => {
                if !result.success {
                    return Err(echo_agent::error::ReactError::Other(format!(
                        "demo10 验收失败：工具流式执行中 `{name}` 出错: {}",
                        result.error.unwrap_or(result.output)
                    )));
                }
                println!("  📤 工具结果: [{name}] → {}", truncate(&result.output, 60));
            }
            AgentEvent::FinalAnswer(answer) => {
                final_answer = answer.clone();
                println!("\n  ✅ 最终答案: {}", truncate(&answer, 80));
            }
            AgentEvent::Cancelled => {
                println!("\n  ⚠️ 执行已取消");
            }
            _ => {}
        }
    }

    if final_answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo10 验收失败：工具流式执行未产生最终答案".to_string(),
        ));
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
