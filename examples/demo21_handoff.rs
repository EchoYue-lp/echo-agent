//! Handoff 示例：Agent 间控制权转移
//!
//! 演示如何在多个 Agent 之间传递控制权和上下文。
//!
//! ```bash
//! cargo run --example demo21_handoff
//! ```

use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("=== Handoff 示例：Agent 间控制权转移 ===\n");

    // 创建多个专业 Agent
    let translator = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("translator")
        .system_prompt("你是一个专业的翻译助手，负责将中文翻译为英文。只返回翻译结果，不要解释。")
        .build()?;

    let summarizer = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("summarizer")
        .system_prompt("你是一个摘要助手，负责将文本压缩为一句话摘要。只返回摘要，不要解释。")
        .build()?;

    // 创建 Handoff 管理器
    let mut manager = HandoffManager::new();
    manager.register("translator", translator);
    manager.register("summarizer", summarizer);

    println!("已注册 Agent: {:?}\n", manager.registered_agents());

    // ── 单次 Handoff ──────────────────────────────────────────
    println!("--- 1. 单次 Handoff ---");

    let target = HandoffTarget::new("translator")
        .with_message("请翻译：人工智能正在改变世界");

    let context = HandoffContext::new()
        .with_source("orchestrator")
        .with_metadata("task_type", "translation")
        .with_metadata("source_lang", "zh");

    let result = manager.handoff(target, context).await?;
    println!("翻译结果: {}\n", result.output);

    // ── Handoff 链 ────────────────────────────────────────────
    println!("--- 2. Handoff 链（翻译 → 摘要）---");

    let targets = vec![
        HandoffTarget::new("translator")
            .with_message("请翻译以下段落为英文：\n\n人工智能技术近年来取得了突破性进展。大语言模型能够理解和生成自然语言，在翻译、编程、写作等领域展现出强大能力。"),
        HandoffTarget::new("summarizer")
            .with_message("请将以下英文文本总结为一句话"),
    ];

    let initial_ctx = HandoffContext::new()
        .with_source("user")
        .with_metadata("pipeline", "translate-then-summarize");

    let results = manager.handoff_chain(targets, initial_ctx).await?;
    for (i, r) in results.iter().enumerate() {
        println!("步骤 {}: Agent '{}' → {}", i + 1, r.target_agent, r.output);
    }

    println!("\n=== Handoff 示例完成 ===");
    Ok(())
}
