//! demo11_callbacks.rs —— 事件回调系统综合演示

use echo_agent::agent::{Agent, AgentCallback, AgentEvent};
use echo_agent::error::ReactError;

use echo_agent::prelude::*;
use echo_agent::tool;
use futures::StreamExt;
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tool(name = "add", description = "两数相加")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} + {} = {}", a, b, a + b)))
}

#[tool(name = "subtract", description = "两数相减")]
async fn subtract(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} - {} = {}", a, b, a - b)))
}

#[tool(name = "multiply", description = "两数相乘")]
async fn multiply(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} * {} = {}", a, b, a * b)))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo11_callbacks=info".into()),
        )
        .init();

    print_banner();

    println!("{}", "─".repeat(64));
    println!("Part 1: 简单日志回调\n");
    demo_log_callback().await?;

    println!("\n{}", "─".repeat(64));
    println!("Part 2: 统计指标回调（Metrics）\n");
    demo_metrics_callback().await?;

    println!("\n{}", "─".repeat(64));
    println!("Part 3: 多回调组合 + 流式执行\n");
    demo_multi_callback_stream().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo11 完成");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── 简单日志回调 ──────────────────────────────────────────────────────────────

struct LogCallback {
    label: String,
}

impl LogCallback {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl AgentCallback for LogCallback {
    fn on_iteration<'a>(
        &'a self,
        agent: &'a str,
        iteration: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            println!(
                "  [{}] 🔄 迭代 {} agent={}",
                self.label,
                iteration + 1,
                agent
            );
        })
    }

    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        args: &'a Value,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            println!(
                "  [{}] 🔧 工具调用: {} args={}",
                self.label,
                tool,
                compact_args(args)
            );
        })
    }

    fn on_tool_end<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        result: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            println!(
                "  [{}] ✅ 工具成功: {} result=\"{}\"",
                self.label,
                tool,
                truncate(result, 60)
            );
        })
    }

    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        err: &'a ReactError,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            println!("  [{}] ❌ 工具错误: {} err={}", self.label, tool, err);
        })
    }

    fn on_final_answer<'a>(
        &'a self,
        _agent: &'a str,
        answer: &'a str,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            println!(
                "  [{}] 🏁 最终答案: \"{}\"",
                self.label,
                truncate(answer, 80)
            );
        })
    }
}

// ── Part 1: 简单日志回调 Demo ──────────────────────────────────────────────────

async fn demo_log_callback() -> echo_agent::error::Result<()> {
    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("log_agent")
        .system_prompt("你是一个计算助手，必须通过工具完成所有计算。")
        .enable_tools()
        .max_iterations(8)
        .callback(Arc::new(LogCallback::new("LOG")))
        .build()?;

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));

    let task = "计算 (3 + 7) × 5";
    println!("  任务: {task}\n");

    let answer = agent.execute(task).await?;
    if answer.trim().is_empty() {
        return Err(ReactError::Other(
            "demo11 验收失败：日志回调示例返回空答案".to_string(),
        ));
    }
    println!("\n  最终答案: {answer}");

    Ok(())
}

// ── Part 2: 统计指标回调 Demo ──────────────────────────────────────────────────

struct MetricsCallback {
    iterations: AtomicUsize,
    tool_calls: AtomicUsize,
    tool_errors: AtomicUsize,
}

impl MetricsCallback {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            iterations: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
            tool_errors: AtomicUsize::new(0),
        })
    }

    fn print_report(&self) {
        println!("  ┌─────────────────────────────────────────┐");
        println!("  │           Metrics Report                │");
        println!("  ├─────────────────────────────────────────┤");
        println!(
            "  │ 总迭代次数     : {:>5}                   │",
            self.iterations.load(Ordering::Relaxed)
        );
        println!(
            "  │ 工具调用次数   : {:>5}                   │",
            self.tool_calls.load(Ordering::Relaxed)
        );
        println!(
            "  │ 工具错误次数   : {:>5}                   │",
            self.tool_errors.load(Ordering::Relaxed)
        );
        println!("  └─────────────────────────────────────────┘");
    }
}

impl AgentCallback for MetricsCallback {
    fn on_iteration<'a>(
        &'a self,
        _agent: &'a str,
        iteration: usize,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.iterations.store(iteration + 1, Ordering::Relaxed);
        Box::pin(async {})
    }

    fn on_tool_start<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _args: &'a Value,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
        Box::pin(async {})
    }

    fn on_tool_error<'a>(
        &'a self,
        _agent: &'a str,
        _tool: &'a str,
        _err: &'a ReactError,
    ) -> futures::future::BoxFuture<'a, ()> {
        self.tool_errors.fetch_add(1, Ordering::Relaxed);
        Box::pin(async {})
    }
}

async fn demo_metrics_callback() -> echo_agent::error::Result<()> {
    let metrics = MetricsCallback::new();

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("metrics_agent")
        .system_prompt("你是一个计算助手，必须通过工具完成所有计算。")
        .enable_tools()
        .max_iterations(10)
        .callback(metrics.clone())
        .build()?;

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    let task = "计算：(10 + 20) × 3 - 15";
    println!("  任务: {task}\n");

    let answer = agent.execute(task).await?;
    if answer.trim().is_empty() {
        return Err(ReactError::Other(
            "demo11 验收失败：指标回调示例返回空答案".to_string(),
        ));
    }
    println!("\n  最终答案: {answer}\n");
    metrics.print_report();
    if metrics.tool_calls.load(Ordering::Relaxed) == 0 {
        return Err(ReactError::Other(
            "demo11 验收失败：MetricsCallback 未记录到任何工具调用".to_string(),
        ));
    }

    Ok(())
}

// ── Part 3: 多回调组合 + 流式执行 Demo ─────────────────────────────────────────

async fn demo_multi_callback_stream() -> echo_agent::error::Result<()> {
    let metrics = MetricsCallback::new();

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("stream_cb_agent")
        .system_prompt("你是一个计算助手，必须通过工具完成所有计算。")
        .enable_tools()
        .max_iterations(10)
        .callback(Arc::new(LogCallback::new("STREAM-LOG")))
        .callback(metrics.clone())
        .build()?;

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    let task = "计算：(5 + 3) × (10 - 4)";
    println!("  任务: {task}\n");

    let mut stream = agent.execute_stream(task).await?;
    let mut final_answer = String::new();

    while let Some(ev) = stream.next().await {
        match ev? {
            AgentEvent::Token(t) => {
                print!("{}", t);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args } => {
                println!("\n  [ToolCall] {name}({})", compact_args(&args));
            }
            AgentEvent::ToolResult { name, output } => {
                println!("  [ToolResult] [{name}] {}", truncate(&output, 60));
            }
            AgentEvent::ToolError { name, error } => {
                return Err(ReactError::Other(format!(
                    "demo11 验收失败：流式回调示例中工具 `{name}` 出错: {error}"
                )));
            }
            AgentEvent::FinalAnswer(ans) => {
                final_answer = ans.clone();
                println!("\n  [FinalAnswer] {}", truncate(&ans, 80));
            }
            AgentEvent::Cancelled => {
                println!("\n  [Cancelled] 执行已取消");
            }
            _ => {}
        }
    }

    println!("\n  --- Metrics ---");
    metrics.print_report();
    if final_answer.trim().is_empty() || metrics.tool_calls.load(Ordering::Relaxed) == 0 {
        return Err(ReactError::Other(
            "demo11 验收失败：流式回调示例未得到最终答案或未记录工具调用".to_string(),
        ));
    }

    Ok(())
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{out}…")
    } else {
        out
    }
}

fn compact_args(args: &Value) -> String {
    match args {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                format!(
                    "{k}={}",
                    match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × 事件回调系统综合演示 (demo11)");
    println!("{}", "═".repeat(64));
    println!();
}
