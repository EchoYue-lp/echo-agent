//! demo59: Code Search — 基于 ripgrep 的跨项目代码搜索
//!
//! 演示 `CodeSearchTool` 的多种搜索模式：
//! 1. 基础正则搜索
//! 2. Glob 过滤（--glob '*.rs'）
//! 3. 文件类型过滤（--type rust）
//! 4. 上下文行（-C N）
//! 5. 大小写不敏感 + 固定字符串
//!
//! 注意：最佳体验需要安装 ripgrep (`rg`)。若未安装，工具会自动回退到
//! 内置的符号搜索（支持 Rust / Python / JS / Go / Java / C++ 符号）。
//!
//! ```bash
//! cargo run --example demo59_code_search --features files
//! ```

use echo_agent::tools::Tool;
use echo_agent::tools::files::code_search::CodeSearchTool;
use serde_json::json;
use std::collections::HashMap;

/// Helper: build a `ToolParameters` map from JSON, then execute the tool.
async fn run_search(tool: &CodeSearchTool, params: serde_json::Value) -> String {
    let map: HashMap<String, serde_json::Value> = match params {
        serde_json::Value::Object(m) => m.into_iter().collect(),
        _ => HashMap::new(),
    };
    match tool.execute(map).await {
        Ok(result) => result.output.clone(),
        Err(e) => format!("Error: {e}"),
    }
}

/// Print a truncated preview of search output.
fn print_preview(output: &str, max_lines: usize) {
    let lines: Vec<&str> = output.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if i >= max_lines {
            println!("    … ({} more lines)", lines.len() - max_lines);
            break;
        }
        println!("    {line}");
    }
    println!();
}

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    demo59: Code Search — ripgrep 代码搜索");
    println!("═══════════════════════════════════════════════════════\n");

    // Search within the echo-agent repo itself.
    let search_root = env!("CARGO_MANIFEST_DIR");
    let tool = CodeSearchTool::new();

    println!("  搜索根目录: {search_root}\n");

    // ── Part 1：基础正则搜索 ──────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1：基础正则搜索 — 搜索 'pub struct' 定义");
    println!("───────────────────────────────────────────────────────\n");

    let output = run_search(
        &tool,
        json!({
            "query": "pub struct \\w+Tool",
            "path": search_root,
            "glob": "*.rs",
            "max_results": 15
        }),
    )
    .await;
    println!("  搜索模式: pub struct \\w+Tool");
    print_preview(&output, 20);

    // ── Part 2：Glob 过滤 ─────────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2：Glob 过滤 — 仅搜索 Cargo.toml 中的依赖");
    println!("───────────────────────────────────────────────────────\n");

    let output = run_search(
        &tool,
        json!({
            "query": "polars",
            "path": search_root,
            "glob": "Cargo.toml",
            "fixed_strings": true,
            "max_results": 10
        }),
    )
    .await;
    println!("  搜索模式: polars (固定字符串)");
    println!("  Glob: Cargo.toml");
    print_preview(&output, 15);

    // ── Part 3：文件类型过滤 ───────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3：文件类型过滤 — --type rust 搜索 trait 定义");
    println!("───────────────────────────────────────────────────────\n");

    let output = run_search(
        &tool,
        json!({
            "query": "pub trait \\w+",
            "path": search_root,
            "file_type": "rust",
            "max_results": 10
        }),
    )
    .await;
    println!("  搜索模式: pub trait \\w+");
    println!("  类型过滤: --type rust");
    print_preview(&output, 15);

    // ── Part 4：上下文行 ──────────────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 4：上下文行（-C 2）— 搜索 fn main 并显示前后 2 行");
    println!("───────────────────────────────────────────────────────\n");

    let output = run_search(
        &tool,
        json!({
            "query": "async fn main",
            "path": format!("{search_root}/examples"),
            "glob": "*.rs",
            "context": 2,
            "max_results": 5
        }),
    )
    .await;
    println!("  搜索模式: async fn main");
    println!("  上下文行: -C 2");
    print_preview(&output, 25);

    // ── Part 5：大小写不敏感 + 全词匹配 ────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 5：大小写不敏感 + 全词匹配");
    println!("───────────────────────────────────────────────────────\n");

    let output = run_search(
        &tool,
        json!({
            "query": "reactagent",
            "path": search_root,
            "glob": "*.rs",
            "case_insensitive": true,
            "word_regexp": true,
            "max_results": 10
        }),
    )
    .await;
    println!("  搜索模式: reactagent (-i -w)");
    print_preview(&output, 15);

    // ── Part 6：符号搜索（rg 不可用时的回退） ──────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 6：符号搜索说明");
    println!("───────────────────────────────────────────────────────\n");
    println!("  当 ripgrep (`rg`) 未安装时，CodeSearchTool 自动回退到内置符号搜索：");
    println!("    - 支持语言: Rust, Python, JavaScript/TypeScript, Go, Java, C/C++");
    println!("    - 符号类型: function, struct, enum, trait, class, interface, type");
    println!("    - 使用 symbol_type 参数指定搜索目标类型");
    println!();
    println!("  示例参数:");
    println!(r#"    {{"query": "ReactAgent", "symbol_type": "struct", "glob": "*.rs"}}"#);

    // ── Summary ─────────────────────────────────────────────────────────
    println!("\n───────────────────────────────────────────────────────");
    println!("参数速查表");
    println!("───────────────────────────────────────────────────────\n");
    println!("  query           : 搜索模式（regex 或固定字符串）");
    println!("  path            : 搜索目录（默认当前目录）");
    println!("  glob            : 文件 glob 过滤（如 '*.rs'）");
    println!("  file_type       : 文件类型过滤（如 'rust', 'python'）");
    println!("  case_insensitive: 大小写不敏感（-i）");
    println!("  fixed_strings   : 固定字符串搜索（-F）");
    println!("  word_regexp     : 全词匹配（-w）");
    println!("  context         : 上下文行数（-C N）");
    println!("  max_count       : 每文件最大匹配数（-m N）");
    println!("  max_results     : 总结果上限（默认 50）");

    println!("\n═══════════════════════════════════════════════════════");
    println!("    demo59 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
