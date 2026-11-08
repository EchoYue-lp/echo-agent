//! demo42_playwright_mcp.rs —— Playwright MCP 浏览器集成演示
//!
//! 展示 echo-agent 通过 MCP 配置文件连接 Playwright MCP 服务端，
//! 使用浏览器自动化能力（导航、快照、截图、点击等）。
//! 原生状态只作只读投影，真实 EOF 后才展示最终答案；用量来自 LlmUsage。
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
//! cargo run -p echo-agent-learning --example demo42_playwright_mcp --features mcp
//! ```

mod support;

use echo_agent::agent::{AgentOutputProjection, AssistantPhase, ExecutionStatusKind};
use echo_agent::error::{AgentError, AgentTerminalKind, ReactError};
use echo_agent::mcp::McpConfigFile;
use echo_agent::prelude::*;
use futures::StreamExt;

const BROWSER_TOOL_TIMEOUT_MS: u64 = 120_000;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();

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
        ));
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

    let llm_config = require_configured_model(Some("deepseek-v4-flash"))?;
    let mut agent = ReactAgentBuilder::new()
        .llm_config(llm_config)
        .name("browser-agent")
        .system_prompt(
            "你是一个浏览器自动化助手。你可以使用 browser_* 系列工具控制浏览器完成任务。\n\
             工作流程：\n\
             1. 先导航，再读取页面结构，不要盲点\n\
             2. 如果需要输入，优先使用 type/fill/click 等最直接的工具\n\
             3. 页面已经足够分析时就直接总结，避免无意义的重复操作\n\
             4. 输出简洁结论，说明你在页面上看到了什么。",
        )
        .enable_tools()
        .tool_execution(ToolExecutionConfig {
            timeout_ms: BROWSER_TOOL_TIMEOUT_MS,
            retry_on_fail: true,
            max_retries: 1,
            retry_delay_ms: 500,
            ..ToolExecutionConfig::default()
        })
        .max_iterations(50)
        .build()?;

    // 从配置文件加载 MCP 服务端
    let clients = agent.load_mcp_from_file("mcp.json").await?;
    if clients.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo42 验收失败：未连接到任何 Playwright MCP 服务端".to_string(),
        ));
    }

    println!("✓ Agent 已创建，包含浏览器工具");
    println!("  已连接 MCP 服务端数: {}", clients.len());

    let task = "请按下面步骤完成任务：1）打开 https://www.baidu.com；2）搜索 rust；3）读取搜索结果页的主要内容；4）用简洁中文总结页面返回了什么。";
    println!("\n  任务: {}\n", task);
    println!("  工具超时: {} 秒\n", BROWSER_TOOL_TIMEOUT_MS / 1000);

    // 使用 execute_stream 获取实时事件
    let mut stream = agent.execute_stream(task).await?;
    let mut tool_calls = 0usize;
    let mut tool_errors = 0usize;
    let owner_run = agent.current_run_id();
    let mut output = AgentOutputProjection::default();
    let mut first_error = None;
    let mut unresolved = None::<ReactError>;
    let same_source = |error: &ReactError, projection: &AgentOutputProjection| {
        matches!(error, ReactError::ExecutionNotSettled { run_id, execution_id }
            if owner_run.as_deref() == Some(run_id.as_str())
                && execution_id.as_deref().is_some()
                && execution_id.as_deref() == projection.execution_id())
    };

    // 工具错误、取消或最终通知都不提前 drop 原 stream。
    while let Some(event) = stream.next().await {
        let observed = match event {
            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                // 未结算观察不是终态；foreign 观察不会被这条 source 的完成覆盖。
                if unresolved
                    .as_ref()
                    .is_none_or(|prior| same_source(prior, &output))
                {
                    unresolved = Some(error);
                }
                Ok(())
            }
            Ok(AgentEvent::ExecutionStatus(status)) => {
                output.observe_status(&status).map(|changed| {
                    if changed
                        && matches!(
                            status.status,
                            ExecutionStatusKind::Settled
                                | ExecutionStatusKind::Failed
                                | ExecutionStatusKind::Cancelled
                        )
                        && unresolved
                            .as_ref()
                            .is_some_and(|prior| same_source(prior, &output))
                    {
                        unresolved = None;
                    }
                    if let Some(error) = output.terminal_failure() {
                        first_error.get_or_insert(error);
                    }
                })
            }
            Ok(AgentEvent::AssistantMessage { block }) => {
                output.observe_block(&block).map(|delta| {
                    if let Some(delta) = delta {
                        // 浏览器示例保留末尾一次展示答案；推理/进度独立显示，不打印重放密文。
                        match delta.block.phase {
                            AssistantPhase::Reasoning if !delta.text.is_empty() => {
                                println!("\n[推理] {}", delta.text)
                            }
                            AssistantPhase::Commentary if !delta.text.is_empty() => {
                                println!("\n[进度] {}", delta.text)
                            }
                            _ => {}
                        }
                    }
                })
            }
            Ok(AgentEvent::LlmUsage {
                model,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                usage_reported,
                ..
            }) => {
                if usage_reported {
                    println!(
                        "\n[LLM 用量] model={model}, prompt={prompt_tokens}, completion={completion_tokens}, total={total_tokens}"
                    );
                } else {
                    println!("\n[LLM 用量] model={model}，上游未报告用量");
                }
                Ok(())
            }
            Ok(AgentEvent::ToolCall { invocation, .. }) => {
                let name = invocation.name;
                let args = invocation.args;
                tool_calls = tool_calls.saturating_add(1);
                println!("\n🔧 调用工具: {}", name);
                let args_str = serde_json::to_string_pretty(&args).unwrap_or_default();
                // 只显示前 200 字符的参数
                let preview: String = args_str.chars().take(800).collect();
                println!("   参数: {}", preview);
                if args_str.chars().count() > 200 {
                    println!("   ... (参数过长，已截断)");
                }
                Ok(())
            }
            Ok(AgentEvent::ToolResult { name, result, .. }) => {
                if result.success {
                    let preview: String = result.output.chars().take(800).collect();
                    println!("\n✅ 工具返回: {}", name);
                    println!("   结果: {}", preview);
                    let output_chars = result.output.chars().count();
                    if output_chars > 400 {
                        println!("   ... (共 {} 字符)", output_chars);
                    }
                } else {
                    tool_errors = tool_errors.saturating_add(1);
                    println!("\n❌ 工具错误: {}", name);
                    println!("   错误: {}", result.error.unwrap_or(result.output));
                }
                Ok(())
            }
            Ok(AgentEvent::FinalAnswer(answer)) => output.confirm_answer(&answer),
            Ok(AgentEvent::Error { failure, .. }) => Err(match failure.terminal_kind {
                AgentTerminalKind::Cancelled => AgentError::Cancelled(failure.message).into(),
                AgentTerminalKind::TimedOut => AgentError::Timeout(failure.message).into(),
                AgentTerminalKind::PermissionDenied => {
                    AgentError::PermissionDenied(failure.message).into()
                }
                _ => ReactError::Other(failure.message),
            }),
            Ok(AgentEvent::Cancelled) => {
                Err(AgentError::Cancelled("原生 Agent source 已取消".into()).into())
            }
            Err(error) => Err(error),
            _ => Ok(()),
        };
        if let Err(error) = observed {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = unresolved {
        return Err(error);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    let final_answer = output.finish()?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("最终答案:");
    println!("{final_answer}");
    println!("═══════════════════════════════════════════════════════\n");

    if tool_calls == 0 || final_answer.trim().is_empty() || tool_errors > 0 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo42 验收失败：浏览器任务未完成（tool_calls={tool_calls}, tool_errors={tool_errors}, final_answer_empty={})",
            final_answer.trim().is_empty(),
        )));
    }

    Ok(())
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────

fn require_configured_model(preferred: Option<&str>) -> echo_agent::error::Result<LlmConfig> {
    support::llm_config(preferred).map_err(|error| {
            echo_agent::error::ReactError::Other(format!(
                "demo42 requires explicit ECHO_AGENT_PROVIDER/BASE_URL/MODEL/API_PROTOCOL settings: {error}"
            ))
        })
}
