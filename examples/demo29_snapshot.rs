//! Agent 状态快照与回滚示例
//!
//! 演示：
//! 1. 通过 Builder 启用自动快照
//! 2. 多轮对话后查看快照列表
//! 3. 手动回滚到之前的对话状态
//!
//! ```bash
//! cargo run --example demo29_snapshot
//! ```

use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("=== Agent 状态快照与回滚 ===\n");

    // ── 1. 创建启用快照的 Agent ────────────────────────────────────────────
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("你是一个简洁的助手，每次回答控制在一句话以内")
        .snapshot_policy(SnapshotPolicy::EveryIteration)
        .max_snapshots(20)
        .build()?;

    println!("快照策略: {:?}", agent.snapshots().len());

    // ── 2. 进行多轮对话 ────────────────────────────────────────────────────
    let topics = ["什么是 Rust？", "什么是所有权？", "什么是生命周期？"];

    for topic in &topics {
        println!("\n用户: {topic}");
        let answer = agent.chat(topic).await?;
        println!("助手: {answer}");
        println!("  (当前快照数: {})", agent.snapshots().len());
    }

    // ── 3. 查看快照列表 ────────────────────────────────────────────────────
    println!("\n--- 快照列表 ---");
    for snap in agent.snapshots() {
        println!(
            "  [{}] iteration={}, messages={}, time={}",
            &snap.id[..8],
            snap.iteration,
            snap.messages.len(),
            snap.created_at,
        );
    }

    // ── 4. 回滚到第一轮对话后的状态 ─────────────────────────────────────────
    println!("\n--- 回滚到第 1 轮对话后的状态 ---");
    let total_snapshots = agent.snapshots().len();
    if total_snapshots >= 2 {
        let steps_back = total_snapshots - 1;
        if let Some(snapshot) = agent.rollback(steps_back) {
            println!(
                "已回滚到快照 {} (iteration={}, messages={})",
                &snapshot.id[..8],
                snapshot.iteration,
                snapshot.messages.len(),
            );
            println!("当前消息数: {}", agent.get_messages().len());
            println!("剩余快照数: {}", agent.snapshots().len());
        }
    }

    // ── 5. 在回滚后继续对话 ────────────────────────────────────────────────
    println!("\n--- 回滚后继续对话 ---");
    let answer = agent.chat("那借用呢？").await?;
    println!("用户: 那借用呢？");
    println!("助手: {answer}");
    println!("当前快照数: {}", agent.snapshots().len());

    // ── 6. 手动快照 ─────────────────────────────────────────────────────────
    println!("\n--- 手动快照 ---");
    if let Some(id) = agent.snapshot() {
        println!("手动快照 ID: {}", &id[..8]);
    }
    println!("最终快照数: {}", agent.snapshots().len());

    Ok(())
}
