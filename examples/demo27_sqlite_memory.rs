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
mod support;

use echo_agent::memory::store::Store;
use echo_agent::prelude::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo27_sqlite_memory=info".into()),
        )
        .init();

    // 准备目录
    let db_path = demo27_db_path();
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    cleanup_sqlite_files(&db_path);

    print_banner();

    // Part 1: 基本 CRUD
    separator("Part 1: 基本 CRUD 操作");
    demo_crud(&db_path).await?;

    // Part 2: FTS5 全文检索
    separator("Part 2: FTS5 全文检索");
    demo_fts5_search(&db_path).await?;

    // Part 3: 命名空间隔离
    separator("Part 3: 命名空间隔离");
    demo_namespace_isolation(&db_path).await?;

    // Part 4: 向量语义检索
    separator("Part 4: 向量语义检索（需要 Embedding 服务）");
    if let Err(err) = demo_semantic_search(&db_path).await {
        println!("  ⚠️ 跳过 Part 4：{}\n", err);
    }

    // Part 5: 与 Agent 集成
    separator("Part 5: SqliteStore × Agent 集成");
    if let Err(err) = demo_agent_integration(&db_path).await {
        println!("  ⚠️ 跳过 Part 5：{}\n", err);
    }

    // Part 6: 持久化验证
    separator("Part 6: 持久化验证");
    demo_persistence(&db_path).await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo27 完成");
    println!("  数据库文件: {}", db_path.display());
    println!("{}", "═".repeat(64));

    cleanup_sqlite_files(&db_path);

    Ok(())
}

// ── Part 1: CRUD ────────────────────────────────────────────────────────────

async fn demo_crud(db_path: &Path) -> echo_agent::error::Result<()> {
    let store = SqliteStore::new(db_path)?;
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
    let Some(item) = item else {
        return Err(echo_agent::error::ReactError::Other(
            "demo27 验收失败：CRUD get 未读取到 user-pref-001".to_string(),
        ));
    };
    println!(
        "  ✅ get: key={}, value={}",
        item.key,
        serde_json::to_string(&item.value).unwrap_or_default()
    );

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
    if !deleted {
        return Err(echo_agent::error::ReactError::Other(
            "demo27 验收失败：删除 user-pref-001 返回 false".to_string(),
        ));
    }
    println!("  ✅ delete: user-pref-001, found={deleted}");

    let gone = store.get(ns, "user-pref-001").await?;
    if gone.is_some() {
        return Err(echo_agent::error::ReactError::Other(
            "demo27 验收失败：delete 后仍能读取到 user-pref-001".to_string(),
        ));
    }
    println!("  ✅ get after delete: {:?}", gone.map(|i| i.key));

    println!();
    Ok(())
}

// ── Part 2: FTS5 ────────────────────────────────────────────────────────────

async fn demo_fts5_search(db_path: &Path) -> echo_agent::error::Result<()> {
    let store = SqliteStore::new(db_path)?;
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
        if results.is_empty() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo27 验收失败：FTS5 查询 `{query}` 没有命中"
            )));
        }
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

async fn demo_namespace_isolation(db_path: &Path) -> echo_agent::error::Result<()> {
    let store = SqliteStore::new(db_path)?;

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
    if !cross_search.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo27 验收失败：namespace 隔离失效，Bob 搜到了 Alice 的内容".to_string(),
        ));
    }
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

async fn demo_semantic_search(db_path: &Path) -> echo_agent::error::Result<()> {
    let store = SqliteStore::with_embedder(db_path, load_verified_embedder_from_config().await?)?;
    let ns = &["demo27", "semantic"];

    println!("  hybrid/semantic search 已启用（embedder 已配置）\n");

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
        let results = store
            .search_with(ns, echo_agent::memory::SearchQuery::semantic(query, 2))
            .await?;
        if results.is_empty() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo27 验收失败：语义查询 `{query}` 没有命中"
            )));
        }
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
    let sem_results = store
        .search_with(ns, echo_agent::memory::SearchQuery::semantic(query, 2))
        .await?;
    if sem_results.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo27 验收失败：语义检索对比查询没有命中".to_string(),
        ));
    }
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

async fn demo_agent_integration(db_path: &Path) -> echo_agent::error::Result<()> {
    let llm_config = require_configured_model(None)?;
    let model_name = llm_config.model.clone();
    println!("  使用模型: {model_name}\n");

    let store: Arc<dyn Store> = Arc::new(SqliteStore::new(db_path)?);
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

    let mut agent = ReactAgentBuilder::new()
        .llm_config(llm_config)
        .name("agent_demo")
        .system_prompt("你是 Alice 的私人助手，善于结合长期记忆给出个性化建议。用中文简洁回答。")
        .enable_tools()
        .max_iterations(5)
        .build()?;

    agent.set_memory_store(store);

    println!("  👤 用户: 推荐一个适合我的数据库方案");
    let answer = agent.execute("推荐一个适合我的数据库方案").await?;
    if answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo27 验收失败：Agent 集成回答为空".to_string(),
        ));
    }
    println!("  🤖 Agent: {answer}");

    println!();
    Ok(())
}

// ── Part 6: 持久化验证 ──────────────────────────────────────────────────────

async fn demo_persistence(db_path: &Path) -> echo_agent::error::Result<()> {
    println!("  验证 SQLite 数据跨实例持久化\n");

    let ns = &["demo27", "persist"];

    // 实例 1：写入数据
    {
        let store = SqliteStore::new(db_path)?;
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
        let store = SqliteStore::new(db_path)?;
        let item = store.get(ns, "persist-key").await?;
        let Some(item) = item else {
            return Err(echo_agent::error::ReactError::Other(
                "demo27 验收失败：跨实例持久化数据丢失".to_string(),
            ));
        };
        println!(
            "  ✅ 实例 2: 读取成功 → {}",
            item.value["content"].as_str().unwrap_or("")
        );

        // FTS5 索引也能跨实例
        let results = store.search(ns, "记忆 保留", 5).await?;
        if results.is_empty() {
            return Err(echo_agent::error::ReactError::Other(
                "demo27 验收失败：跨实例 FTS5 检索没有命中".to_string(),
            ));
        }
        println!(
            "  ✅ 实例 2: FTS5 搜索 \"记忆 保留\" → {} 条命中",
            results.len()
        );

        // 清理
        store.delete(ns, "persist-key").await?;
    }

    // 打印数据库文件大小
    if let Ok(meta) = std::fs::metadata(db_path) {
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

fn load_embedder_from_config() -> echo_agent::error::Result<Arc<dyn echo_agent::memory::Embedder>> {
    Ok(Arc::new(HttpEmbedder::from_env()))
}

async fn load_verified_embedder_from_config()
-> echo_agent::error::Result<Arc<dyn echo_agent::memory::Embedder>> {
    let embedder = load_embedder_from_config()?;
    embedder
        .embed("demo27 sqlite memory health check")
        .await
        .map_err(|e| {
            echo_agent::error::ReactError::Other(format!(
                "demo27 验收失败：embedding 健康检查失败: {e}"
            ))
        })?;
    Ok(embedder)
}

fn demo27_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("echo-agent-demo27-{}.db", std::process::id()))
}

fn cleanup_sqlite_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

fn require_configured_model(preferred: Option<&str>) -> echo_agent::error::Result<LlmConfig> {
    support::llm_config(preferred).map_err(|error| {
        echo_agent::error::ReactError::Other(format!(
            "demo27 requires explicit provider/model environment settings: {error}"
        ))
    })
}
