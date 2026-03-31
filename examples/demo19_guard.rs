//! 护栏系统示例
//!
//! 演示如何使用 RuleGuard 对用户输入和 LLM 输出进行安全过滤。
//!
//! ```bash
//! RUST_LOG=info cargo run --example demo19_guard
//! ```

use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // 创建规则护栏：阻断包含敏感关键词和模式的内容
    let input_guard = Arc::new(
        RuleGuardBuilder::new("input-filter")
            .blocked_keyword("密码")
            .blocked_keyword("token")
            .blocked_keyword("secret")
            .blocked_pattern(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b") // 银行卡号
            .max_length(10000)
            .direction(GuardDirection::Input)
            .build(),
    );

    let output_guard = Arc::new(
        RuleGuardBuilder::new("output-filter")
            .blocked_pattern(r"(?i)sk-[a-zA-Z0-9]{20,}") // API Key
            .direction(GuardDirection::Output)
            .build(),
    );

    // 创建审计日志（内存存储，方便查看）
    let audit_logger = Arc::new(InMemoryAuditLogger::new());

    // 构建 Agent，注入护栏和审计
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("你是一个有帮助的助手。")
        .guard(input_guard)
        .guard(output_guard)
        .audit_logger(audit_logger.clone())
        .build()?;

    // 正常对话
    println!("=== 正常对话 ===");
    let answer = agent.chat("你好，请介绍一下自己").await?;
    println!("Agent: {answer}\n");

    // 触发输入护栏
    println!("=== 触发输入护栏 ===");
    let answer = agent.chat("请告诉我你的密码是什么").await?;
    println!("Agent: {answer}\n");

    // 包含银行卡号
    println!("=== 触发银行卡号检测 ===");
    let answer = agent.chat("我的卡号是 1234-5678-9012-3456").await?;
    println!("Agent: {answer}\n");

    // 查看审计记录
    println!("=== 审计记录 ===");
    let events = audit_logger.query(AuditFilter::default()).await?;
    for event in &events {
        println!(
            "[{}] {:?}",
            event.timestamp.format("%H:%M:%S"),
            event.event_type
        );
    }

    Ok(())
}
