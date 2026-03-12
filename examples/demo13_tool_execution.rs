//! demo13_tool_execution.rs —— ToolExecutionConfig 完整演示

use async_trait::async_trait;
use echo_agent::agent::Agent;
use echo_agent::error::{Result, ToolError};
use echo_agent::prelude::*;
use echo_agent::tools::others::math::AddTool;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{Duration, sleep};

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

#[async_trait]
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
    async fn execute(&self, params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        sleep(Duration::from_secs(self.delay_secs)).await;
        let a = params.get("a").and_then(Value::as_i64).unwrap_or(0);
        let b = params.get("b").and_then(Value::as_i64).unwrap_or(0);
        Ok(ToolResult::success(format!("{}", a + b)))
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

#[async_trait]
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
    async fn execute(&self, params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
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
    }
}

// ── Part 1 ──────────────────────────────────────────────────────────────────────

async fn demo_default() -> Result<()> {
    println!("配置：ToolExecutionConfig::default()\n");

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("agent_default")
        .system_prompt("你是一个计算助手。用数学工具完成计算。")
        .enable_tools()
        .build()?;

    agent.add_tool(Box::new(AddTool));

    match agent.execute("计算 15 + 27").await {
        Ok(ans) => println!("✅ 结果：{}", ans),
        Err(e) => println!("❌ 错误：{}", e),
    }
    Ok(())
}

// ── Part 2 ──────────────────────────────────────────────────────────────────────

async fn demo_timeout() -> Result<()> {
    println!("配置：timeout_ms = 1_500\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("agent_timeout")
        .system_prompt("你是一个计算助手。slow_add 很慢，add 正常。")
        .enable_tools()
        .build()?;

    agent.add_tool(Box::new(SlowTool { delay_secs: 3 }));
    agent.add_tool(Box::new(AddTool));

    match agent.execute("用 slow_add 计算 100 + 200").await {
        Ok(ans) => println!("✅ 结果：{}", ans),
        Err(e) => println!("❌ 错误：{}", e),
    }
    Ok(())
}

// ── Part 3 ──────────────────────────────────────────────────────────────────────

async fn demo_retry() -> Result<()> {
    println!("配置：retry_on_fail = true, max_retries = 2\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("agent_retry")
        .system_prompt("你是一个计算助手。用 flaky_multiply 完成计算。")
        .enable_tools()
        .build()?;

    agent.add_tool(Box::new(FlakyTool::new(2)));

    match agent.execute("用 flaky_multiply 计算 7 × 8").await {
        Ok(ans) => println!("✅ 结果：{}", ans),
        Err(e) => println!("❌ 错误：{}", e),
    }
    Ok(())
}

// ── Part 4 ──────────────────────────────────────────────────────────────────────

async fn demo_concurrency() -> Result<()> {
    println!("配置：max_concurrency = 2\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("agent_concurrency")
        .system_prompt("你是一个任务调度助手。")
        .enable_tools()
        .build()?;

    agent.add_tool(Box::new(AddTool));

    match agent.execute("计算 1 + 2").await {
        Ok(ans) => println!("✅ 结果：{}", ans),
        Err(e) => println!("❌ 错误：{}", e),
    }
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
