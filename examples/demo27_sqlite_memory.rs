//! demo27_sqlite_memory.rs —— SQLite 持久化记忆（FTS5 + 向量检索）演示
//!
//! 展示 SqliteStore 的完整能力：
//! 1. 基本 CRUD 操作
//! 2. FTS5 全文检索（BM25 排序）
//! 3. 向量语义检索（余弦相似度）
//! 4. 命名空间隔离
//! 5. 与 Agent 集成
//!
//! 运行方式：
//! ```bash
//! cargo run --example demo27_sqlite_memory --features sqlite
//! ```

use echo_agent::memory::store::Store;
use echo_agent::prelude::*;
use serde_json::json;
use std::sync::Arc;

const DB_PATH: &str = "/tmp/echo-agent-demo27/memory.db";

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo27_sqlite_memory=info".into()),
        )
        .init();

    // 准备目录
    tokio::fs::create_dir_all("/tmp/echo-agent-demo27")
        .await
        .ok();

    print_banner();

    // Part 1: 基本 CRUD
    separator("Part 1: 基本 CRUD 操作");
    demo_crud().await?;

    // Part 2: FTS5 全文检索
    separator("Part 2: FTS5 全文检索");
    demo_fts5_search().await?;

    // Part 3: 命名空间隔离
    separator("Part 3: 命名空间隔离");
    demo_namespace_isolation().await?;

    // Part 4: 向量语义检索
    separator("Part 4: 向量语义检索（需要 Embedding 服务）");
    demo_semantic_search().await?;

    // Part 5: 与 Agent 集成
    separator("Part 5: SqliteStore × Agent 集成");
    demo_agent_integration().await?;

    // Part 6: 持久化验证
    separator("Part 6: 持久化验证");
    demo_persistence().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo27 完成");
    println!("  数据库文件: {DB_PATH}");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1: CRUD ────────────────────────────────────────────────────────────

async fn demo_crud() -> echo_agent::error::Result<()> {
    let store = SqliteStore::new(DB_PATH)?;
    let ns = &["demo27", "crud"];

    // 写入
    store
        .put(
            ns,
            "user-pref-001",
            json!({
                "content": "用户偏好深色主题",
                "importance": 8,
                "tags": ["偏好", "界面"]
            }),
        )
        .await?;
    println!("  ✅ put: user-pref-001");

    store
        .put(
            ns,
            "user-pref-002",
            json!({
                "content": "用户使用 Rust 语言开发后端服务",
                "importance": 9,
                "tags": ["技术", "编程"]
            }),
        )
        .await?;
    println!("  ✅ put: user-pref-002");

    // 读取
    let item = store.get(ns, "user-pref-001").await?;
    if let Some(item) = item {
        println!(
            "  ✅ get: key={}, value={}",
            item.key,
            serde_json::to_string(&item.value).unwrap_or_default()
        );
    }

    // 更新（upsert）
    store
        .put(
            ns,
            "user-pref-001",
            json!({
                "content": "用户偏好深色主题，且喜欢 Monokai 配色",
                "importance": 9,
                "tags": ["偏好", "界面", "配色"]
            }),
        )
        .await?;
    let updated = store.get(ns, "user-pref-001").await?.unwrap();
    println!(
        "  ✅ upsert: updated_at={}, value 已更新",
        updated.updated_at
    );

    // 删除
    let deleted = store.delete(ns, "user-pref-001").await?;
    println!("  ✅ delete: user-pref-001, found={deleted}");

    let gone = store.get(ns, "user-pref-001").await?;
    println!("  ✅ get after delete: {:?}", gone.map(|i| i.key));

    println!();
    Ok(())
}

// ── Part 2: FTS5 ────────────────────────────────────────────────────────────

async fn demo_fts5_search() -> echo_agent::error::Result<()> {
    let store = SqliteStore::new(DB_PATH)?;
    let ns = &["demo27", "fts5"];

    // 写入测试数据
    let memories = [
        (
            "m1",
            "Rust is a systems programming language focused on safety and performance",
        ),
        (
            "m2",
            "Python is widely used in machine learning and data science",
        ),
        (
            "m3",
            "JavaScript powers the web frontend with frameworks like React and Vue",
        ),
        (
            "m4",
            "Go is designed for cloud native applications and microservices",
        ),
        (
            "m5",
            "Rust provides memory safety without garbage collection through ownership",
        ),
    ];

    for (key, content) in &memories {
        store.put(ns, key, json!({"content": content})).await?;
    }
    println!("  📝 写入 {} 条测试数据\n", memories.len());

    // FTS5 搜索
    let queries = [
        ("Rust", "单关键词搜索"),
        ("memory safety", "多关键词搜索"),
        ("machine learning", "精确短语搜索"),
        ("web frontend", "跨记录搜索"),
    ];

    for (query, desc) in &queries {
        let results = store.search(ns, query, 3).await?;
        println!("  🔍 \"{query}\" ({desc}):");
        if results.is_empty() {
            println!("     无结果");
        } else {
            for item in &results {
                println!(
                    "     [{:.3}] {} → {}",
                    item.score.unwrap_or(0.0),
                    item.key,
                    item.value["content"].as_str().unwrap_or(""),
                );
            }
        }
        println!();
    }

    Ok(())
}

// ── Part 3: 命名空间隔离 ────────────────────────────────────────────────────

async fn demo_namespace_isolation() -> echo_agent::error::Result<()> {
    let store = SqliteStore::new(DB_PATH)?;

    // 不同用户写入同 key
    store
        .put(
            &["alice", "memories"],
            "secret",
            json!({"content": "Alice 的密码是 ****"}),
        )
        .await?;
    store
        .put(
            &["bob", "memories"],
            "secret",
            json!({"content": "Bob 的密码是 ####"}),
        )
        .await?;

    println!("  📝 Alice 和 Bob 各写入 1 条同 key 记忆\n");

    // Alice 只能看到自己的
    let alice_item = store.get(&["alice", "memories"], "secret").await?;
    let bob_item = store.get(&["bob", "memories"], "secret").await?;

    println!(
        "  🔒 Alice 的 secret: {}",
        alice_item.unwrap().value["content"]
    );
    println!(
        "  🔒 Bob   的 secret: {}",
        bob_item.unwrap().value["content"]
    );

    // Bob 搜索 Alice 的内容
    let cross_search = store.search(&["bob", "memories"], "Alice", 10).await?;
    println!(
        "\n  🔍 Bob 搜索 \"Alice\": {} 条命中 ✅ (跨 namespace 不可见)",
        cross_search.len()
    );

    // 列出所有命名空间
    let all_ns = store.list_namespaces(None).await?;
    println!("  📋 所有命名空间: {:?}", all_ns);

    let alice_ns = store.list_namespaces(Some(&["alice"])).await?;
    println!("  📋 Alice 的命名空间: {:?}", alice_ns);

    println!();
    Ok(())
}

// ── Part 4: 向量语义检索 ────────────────────────────────────────────────────

async fn demo_semantic_search() -> echo_agent::error::Result<()> {
    // 检查是否配置了 Embedding 服务
    let has_embedding = std::env::var("EMBEDDING_APIKEY")
        .or_else(|_| std::env::var("EMBEDDING_API_KEY"))
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .is_ok();

    if !has_embedding {
        println!("  ℹ️  未配置 Embedding API Key，跳过向量检索演示");
        println!("  💡 设置 EMBEDDING_APIKEY / EMBEDDING_API_KEY / OPENAI_API_KEY 后可体验");
        println!("     支持 OpenAI、DashScope 等兼容 /v1/embeddings 接口的服务\n");
        return Ok(());
    }

    let embedder = Arc::new(HttpEmbedder::from_env());
    let db_path = "/tmp/echo-agent-demo27/memory_vec.db";
    let store = SqliteStore::with_embedder(db_path, embedder)?;
    let ns = &["demo27", "semantic"];

    println!(
        "  supports_semantic_search = {}\n",
        store.supports_semantic_search()
    );

    // 写入中文记忆
    let memories = [
        ("mem-1", "用户偏好深色主题和 Monokai 配色方案"),
        ("mem-2", "用户是一名资深 Rust 开发者，擅长系统编程"),
        ("mem-3", "用户喜欢古典音乐，特别是肖邦的夜曲"),
        ("mem-4", "用户使用 macOS 进行日常开发工作"),
    ];

    for (key, content) in &memories {
        store.put(ns, key, json!({"content": content})).await?;
    }
    println!(
        "  📝 写入 {} 条中文记忆（已计算嵌入向量）\n",
        memories.len()
    );

    // 语义搜索：用英文查询中文内容
    let queries = [
        ("dark mode color scheme", "英文查询 → 中文记忆"),
        ("programming language expert", "英文查询 → 中文记忆"),
        ("classical music", "英文查询 → 中文记忆"),
    ];

    for (query, desc) in &queries {
        let results = store.semantic_search(ns, query, 2).await?;
        println!("  🧠 \"{query}\" ({desc}):");
        for item in &results {
            println!(
                "     [{:.4}] {} → {}",
                item.score.unwrap_or(0.0),
                item.key,
                item.value["content"].as_str().unwrap_or(""),
            );
        }
        println!();
    }

    // 对比：关键词检索相同的英文查询
    println!("  📊 对比：关键词检索 vs 语义检索\n");
    let query = "music preference";
    let kw_results = store.search(ns, query, 2).await?;
    let sem_results = store.semantic_search(ns, query, 2).await?;
    println!("  🔍 \"{query}\"");
    println!(
        "     关键词检索: {} 条命中 {}",
        kw_results.len(),
        if kw_results.is_empty() { "❌" } else { "✅" }
    );
    println!(
        "     语义检索:   {} 条命中 {}",
        sem_results.len(),
        if sem_results.is_empty() { "❌" } else { "✅" }
    );

    println!();
    Ok(())
}

// ── Part 5: Agent 集成 ──────────────────────────────────────────────────────

async fn demo_agent_integration() -> echo_agent::error::Result<()> {
    let models = echo_agent::llm::config::Config::list_models();
    if models.is_empty() {
        println!("  ℹ️  无模型配置，跳过 Agent 集成测试");
        println!("  💡 创建 echo-agent.yaml 配置模型后可体验\n");
        return Ok(());
    }

    let model_name = &models[0];
    println!("  使用模型: {model_name}\n");

    let store: Arc<dyn Store> = Arc::new(SqliteStore::new(DB_PATH)?);
    let ns = &["agent_demo", "memories"];

    // 预填充记忆
    store
        .put(
            ns,
            "m1",
            json!({"content": "用户叫 Alice，是一名 Rust 开发者"}),
        )
        .await?;
    store
        .put(
            ns,
            "m2",
            json!({"content": "用户偏好使用 SQLite 作为嵌入式数据库"}),
        )
        .await?;
    println!("  📚 预填充 2 条长期记忆\n");

    let llm_config = LlmConfig::from_model(model_name)?;
    let mut agent = ReactAgentBuilder::new()
        .llm_config(llm_config)
        .name("agent_demo")
        .system_prompt("你是 Alice 的私人助手，善于结合长期记忆给出个性化建议。用中文简洁回答。")
        .enable_tools()
        .max_iterations(5)
        .build()?;

    agent.set_memory_store(store);

    println!("  👤 用户: 推荐一个适合我的数据库方案");
    match agent.execute("推荐一个适合我的数据库方案").await {
        Ok(answer) => println!("  🤖 Agent: {answer}"),
        Err(e) => println!("  ⚠️  执行出错: {e}"),
    }

    println!();
    Ok(())
}

// ── Part 6: 持久化验证 ──────────────────────────────────────────────────────

async fn demo_persistence() -> echo_agent::error::Result<()> {
    println!("  验证 SQLite 数据跨实例持久化\n");

    let ns = &["demo27", "persist"];

    // 实例 1：写入数据
    {
        let store = SqliteStore::new(DB_PATH)?;
        store
            .put(
                ns,
                "persist-key",
                json!({"content": "这条记忆会在数据库关闭后保留"}),
            )
            .await?;
        println!("  ✅ 实例 1: 写入 persist-key");
        // store dropped here
    }

    // 实例 2：重新打开数据库，验证数据还在
    {
        let store = SqliteStore::new(DB_PATH)?;
        let item = store.get(ns, "persist-key").await?;
        match item {
            Some(item) => {
                println!(
                    "  ✅ 实例 2: 读取成功 → {}",
                    item.value["content"].as_str().unwrap_or("")
                );
            }
            None => {
                println!("  ❌ 实例 2: 数据丢失！");
            }
        }

        // FTS5 索引也能跨实例
        let results = store.search(ns, "记忆 保留", 5).await?;
        println!(
            "  ✅ 实例 2: FTS5 搜索 \"记忆 保留\" → {} 条命中",
            results.len()
        );

        // 清理
        store.delete(ns, "persist-key").await?;
    }

    // 打印数据库文件大小
    if let Ok(meta) = std::fs::metadata(DB_PATH) {
        println!(
            "\n  📁 数据库文件大小: {:.1} KB",
            meta.len() as f64 / 1024.0
        );
    }

    println!();
    Ok(())
}

// ── 辅助 ────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × SQLite 持久化记忆 (demo27)");
    println!("      FTS5 全文检索 + 向量语义检索");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
