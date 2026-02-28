//! demo10_streaming.rs —— 流式输出综合演示
//!
//! 从底层到上层，完整展示三个层次的流式能力：
//!
//! ```text
//! Part 1: 原始 LLM 层流式调用（stream_chat）
//!         直接从 SSE 流接收 token，实时打印，适合底层集成场景
//!
//! Part 2: Agent 流式执行 —— 纯文本响应
//!         execute_stream 收到 Token / FinalAnswer 事件
//!         展示流式输出与完整答案的关系
//!
//! Part 3: Agent 流式执行 —— 工具调用 ReAct 循环
//!         完整事件序列：Token → ToolCall → ToolResult → ... → FinalAnswer
//!         配合数学工具演示多步推理的实时可观测性
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo10_streaming
//! ```

use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::agent::{Agent, AgentEvent};
use echo_agent::llm::stream_chat;
use echo_agent::llm::types::Message;
use echo_agent::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};
use futures::StreamExt;
use reqwest::Client;
use std::io::Write;
use std::sync::Arc;

// ── 入口 ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    // 只打印 WARN 以上的框架日志，让流式输出保持干净
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo10_streaming=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1 ────────────────────────────────────────────────────────────────
    println!("{}", "─".repeat(60));
    println!("Part 1: LLM 原始流式调用（stream_chat）\n");
    demo_raw_stream().await?;

    println!();

    // ── Part 2 ────────────────────────────────────────────────────────────────
    println!("{}", "─".repeat(60));
    println!("Part 2: Agent 流式执行 —— 纯文本响应\n");
    demo_agent_text_stream().await?;

    println!();

    // ── Part 3 ────────────────────────────────────────────────────────────────
    println!("{}", "─".repeat(60));
    println!("Part 3: Agent 流式执行 —— 工具调用 ReAct 循环\n");
    demo_agent_tool_stream().await?;

    println!();
    println!("{}", "═".repeat(60));
    println!("  demo10 完成");
    println!("{}", "═".repeat(60));

    Ok(())
}

// ── Part 1: 原始 LLM 流式调用 ────────────────────────────────────────────────

async fn demo_raw_stream() -> echo_agent::error::Result<()> {
    println!("直接调用 stream_chat，逐 token 打印（无工具、无 Agent 包装）\n");

    let client = Arc::new(Client::new());
    let messages = vec![
        Message::system("你是一个助手，请用中文简洁作答。".to_string()),
        Message::user(
            "用三句话解释什么是流式输出（streaming output），以及它对用户体验的好处。".to_string(),
        ),
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
        )
        .await?,
    );

    print!("  🤖 LLM: ");
    std::io::stdout().flush().ok();

    let mut token_count = 0usize;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        if let Some(choice) = chunk.choices.first() {
            if let Some(content) = &choice.delta.content {
                if !content.is_empty() {
                    print!("{}", content);
                    std::io::stdout().flush().ok();
                    token_count += 1;
                }
            }
        }
    }

    println!("\n");
    println!("  📊 共收到 {token_count} 个 token 增量");

    Ok(())
}

// ── Part 2: Agent 流式 —— 纯文本 ─────────────────────────────────────────────

async fn demo_agent_text_stream() -> echo_agent::error::Result<()> {
    println!("Agent 不调用任何工具时，流式文本逐字显示，最后汇总为 FinalAnswer\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "stream_text_agent",
        "你是一个知识渊博的助手，用中文简洁回答问题。不需要调用任何工具。",
    )
    .enable_tool(false)
    .enable_task(false)
    .max_iterations(3);

    let mut agent = ReactAgent::new(config);

    println!("  任务: 列举三个 Rust 语言最显著的特点，每点一句话\n");
    println!("  事件流:");

    let mut event_stream = agent
        .execute_stream("列举三个 Rust 语言最显著的特点，每点一句话。")
        .await?;

    let mut token_buf = String::new();
    let mut event_idx = 0usize;

    print!("  [{:>2}] Token    ▶ ", event_idx);
    std::io::stdout().flush().ok();

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::Token(token) => {
                token_buf.push_str(&token);
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args } => {
                println!();
                event_idx += 1;
                println!("  [{event_idx:>2}] ToolCall  ▶ {name}({args})");
                event_idx += 1;
                print!("  [{event_idx:>2}] Token    ▶ ");
                std::io::stdout().flush().ok();
                token_buf.clear();
            }
            AgentEvent::ToolResult { name, output } => {
                println!();
                event_idx += 1;
                let preview = truncate_chars(&output, 60);
                println!("  [{event_idx:>2}] ToolResult▶ [{name}] {preview}");
                event_idx += 1;
                print!("  [{event_idx:>2}] Token    ▶ ");
                std::io::stdout().flush().ok();
                token_buf.clear();
            }
            AgentEvent::FinalAnswer(answer) => {
                println!();
                println!();
                println!("  ✅ FinalAnswer ({} 字符)", answer.len());
            }
        }
    }

    Ok(())
}

// ── Part 3: Agent 流式 —— 工具调用 ───────────────────────────────────────────

async fn demo_agent_tool_stream() -> echo_agent::error::Result<()> {
    println!("多步数学推理：Agent 发出工具调用，每个事件实时可观测\n");

    let system_prompt = r#"你是一个计算助手，必须通过工具完成所有计算。

规则：
1. 按顺序调用 add/subtract/multiply/divide 执行计算
2. 所有步骤完成后，用 final_answer 报告完整计算过程和最终结果
"#;

    let config = AgentConfig::new("qwen3-max", "stream_math_agent", system_prompt)
        .enable_tool(true)
        .enable_task(false)
        .max_iterations(10);

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(DivideTool));

    let task = "计算：(15 + 27) × 4 - (100 / 5)";
    println!("  任务: {task}\n");
    println!("  事件流 (实时):\n");

    let mut event_stream = agent.execute_stream(task).await?;

    let mut iteration = 0usize;
    let mut in_token_line = false;

    while let Some(event_result) = event_stream.next().await {
        match event_result? {
            AgentEvent::Token(token) => {
                if !in_token_line {
                    iteration += 1;
                    print!("  [iter {iteration}] 💭 思考: ");
                    std::io::stdout().flush().ok();
                    in_token_line = true;
                }
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args } => {
                if in_token_line {
                    println!();
                    in_token_line = false;
                }
                // 格式化参数，只显示值部分
                let args_display = format_args_compact(&args);
                println!("  [iter {iteration}] 🔧 工具调用: {name}({args_display})");
            }
            AgentEvent::ToolResult { name, output } => {
                if in_token_line {
                    println!();
                    in_token_line = false;
                }
                let preview = truncate_chars(&output, 80);
                println!("  [iter {iteration}] 📤 工具结果: [{name}] → {preview}");
            }
            AgentEvent::FinalAnswer(answer) => {
                if in_token_line {
                    println!();
                    in_token_line = false;
                }
                println!();
                println!("  ✅ 最终答案:");
                // 每行缩进打印
                for line in answer.lines() {
                    println!("     {line}");
                }
            }
        }
    }

    println!();
    println!("  📊 共经历 {iteration} 轮 LLM 推理");

    Ok(())
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(60));
    println!("      Echo Agent × 流式输出综合演示 (demo10)");
    println!("{}", "═".repeat(60));
    println!();
    println!("  本 demo 从三个层次展示流式能力：");
    println!("    Part 1  原始 LLM SSE 流（stream_chat）");
    println!("    Part 2  Agent 流式文本响应（execute_stream）");
    println!("    Part 3  Agent 流式工具调用（ReAct 全事件观测）");
    println!();
}

/// 按字符数截断字符串，避免字节切片 panic
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// 将 serde_json::Value 的参数紧凑显示，适合单行日志
fn format_args_compact(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}={val}")
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}
