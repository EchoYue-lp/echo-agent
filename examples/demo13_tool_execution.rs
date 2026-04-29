//! demo13_tool_execution.rs —— ToolExecutionConfig 完整演示

use echo_agent::agent::Agent;
use echo_agent::error::{Result, ToolError};
use echo_agent::prelude::*;
use echo_agent::tool;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, sleep};

#[tool(name = "add", description = "两数相加")]
async fn add(
    /// 第一个数
    a: i64,
    /// 第二个数
    b: i64,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo13_tool_execution=info".into()),
        )
        .init();

    print_banner();

    sep("Part 1: 默认配置 —— 正常执行");
    demo_default().await?;

    sep("Part 2: timeout_ms = 1_500 —— 工具超时");
    demo_timeout().await?;

    sep("Part 3: retry_on_fail = true —— 自动重试");
    demo_retry().await?;

    sep("Part 4: max_concurrency = 2 —— 并发限流");
    demo_concurrency().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo13 完成");
    println!("{}", "═".repeat(64));
    Ok(())
}

// ── 自定义测试工具 ──────────────────────────────────────────────────────────────

struct SlowTool {
    delay_secs: u64,
}

impl Tool for SlowTool {
    fn name(&self) -> &str {
        "slow_add"
    }
    fn description(&self) -> &str {
        "一个故意很慢的加法工具"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } }, "required": ["a", "b"] })
    }
    fn execute(
        &self,
        params: ToolParameters,
    ) -> futures::future::BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        Box::pin(async move {
            sleep(Duration::from_secs(self.delay_secs)).await;
            let a = params.get("a").and_then(Value::as_i64).unwrap_or(0);
            let b = params.get("b").and_then(Value::as_i64).unwrap_or(0);
            Ok(ToolResult::success(format!("{}", a + b)))
        })
    }
}

struct FlakyTool {
    call_count: Arc<AtomicUsize>,
    fail_times: usize,
}

impl FlakyTool {
    fn new(fail_times: usize) -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
            fail_times,
        }
    }
}

impl Tool for FlakyTool {
    fn name(&self) -> &str {
        "flaky_multiply"
    }
    fn description(&self) -> &str {
        "一个偶尔出错的乘法工具"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": { "a": { "type": "integer" }, "b": { "type": "integer" } }, "required": ["a", "b"] })
    }
    fn execute(
        &self,
        params: ToolParameters,
    ) -> futures::future::BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        Box::pin(async move {
            let attempt = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.fail_times {
                return Err(ToolError::ExecutionFailed {
                    tool: "flaky_multiply".into(),
                    message: format!("模拟第 {attempt} 次故障"),
                }
                .into());
            }
            let a = params.get("a").and_then(Value::as_i64).unwrap_or(0);
            let b = params.get("b").and_then(Value::as_i64).unwrap_or(0);
            Ok(ToolResult::success(format!("{}", a * b)))
        })
    }
}

// ── Part 1 ──────────────────────────────────────────────────────────────────────

async fn demo_default() -> Result<()> {
    println!("配置：ToolExecutionConfig::default()\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_default",
        "你是一个计算助手。用数学工具完成计算。",
    )
    .tool_execution(ToolExecutionConfig::default());
    assert_eq!(config.get_tool_execution().timeout_ms, 30_000);
    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(AddTool));

    let ans = agent.execute("计算 15 + 27").await?;
    if !ans.contains("42") {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo13 验收失败：默认工具执行未得到 42，实际输出: {ans}"
        )));
    }
    println!("✅ 结果：{}", ans);
    Ok(())
}

// ── Part 2 ──────────────────────────────────────────────────────────────────────

async fn demo_timeout() -> Result<()> {
    println!("配置：timeout_ms = 1_500\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_timeout",
        "你是一个计算助手。必须使用 slow_add 完成计算。",
    )
    .tool_execution(ToolExecutionConfig {
        timeout_ms: 1_500,
        ..ToolExecutionConfig::default()
    })
    .tool_error_feedback(false);
    assert_eq!(config.get_tool_execution().timeout_ms, 1_500);
    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(SlowTool { delay_secs: 3 }));

    match agent.execute("用 slow_add 计算 100 + 200").await {
        Ok(ans) => {
            return Err(echo_agent::error::ReactError::Other(format!(
                "demo13 验收失败：超时配置未生效，实际成功返回: {ans}"
            )));
        }
        Err(e) => println!("✅ 超时符合预期：{}", e),
    }
    Ok(())
}

// ── Part 3 ──────────────────────────────────────────────────────────────────────

async fn demo_retry() -> Result<()> {
    println!("配置：retry_on_fail = true, max_retries = 2\n");

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_retry",
        "你是一个计算助手。用 flaky_multiply 完成计算。",
    )
    .tool_execution(ToolExecutionConfig {
        retry_on_fail: true,
        max_retries: 2,
        retry_delay_ms: 50,
        ..ToolExecutionConfig::default()
    });
    assert!(config.get_tool_execution().retry_on_fail);
    let mut agent = ReactAgent::new(config);

    let tool = FlakyTool::new(2);
    let call_count = tool.call_count.clone();
    agent.add_tool(Box::new(tool));

    let ans = agent.execute("用 flaky_multiply 计算 7 × 8").await?;
    if !ans.contains("56") {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo13 验收失败：重试后未得到正确结果 56，实际输出: {ans}"
        )));
    }
    if call_count.load(Ordering::SeqCst) < 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo13 验收失败：工具重试次数不足，实际调用 {} 次",
            call_count.load(Ordering::SeqCst)
        )));
    }
    println!("✅ 结果：{}", ans);
    Ok(())
}

// ── Part 4 ──────────────────────────────────────────────────────────────────────

async fn demo_concurrency() -> Result<()> {
    println!("配置：max_concurrency = 2\n");

    let config = AgentConfig::new("qwen3-max", "agent_concurrency", "你是一个任务调度助手。")
        .tool_execution(ToolExecutionConfig {
            max_concurrency: Some(2),
            ..ToolExecutionConfig::default()
        });
    assert_eq!(config.get_tool_execution().max_concurrency, Some(2));
    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(AddTool));

    let ans = agent.execute("计算 1 + 2").await?;
    if !ans.contains('3') {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo13 验收失败：并发配置场景下基础工具执行失败: {ans}"
        )));
    }
    println!("✅ 结果：{}", ans);
    Ok(())
}

// ── 辅助 ────────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("  demo13 — ToolExecutionConfig 演示");
    println!("{}", "═".repeat(64));
    println!();
}

fn sep(title: &str) {
    println!("\n{}", "─".repeat(64));
    println!("  {}", title);
    println!("{}", "─".repeat(64));
    println!();
}
