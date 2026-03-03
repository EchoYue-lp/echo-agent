//! demo18_semantic_memory.rs —— Store 语义搜索（向量检索）综合演示
//!
//! 展示 `EmbeddingStore` 相对于默认关键词搜索的核心优势：
//!
//! ```text
//! Part 1: 关键词搜索 vs 语义搜索 —— 跨语言/同义词命中对比
//!         关键词：中文存储 + 英文查询，关键词检索命中率为 0
//!         语义搜索：向量空间对齐，跨语言相似语义正确召回
//!
//! Part 2: remember + recall 工具（语义模式）
//!         Agent 通过 remember 工具写入记忆，recall 工具语义检索
//!
//! Part 3: set_memory_store —— 运行时替换为 EmbeddingStore
//!         ReactAgent::new() 后通过 set_memory_store() 热替换 Store
//! ```
//!
//! # 运行前置条件
//!
//! 需要嵌入 API 凭证（支持 OpenAI / Qwen / 其他兼容接口）：
//!
//! ```bash
//! # OpenAI
//! export EMBEDDING_API_KEY="sk-..."
//!
//! # Qwen（DashScope）
//! export EMBEDDING_API_URL="https://dashscope.aliyuncs.com/compatible-mode"
//! export EMBEDDING_API_KEY="sk-..."
//! export EMBEDDING_MODEL="text-embedding-v3"
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo18_semantic_memory
//! ```

use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::memory::store::{InMemoryStore, Store};
use echo_agent::memory::{EmbeddingStore, HttpEmbedder};
use serde_json::json;
use std::sync::Arc;

// ── 入口 ──────────────────────────────────────────────────────────────────────

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

    println!();
    println!("{}", "═".repeat(64));
    println!("  demo18 完成");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1: 关键词搜索 vs 语义搜索 ───────────────────────────────────────────

async fn demo_keyword_vs_semantic() -> echo_agent::error::Result<()> {
    println!("  对比「关键词检索」和「语义检索」在跨语言查询上的差异\n");

    // ── 准备数据 ──────────────────────────────────────────────────────────────
    let memories = [
        ("用户偏好深色主题", vec!["偏好", "界面"]),
        ("用户喜欢古典音乐，尤其是肖邦", vec!["偏好", "音乐"]),
        ("用户是 Rust 程序员，主要做后端开发", vec!["职业", "编程"]),
        ("用户希望每周一总结上周工作进展", vec!["习惯", "工作"]),
        ("用户有两只猫，叫奶茶和布丁", vec!["宠物"]),
    ];

    // 关键词 Store
    let kw_store = Arc::new(InMemoryStore::new());
    // 语义 Store（包装同一份内存 Store）
    let embedder = Arc::new(HttpEmbedder::from_env());
    let inner = Arc::new(InMemoryStore::new());
    let sem_store = Arc::new(EmbeddingStore::new(inner.clone(), embedder));

    let ns = &["demo", "memories"];

    // 写入两个 Store
    for (i, (content, tags)) in memories.iter().enumerate() {
        let key = format!("mem-{i:03}");
        let value = json!({ "content": content, "importance": 7, "tags": tags });
        kw_store.put(ns, &key, value.clone()).await?;
        sem_store.put(ns, &key, value).await?;
    }

    println!("  已写入 {} 条记忆（中文）\n", memories.len());

    // ── 测试查询（故意用英文，命中不同语言和同义词）─────────────────────────
    let queries = [
        (
            "music preference",
            "英文查询「音乐偏好」← 应命中「古典音乐」",
        ),
        ("dark mode", "英文查询「深色模式」← 应命中「深色主题」"),
        ("pet", "英文查询「宠物」← 应命中「猫」"),
        ("编程语言", "中文同义词查询 ← 应命中「Rust 程序员」"),
    ];

    for (query, desc) in &queries {
        let kw_hits = kw_store.search(ns, query, 3).await?;
        let sem_hits = sem_store.semantic_search(ns, query, 3).await?;

        println!("  🔍 查询: \"{}\"  ({})", query, desc);
        println!(
            "     关键词检索: {} 条命中  {}",
            kw_hits.len(),
            if kw_hits.is_empty() {
                "❌ 未命中"
            } else {
                "✅"
            }
        );
        for item in &kw_hits {
            let c = item
                .value
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            println!("       - [score={:.3}] {}", item.score.unwrap_or(0.0), c);
        }
        println!(
            "     语义搜索:   {} 条命中  {}",
            sem_hits.len(),
            if !sem_hits.is_empty() {
                "✅"
            } else {
                "❌ 未命中"
            }
        );
        for item in &sem_hits {
            let c = item
                .value
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            println!("       - [score={:.3}] {}", item.score.unwrap_or(0.0), c);
        }
        println!();
    }

    Ok(())
}

// ── Part 2: Agent + 语义记忆工具 ──────────────────────────────────────────────

async fn demo_agent_with_semantic_memory() -> echo_agent::error::Result<()> {
    println!("  Agent 使用 set_memory_store() 启用语义记忆，通过 recall 工具语义检索\n");

    // 创建 EmbeddingStore
    let inner = Arc::new(InMemoryStore::new());
    let embedder = Arc::new(HttpEmbedder::from_env());
    let store = Arc::new(EmbeddingStore::new(
        inner as Arc<dyn echo_agent::memory::store::Store>,
        embedder,
    ));

    // 预填充记忆（模拟跨会话积累的长期记忆）
    let ns_str = vec!["memory_agent".to_string(), "memories".to_string()];
    let ns: Vec<&str> = ns_str.iter().map(String::as_str).collect();
    store
        .put(
            &ns,
            "m1",
            json!({"content": "用户叫 Alice，是一名数据科学家", "importance": 9, "tags": ["身份"]}),
        )
        .await?;
    store
        .put(&ns, "m2", json!({"content": "用户偏好 Python 和 PyTorch，日常使用 JupyterLab", "importance": 8, "tags": ["技术偏好"]}))
        .await?;
    store
        .put(&ns, "m3", json!({"content": "用户在研究大语言模型的 fine-tuning 方法", "importance": 9, "tags": ["项目", "研究"]}))
        .await?;
    store
        .put(&ns, "m4", json!({"content": "用户喜欢跑步，每周三次，目标完成半马", "importance": 5, "tags": ["运动"]}))
        .await?;

    println!(
        "  📚 预填充 {} 条长期记忆\n",
        store.list_namespaces(None).await?.len()
    );

    // 创建 Agent
    let config = AgentConfig::new(
        "qwen3-max",
        "memory_agent",
        "你是 Alice 的私人助手，用中文回答，结合长期记忆给出个性化建议。",
    )
    .enable_tool(true)
    .enable_task(false)
    .enable_memory(false) // 不自动创建 FileStore
    .max_iterations(5);

    let mut agent = ReactAgent::new(config);
    agent.set_memory_store(store); // ← 替换为 EmbeddingStore，同时重注册工具

    // 用 recall 工具查询（Agent 会自动调用并引用语义相关记忆）
    let queries = [
        "帮我推荐适合数据科学研究的 Rust 库",
        "我的 fine-tuning 研究下一步应该关注什么方向？",
    ];

    for query in &queries {
        println!("  👤 用户: {query}");
        match agent.execute(query).await {
            Ok(answer) => println!("  🤖 Agent: {answer}\n"),
            Err(e) => println!("  ❌ 错误: {e}\n"),
        }
    }

    Ok(())
}

// ── Part 3: set_memory_store 运行时替换 ──────────────────────────────────────

async fn demo_set_memory_store() -> echo_agent::error::Result<()> {
    println!("  展示 set_memory_store() 在 new() 之后热替换 Store 的用法\n");

    // 1. 正常创建 Agent（内部有 FileStore 或无 Store）
    let config = AgentConfig::new(
        "qwen3-max",
        "hotswap_agent",
        "你是一个助手，中文简洁回答，一句话即可。不需要工具。",
    )
    .enable_tool(false)
    .enable_memory(false)
    .max_iterations(3);

    let mut agent = ReactAgent::new(config);
    println!("  ✅ ReactAgent 创建完成（无 Store）");

    // 2. 运行时挂载 EmbeddingStore
    let inner = Arc::new(InMemoryStore::new());
    let embedder = Arc::new(HttpEmbedder::from_env());
    let store = Arc::new(EmbeddingStore::new(
        inner as Arc<dyn echo_agent::memory::store::Store>,
        embedder,
    ));

    agent.set_memory_store(store.clone());
    println!("  ✅ EmbeddingStore 已挂载，remember/recall/forget 工具同步更新");

    // 3. 验证 supports_semantic_search()
    if let Some(s) = agent.store() {
        println!(
            "  ✅ store.supports_semantic_search() = {}",
            s.supports_semantic_search()
        );
    }

    println!("\n  ℹ️  接下来 Agent 的 remember/recall 工具将使用向量检索");
    println!("      可调用 agent.execute(\"记住：...\") 写入记忆，再通过 recall 语义查询");

    Ok(())
}

// ── 辅助 ──────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × Store 语义搜索 (demo18)");
    println!("{}", "═".repeat(64));
    println!();
    println!("  本 demo 演示 EmbeddingStore 语义检索能力：");
    println!("    Part 1  关键词搜索 vs 语义搜索（跨语言对比）");
    println!("    Part 2  Agent + remember/recall 工具（语义模式）");
    println!("    Part 3  set_memory_store() 运行时热替换");
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
