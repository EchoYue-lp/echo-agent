//! demo18_semantic_memory.rs —— Store 语义搜索（向量检索）综合演示

use echo_agent::agent::Agent;
use echo_agent::memory::store::{InMemoryStore, Store};
use echo_agent::memory::{EmbeddingStore, HttpEmbedder};
use echo_agent::prelude::*;
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo18_semantic_memory=info".into()),
        )
        .init();

    print_banner();

    separator("Part 1: 关键词搜索 vs 语义搜索");
    demo_keyword_vs_semantic().await?;

    separator("Part 2: remember + recall 工具（语义模式）");
    demo_agent_with_semantic_memory().await?;

    separator("Part 3: set_memory_store — 运行时替换 Store");
    demo_set_memory_store().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo18 完成");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1: 关键词搜索 vs 语义搜索 ───────────────────────────────────────────

async fn demo_keyword_vs_semantic() -> echo_agent::error::Result<()> {
    println!("  对比「关键词检索」和「语义检索」在跨语言查询上的差异\n");

    let memories = [
        ("用户偏好深色主题", vec!["偏好", "界面"]),
        ("用户喜欢古典音乐，尤其是肖邦", vec!["偏好", "音乐"]),
        ("用户是 Rust 程序员，主要做后端开发", vec!["职业", "编程"]),
    ];

    // 关键词 Store
    let kw_store = Arc::new(InMemoryStore::new());

    // 语义 Store
    let embedder = Arc::new(HttpEmbedder::from_env());
    let inner = Arc::new(InMemoryStore::new());
    let sem_store = Arc::new(EmbeddingStore::new(inner.clone(), embedder));

    let ns = &["demo", "memories"];

    // 写入
    for (i, (content, tags)) in memories.iter().enumerate() {
        let key = format!("mem-{i:03}");
        let value = json!({ "content": content, "tags": tags });
        kw_store.put(ns, &key, value.clone()).await?;
        sem_store.put(ns, &key, value).await?;
    }

    println!("  已写入 {} 条记忆（中文）\n", memories.len());

    // 测试查询
    let queries = [
        ("music preference", "英文查询「音乐偏好」"),
        ("dark mode", "英文查询「深色模式」"),
    ];

    for (query, desc) in &queries {
        let kw_hits = kw_store.search(ns, query, 3).await?;
        let sem_hits = sem_store.semantic_search(ns, query, 3).await?;

        println!("  🔍 查询: \"{query}\"  ({desc})");
        println!(
            "     关键词检索: {} 条命中  {}",
            kw_hits.len(),
            if kw_hits.is_empty() { "❌" } else { "✅" }
        );
        println!(
            "     语义搜索:   {} 条命中  {}",
            sem_hits.len(),
            if !sem_hits.is_empty() { "✅" } else { "❌" }
        );
        println!();
    }

    Ok(())
}

// ── Part 2: Agent + 语义记忆工具 ──────────────────────────────────────────────

async fn demo_agent_with_semantic_memory() -> echo_agent::error::Result<()> {
    println!("  Agent 使用语义记忆，通过 recall 工具语义检索\n");

    // 创建 EmbeddingStore
    let inner = Arc::new(InMemoryStore::new());
    let embedder = Arc::new(HttpEmbedder::from_env());
    let store = Arc::new(EmbeddingStore::new(inner as Arc<dyn Store>, embedder));

    // 预填充记忆
    let ns = vec!["memory_agent".to_string(), "memories".to_string()];
    let ns_ref: Vec<&str> = ns.iter().map(String::as_str).collect();

    store
        .put(
            &ns_ref,
            "m1",
            json!({"content": "用户叫 Alice，是一名数据科学家"}),
        )
        .await?;
    store
        .put(
            &ns_ref,
            "m2",
            json!({"content": "用户偏好 Python 和 PyTorch"}),
        )
        .await?;

    println!("  📚 预填充 2 条长期记忆\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("memory_agent")
        .system_prompt("你是 Alice 的私人助手，结合长期记忆给出个性化建议。")
        .enable_tools()
        .max_iterations(5)
        .build()?;

    agent.set_memory_store(store);

    // 执行任务
    println!("  👤 用户: 帮我推荐适合数据科学研究的 Rust 库");
    match agent.execute("帮我推荐适合数据科学研究的 Rust 库").await {
        Ok(answer) => println!("  🤖 Agent: {answer}\n"),
        Err(e) => println!("  ❌ 错误: {e}\n"),
    }

    Ok(())
}

// ── Part 3: set_memory_store 运行时替换 ──────────────────────────────────────

async fn demo_set_memory_store() -> echo_agent::error::Result<()> {
    println!("  展示 set_memory_store() 在 new() 之后热替换 Store 的用法\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("hotswap_agent")
        .system_prompt("你是一个助手。")
        .max_iterations(3)
        .build()?;

    println!("  ✅ ReactAgent 创建完成（无 Store）");

    // 运行时挂载 EmbeddingStore
    let inner = Arc::new(InMemoryStore::new());
    let embedder = Arc::new(HttpEmbedder::from_env());
    let store = Arc::new(EmbeddingStore::new(inner as Arc<dyn Store>, embedder));

    agent.set_memory_store(store.clone());
    println!("  ✅ EmbeddingStore 已挂载");

    if let Some(s) = agent.store() {
        println!(
            "  ✅ store.supports_semantic_search() = {}",
            s.supports_semantic_search()
        );
    }

    println!("\n  ℹ️  接下来 Agent 的 remember/recall 工具将使用向量检索");

    Ok(())
}

// ── 辅助 ──────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × Store 语义搜索 (demo18)");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
