//! demo63_tiered_memory —— TieredMemory 四层记忆架构综合演示
//!
//! 展示 `TieredMemory` 的核心能力：
//!
//! 1. Core Memory — 始终注入系统提示的高优先级事实
//! 2. Short-Term Memory — 带元数据的近期结构化记忆
//! 3. Overflow Queue — 从短期记忆驱逐后的有界缓冲队列
//! 4. Long-Term Store — 可选的持久化存储后端
//!
//! 演示记忆层之间的自动流转、重要性驱逐、关键词召回、
//! token 预算管理以及与 Agent 记忆工具的集成。
//!
//! ```bash
//! cargo run --example demo63_tiered_memory
//! ```

use echo_agent::prelude::*;
use echo_core::memory::core_memory::{CoreMemory, CoreMemoryBlock};
use echo_core::memory::tiered::{MemoryEntry, TieredMemory};
use std::sync::Arc;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("echo_agent=info")
        .init();

    print_banner();

    // ── Part 1: Core Memory — 高优先级固定记忆 ──────────────────────────────
    separator("Part 1: Core Memory — 始终注入系统提示");
    demo_core_memory()?;

    // ── Part 2: Short-Term Memory — 带元数据的近期记忆 ──────────────────────
    separator("Part 2: Short-Term Memory — 结构化近期记忆");
    demo_short_term_memory()?;

    // ── Part 3: 自动溢出 — 重要性驱逐到 Overflow Queue ─────────────────────
    separator("Part 3: 自动溢出 — 重要性驱逐机制");
    demo_overflow_eviction()?;

    // ── Part 4: Overflow → Long-Term Store 持久化 ──────────────────────────
    separator("Part 4: Overflow 刷写到 Long-Term Store");
    demo_flush_to_long_term().await?;

    // ── Part 5: 关键词召回 + 上下文注入 ─────────────────────────────────────
    separator("Part 5: 关键词召回 + 上下文注入");
    demo_recall_and_context_injection()?;

    // ── Part 6: Token 预算管理 ──────────────────────────────────────────────
    separator("Part 6: Token 预算管理 — Core Memory 字符预算");
    demo_token_budget()?;

    // ── Part 7: 与 Agent 记忆工具集成 ───────────────────────────────────────
    separator("Part 7: 与 Agent 记忆工具集成");
    demo_agent_integration().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo63 完成 ✓");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1: Core Memory ─────────────────────────────────────────────────────

fn demo_core_memory() -> echo_agent::error::Result<()> {
    let mut core = CoreMemory::new(2000);

    // 插入高优先级的核心记忆块
    core.upsert(CoreMemoryBlock::new("user_name", "name", "Alice").with_importance(9.0));
    core.upsert(
        CoreMemoryBlock::new("user_role", "role", "Senior Rust Engineer").with_importance(8.0),
    );
    core.upsert(
        CoreMemoryBlock::new("project", "project", "Building an AI Agent framework")
            .with_importance(7.0),
    );
    core.upsert(
        CoreMemoryBlock::new(
            "pref_theme",
            "preference",
            "Dark theme, JetBrains Mono font",
        )
        .with_importance(6.0),
    );

    println!(
        "  已插入 {} 个 Core Memory 块 (总字符: {})",
        core.len(),
        core.total_chars()
    );

    // 生成系统提示注入片段
    let fragment = core.to_system_prompt_fragment().unwrap();
    println!("  系统提示注入片段:");
    for line in fragment.lines() {
        println!("    {line}");
    }

    assert!(fragment.contains("Alice"));
    assert!(fragment.contains("Senior Rust Engineer"));
    assert_eq!(core.len(), 4);

    // 更新已有块（相同 id → 替换）
    core.upsert(
        CoreMemoryBlock::new(
            "project",
            "project",
            "Building EchoCoWork agent framework with LSP",
        )
        .with_importance(7.5),
    );
    assert_eq!(core.len(), 4); // 数量不变，内容更新
    println!("\n  更新 project 块 → 内容已替换，总数不变 ✓");

    println!("  → Core Memory ✓");
    Ok(())
}

// ── Part 2: Short-Term Memory ───────────────────────────────────────────────

fn demo_short_term_memory() -> echo_agent::error::Result<()> {
    let mut tm = TieredMemory::new(5, 2000); // max 5 short-term entries, 2000 core chars

    // 使用结构化 MemoryEntry 添加短期记忆
    tm.add_short_term(MemoryEntry::new(
        "用户讨论了 Rust 异步运行时的选择".to_string(),
        7.0,
        vec!["rust".into(), "async".into()],
        "conversation".into(),
    ));
    tm.add_short_term(MemoryEntry::new(
        "发现了一个解析器 bug，位于 token_stream() 函数".to_string(),
        9.0,
        vec!["rust".into(), "bug".into(), "parser".into()],
        "tool_result".into(),
    ));
    tm.add_short_term(MemoryEntry::new(
        "用户偏好使用 Tokio 作为异步运行时".to_string(),
        6.0,
        vec!["preference".into(), "tokio".into()],
        "conversation".into(),
    ));

    println!("  已添加 {} 条短期记忆", tm.short_term.len());
    assert_eq!(tm.short_term.len(), 3);

    // 按重要性排序
    let sorted = tm.short_term_by_importance();
    println!("  按重要性排序:");
    for (i, entry) in sorted.iter().enumerate() {
        println!(
            "    {}. [imp={:.0}] {} (tags: {:?})",
            i + 1,
            entry.importance,
            entry.content,
            entry.tags
        );
    }
    // 最重要的应该在第一位
    assert!(sorted[0].importance >= sorted[1].importance);

    // 简便方法：不带元数据的简单条目
    tm.add_short_term_simple("用户问了一个关于所有权的问题".to_string());
    assert_eq!(tm.short_term.len(), 4);
    println!("\n  add_short_term_simple() → 默认 importance=5.0, source=conversation ✓");

    println!("  → Short-Term Memory ✓");
    Ok(())
}

// ── Part 3: 自动溢出 ────────────────────────────────────────────────────────

fn demo_overflow_eviction() -> echo_agent::error::Result<()> {
    // max_short_term = 3 → 超出时驱逐最低重要性的条目
    let mut tm = TieredMemory::new(3, 2000);

    tm.add_short_term(MemoryEntry::new(
        "低优先级: 日常闲聊".to_string(),
        2.0,
        vec![],
        "conversation".into(),
    ));
    tm.add_short_term(MemoryEntry::new(
        "中优先级: 代码审查反馈".to_string(),
        6.0,
        vec!["review".into()],
        "conversation".into(),
    ));
    tm.add_short_term(MemoryEntry::new(
        "高优先级: 严重安全漏洞".to_string(),
        9.5,
        vec!["security".into(), "critical".into()],
        "tool_result".into(),
    ));

    assert_eq!(tm.short_term.len(), 3);
    assert_eq!(tm.overflow_queue.len(), 0);

    println!("  短期记忆已满 (3/3), overflow = 0");
    println!("  当前条目:");
    for entry in &tm.short_term {
        println!("    [imp={:.1}] {}", entry.importance, entry.content);
    }

    // 添加第4条 → 驱逐最低重要性的条目 ("日常闲聊", imp=2.0)
    tm.add_short_term(MemoryEntry::new(
        "新条目: 性能优化方案".to_string(),
        7.0,
        vec!["performance".into()],
        "conversation".into(),
    ));

    assert_eq!(tm.short_term.len(), 3);
    assert_eq!(tm.overflow_queue.len(), 1);
    // 被驱逐的应该是最低重要性的
    assert_eq!(tm.overflow_queue[0].content, "低优先级: 日常闲聊");

    println!("\n  添加第4条 (imp=7.0) → 驱逐最低重要性条目:");
    println!("    驱逐: \"{}\" (imp=2.0)", tm.overflow_queue[0].content);
    println!("    保留:");
    for entry in &tm.short_term {
        println!("      [imp={:.1}] {}", entry.importance, entry.content);
    }

    // 验证自动汇总阈值
    let threshold = tm.auto_summarize_threshold(); // max_short_term * 2 = 6
    println!("\n  自动汇总阈值: {threshold} (max_short_term × 2)");
    println!(
        "  当前待处理条目: {} (short_term) + {} (overflow) = {}",
        tm.short_term.len(),
        tm.overflow_queue.len(),
        tm.total_pending_entries()
    );
    println!(
        "  是否需要汇总: {}",
        if tm.needs_summarization() {
            "是"
        } else {
            "否"
        }
    );

    println!("  → 自动溢出 ✓");
    Ok(())
}

// ── Part 4: Overflow → Long-Term Store ──────────────────────────────────────

async fn demo_flush_to_long_term() -> echo_agent::error::Result<()> {
    let store = Arc::new(InMemoryStore::new());

    // 创建带 Long-Term Store 的 TieredMemory，限制 overflow 大小为 3
    let mut tm = TieredMemory::new(2, 2000)
        .with_overflow_bound(3)
        .with_store(store.clone() as Arc<dyn Store>);

    // 添加足够多的条目来触发溢出
    for i in 0..5 {
        tm.add_short_term(MemoryEntry::new(
            format!("记忆条目 #{i}: 重要性等级 {}", i + 1),
            (i + 1) as f64,
            vec![format!("tag_{i}")],
            "conversation".into(),
        ));
    }

    println!("  short_term: {} 条 (max 2)", tm.short_term.len());
    println!("  overflow:   {} 条", tm.overflow_queue.len());

    // 刷写 overflow 到 long-term store
    let flushed = tm.flush_overflow().await;
    println!("  已刷写 {} 条到 Long-Term Store", flushed);
    assert!(flushed > 0);
    assert_eq!(tm.overflow_queue.len(), 0);

    // 验证 long-term store 中有数据
    let results = store
        .search(&["memories", "short_term"], "记忆", 10)
        .await?;
    println!("  Long-Term Store 搜索 '记忆': {} 条结果", results.len());

    // 没有 Store 时的行为
    let mut tm_no_store = TieredMemory::new(1, 2000).with_overflow_bound(3);
    for i in 0..5 {
        tm_no_store.add_short_term(MemoryEntry::new(
            format!("no-store entry #{i}"),
            (i + 1) as f64,
            vec![],
            "conversation".into(),
        ));
    }
    // overflow 有界，超出部分会被驱逐（按最低重要性）
    assert!(tm_no_store.overflow_queue.len() <= 3);
    println!(
        "\n  无 Store 时 overflow 有界: {} 条 (max 3)",
        tm_no_store.overflow_queue.len()
    );
    println!("  → Overflow 刷写 ✓");
    Ok(())
}

// ── Part 5: 关键词召回 + 上下文注入 ─────────────────────────────────────────

fn demo_recall_and_context_injection() -> echo_agent::error::Result<()> {
    let mut tm = TieredMemory::new(5, 2000);

    // 设置 Core Memory
    tm.core
        .upsert(CoreMemoryBlock::new("user", "name", "Alice").with_importance(9.0));
    tm.core.upsert(
        CoreMemoryBlock::new("project", "project", "EchoCoWork Agent Framework")
            .with_importance(8.0),
    );

    // 添加不同主题的短期记忆
    tm.add_short_term(MemoryEntry::new(
        "Rust 编译器报告了生命周期错误".to_string(),
        8.0,
        vec!["rust".into(), "error".into()],
        "conversation".into(),
    ));
    tm.add_short_term(MemoryEntry::new(
        "Python 数据分析任务已完成".to_string(),
        5.0,
        vec!["python".into(), "data".into()],
        "tool_result".into(),
    ));
    tm.add_short_term(MemoryEntry::new(
        "Rust 性能优化：使用 rayon 并行处理".to_string(),
        7.0,
        vec!["rust".into(), "performance".into()],
        "conversation".into(),
    ));

    // 关键词召回
    let rust_results = tm.recall("rust", 10);
    println!("  recall(\"rust\") → {} 条匹配:", rust_results.len());
    for entry in &rust_results {
        println!("    [imp={:.0}] {}", entry.importance, entry.content);
    }
    assert_eq!(rust_results.len(), 2);
    // 按重要性排序：8.0 > 7.0
    assert!(rust_results[0].importance >= rust_results[1].importance);

    let py_results = tm.recall("python", 10);
    println!("\n  recall(\"python\") → {} 条匹配", py_results.len());
    assert_eq!(py_results.len(), 1);

    // 上下文注入：Core Memory + Short-Term 按重要性排序
    let context = tm.build_context_injection().unwrap();
    println!("\n  完整上下文注入:");
    for line in context.lines() {
        println!("    {line}");
    }

    assert!(context.contains("Alice")); // Core Memory
    assert!(context.contains("EchoCoWork")); // Core Memory
    assert!(context.contains("生命周期错误")); // Short-Term (高重要性)
    // 高重要性的 Rust 条目应排在低重要性的 Python 条目之前
    let rust_pos = context.find("生命周期错误").unwrap();
    let py_pos = context.find("Python").unwrap();
    assert!(rust_pos < py_pos, "高重要性条目应排在前面");

    println!("\n  重要性加权排序 ✓");
    println!("  → 关键词召回 + 上下文注入 ✓");
    Ok(())
}

// ── Part 6: Token 预算管理 ──────────────────────────────────────────────────

fn demo_token_budget() -> echo_agent::error::Result<()> {
    // 创建一个预算较小的 Core Memory (30 字符)
    let mut core = CoreMemory::new(30);

    core.upsert(CoreMemoryBlock::new("high", "critical", "FACT_A").with_importance(10.0));
    core.upsert(CoreMemoryBlock::new("mid", "important", "FACT_B").with_importance(7.0));

    println!("  预算: 30 字符");
    println!(
        "  插入 2 块后: {} 块, {} 字符",
        core.len(),
        core.total_chars()
    );
    assert_eq!(core.len(), 2); // 6+6 = 12 < 30, 都能放下

    // 添加一个稍大的块，迫使低重要性的块被驱逐
    core.upsert(
        CoreMemoryBlock::new("huge", "massive", "LARGE_MEMORY_VALUE_XYZ").with_importance(9.0),
    );

    println!(
        "  插入大块后: {} 块, {} 字符",
        core.len(),
        core.total_chars()
    );
    // 总字符: FACT_A(6) + FACT_B(6) + LARGE_MEMORY_VALUE_XYZ(22) = 34 > 30
    // 最低重要性的 "mid" (imp=7.0) 被驱逐
    // 剩余: FACT_A(6) + LARGE_MEMORY_VALUE_XYZ(22) = 28 ≤ 30
    assert!(core.total_chars() <= 30);
    let labels: Vec<&str> = core.blocks().iter().map(|b| b.label.as_str()).collect();
    assert!(
        labels.contains(&"critical"),
        "critical (imp=10) should be retained"
    );
    assert!(
        labels.contains(&"massive"),
        "massive (imp=9) should be retained"
    );
    assert!(
        !labels.contains(&"important"),
        "important (imp=7) should be evicted"
    );
    println!("  驱逐: \"important\" (imp=7.0) 因为超出预算");
    println!("  保留: \"critical\" (imp=10.0) + \"massive\" (imp=9.0)");

    // 截断机制：单个块的 limit
    let mut core2 = CoreMemory::new(500);
    core2.upsert(
        CoreMemoryBlock::new("trunc", "truncated", "Hello World, this is a long text")
            .with_importance(5.0)
            .with_limit(5),
    );
    let val = &core2.blocks()[0].value;
    println!("\n  块截断 (limit=5): \"{val}\"");
    assert!(val.len() <= 10); // 5 chars + "…"

    // 动态调整预算
    let mut core3 = CoreMemory::new(200);
    core3.upsert(CoreMemoryBlock::new("a", "a", "short").with_importance(3.0));
    core3.upsert(CoreMemoryBlock::new("b", "b", "also_short").with_importance(2.0));
    assert_eq!(core3.len(), 2);
    core3.set_max_chars(5); // 紧缩预算 → 驱逐低重要性块
    println!("\n  动态紧缩预算到 5 字符 → 剩余 {} 块", core3.len());
    assert!(core3.len() <= 1);

    println!("  → Token 预算管理 ✓");
    Ok(())
}

// ── Part 7: 与 Agent 记忆工具集成 ───────────────────────────────────────────

async fn demo_agent_integration() -> echo_agent::error::Result<()> {
    // 创建带 InMemoryStore 的 Agent
    let store = Arc::new(InMemoryStore::new());

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("tiered_memory_agent")
        .system_prompt("你是一个具有分层记忆能力的助手")
        .with_memory_tools(store.clone())
        .build()?;

    // 验证记忆工具已注册
    let tools = agent.tool_names();
    let has_remember = tools.contains(&"remember".to_string());
    let has_recall = tools.contains(&"recall".to_string());
    let has_search = tools.contains(&"search_memory".to_string());
    let has_forget = tools.contains(&"forget".to_string());

    println!("  Agent 工具列表:");
    for name in &tools {
        let tag = match name.as_str() {
            "remember" | "recall" | "search_memory" | "forget" => "memory",
            "final_answer" => "builtin",
            _ => "other",
        };
        println!("    [{tag:7}] {name}");
    }
    assert!(has_remember && has_recall && has_search && has_forget);

    // 模拟：先写入 Store，再通过 TieredMemory 读取
    let ns = &["tiered_memory_agent", "memories"];
    store
        .put(
            ns,
            "lt_001",
            serde_json::json!({
                "content": "用户是一名 Rust 后端工程师，擅长 Tokio 异步编程",
                "importance": 9,
                "tags": ["profile", "rust"]
            }),
        )
        .await?;

    let results = store.search(ns, "Rust", 5).await?;
    println!("\n  Store 搜索 'Rust': {} 条结果", results.len());
    for item in &results {
        println!("    → {}", item.value);
    }
    assert!(!results.is_empty());

    // 构建 TieredMemory 并预填充
    let mut tm = TieredMemory::new(5, 2000).with_store(store.clone() as Arc<dyn Store>);

    tm.core.upsert(
        CoreMemoryBlock::new("user_profile", "profile", "Rust 后端工程师").with_importance(9.0),
    );
    tm.add_short_term(MemoryEntry::new(
        "用户刚讨论了 Agent 记忆架构设计".to_string(),
        8.0,
        vec!["architecture".into(), "memory".into()],
        "conversation".into(),
    ));

    let ctx = tm.build_context_injection().unwrap();
    println!("\n  TieredMemory 上下文注入:");
    for line in ctx.lines() {
        println!("    {line}");
    }
    assert!(ctx.contains("Rust"));
    assert!(ctx.contains("记忆架构"));

    println!("\n  → Agent 记忆工具集成 ✓");
    Ok(())
}

// ── 辅助 ─────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × Tiered Memory (demo63)");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
