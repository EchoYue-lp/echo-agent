//! demo35_dynamic_tools —— 动态 Tool 注册 / 注销 / 替换完整演示
//!
//! 演示在 ReAct 循环中根据阶段动态调整可用工具集：
//! - `add_tool(tool)` — 动态添加新工具
//! - `remove_tool(name)` — 移除不再需要的工具
//! - `replace_tool(tool)` — 替换为同名的新版本工具
//!
//! # 应用场景
//!
//! ```text
//! 场景：多阶段代码生成任务
//!
//! ┌─────────────────────────────────────────────────────────┐
//! │ Phase 1: 研究阶段                                       │
//! │   可用工具: [search_web, read_document, fetch_github]   │
//! │   → 收集信息，理解需求                                  │
//! ├─────────────────────────────────────────────────────────┤
//! │ Phase 2: 编码阶段                                       │
//! │   移除: [search_web, read_document]                     │
//! │   添加: [write_file, execute_code, test_code]           │
//! │   → 实现代码，编写测试                                  │
//! ├─────────────────────────────────────────────────────────┤
//! │ Phase 3: 部署阶段                                       │
//! │   移除: [write_file, execute_code]                      │
//! │   添加: [deploy_service, health_check, monitor]         │
//! │   → 部署服务，监控健康                                  │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # 运行方式
//!
//! ```bash
//! # Part 1-2: API 演示（无需 LLM）
//! cargo run --example demo35_dynamic_tools
//!
//! # Part 3: 实际 Agent 使用（需要 LLM）
//! QWEN_API_KEY=your_key cargo run --example demo35_dynamic_tools
//! ```

use echo_agent::prelude::*;
use echo_agent::tool;
use serde_json::json;

// ── 模拟工具定义 ─────────────────────────────────────────────────────────────────

#[tool(name = "search_web", description = "搜索网络信息")]
async fn search_web(#[arg(description = "搜索关键词")] query: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!(
        "搜索 '{}': 找到 5 个相关结果",
        query
    )))
}

#[tool(name = "read_document", description = "读取文档内容")]
async fn read_document(#[arg(description = "文档路径")] path: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!(
        "读取 '{}': 文档内容，共 1200 字",
        path
    )))
}

#[tool(name = "write_code", description = "编写代码")]
async fn write_code(#[arg(description = "代码内容")] code: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("代码已写入:\n{}", code)))
}

#[tool(name = "execute_code", description = "执行代码")]
async fn execute_code(#[arg(description = "代码")] code: String) -> Result<ToolResult> {
    // 简单计算示例
    if code.contains("1+1") {
        return Ok(ToolResult::success("2".to_string()));
    }
    if code.contains("2*3") {
        return Ok(ToolResult::success("6".to_string()));
    }
    Ok(ToolResult::success("执行成功".to_string()))
}

#[tool(name = "deploy_service", description = "部署服务")]
async fn deploy_service(#[arg(description = "服务名")] name: String) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("服务 '{}' 已部署", name)))
}

#[tool(name = "health_check", description = "健康检查")]
async fn health_check() -> Result<ToolResult> {
    Ok(ToolResult::success(
        json!({"status": "healthy", "uptime": "2h"}).to_string(),
    ))
}

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=warn,demo35=info".into()),
        )
        .init();

    println!("═══ Dynamic Tool Registration Demo ═══\n");

    // ── Part 1: API 基础演示 ─────────────────────────────────────────────────
    demo_api_basics();

    // ── Part 2: 边界情况处理 ───────────────────────────────────────────────────
    demo_edge_cases();

    // ── Part 3: 实际 Agent 使用场景 ─────────────────────────────────────────────
    if has_llm_config() {
        demo_agent_with_dynamic_tools().await?;
    } else {
        println!("[跳过 Part 3] 未配置 LLM API 密钥\n");
    }

    println!("═══ Demo Complete ═══");
    Ok(())
}

// ── Part 1: API 基础演示 ───────────────────────────────────────────────────────

fn demo_api_basics() {
    println!("─────────────────────────────────────────────");
    println!("Part 1: 动态工具 API 演示");
    println!("─────────────────────────────────────────────\n");

    let config = AgentConfig::minimal("qwen3-max", "phase_agent");
    let mut agent = echo_agent::agents::react::ReactAgent::new(config);

    // 1.1 初始状态
    println!("  [1.1] 初始工具集");
    println!("    工具: {:?}\n", agent.tool_names());

    // 1.2 Phase 1: 研究阶段
    println!("  [1.2] Phase 1: 研究阶段 — 添加搜索工具");
    agent.add_tool(Box::new(SearchWebTool));
    agent.add_tool(Box::new(ReadDocumentTool));
    println!("    添加: search_web, read_document");
    println!("    工具: {:?}\n", agent.tool_names());

    // 1.3 Phase 2: 编码阶段
    println!("  [1.3] Phase 2: 编码阶段 — 切换为执行工具");

    let removed_search = agent.remove_tool("search_web");
    let removed_doc = agent.remove_tool("read_document");

    assert!(removed_search.is_some());
    assert!(removed_doc.is_some());

    agent.add_tool(Box::new(WriteCodeTool));
    agent.add_tool(Box::new(ExecuteCodeTool));

    println!("    移除: search_web, read_document");
    println!("    添加: write_code, execute_code");
    println!("    工具: {:?}\n", agent.tool_names());

    // 1.4 Phase 3: 替换工具
    println!("  [1.4] Phase 3: 替换工具 — 用安全版本替换");

    let old = agent.replace_tool(Box::new(ExecuteCodeTool));
    assert!(old.is_some());
    println!(
        "    replace_tool(execute_code) 返回旧工具: {}",
        old.is_some()
    );
    println!("    工具: {:?}\n", agent.tool_names());

    // 1.5 Phase 4: 部署阶段
    println!("  [1.5] Phase 4: 部署阶段 — 切换为部署工具");

    agent.remove_tool("write_code");
    agent.add_tool(Box::new(DeployServiceTool));
    agent.add_tool(Box::new(HealthCheckTool));

    println!("    移除: write_code");
    println!("    添加: deploy_service, health_check");
    println!("    最终工具: {:?}\n", agent.tool_names());
}

// ── Part 2: 边界情况处理 ───────────────────────────────────────────────────────

fn demo_edge_cases() {
    println!("─────────────────────────────────────────────");
    println!("Part 2: 边界情况处理");
    println!("─────────────────────────────────────────────\n");

    let config = AgentConfig::minimal("qwen3-max", "test_agent");
    let mut agent = echo_agent::agents::react::ReactAgent::new(config);

    // 2.1 移除不存在的工具
    println!("  [2.1] 移除不存在的工具");
    let not_found = agent.remove_tool("nonexistent_tool");
    assert!(not_found.is_none());
    println!("    remove_tool('nonexistent_tool') = None ✓\n");

    // 2.2 replace_tool 新工具不存在时自动注册
    println!("  [2.2] replace_tool 不存在时自动注册");
    let old = agent.replace_tool(Box::new(SearchWebTool));
    assert!(old.is_none());
    assert!(agent.tool_names().contains(&"search_web"));
    println!("    replace_tool('new_tool') 当工具不存在时自动注册 ✓\n");

    // 2.3 多次添加同名工具
    println!("  [2.3] 多次添加同名工具（后者覆盖）");
    agent.add_tool(Box::new(ReadDocumentTool));
    let tool_count_before = agent.tool_names().len();
    agent.add_tool(Box::new(ReadDocumentTool));
    let tool_count_after = agent.tool_names().len();
    assert_eq!(tool_count_before, tool_count_after);
    println!("    重复添加同名工具: 后者覆盖，工具数量不变 ✓\n");
}

// ── Part 3: 实际 Agent 使用场景 ─────────────────────────────────────────────────

async fn demo_agent_with_dynamic_tools() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 3: 实际 Agent 使用场景");
    println!("─────────────────────────────────────────────\n");

    // 3.1 多阶段任务：研究 → 编码 → 验证
    println!("  [3.1] 场景：多阶段代码生成任务\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("multi_phase_agent")
        .system_prompt("你是一个智能助手，会根据任务阶段使用不同的工具。")
        .enable_tools()
        .build()?;

    // Phase 1: 添加研究工具
    agent.add_tool(Box::new(SearchWebTool));
    agent.add_tool(Box::new(ReadDocumentTool));

    println!("  Phase 1: 研究阶段");
    println!("  可用工具: {:?}\n", agent.tool_names());

    let task1 = "搜索 Rust 的所有权机制相关信息";

    println!("  任务: {}\n", task1);

    match agent.execute(task1).await {
        Ok(result) => {
            println!("  结果: {}\n", result);
        }
        Err(e) => {
            println!("  错误: {}\n", e);
        }
    }

    // Phase 2: 切换到编码工具
    println!("  ─────────────────────────────────────────────────");
    println!("  Phase 2: 编码阶段（切换工具）\n");

    agent.remove_tool("search_web");
    agent.remove_tool("read_document");
    agent.add_tool(Box::new(WriteCodeTool));
    agent.add_tool(Box::new(ExecuteCodeTool));

    println!("  可用工具: {:?}\n", agent.tool_names());

    let task2 = "计算 1+1 和 2*3 的结果";

    println!("  任务: {}\n", task2);

    match agent.execute(task2).await {
        Ok(result) => {
            println!("  结果: {}\n", result);
        }
        Err(e) => {
            println!("  错误: {}\n", e);
        }
    }

    // 3.2 工具替换场景
    println!("  ─────────────────────────────────────────────────");
    println!("  [3.2] 工具替换场景：安全升级\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("upgrade_agent")
        .system_prompt("你是一个助手，会根据情况使用不同版本的代码执行工具。")
        .enable_tools()
        .build()?;

    // 添加基础版本的执行工具
    agent.add_tool(Box::new(ExecuteCodeTool));
    println!("  初始版本: execute_code (基础版)");
    println!("  可用工具: {:?}\n", agent.tool_names());

    // 执行一个计算
    match agent.execute("计算 1+1").await {
        Ok(result) => {
            println!("  结果: {}\n", result);
        }
        Err(e) => {
            println!("  错误: {}\n", e);
        }
    }

    // 替换为增强版本（带日志、限制等）
    println!("  ─────────────────────────────────────────────────");
    println!("  升级为增强版 execute_code...\n");

    let old = agent.replace_tool(Box::new(ExecuteCodeTool));
    println!("  替换完成，旧工具已移除: {}", old.is_some());

    match agent.execute("计算 2*3").await {
        Ok(result) => {
            println!("  结果: {}\n", result);
        }
        Err(e) => {
            println!("  错误: {}\n", e);
        }
    }

    Ok(())
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────────

fn has_llm_config() -> bool {
    std::env::var("QWEN_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
}
