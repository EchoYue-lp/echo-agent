//! demo42_browser_mcp.rs —— Playwright MCP 浏览器集成演示
//!
//! 展示 echo-agent 通过 MCP 配置文件连接 Playwright MCP 服务端，
//! 使用浏览器自动化能力（导航、快照、截图、点击等）。
//!
//! # 架构
//!
//! ```text
//! echo-agent (MCP Client)
//!    │ 读取 mcp.json 配置
//!    ▼
//! @playwright/mcp (MCP Server)
//!    │ Playwright
//!    ▼
//! Chromium / Firefox / WebKit
//! ```
//!
//! # 前置条件
//!
//! 1. 安装 Node.js 和 npm
//! 2. 复制 `examples/mcp.json.example` 为 `mcp.json` 并配置
//! 3. 设置 LLM API 密钥（DEEPSEEK_API_KEY / QWEN_API_KEY / OPENAI_API_KEY）
//!
//! # 运行方式
//!
//! ```bash
//! # 1. 复制配置文件
//! cp examples/mcp.json.example mcp.json
//!
//! # 2. 运行示例
//! cargo run --example demo42_browser_mcp --features mcp
//! ```

use echo_agent::mcp::McpConfigFile;
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=warn,demo43=info".into()),
        )
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    Echo Agent × Playwright MCP 集成演示");
    println!("═══════════════════════════════════════════════════════\n");

    // 加载 MCP 配置文件
    let mcp_config_path = std::path::Path::new("mcp.json");
    let mcp_config = if mcp_config_path.exists() {
        McpConfigFile::from_file(mcp_config_path)?
    } else {
        return Err(echo_agent::error::ReactError::Other(
            "demo42 验收失败：未找到 mcp.json 配置文件".to_string(),
        )
        .into());
    };

    // Part 1: MCP 工具发现
    // demo_tool_discovery(&mcp_config).await?;

    // Part 2: Agent 集成浏览器任务
    demo_agent_browser_task(&mcp_config).await?;

    Ok(())
}

// ── Part 2: Agent + Browser ───────────────────────────────────────────────────

async fn demo_agent_browser_task(_config: &McpConfigFile) -> echo_agent::error::Result<()> {
    println!("{}", "─".repeat(55));
    println!("Part 2: Agent 集成浏览器自动化（显示思考过程和工具返回）\n");

    let mut agent = ReactAgentBuilder::new()
        .model("deepseek-chat")
        .name("browser-agent")
        .system_prompt(
            "你是一个浏览器自动化助手。你可以使用 playwright_* 系列工具控制浏览器完成任务。\n\
             工作流程：\n\
             1. 使用 playwright_navigate 导航到目标页面\n\
             2. 使用 playwright_snapshot 获取页面结构\n\
             3. 根据页面结构使用 playwright_click / playwright_type 等工具操作页面\n\
             4. 使用 playwright_screenshot 截图确认结果\n\
             完成任务后使用 playwright_close 关闭浏览器。",
        )
        .enable_tools()
        .max_iterations(50)
        .build()?;

    // 从配置文件加载 MCP 服务端
    let clients = agent.load_mcp_from_file("mcp.json").await?;
    if clients.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo42 验收失败：未连接到任何 Playwright MCP 服务端".to_string(),
        )
        .into());
    }

    println!("✓ Agent 已创建，包含浏览器工具");
    println!("  已连接 MCP 服务端数: {}", clients.len());

    let task = "请用浏览器打开 https://www.baidu.com ，然后搜索 rust ,分析页面返回的内容。";
    println!("\n  任务: {}\n", task);

    // 使用 execute_stream 获取实时事件
    let mut stream = agent.execute_stream(task).await?;
    let mut tool_calls = 0usize;
    let mut final_answer = String::new();
    let mut tool_errors = 0usize;

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Token(_) => {
                // Token 流式输出，这里不实时显示以避免刷屏
            }
            AgentEvent::ThinkStart => {
                print!("\n🤔 思考中...");
            }
            AgentEvent::ThinkEnd {
                prompt_tokens,
                completion_tokens,
            } => {
                println!(
                    " (prompt={}, completion={})",
                    prompt_tokens, completion_tokens
                );
            }
            AgentEvent::ToolCall { name, args } => {
                tool_calls += 1;
                println!("\n🔧 调用工具: {}", name);
                let args_str = serde_json::to_string_pretty(&args).unwrap_or_default();
                // 只显示前 200 字符的参数
                let preview: String = args_str.chars().take(800).collect();
                println!("   参数: {}", preview);
                if args_str.len() > 200 {
                    println!("   ... (参数过长，已截断)");
                }
            }
            AgentEvent::ToolResult { name, output } => {
                let preview: String = output.chars().take(800).collect();
                println!("\n✅ 工具返回: {}", name);
                println!("   结果: {}", preview);
                if output.len() > 400 {
                    println!("   ... (共 {} 字符)", output.len());
                }
            }
            AgentEvent::ToolError { name, error } => {
                tool_errors += 1;
                println!("\n❌ 工具错误: {}", name);
                println!("   错误: {}", error);
            }
            AgentEvent::FinalAnswer(answer) => {
                final_answer = answer.clone();
                println!("\n═══════════════════════════════════════════════════════");
                println!("最终答案:");
                println!("{}", answer);
                println!("═══════════════════════════════════════════════════════\n");
            }
            AgentEvent::Cancelled => {
                println!("\n⚠️  Agent 执行被取消\n");
                break;
            }
            _ => {
                // 其他事件暂时忽略
            }
        }
    }

    if tool_calls == 0 || final_answer.trim().is_empty() || tool_errors > 0 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo42 验收失败：浏览器任务未完成（tool_calls={tool_calls}, tool_errors={tool_errors}, final_answer_empty={})",
            final_answer.trim().is_empty(),
        ))
        .into());
    }

    Ok(())
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────
