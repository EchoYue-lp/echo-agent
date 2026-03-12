//! demo11_callbacks.rs —— 事件回调系统综合演示

use async_trait::async_trait;

use echo_agent::agent::{Agent, AgentCallback, AgentEvent};
use echo_agent::error::ReactError;

use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, MultiplyTool, SubtractTool};
use futures::StreamExt;
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

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

#[async_trait]
impl AgentCallback for LogCallback {
    async fn on_iteration(&self, agent: &str, iteration: usize) {
        println!(
            "  [{}] 🔄 迭代 {} agent={}",
            self.label,
            iteration + 1,
            agent
        );
    }

    async fn on_tool_start(&self, _agent: &str, tool: &str, args: &Value) {
        println!(
            "  [{}] 🔧 工具调用: {} args={}",
            self.label,
            tool,
            compact_args(args)
        );
    }

    async fn on_tool_end(&self, _agent: &str, tool: &str, result: &str) {
        println!(
            "  [{}] ✅ 工具成功: {} result=\"{}\"",
            self.label,
            tool,
            truncate(result, 60)
        );
    }

    async fn on_tool_error(&self, _agent: &str, tool: &str, err: &ReactError) {
        println!("  [{}] ❌ 工具错误: {} err={}", self.label, tool, err);
    }

    async fn on_final_answer(&self, _agent: &str, answer: &str) {
        println!(
            "  [{}] 🏁 最终答案: \"{}\"",
            self.label,
            truncate(answer, 80)
        );
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

#[async_trait]
impl AgentCallback for MetricsCallback {
    async fn on_iteration(&self, _agent: &str, iteration: usize) {
        self.iterations.store(iteration + 1, Ordering::Relaxed);
    }

    async fn on_tool_start(&self, _agent: &str, _tool: &str, _args: &Value) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    async fn on_tool_error(&self, _agent: &str, _tool: &str, _err: &ReactError) {
        self.tool_errors.fetch_add(1, Ordering::Relaxed);
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
    println!("\n  最终答案: {answer}\n");
    metrics.print_report();

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
            AgentEvent::FinalAnswer(ans) => {
                println!("\n  [FinalAnswer] {}", truncate(&ans, 80));
            }
            AgentEvent::Cancelled => {
                println!("\n  [Cancelled] 执行已取消");
            }
        }
    }

    println!("\n  --- Metrics ---");
    metrics.print_report();

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
