//! demo41_web_tools.rs —— Web 搜索 + 页面获取综合演示
//!
//! 展示 echo-agent 的 Web 工具能力：
//! - DuckDuckGo / Brave / Tavily 三个搜索 Provider 独立演示
//! - WebSearchTool::auto() 自动选择策略
//! - WebFetchTool: 获取网页内容并转换为可读文本
//! - Agent 集成：让 LLM 自主决定何时搜索/获取
//!
//! # 运行方式
//!
//! ```bash
//! # DuckDuckGo（免费，无需 API Key）
//! cargo run -p echo-agent-learning --example demo41_web_tools --features web
//!
//! # Brave Search（需 API Key）
//! BRAVE_SEARCH_API_KEY=xxx cargo run -p echo-agent-learning --example demo41_web_tools --features web
//!
//! # Tavily（需 API Key）
//! TAVILY_API_KEY=xxx cargo run -p echo-agent-learning --example demo41_web_tools --features web
//!
//! # 多个 Key 同时配置时，auto() 自动选择最佳
//! TAVILY_API_KEY=xxx BRAVE_SEARCH_API_KEY=xxx cargo run -p echo-agent-learning --example demo41_web_tools --features web
//! ```

use echo_agent::agent::Agent;
use echo_agent::prelude::*;
use echo_agent::tools::web::providers::SearchProvider;
use echo_agent::tools::web::providers::brave::BraveSearchProvider;
use echo_agent::tools::web::providers::duckduckgo::DuckDuckGoProvider;
use echo_agent::tools::web::providers::tavily::TavilyProvider;
use echo_agent::tools::web::{WebFetchTool, WebSearchTool};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo41_web_tools=info".into()),
        )
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("      Echo Agent × Web 搜索 + 页面获取 演示");
    println!("═══════════════════════════════════════════════════════\n");

    // Part 1: Provider 状态总览
    demo_provider_overview();

    // Part 2 ~ 4: 三个搜索引擎独立演示
    demo_duckduckgo_search().await?;
    demo_brave_search().await?;
    demo_tavily_search().await?;

    // Part 5: 自动选择策略
    demo_auto_search().await?;

    // Part 6: WebFetch 页面获取
    demo_web_fetch().await?;

    // Part 7: Agent 集成
    demo_agent_web_task().await?;

    Ok(())
}

// ── Part 1: Provider 状态总览 ──────────────────────────────────────────────────

fn demo_provider_overview() {
    println!("{}", "─".repeat(55));
    println!("Part 1: 搜索引擎 Provider 状态总览\n");

    let providers: Vec<(&str, &str, bool)> = vec![
        ("DuckDuckGo", "免费，无需 API Key，抓取 HTML 页面", true),
        (
            "Brave Search",
            "付费 API，每月 2000 次免费额度",
            BraveSearchProvider::from_env().is_some(),
        ),
        (
            "Tavily",
            "AI 优化搜索，专为 Agent 设计",
            TavilyProvider::from_env().is_some(),
        ),
    ];

    for (name, desc, available) in &providers {
        let status = if *available {
            "✅ 可用"
        } else {
            "❌ 未配置"
        };
        println!("  {name} — {status}");
        println!("    {desc}\n");
    }
}

// ── Part 2: DuckDuckGo 搜索 ───────────────────────────────────────────────────

async fn demo_duckduckgo_search() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 2: DuckDuckGo 搜索（免费，无需 API Key）\n");

    let query = "Rust programming language";
    println!("  Provider: DuckDuckGo");
    println!("  搜索: \"{query}\"\n");

    let provider = DuckDuckGoProvider::new();
    let results = provider.search(query, 3).await?;
    ensure_search_results("DuckDuckGo", &results)?;
    print_results(&results);
    Ok(())
}

// ── Part 3: Brave Search ──────────────────────────────────────────────────────

async fn demo_brave_search() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 3: Brave Search 搜索\n");

    let Some(provider) = BraveSearchProvider::from_env() else {
        return Err(echo_agent::error::ReactError::Other(
            "demo41 验收失败：未配置 BRAVE_SEARCH_API_KEY".to_string(),
        ));
    };

    let query = "Rust programming language";
    println!("  Provider: Brave Search");
    println!("  搜索: \"{query}\"\n");

    let results = provider.search(query, 3).await?;
    ensure_search_results("Brave", &results)?;
    print_results(&results);
    Ok(())
}

// ── Part 4: Tavily 搜索 ──────────────────────────────────────────────────────

async fn demo_tavily_search() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 4: Tavily 搜索（AI 优化）\n");

    let Some(provider) = TavilyProvider::from_env() else {
        return Err(echo_agent::error::ReactError::Other(
            "demo41 验收失败：未配置 TAVILY_API_KEY".to_string(),
        ));
    };

    let query = "Rust programming language";
    println!("  Provider: Tavily");
    println!("  搜索: \"{query}\"\n");

    let results = provider.search(query, 3).await?;
    ensure_search_results("Tavily", &results)?;
    print_results(&results);
    Ok(())
}

// ── Part 5: 自动选择策略 ─────────────────────────────────────────────────────

async fn demo_auto_search() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 5: 自动选择策略（Tavily > Brave > DuckDuckGo）\n");

    let query = "Rust async runtime tokio";
    println!("  搜索: \"{query}\"\n");

    // 直接使用 WebSearchTool::auto()，框架自动选择最佳可用 Provider
    let tool = WebSearchTool::auto();
    println!("  自动选择 Provider: {}\n", tool.provider_name());

    let mut params = std::collections::HashMap::new();
    params.insert("query".to_string(), serde_json::json!(query));
    params.insert("max_results".to_string(), serde_json::json!(3));

    let result = tool.execute(params).await?;
    if !result.success || result.output.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo41 验收失败：auto search 执行失败: {:?}",
            result.error
        )));
    }
    println!("{}", result.output);
    println!();
    Ok(())
}

// ── Part 6: WebFetch 页面获取 ────────────────────────────────────────────────

async fn demo_web_fetch() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 6: WebFetch 页面获取\n");

    let url = "https://www.rust-lang.org/";
    println!("  获取: {url}\n");

    let tool = WebFetchTool::new().with_max_content_length(500);

    let mut params = std::collections::HashMap::new();
    params.insert("url".to_string(), serde_json::json!(url));
    params.insert("max_length".to_string(), serde_json::json!(500));

    let result = tool.execute(params).await?;
    if !result.success || result.output.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo41 验收失败：web fetch 执行失败: {:?}",
            result.error
        )));
    }
    let output_char_count = result.output.chars().count();
    let preview: String = result.output.chars().take(500).collect();
    println!("  {preview}");
    if output_char_count > 500 {
        println!("  ... (共 {output_char_count} 字符)");
    }
    println!();
    Ok(())
}

// ── Part 7: Agent 集成 ───────────────────────────────────────────────────────

async fn demo_agent_web_task() -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 7: Agent 集成 Web 工具\n");

    let mut agent = ReactAgentBuilder::new()
        .model("deepseek-v4-flash")
        .name("web-agent")
        .system_prompt(
            "你是一个信息搜索助手。\
             当用户提出问题时，你可以使用 web_search 工具搜索互联网，\
             也可以使用 web_fetch 工具获取网页详细内容。\
             请基于搜索结果给出准确、有用的回答。",
        )
        .enable_tools()
        .max_iterations(10)
        .build()?;

    // 注册 Web 工具
    agent.add_tool(Box::new(WebSearchTool::auto()));
    agent.add_tool(Box::new(WebFetchTool::new()));

    let task = "搜索 Rust 2024 edition 有什么新特性，并总结关键变化";
    println!("  任务: {task}\n");

    let result = agent.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo41 验收失败：Agent Web 任务返回空结果".to_string(),
        ));
    }
    println!("✓ 结果:\n{result}\n");

    Ok(())
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

fn print_results(results: &[echo_agent::tools::web::providers::SearchResult]) {
    if results.is_empty() {
        println!("  （无结果）\n");
        return;
    }
    for (i, r) in results.iter().enumerate() {
        println!("  {}. {}", i + 1, r.title);
        println!("     {}", r.url);
        if !r.snippet.is_empty() {
            let snippet: String = r.snippet.chars().take(120).collect();
            println!("     {snippet}");
        }
        println!();
    }
}

fn ensure_search_results(
    provider: &str,
    results: &[echo_agent::tools::web::providers::SearchResult],
) -> echo_agent::error::Result<()> {
    if results.is_empty() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo41 验收失败：{provider} 搜索没有返回任何结果"
        )));
    }
    Ok(())
}
