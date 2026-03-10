//! demo17_chat.rs —— 多轮对话模式（chat / chat_stream）综合演示

use echo_agent::agent::{Agent, AgentEvent};
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool, SubtractTool};
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=warn,demo17_chat=info".into()),
        )
        .init();

    print_banner();

    separator("Part 1: chat() — 基础多轮对话（上下文记忆）");
    demo_basic_chat().await?;

    separator("Part 2: chat() + 工具调用 — 多轮数学推理");
    demo_chat_with_tools().await?;

    separator("Part 3: chat_stream() — 流式多轮对话");
    demo_chat_stream().await?;

    separator("Part 4: execute() vs chat() 行为对比");
    demo_execute_vs_chat().await?;

    separator("Part 5: reset() 会话生命周期");
    demo_reset_lifecycle().await?;

    println!("\n{}", "═".repeat(62));
    println!("  demo17 完成");
    println!("{}", "═".repeat(62));

    Ok(())
}

// ── Part 1: 基础多轮对话 ──────────────────────────────────────────────────────

async fn demo_basic_chat() -> echo_agent::error::Result<()> {
    println!("  同一个 agent 实例连续调用 chat()，每轮都能看到之前的对话\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("chat_agent")
        .system_prompt("你是一个友好的助手，请用中文回答，保持简洁。")
        .max_iterations(3)
        .build()?;

    // 第一轮
    println!("  👤 用户: 你好，我叫小明，我是一名 Rust 程序员。");
    let r1 = agent.chat("你好，我叫小明，我是一名 Rust 程序员。").await?;
    println!("  🤖 Agent: {r1}\n");

    // 第二轮
    println!("  👤 用户: 你还记得我的名字和职业吗？");
    let r2 = agent.chat("你还记得我的名字和职业吗？").await?;
    println!("  🤖 Agent: {r2}\n");

    agent.reset();
    println!("  🔄 reset() 后上下文已清除");

    Ok(())
}

// ── Part 2: chat() + 工具调用 ─────────────────────────────────────────────────

async fn demo_chat_with_tools() -> echo_agent::error::Result<()> {
    println!("  多轮数学推理：Agent 记住前一轮的计算结果\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("math_chat_agent")
        .system_prompt("你是一个计算助手，必须通过工具完成计算。记住每轮的计算结果。")
        .enable_tools()
        .max_iterations(8)
        .build()?;

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    println!("  👤 用户: 计算 15 + 27");
    let r1 = agent.chat("计算 15 + 27，记住这个结果。").await?;
    println!("  🤖 Agent: {r1}\n");

    println!("  👤 用户: 把上一步的结果再乘以 3");
    let r2 = agent.chat("把上一步的结果再乘以 3。").await?;
    println!("  🤖 Agent: {r2}\n");

    Ok(())
}

// ── Part 3: chat_stream() ─────────────────────────────────────────────────────

async fn demo_chat_stream() -> echo_agent::error::Result<()> {
    println!("  使用 chat_stream() 进行流式多轮对话\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("stream_chat_agent")
        .system_prompt("你是一个助手，用中文回答，不超过三句话。")
        .max_iterations(3)
        .build()?;

    let messages = [
        "我在学习 Rust，想了解异步编程。",
        "能给我一个 async/await 的简单例子吗？",
    ];

    for msg in &messages {
        println!("  👤 用户: {msg}");
        print!("  🤖 Agent: ");
        std::io::stdout().flush().ok();

        let mut stream = agent.chat_stream(msg).await?;

        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::Token(token) => {
                    print!("{token}");
                    std::io::stdout().flush().ok();
                }
                AgentEvent::FinalAnswer(_) => break,
                _ => {}
            }
        }
        println!("\n");
    }

    Ok(())
}

// ── Part 4: execute() vs chat() 对比 ─────────────────────────────────────────

async fn demo_execute_vs_chat() -> echo_agent::error::Result<()> {
    println!("  对比 execute() 和 chat() 的行为差异\n");

    // execute() 每次重置
    let mut exec_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("exec_agent")
        .system_prompt("你是一个助手，用中文简洁回答。")
        .max_iterations(3)
        .build()?;

    println!("  ── execute() 模式（每次独立）──");
    println!("  👤 第1轮: 记住：我的幸运数字是 42。");
    let _ = exec_agent.execute("记住：我的幸运数字是 42。").await?;
    println!("  👤 第2轮: 我的幸运数字是多少？");
    let r = exec_agent.execute("我的幸运数字是多少？").await?;
    println!("  🤖 Agent: {r}");
    println!("  ℹ️  execute() 第2轮看不到第1轮的信息\n");

    // chat() 保留历史
    let mut chat_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("chat_cmp_agent")
        .system_prompt("你是一个助手，用中文简洁回答。")
        .max_iterations(3)
        .build()?;

    println!("  ── chat() 模式（保留历史）──");
    println!("  👤 第1轮: 记住：我的幸运数字是 42。");
    let _ = chat_agent.chat("记住：我的幸运数字是 42。").await?;
    println!("  👤 第2轮: 我的幸运数字是多少？");
    let r = chat_agent.chat("我的幸运数字是多少？").await?;
    println!("  🤖 Agent: {r}");
    println!("  ℹ️  chat() 第2轮能看到第1轮的信息\n");

    Ok(())
}

// ── Part 5: reset() 会话生命周期 ────────────────────────────────────────────

async fn demo_reset_lifecycle() -> echo_agent::error::Result<()> {
    println!("  reset() 清除上下文，开启新会话\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent: Box<dyn Agent> = Box::new(
        ReactAgentBuilder::new()
            .model("qwen3-max")
            .name("lifecycle_agent")
            .system_prompt("你是一个助手，用中文简洁回答。")
            .max_iterations(3)
            .build()?,
    );

    println!("  ── 会话 1 ──");
    println!("  👤 用户: 记住：我最喜欢的颜色是蓝色。");
    let r1 = agent.chat("记住：我最喜欢的颜色是蓝色。").await?;
    println!("  🤖 Agent: {r1}");

    println!("  👤 用户: 我最喜欢什么颜色？");
    let r2 = agent.chat("我最喜欢什么颜色？").await?;
    println!("  🤖 Agent: {r2}");

    agent.reset();
    println!("\n  🔄 agent.reset() ← 清除上下文\n");

    println!("  ── 会话 2（全新）──");
    println!("  👤 用户: 我最喜欢什么颜色？");
    let r3 = agent.chat("我最喜欢什么颜色？").await?;
    println!("  🤖 Agent: {r3}");
    println!("  ℹ️  reset() 后 Agent 不再记得「蓝色」");

    Ok(())
}

// ── 辅助 ──────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(62));
    println!("      Echo Agent × 多轮对话模式 (demo17)");
    println!("{}", "═".repeat(62));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(62));
    println!("{title}\n");
}
