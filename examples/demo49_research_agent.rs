//! 综合示例：智能研究报告助手
//!
//! 展示 echo-agent 在研究和报告场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | Web 搜索 | `WebSearchTool::auto()` 自动选择搜索引擎 |
//! | 网页抓取 | `WebFetchTool` 获取页面详细内容 |
//! | 文件处理 | `FileSystemSkill` + 数据处理工具 |
//! | Workflow | `GraphBuilder` 构建研究流程 |
//! | 结构化输出 | `extract<T>()` 提取结构化数据 |
//! | 长期记忆 | `SqliteStore` 持久化研究历史 |
//! | Token 预算 | `max_tool_output_tokens` 控制内容长度 |
//! | 流式输出 | `execute_stream()` 实时显示进度 |
//! | Agent 编排 | 主编排 Agent + 研究子 Agent |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行（需要 LLM API Key 和 web feature + sqlite）
//! QWEN_API_KEY=your_key cargo run --example demo49_research_agent --features "web sqlite"
//!
//!# 带详细日志
//! RUST_LOG=info QWEN_API_KEY=your_key cargo run --example demo49_research_agent --features "web sqlite"
//! ```

use echo_agent::memory::SqliteStore;
use echo_agent::prelude::*;
use echo_agent::skills::builtin::FileSystemSkill;
use echo_agent::tools::web::{WebFetchTool, WebSearchTool};
use echo_agent::workflow::{GraphBuilder, SharedState};
use futures::StreamExt;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 结构化输出类型
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResearchReport {
    topic: String,
    summary: String,
    key_findings: Vec<String>,
    sources: Vec<String>,
    confidence: f64,
    recommendations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TechComparison {
    languages: Vec<LanguageInfo>,
    winner: String,
    reason: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LanguageInfo {
    name: String,
    pros: Vec<String>,
    cons: Vec<String>,
    use_cases: Vec<String>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,research_agent=info".into()),
        )
        .init();

    print_banner();

    println!("🔬 正在初始化研究报告助手...\n");

    let db_path = research_db_path();
    cleanup_sqlite_files(&db_path);

    // ── Part 0: 研究历史存储（SQLite 长期记忆）──────────────────────────────────────
    demo_research_memory(&db_path).await?;

    // ── Part 1: 单 Agent 简单研究任务 ───────────────────────────────────────────────
    demo_simple_research().await?;

    // ── Part 2: Workflow 编排研究流程 ────────────────────────────────────────────
    demo_workflow_research().await?;

    // ── Part 3: 结构化输出提取 ───────────────────────────────────────────────────
    demo_structured_extraction().await?;

    // ── Part 4: 多 Agent 协作研究 ─────────────────────────────────────────────────
    demo_multi_agent_research().await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    cleanup_sqlite_files(&db_path);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 0: 研究历史存储（SQLite 长期记忆）
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_research_memory(db_path: &Path) -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 0: 研究历史存储（SQLite 长期记忆）");
    println!("═══════════════════════════════════════════════════════\n");

    let store = Arc::new(SqliteStore::new(db_path)?);
    let ns = &["research", "history"];

    println!("  📁 数据库路径: {}\n", db_path.display());

    // 存储一些历史研究记录
    store
        .put(
            ns,
            "rust_2024",
            json!({
                "topic": "Rust 语言 2024 年发展",
                "summary": "Rust 在 2024 年迎来了异步生态的成熟，主流框架全面支持 async/await。",
                "key_findings": ["async/await 成为主流", "WebAssembly 支持增强", "嵌入式领域快速增长"],
                "researched_at": "2024-06-15"
            }),
        )
        .await?;

    store
        .put(
            ns,
            "go_vs_rust",
            json!({
                "topic": "Go vs Rust 性能对比",
                "summary": "Rust 在计算密集型任务中表现更优，Go 在网络服务并发场景下开发效率更高。",
                "key_findings": ["Rust 内存安全零开销", "Go goroutine 调度轻量", "各有所长，选型看场景"],
                "researched_at": "2024-07-20"
            }),
        )
        .await?;

    println!("  ✓ 已存储 2 条研究历史记录\n");

    // 演示全文检索
    println!("  🔍 全文检索测试:");

    let results: Vec<_> = store.search(ns, "Rust async", 5).await?;
    if results.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：研究历史全文检索没有命中".to_string(),
        ));
    }
    for (i, item) in results.iter().enumerate() {
        println!(
            "    [{}] {} - {}",
            i + 1,
            item.key,
            item.value["summary"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(50)
                .collect::<String>()
        );
    }
    println!();

    // 演示获取单条记录
    let record = store.get(ns, "rust_2024").await?;
    let Some(item) = record else {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：无法读取 rust_2024 研究记录".to_string(),
        ));
    };
    println!("  📄 获取单条记录 (rust_2024):");
    println!("    主题: {}", item.value["topic"]);
    println!("    研究日期: {}", item.value["researched_at"]);
    println!();
    if item.value["topic"] != "Rust 语言 2024 年发展" {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：研究记录内容不符合预期".to_string(),
        ));
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 1: 单 Agent 简单研究任务
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_simple_research() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 1: 单 Agent 简单研究任务");
    println!("═══════════════════════════════════════════════════════\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("research-agent")
        .system_prompt(
            "你是一个研究助手，擅长搜索和分析信息。

工作流程：
1. 使用 web_search 搜索相关信息
2. 使用 web_fetch 获取详细页面内容
3. 分析并总结发现
4. 使用 final_answer 给出最终报告",
        )
        .enable_tools()
        .max_tool_output_tokens(2000) // 控制网页内容长度
        .max_iterations(25)
        .build()?;

    // 添加 Web 工具
    agent.add_tool(Box::new(WebSearchTool::auto()));
    agent.add_tool(Box::new(WebFetchTool::new()));

    let task = "研究 Rust 语言在 2024 年的发展趋势和主要新特性";
    println!("📝 研究任务: {}\n", task);

    println!("执行中...\n");

    let mut stream = agent.execute_stream(task).await?;
    let mut used_search = false;
    let mut used_fetch = false;
    let mut final_answer = String::new();

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::ThinkStart => print!("🤔 "),
            AgentEvent::ThinkEnd { .. } => println!(),
            AgentEvent::ToolCall { name, .. } => {
                if name == "web_search" {
                    used_search = true;
                }
                if name == "web_fetch" {
                    used_fetch = true;
                }
                println!("🔧 使用工具: {}", name);
            }
            AgentEvent::ToolResult { output, .. } => {
                let preview: String = output.chars().take(150).collect();
                println!("   ✓ 结果: {}...", preview);
            }
            AgentEvent::Token(token) => {
                final_answer.push_str(&token);
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::FinalAnswer(_) => println!(),
            _ => {}
        }
    }

    if !used_search || !used_fetch {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：单 Agent 研究未完整使用 web_search/web_fetch（search={used_search}, fetch={used_fetch}）"
        )));
    }
    if final_answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：单 Agent 研究返回空答案".to_string(),
        ));
    }

    println!();
    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 2: Workflow 编排研究流程
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_workflow_research() -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("Part 2: Workflow 编排研究流程");
    println!("═══════════════════════════════════════════════════════\n");

    let topic = "Go vs Rust 在后端开发中的对比";

    println!("📝 研究主题: {}\n", topic);

    // 创建研究工作流
    let graph = GraphBuilder::new("research_workflow")
        // 步骤1: 搜索 Go 语言信息
        .add_function_node("search_go", |state: &SharedState| {
            Box::pin(async move {
                let t: String = state.get("topic").unwrap_or_default();
                let _ = state.set("query", format!("{} {} 性能对比", t, "Go语言"));
                println!("  🔍 搜索 Go 语言相关信息...");
                Ok(())
            })
        })
        // 步骤2: 搜索 Rust 语言信息
        .add_function_node("search_rust", |state: &SharedState| {
            Box::pin(async move {
                let t: String = state.get("topic").unwrap_or_default();
                let _ = state.set("query", format!("{} {} 性能对比", t, "Rust语言"));
                println!("  🔍 搜索 Rust 语言相关信息...");
                Ok(())
            })
        })
        // 步骤3: 汇总分析
        .add_function_node("analyze", |state: &SharedState| {
            Box::pin(async move {
                let t: String = state.get("topic").unwrap_or_default();
                let _ = state.set("analysis", format!("{} 的综合分析报告", t));
                println!("  📊 正在汇总分析...");
                Ok(())
            })
        })
        .set_entry("search_go")
        .add_parallel_edge("search_go", vec!["search_rust".into()], "analyze")
        .set_finish("analyze")
        .build()?;

    // 执行工作流
    let state = SharedState::new();
    let _ = state.set("topic", topic.to_string());

    let result = graph.run(state).await?;
    if result.steps != 3 || result.path.len() != 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：研究工作流执行结果不符合预期（steps={}, path={:?}）",
            result.steps, result.path
        )));
    }
    let analysis = result.state.get::<String>("analysis").ok_or_else(|| {
        echo_agent::error::ReactError::Other("综合验收失败：研究工作流未产出 analysis".to_string())
    })?;

    println!("  ✓ 工作流完成");
    println!("    路径: {:?}", result.path);
    println!("    步骤数: {}", result.steps);
    println!("    分析结果: {}\n", analysis);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 3: 结构化输出提取
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_structured_extraction() -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("Part 3: 结构化输出提取");
    println!("═══════════════════════════════════════════════════════\n");

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("extraction-agent")
        .system_prompt("你是一个信息提取助手，请严格按照指定的 JSON Schema 返回结果。")
        .max_iterations(5)
        .build()?;

    println!("📝 任务: 技术选型对比分析\n");

    let schema = typed_response_format::<TechComparison>("tech_comparison");

    let prompt = "对比 Go、Rust、Python 三种语言在后端开发中的优劣，并给出推荐。";

    println!("  问题: {}\n", prompt);

    let result: TechComparison = agent.extract(prompt, schema).await?;
    if result.languages.is_empty()
        || result.winner.trim().is_empty()
        || result.reason.trim().is_empty()
    {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：结构化技术对比结果不完整".to_string(),
        ));
    }
    println!("  ✓ 结构化提取成功:\n");
    println!("  推荐语言: {}", result.winner);
    println!("  推荐理由: {}", result.reason);
    println!("\n  对比语言:");
    for lang in &result.languages {
        println!("    • {} (优势: {})", lang.name, lang.pros.join(", "));
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 4: 多 Agent 协作研究
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_multi_agent_research() -> Result<()> {
    println!("\n═══════════════════════════════════════════════════════");
    println!("Part 4: 多 Agent 协作研究");
    println!("═══════════════════════════════════════════════════════\n");

    // 创建研究子 Agent
    let mut web_researcher = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("web-researcher")
        .system_prompt("你是网络搜索专家，擅长使用 web_search 和 web_fetch 工具收集信息。")
        .enable_tools()
        .max_tool_output_tokens(1500)
        .build()?;

    web_researcher.add_tool(Box::new(WebSearchTool::auto()));
    web_researcher.add_tool(Box::new(WebFetchTool::new()));

    // 创建文件处理子 Agent
    let mut file_processor = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("file-processor")
        .system_prompt("你是文件处理专家，擅长分析和处理各种文件格式。")
        .max_tool_output_tokens(2000)
        .build()?;

    file_processor.add_skill(Box::new(FileSystemSkill::with_base_dir("/tmp")));

    // 创建主编排 Agent
    let mut orchestrator = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("orchestrator")
        .system_prompt(
            "你是研究任务的主编排者，负责：
1. 将复杂研究任务分解为子任务
2. 调度 web-researcher 和 file-processor 子 Agent
3. 汇总所有子 Agent 的研究结果
4. 生成最终的研究报告",
        )
        .role(echo_agent::agent::AgentRole::Orchestrator)
        .enable_subagent()
        .enable_planning()
        .max_iterations(30)
        .build()?;

    orchestrator.register_agent(Box::new(web_researcher));
    orchestrator.register_agent(Box::new(file_processor));

    println!("📝 任务: 协作研究 AI Agent 框架的最佳实践\n");

    let task = "请研究 AI Agent 框架的最佳实践，包括：
1. 使用网络搜索收集最新的 Agent 框架信息
2. 分析不同框架的优缺点
3. 生成一份综合研究报告

请通过调度子 Agent 完成这个研究任务。";

    println!("执行中...\n");

    let result = orchestrator.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：多 Agent 协作研究返回空报告".to_string(),
        ));
    }
    println!("✓ 研究完成!\n");
    println!("═══════════════════════════════════════════════════════");
    println!("最终报告:");
    println!("═══════════════════════════════════════════════════════");
    println!("{}\n", result);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          Echo Agent 智能研究报告助手 - 综合示例          ║");
    println!("║                                                                ║");
    println!("║  展示核心能力：                                                 ║");
    println!("║  • Web 搜索 • 网页抓取 • 文件处理 • 流程编排                  ║");
    println!("║  • 结构化输出 • 重试策略 • Token 预算 • Agent 编排            ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}

fn typed_response_format<T: JsonSchema>(name: &str) -> ResponseFormat {
    let schema = schema_for!(T);
    let schema_value = serde_json::to_value(schema).expect("schema should serialize");
    ResponseFormat::json_schema(name, schema_value)
}

fn research_db_path() -> PathBuf {
    std::env::temp_dir().join(format!("echo_agent_research_{}.db", std::process::id()))
}

fn cleanup_sqlite_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}
