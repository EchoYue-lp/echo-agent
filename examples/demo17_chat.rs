//! demo17_chat.rs —— 多轮对话模式（chat / chat_stream）综合演示
//!
//! 展示 `chat()` / `chat_stream()` 与 `execute()` / `execute_stream()` 的核心区别：
//!
//! ```text
//! Part 1: chat() 基础多轮对话
//!         连续发送三条消息，Agent 全程记住上下文（姓名、偏好）
//!
//! Part 2: chat() + 工具调用
//!         多轮数学对话，Agent 记住前几轮的中间计算结果
//!
//! Part 3: chat_stream() 流式多轮对话
//!         流式接收 Token 事件，同时保留跨轮历史
//!
//! Part 4: execute() vs chat() 对比
//!         execute() 每轮重置上下文；chat() 持续累积
//!
//! Part 5: reset() 会话生命周期
//!         reset() 是 Agent trait 方法，可通过 dyn Agent 调用
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo17_chat
//! ```

use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::agent::{Agent, AgentEvent};
use echo_agent::tools::others::math::{AddTool, MultiplyTool, SubtractTool};
use futures::StreamExt;
use std::io::Write;

// ── 入口 ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=warn,demo17_chat=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1 ────────────────────────────────────────────────────────────────
    separator("Part 1: chat() — 基础多轮对话（上下文记忆）");
    demo_basic_chat().await?;

    // ── Part 2 ────────────────────────────────────────────────────────────────
    separator("Part 2: chat() + 工具调用 — 多轮数学推理");
    demo_chat_with_tools().await?;

    // ── Part 3 ────────────────────────────────────────────────────────────────
    separator("Part 3: chat_stream() — 流式多轮对话");
    demo_chat_stream().await?;

    // ── Part 4 ────────────────────────────────────────────────────────────────
    separator("Part 4: execute() vs chat() 行为对比");
    demo_execute_vs_chat().await?;

    // ── Part 5 ────────────────────────────────────────────────────────────────
    separator("Part 5: reset() 会话生命周期（通过 dyn Agent 调用）");
    demo_reset_lifecycle().await?;

    println!();
    println!("{}", "═".repeat(62));
    println!("  demo17 完成");
    println!("{}", "═".repeat(62));

    Ok(())
}

// ── Part 1: 基础多轮对话 ──────────────────────────────────────────────────────

async fn demo_basic_chat() -> echo_agent::error::Result<()> {
    println!("  同一个 agent 实例连续调用 chat()，每轮都能看到之前的对话\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "chat_agent",
        "你是一个友好的助手，请用中文回答，保持简洁（不超过两句话）。不需要调用工具。",
    )
    .enable_tool(false)
    .enable_task(false)
    .max_iterations(3);

    let mut agent = ReactAgent::new(config);

    // 第一轮：自我介绍
    let msg1 = "你好，我叫小明，我是一名 Rust 程序员。";
    println!("  👤 用户: {msg1}");
    let r1 = agent.chat(msg1).await?;
    println!("  🤖 Agent: {r1}\n");

    // 第二轮：继续对话，Agent 应记住"小明"和"Rust 程序员"
    let msg2 = "你还记得我的名字和职业吗？";
    println!("  👤 用户: {msg2}");
    let r2 = agent.chat(msg2).await?;
    println!("  🤖 Agent: {r2}\n");

    // 第三轮：基于前两轮的信息做出回应
    let msg3 = "根据我的背景，你有什么学习建议吗？";
    println!("  👤 用户: {msg3}");
    let r3 = agent.chat(msg3).await?;
    println!("  🤖 Agent: {r3}\n");

    // 显示对话历史长度
    let (msg_count, token_est) = agent.context_stats();
    println!("  📊 当前上下文：{msg_count} 条消息，估算 ~{token_est} tokens");

    // reset() 清除历史，开启新会话
    agent.reset();
    let (msg_count_after, _) = agent.context_stats();
    println!("  🔄 reset() 后上下文：{msg_count_after} 条消息（仅保留 system prompt）");

    Ok(())
}

// ── Part 2: chat() + 工具调用 ─────────────────────────────────────────────────

async fn demo_chat_with_tools() -> echo_agent::error::Result<()> {
    println!("  多轮数学推理：Agent 记住前一轮的计算结果，用于后续计算\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "math_chat_agent",
        r#"你是一个计算助手。
规则：
1. 需要计算时必须通过工具完成（add/subtract/multiply）
2. 记住每轮对话的计算结果，后续轮次可以引用
3. 用 final_answer 报告本轮结果"#,
    )
    .enable_tool(true)
    .enable_task(false)
    .max_iterations(8);

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    // 第一轮计算
    let msg1 = "计算 15 + 27，记住这个结果。";
    println!("  👤 用户: {msg1}");
    let r1 = agent.chat(msg1).await?;
    println!("  🤖 Agent: {r1}\n");

    // 第二轮引用上一轮结果
    let msg2 = "把上一步的结果再乘以 3。";
    println!("  👤 用户: {msg2}");
    let r2 = agent.chat(msg2).await?;
    println!("  🤖 Agent: {r2}\n");

    // 第三轮继续累积
    let msg3 = "从这个结果中减去 10，给我最终答案。";
    println!("  👤 用户: {msg3}");
    let r3 = agent.chat(msg3).await?;
    println!("  🤖 Agent: {r3}\n");

    let (msg_count, token_est) = agent.context_stats();
    println!("  📊 对话上下文：{msg_count} 条消息，估算 ~{token_est} tokens");

    Ok(())
}

// ── Part 3: chat_stream() ─────────────────────────────────────────────────────

async fn demo_chat_stream() -> echo_agent::error::Result<()> {
    println!("  使用 chat_stream() 进行流式多轮对话，Token 实时输出且历史跨轮保留\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "stream_chat_agent",
        "你是一个助手，请用中文回答，每次回答不超过三句话。不需要调用工具。",
    )
    .enable_tool(false)
    .enable_task(false)
    .max_iterations(3);

    let mut agent = ReactAgent::new(config);

    let messages = [
        "我在学习 Rust，想了解异步编程。",
        "能给我一个 async/await 的简单例子吗？",
        "基于我刚才的问题，你觉得我下一步应该学什么？",
    ];

    for (i, msg) in messages.iter().enumerate() {
        println!("  👤 用户: {msg}");
        print!("  🤖 Agent: ");
        std::io::stdout().flush().ok();

        let mut stream = agent.chat_stream(msg).await?;
        let mut final_answer_len = 0usize;

        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::Token(token) => {
                    print!("{token}");
                    std::io::stdout().flush().ok();
                }
                AgentEvent::FinalAnswer(answer) => {
                    final_answer_len = answer.len();
                    break;
                }
                AgentEvent::ToolCall { name, .. } => {
                    print!("\n  [工具调用: {name}] ");
                    std::io::stdout().flush().ok();
                }
                AgentEvent::ToolResult { .. } => {}
            }
        }

        println!();
        if i < messages.len() - 1 {
            println!("  （{} 字符，跨轮历史已保留）\n", final_answer_len);
        }
    }

    let (msg_count, token_est) = agent.context_stats();
    println!("\n  📊 三轮对话后上下文：{msg_count} 条消息，估算 ~{token_est} tokens");

    Ok(())
}

// ── Part 4: execute() vs chat() 对比 ─────────────────────────────────────────

async fn demo_execute_vs_chat() -> echo_agent::error::Result<()> {
    println!("  对比 execute()（每次重置）和 chat()（保留历史）的行为差异\n");

    let system = "你是一个助手，请用中文简洁回答（一句话即可）。不需要工具。";

    // ── execute() 演示：上下文每次重置 ──────────────────────────────────────
    println!("  ── execute() 模式（每次独立，无跨轮记忆）──");

    let cfg_exec = AgentConfig::new("qwen3-max", "exec_agent", system)
        .enable_tool(false)
        .enable_task(false)
        .max_iterations(3);
    let mut exec_agent = ReactAgent::new(cfg_exec);

    let intro = "记住：我的幸运数字是 42。";
    println!("  👤 第1轮: {intro}");
    let r = exec_agent.execute(intro).await?;
    println!("  🤖 Agent: {r}");

    let query = "我的幸运数字是多少？";
    println!("  👤 第2轮: {query}");
    let r = exec_agent.execute(query).await?;
    println!("  🤖 Agent: {r}");
    println!("  ℹ️  execute() 第2轮已看不到第1轮的「42」\n");

    // ── chat() 演示：上下文持续保留 ─────────────────────────────────────────
    println!("  ── chat() 模式（持续对话，保留跨轮历史）──");

    let cfg_chat = AgentConfig::new("qwen3-max", "chat_cmp_agent", system)
        .enable_tool(false)
        .enable_task(false)
        .max_iterations(3);
    let mut chat_agent = ReactAgent::new(cfg_chat);

    println!("  👤 第1轮: {intro}");
    let r = chat_agent.chat(intro).await?;
    println!("  🤖 Agent: {r}");

    println!("  👤 第2轮: {query}");
    let r = chat_agent.chat(query).await?;
    println!("  🤖 Agent: {r}");
    println!("  ℹ️  chat() 第2轮能看到第1轮的「42」\n");

    // ── 总结 ─────────────────────────────────────────────────────────────────
    println!("  ┌──────────────┬──────────────────────┬─────────────────────┐");
    println!("  │              │ execute()            │ chat()              │");
    println!("  ├──────────────┼──────────────────────┼─────────────────────┤");
    println!("  │ 上下文重置   │ ✅ 每次重置          │ ❌ 保留历史         │");
    println!("  │ 跨轮记忆     │ ❌ 无               │ ✅ 有               │");
    println!("  │ 适用场景     │ 独立单次任务         │ 连续多轮对话        │");
    println!("  └──────────────┴──────────────────────┴─────────────────────┘");

    Ok(())
}

// ── Part 5: reset() 会话生命周期 ──────────────────────────────────────────────

async fn demo_reset_lifecycle() -> echo_agent::error::Result<()> {
    println!("  reset() 是 Agent trait 的方法，可通过 dyn Agent 调用，\n  实现会话的清晰分隔\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "lifecycle_agent",
        "你是一个助手，请用中文简洁回答（一句话）。不需要工具。",
    )
    .enable_tool(false)
    .enable_task(false)
    .max_iterations(3);

    // 以 dyn Agent 持有实例，展示 reset() 是 trait 方法
    let mut agent: Box<dyn Agent> = Box::new(ReactAgent::new(config));

    println!("  ── 会话 1 ──");
    let msg1 = "记住：我最喜欢的颜色是蓝色。";
    println!("  👤 用户: {msg1}");
    let r1 = agent.chat(msg1).await?;
    println!("  🤖 Agent: {r1}");

    let msg2 = "我最喜欢什么颜色？";
    println!("  👤 用户: {msg2}");
    let r2 = agent.chat(msg2).await?;
    println!("  🤖 Agent: {r2}");

    // 通过 dyn Agent 调用 reset()，开启新会话
    agent.reset();
    println!("\n  🔄 agent.reset()  ← dyn Agent 调用，清除上下文\n");

    println!("  ── 会话 2（全新）──");
    println!("  👤 用户: {msg2}");
    let r3 = agent.chat(msg2).await?;
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
    println!("  本 demo 演示 chat() / chat_stream() 多轮对话能力：");
    println!("    Part 1  基础多轮对话（上下文记忆）");
    println!("    Part 2  多轮对话 + 工具调用");
    println!("    Part 3  流式多轮对话（chat_stream）");
    println!("    Part 4  execute() vs chat() 行为对比");
    println!("    Part 5  reset() 会话生命周期（dyn Agent）");
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(62));
    println!("{title}\n");
}
