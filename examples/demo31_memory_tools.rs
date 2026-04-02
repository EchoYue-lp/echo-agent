//! demo31_memory_tools —— Memory Tool 自动注入 ReAct 循环
//!
//! 演示 `ReactAgentBuilder::with_memory_tools(store)` 如何一行代码
//! 自动注册 remember / recall / search_memory / forget 四个内置工具，
//! 将 Store 直接暴露给 LLM，实现 Agent 自主使用记忆。
//!
//! ```bash
//! cargo run --example demo31_memory_tools
//! ```

use echo_agent::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("echo_agent=info")
        .init();

    println!("═══ Memory Tool Auto-Injection Demo ═══\n");

    // ── 1. 使用 with_memory_tools 构建 Agent ──────────────────────────────
    let store = Arc::new(InMemoryStore::new());

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("memory_agent")
        .system_prompt("你是一个具有长期记忆能力的助手")
        .with_memory_tools(store.clone())
        .build()?;

    let tools = agent.tool_names();
    println!("[1] with_memory_tools() 自动注册的工具：");
    for name in &tools {
        let tag = match *name {
            "remember" | "recall" | "search_memory" | "forget" => "memory",
            "final_answer" => "builtin",
            _ => "other",
        };
        println!("    [{tag:7}] {name}");
    }

    assert!(tools.contains(&"remember"));
    assert!(tools.contains(&"recall"));
    assert!(tools.contains(&"search_memory"));
    assert!(tools.contains(&"forget"));
    println!("    → 全部 4 个记忆工具 ✓\n");

    // ── 2. 手动写入 Store 模拟已有记忆 ────────────────────────────────────
    let ns = &["memory_agent", "memories"];
    store
        .put(
            ns,
            "pref_001",
            serde_json::json!({
                "content": "用户偏好暗色主题，代码使用 JetBrains Mono 字体",
                "importance": 8,
                "tags": ["偏好", "UI"]
            }),
        )
        .await?;
    store
        .put(
            ns,
            "fact_002",
            serde_json::json!({
                "content": "用户的项目使用 Rust + Tokio 技术栈",
                "importance": 9,
                "tags": ["技术", "项目"]
            }),
        )
        .await?;

    println!("[2] 手动写入 2 条模拟记忆");

    // ── 3. 通过 recall 工具搜索记忆 ───────────────────────────────────────
    let results = store.search(ns, "主题偏好", 5).await?;
    println!("[3] store.search('主题偏好') 返回 {} 条结果", results.len());
    for item in &results {
        println!("    ID: {} → {}", &item.key[..8], item.value);
    }

    // ── 4. set_memory_store 也同样注册所有记忆工具 ─────────────────────────
    let config = AgentConfig::minimal("qwen3-max", "bare_agent");
    let mut bare_agent = echo_agent::agent::react_agent::ReactAgent::new(config);
    assert!(!bare_agent.tool_names().contains(&"search_memory"));

    bare_agent.set_memory_store(Arc::new(InMemoryStore::new()));
    assert!(bare_agent.tool_names().contains(&"search_memory"));
    println!("\n[4] set_memory_store() 也能注入记忆工具 ✓");

    println!("\n═══ Demo Complete ═══");
    Ok(())
}
