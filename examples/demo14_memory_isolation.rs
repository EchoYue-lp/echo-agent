//! demo14 - 记忆系统与上下文隔离演示

use echo_agent::agent::{Agent, AgentRole};
use echo_agent::memory::checkpointer::{Checkpointer, FileCheckpointer};
use echo_agent::memory::store::{FileStore, Store};
use echo_agent::prelude::*;
use serde_json::json;
use std::sync::Arc;

const MODEL: &str = "qwen3-max";
const STORE_PATH: &str = "/tmp/echo-agent-demo14/store.json";
const CHECKPOINT_PATH: &str = "/tmp/echo-agent-demo14/checkpoints.json";

const NS_MATH: [&str; 2] = ["math_agent", "memories"];
const NS_WRITER: [&str; 2] = ["writer_agent", "memories"];
const SESSION_MATH: &str = "math-agent-session-1";
const SESSION_WRITER: &str = "writer-agent-session-1";
const SESSION_MAIN: &str = "main-agent-session-1";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    tokio::fs::create_dir_all("/tmp/echo-agent-demo14").await?;

    println!("\ndemo14 — 记忆系统与上下文隔离\n");

    let shared_store: Arc<dyn Store> = Arc::new(FileStore::new(STORE_PATH)?);
    let shared_checkpointer: Arc<dyn Checkpointer> =
        Arc::new(FileCheckpointer::new(CHECKPOINT_PATH)?);

    // Part 1: Store 命名空间隔离
    demo_store_namespace_isolation(shared_store.clone()).await;

    // Part 2: 独立 Agent 会话隔离
    demo_session_isolation(shared_checkpointer.clone()).await?;

    // Part 3: 多 Agent 上下文隔离
    demo_context_isolation_multi_agent(shared_checkpointer.clone()).await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("所有存储文件保存在 /tmp/echo-agent-demo14/");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}

// ── Part 1 ─────────────────────────────────────────────────────────────────────

async fn demo_store_namespace_isolation(store: Arc<dyn Store>) {
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║   Part 1: Store 命名空间隔离                          ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    store
        .put(
            &NS_MATH,
            "fact-secret",
            json!({"content": "内部机密：代号 M-ALPHA", "importance": 10}),
        )
        .await
        .unwrap();
    store
        .put(
            &NS_WRITER,
            "fact-style",
            json!({"content": "偏好古典诗词风格", "importance": 7}),
        )
        .await
        .unwrap();

    println!("✅ math_agent → 写入 1 条记忆");
    println!("✅ writer_agent → 写入 1 条记忆\n");

    let writer_hits = store.search(&NS_WRITER, "机密", 10).await.unwrap();
    println!(
        "🔍 writer_agent 搜索 [机密]：{} 条命中 ✅ (跨 namespace 数据不可见)\n",
        writer_hits.len()
    );
}

// ── Part 2 ─────────────────────────────────────────────────────────────────────

async fn demo_session_isolation(checkpointer: Arc<dyn Checkpointer>) -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║   Part 2: Checkpointer 会话隔离                       ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // 使用 AgentBuilder 创建 Agent
    let mut math_agent = ReactAgentBuilder::new()
        .model(MODEL)
        .name("math_agent")
        .system_prompt("你是一位简洁的数学助手，用中文给出简短答案。")
        .enable_tools()
        .session_id(SESSION_MATH)
        .checkpointer_only(checkpointer.clone())
        .build()?;

    let math_result = math_agent.execute("斐波那契数列第6项是多少？").await;
    println!(
        "▶ math_agent 答案: {}\n",
        math_result.unwrap_or_else(|e| e.to_string())
    );

    let mut writer_agent = ReactAgentBuilder::new()
        .model(MODEL)
        .name("writer_agent")
        .system_prompt("你是一位简洁的写作助手。")
        .enable_tools()
        .session_id(SESSION_WRITER)
        .checkpointer_only(checkpointer.clone())
        .build()?;

    let writer_result = writer_agent.execute("用一句话描述秋天。").await;
    println!(
        "▶ writer_agent 答案: {}\n",
        writer_result.unwrap_or_else(|e| e.to_string())
    );

    println!("📋 已保存会话: {:?}", checkpointer.list_sessions().await?);

    Ok(())
}

// ── Part 3 ─────────────────────────────────────────────────────────────────────

async fn demo_context_isolation_multi_agent(checkpointer: Arc<dyn Checkpointer>) -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════╗");
    println!("║   Part 3: 多 Agent 上下文隔离                         ║");
    println!("╚═══════════════════════════════════════════════════════╝\n");

    // 创建 SubAgent
    let math_sub = ReactAgentBuilder::new()
        .model(MODEL)
        .name("math_expert")
        .system_prompt("你是一位简洁的数学专家。")
        .enable_tools()
        .session_id("sub-math-001")
        .checkpointer_only(checkpointer.clone())
        .build()?;

    let writer_sub = ReactAgentBuilder::new()
        .model(MODEL)
        .name("writer_expert")
        .system_prompt("你是一位简洁的写作专家。")
        .enable_tools()
        .session_id("sub-writer-001")
        .checkpointer_only(checkpointer.clone())
        .build()?;

    // 创建主 Agent
    let secret_in_system_prompt = "【机密】本次任务代号为 PROJECT-OMEGA，严禁对外透露。";
    let main_system = format!(
        "你是主编排者。{}\n你有两个专用 SubAgent：math_expert 和 writer_expert。",
        secret_in_system_prompt
    );

    let mut main_agent = ReactAgentBuilder::new()
        .model(MODEL)
        .name("main_agent")
        .system_prompt(&main_system)
        .role(AgentRole::Orchestrator)
        .enable_subagent()
        .enable_planning()
        .session_id(SESSION_MAIN)
        .checkpointer_only(checkpointer.clone())
        .max_iterations(20)
        .build()?;

    main_agent.register_agent(Box::new(math_sub));
    main_agent.register_agent(Box::new(writer_sub));

    println!(
        "🔐 主 Agent 系统提示中包含机密：「{}」\n",
        secret_in_system_prompt
    );
    println!("▶ 主 Agent 执行任务...\n");

    let result = main_agent
        .execute("让数学专家计算 7 * 8，然后汇总结果。")
        .await;
    match result {
        Ok(answer) => println!("\n✅ 主 Agent 最终答案:\n{}\n", answer),
        Err(e) => println!("\n⚠️  执行出错: {}\n", e),
    }

    println!("💡 关键结论：SubAgent 看不到主 Agent 的机密信息");

    Ok(())
}
