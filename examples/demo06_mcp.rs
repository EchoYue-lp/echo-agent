//! demo06_mcp.rs —— MCP (Model Context Protocol) 集成演示
//!
//! 演示如何将 MCP 服务端的工具接入 echo-agent 框架，
//! 让 ReAct Agent 通过 MCP 协议使用外部工具完成任务。
//!
//! # 架构说明
//!
//! ```text
//! ReactAgent
//!    │ register_tools()
//!    ▼
//! McpToolAdapter  ──→  McpClient  ──→  [stdio Transport]  ──→  MCP Server Process
//!  (Tool trait)         (Arc)           stdin/stdout              npx @mcp/server-*
//! ```
//!
//! # 前置条件
//!
//! 此示例使用官方文件系统 MCP 服务端，需要 Node.js (v18+) / npx：
//!   https://nodejs.org/
//!
//! 首次运行时 npx 会自动下载 @modelcontextprotocol/server-filesystem。
//!
//! # 运行方式
//!
//! ```bash
//! cargo run --example demo06_mcp
//! ```

use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::mcp::{McpManager, McpServerConfig};
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=info,demo06_mcp=info".into()),
        )
        .init();

    // ── 检查 npx 是否可用 ────────────────────────────────────────────────────
    if !npx_available() {
        eprintln!("\n[错误] 未找到 npx，请先安装 Node.js: https://nodejs.org/");
        eprintln!("安装完成后重新运行: cargo run --example demo06_mcp\n");
        return Ok(());
    }

    println!("═══════════════════════════════════════════════════════");
    println!("          Echo Agent × MCP 集成演示");
    println!("═══════════════════════════════════════════════════════\n");

    // ── Part 1: 仅演示 MCP 工具发现与调用（不依赖 LLM）─────────────────────
    demo_raw_mcp_call().await?;

    // ── Part 2: 将 MCP 工具接入 ReAct Agent（依赖 LLM 配置）────────────────
    println!("\n{}", "─".repeat(55));
    println!("Part 2: MCP 工具 + ReAct Agent 联合演示");
    println!("{}\n", "─".repeat(55));

    demo_agent_with_mcp().await?;

    Ok(())
}

/// Part 1: 直接使用 McpClient API，无需 LLM
async fn demo_raw_mcp_call() -> echo_agent::error::Result<()> {
    println!("Part 1: 直接通过 MCP 协议调用工具\n");

    let mut manager = McpManager::new();

    // 连接文件系统 MCP 服务端
    // 服务端会访问 /tmp 目录（可按需修改路径）
    let config = McpServerConfig::stdio(
        "filesystem",
        "npx",
        vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
    );

    println!("正在启动 MCP 文件系统服务端...");
    let tools = manager.connect(config).await?;

    println!("✓ 已发现 {} 个工具:", tools.len());
    for tool in &tools {
        println!("  • {} — {}", tool.name(), tool.description());
    }

    // 直接通过客户端 API 调用工具
    if let Some(client) = manager.get_client("filesystem") {
        println!("\n调用 list_directory 工具列出 /tmp 目录...");

        let result = client
            .call_tool("list_directory", serde_json::json!({ "path": "/tmp" }))
            .await;

        match result {
            Ok(r) => {
                let text = echo_agent::mcp::McpClient::content_to_text(&r.content);
                println!("工具返回结果:\n{}", text);
            }
            Err(e) => {
                println!("工具调用失败（可能目录不存在）: {}", e);
            }
        }
    }

    manager.close_all().await;
    Ok(())
}

/// Part 2: 将 MCP 工具注册到 ReAct Agent
async fn demo_agent_with_mcp() -> echo_agent::error::Result<()> {
    // 检查是否配置了 LLM 环境变量
    if std::env::var("OPENAI_API_KEY").is_err()
        && std::env::var("DEEPSEEK_API_KEY").is_err()
        && std::env::var("QWEN_API_KEY").is_err()
    {
        println!("跳过 Part 2：未检测到 LLM API 密钥");
        println!("（设置 OPENAI_API_KEY / DEEPSEEK_API_KEY / QWEN_API_KEY 后可启用）");
        return Ok(());
    }

    let mut manager = McpManager::new();

    // 连接文件系统 MCP 服务端
    let mcp_tools = manager
        .connect(McpServerConfig::stdio(
            "filesystem",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
        ))
        .await?;

    println!("MCP 工具已准备好，共 {} 个", mcp_tools.len());

    // 创建 ReAct Agent 并注册 MCP 工具
    let system_prompt = "你是一个文件操作助手，可以使用 MCP 文件系统工具完成文件读写任务。\
                         在执行操作前先分析步骤，再依次执行。";
    let config = AgentConfig::new("qwen3-max", "file-agent", system_prompt)
        .enable_tool(true)
        .enable_task(false)
        .enable_human_in_loop(false)
        .enable_subagent(false);

    let mut agent = ReactAgent::new(config);
    agent.add_tools(mcp_tools);

    // 执行任务
    let task = "请帮我查看 /tmp 目录下有哪些文件，并创建一个名为 mcp_test.txt 的文件，\
                内容为 'Hello from echo-agent MCP!'，最后读取这个文件确认内容正确。";

    println!("\n任务: {}\n", task);

    match agent.execute(task).await {
        Ok(result) => {
            println!("\n✓ 任务完成！\n{}", result);
        }
        Err(e) => {
            println!("\n✗ 执行失败: {}", e);
        }
    }

    // 关闭所有 MCP 连接
    manager.close_all().await;
    Ok(())
}

/// 检查 npx 是否安装
fn npx_available() -> bool {
    std::process::Command::new("npx")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
