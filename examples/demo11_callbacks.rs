//! demo11_callbacks.rs —— 事件回调系统综合演示
//!
//! 展示如何通过 `AgentCallback` 监听 Agent 执行全生命周期事件：
//!
//! ```text
//! Part 1: 简单日志回调
//!         实现 on_think_start/end、on_tool_start/end/error、
//!         on_iteration、on_final_answer，全程打印执行轨迹
//!
//! Part 2: 统计指标回调（Metrics）
//!         无侵入地收集迭代次数、工具调用次数、执行耗时等运行指标
//!
//! Part 3: 多回调组合 + 流式执行
//!         同时挂载日志回调与指标回调，演示多回调并发触发场景
//!         结合 execute_stream 观察流式路径下的回调时序
//!
//! Part 4: 错误感知回调
//!         注册一个会失败的自定义工具，验证 on_tool_error 回调
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo11_callbacks
//! ```

use async_trait::async_trait;
use echo_agent::agent::react_agent::ReactAgent;
use echo_agent::agent::react_agent::StepType;
use echo_agent::agent::{Agent, AgentCallback, AgentConfig, AgentEvent};
use echo_agent::error::ReactError;
use echo_agent::llm::types::Message;
use echo_agent::tools::others::math::{AddTool, MultiplyTool, SubtractTool};
use futures::StreamExt;
use serde_json::Value;
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── 入口 ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
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

    println!();
    println!("{}", "─".repeat(64));
    println!("Part 2: 统计指标回调（Metrics）\n");
    demo_metrics_callback().await?;

    println!();
    println!("{}", "─".repeat(64));
    println!("Part 3: 多回调组合 + 流式执行\n");
    demo_multi_callback_stream().await?;

    println!();
    println!("{}", "─".repeat(64));
    println!("Part 4: 错误感知回调\n");
    demo_error_callback().await?;

    println!();
    println!("{}", "═".repeat(64));
    println!("  demo11 完成");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// 回调实现
// ══════════════════════════════════════════════════════════════════════════════

// ── 1. 简单日志回调 ────────────────────────────────────────────────────────────

/// 将 Agent 执行的每个关键事件以可读格式打印到终端
struct LogCallback {
    /// 回调实例标识，方便区分多个实例
    label: String,
}

impl LogCallback {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    fn tag(&self) -> String {
        format!("[{}]", self.label)
    }
}

#[async_trait]
impl AgentCallback for LogCallback {
    async fn on_iteration(&self, agent: &str, iteration: usize) {
        println!(
            "  {} 🔄 on_iteration  agent={agent}  iter={}",
            self.tag(),
            iteration + 1
        );
    }

    async fn on_think_start(&self, agent: &str, messages: &[Message]) {
        println!(
            "  {} 🧠 on_think_start agent={agent}  ctx_len={}",
            self.tag(),
            messages.len()
        );
    }

    async fn on_think_end(&self, agent: &str, steps: &[StepType]) {
        let summary: Vec<String> = steps
            .iter()
            .map(|s| match s {
                StepType::Thought(_) => "Thought".to_string(),
                StepType::Call { function_name, .. } => format!("Call({function_name})"),
            })
            .collect();
        println!(
            "  {} 💡 on_think_end   agent={agent}  steps=[{}]",
            self.tag(),
            summary.join(", ")
        );
    }

    async fn on_tool_start(&self, agent: &str, tool: &str, args: &Value) {
        let args_str = compact_args(args);
        println!(
            "  {} 🔧 on_tool_start  agent={agent}  tool={tool}  args={args_str}",
            self.tag()
        );
    }

    async fn on_tool_end(&self, agent: &str, tool: &str, result: &str) {
        let preview = truncate(result, 60);
        println!(
            "  {} ✅ on_tool_end    agent={agent}  tool={tool}  result=\"{preview}\"",
            self.tag()
        );
    }

    async fn on_tool_error(&self, agent: &str, tool: &str, err: &ReactError) {
        println!(
            "  {} ❌ on_tool_error  agent={agent}  tool={tool}  err={err}",
            self.tag()
        );
    }

    async fn on_final_answer(&self, agent: &str, answer: &str) {
        let preview = truncate(answer, 80);
        println!(
            "  {} 🏁 on_final_answer agent={agent}  answer=\"{preview}\"",
            self.tag()
        );
    }
}

// ── 2. 指标统计回调 ────────────────────────────────────────────────────────────

/// 无侵入地采集 Agent 运行指标，线程安全，可在执行完毕后读取
struct MetricsCallback {
    iterations: AtomicUsize,
    llm_calls: AtomicUsize,
    tool_calls: AtomicUsize,
    tool_errors: AtomicUsize,
    /// 用原子 u64 存储 Unix 毫秒时间戳
    start_ms: AtomicU64,
    end_ms: AtomicU64,
    /// 记录每次工具调用的耗时（毫秒）
    tool_timings: Mutex<Vec<(String, u128)>>,
    /// 每次工具调用的开始时刻
    tool_start_time: Mutex<Option<Instant>>,
    tool_start_name: Mutex<String>,
}

impl MetricsCallback {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            iterations: AtomicUsize::new(0),
            llm_calls: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
            tool_errors: AtomicUsize::new(0),
            start_ms: AtomicU64::new(0),
            end_ms: AtomicU64::new(0),
            tool_timings: Mutex::new(Vec::new()),
            tool_start_time: Mutex::new(None),
            tool_start_name: Mutex::new(String::new()),
        })
    }

    fn total_elapsed(&self) -> Duration {
        let s = self.start_ms.load(Ordering::Relaxed);
        let e = self.end_ms.load(Ordering::Relaxed);
        if s == 0 || e == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(e - s)
        }
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
            "  │ LLM 推理次数   : {:>5}                   │",
            self.llm_calls.load(Ordering::Relaxed)
        );
        println!(
            "  │ 工具调用次数   : {:>5}                   │",
            self.tool_calls.load(Ordering::Relaxed)
        );
        println!(
            "  │ 工具错误次数   : {:>5}                   │",
            self.tool_errors.load(Ordering::Relaxed)
        );
        println!(
            "  │ 总耗时         : {:>5} ms                │",
            self.total_elapsed().as_millis()
        );
        println!("  ├─────────────────────────────────────────┤");
        println!("  │ 工具耗时明细：                           │");
        let timings = self.tool_timings.lock().unwrap();
        if timings.is_empty() {
            println!("  │   (无)                                  │");
        } else {
            for (name, ms) in timings.iter() {
                println!("  │   {:<18} : {:>5} ms           │", name, ms);
            }
        }
        println!("  └─────────────────────────────────────────┘");
    }
}

#[async_trait]
impl AgentCallback for MetricsCallback {
    async fn on_iteration(&self, _agent: &str, iteration: usize) {
        self.iterations.store(iteration + 1, Ordering::Relaxed);
        // 第一次迭代时记录开始时间
        if iteration == 0 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            self.start_ms.store(now_ms, Ordering::Relaxed);
        }
    }

    async fn on_think_start(&self, _agent: &str, _messages: &[Message]) {
        self.llm_calls.fetch_add(1, Ordering::Relaxed);
    }

    async fn on_tool_start(&self, _agent: &str, tool: &str, _args: &Value) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
        *self.tool_start_time.lock().unwrap() = Some(Instant::now());
        *self.tool_start_name.lock().unwrap() = tool.to_string();
    }

    async fn on_tool_end(&self, _agent: &str, _tool: &str, _result: &str) {
        let elapsed = self
            .tool_start_time
            .lock()
            .unwrap()
            .take()
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0);
        let name = self.tool_start_name.lock().unwrap().clone();
        self.tool_timings.lock().unwrap().push((name, elapsed));
    }

    async fn on_tool_error(&self, _agent: &str, _tool: &str, _err: &ReactError) {
        self.tool_errors.fetch_add(1, Ordering::Relaxed);
    }

    async fn on_final_answer(&self, _agent: &str, _answer: &str) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.end_ms.store(now_ms, Ordering::Relaxed);
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 1: 简单日志回调 Demo
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_log_callback() -> echo_agent::error::Result<()> {
    let system = "你是一个计算助手，必须通过工具完成所有计算，最后用 final_answer 报告结果。";

    let config = AgentConfig::new("qwen3-max", "log_agent", system)
        .enable_tool(true)
        .max_iterations(8)
        .with_callback(Arc::new(LogCallback::new("LOG")));

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));

    let task = "计算 (3 + 7) × 5";
    println!("  任务: {task}\n");
    println!("  --- 回调事件序列 ---");

    let answer = agent.execute(task).await?;

    println!();
    println!("  最终答案: {answer}");

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 2: 统计指标回调 Demo
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_metrics_callback() -> echo_agent::error::Result<()> {
    let system = "你是一个计算助手，必须通过工具完成所有计算，最后用 final_answer 报告结果。";

    let metrics = MetricsCallback::new();

    let config = AgentConfig::new("qwen3-max", "metrics_agent", system)
        .enable_tool(true)
        .max_iterations(10)
        .with_callback(metrics.clone());

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    let task = "计算：(10 + 20) × 3 - 15";
    println!("  任务: {task}");
    println!("  （指标回调静默运行，不输出事件日志）\n");

    let answer = agent.execute(task).await?;

    println!("  最终答案: {answer}\n");
    metrics.print_report();

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 3: 多回调组合 + 流式执行 Demo
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_multi_callback_stream() -> echo_agent::error::Result<()> {
    println!("  同时挂载 LogCallback + MetricsCallback，通过 execute_stream 执行\n");

    let system = "你是一个计算助手，必须通过工具完成所有计算，最后用 final_answer 报告结果。";

    let metrics = MetricsCallback::new();

    let config = AgentConfig::new("qwen3-max", "stream_cb_agent", system)
        .enable_tool(true)
        .max_iterations(10)
        // 同时挂载两个不同职责的回调
        .with_callback(Arc::new(LogCallback::new("STREAM-LOG")))
        .with_callback(metrics.clone());

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    let task = "计算：(5 + 3) × (10 - 4)";
    println!("  任务: {task}\n");
    println!("  --- 回调事件 + 流式 AgentEvent 混合输出 ---\n");

    let mut stream = agent.execute_stream(task).await?;

    let mut in_token = false;
    while let Some(ev) = stream.next().await {
        match ev? {
            AgentEvent::Token(t) => {
                if !in_token {
                    print!("  [AgentEvent] Token ▶ ");
                    std::io::stdout().flush().ok();
                    in_token = true;
                }
                print!("{}", t);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args } => {
                if in_token {
                    println!();
                    in_token = false;
                }
                println!(
                    "  [AgentEvent] ToolCall   ▶ {name}({})",
                    compact_args(&args)
                );
            }
            AgentEvent::ToolResult { name, output } => {
                if in_token {
                    println!();
                    in_token = false;
                }
                println!(
                    "  [AgentEvent] ToolResult ▶ [{name}] {}",
                    truncate(&output, 60)
                );
            }
            AgentEvent::FinalAnswer(ans) => {
                if in_token {
                    println!();
                    in_token = false;
                }
                println!("\n  [AgentEvent] FinalAnswer ▶ {}", truncate(&ans, 80));
            }
        }
    }

    println!();
    println!("\n  --- Metrics（与日志回调独立采集）---");
    metrics.print_report();

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 4: 错误感知回调 Demo
// ══════════════════════════════════════════════════════════════════════════════

/// 一个永远返回失败的工具，用于触发 on_tool_error 回调
struct BrokenTool;

#[async_trait::async_trait]
impl echo_agent::tools::Tool for BrokenTool {
    fn name(&self) -> &str {
        "broken_tool"
    }
    fn description(&self) -> &str {
        "这个工具总是失败，用于测试错误回调"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "任意输入" }
            },
            "required": ["input"]
        })
    }
    async fn execute(
        &self,
        _params: echo_agent::tools::ToolParameters,
    ) -> echo_agent::error::Result<echo_agent::tools::ToolResult> {
        Ok(echo_agent::tools::ToolResult {
            success: false,
            output: String::new(),
            error: Some("BrokenTool: 模拟工具失败，请使用其他工具完成任务".to_string()),
        })
    }
}

async fn demo_error_callback() -> echo_agent::error::Result<()> {
    println!("  注册一个总是失败的工具，验证 on_tool_error 回调被触发\n");

    // 用一个简单的计数器回调来验证 on_tool_error 确实被调用
    struct ErrorCounter(AtomicUsize);

    #[async_trait]
    impl AgentCallback for ErrorCounter {
        async fn on_tool_error(&self, agent: &str, tool: &str, err: &ReactError) {
            let count = self.0.fetch_add(1, Ordering::Relaxed) + 1;
            println!(
                "  [ErrorCounter] on_tool_error #{count}  agent={agent}  tool={tool}  err={err}"
            );
        }
        async fn on_final_answer(&self, agent: &str, answer: &str) {
            println!(
                "  [ErrorCounter] on_final_answer  agent={agent}  answer=\"{}\"",
                truncate(answer, 80)
            );
        }
    }

    let error_counter = Arc::new(ErrorCounter(AtomicUsize::new(0)));

    let system = r#"你是一个智能助手。
可用工具：broken_tool（已知会失败）、add（加法）。
当 broken_tool 失败时，直接用 final_answer 说明情况，不要重试 broken_tool。"#;

    let config = AgentConfig::new("qwen3-max", "error_agent", system)
        .enable_tool(true)
        .max_iterations(6)
        .with_callback(Arc::new(LogCallback::new("ERR-LOG")))
        .with_callback(error_counter.clone());

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(BrokenTool));
    agent.add_tool(Box::new(AddTool));

    let task = "请调用 broken_tool（input=\"test\"）并告知结果";
    println!("  任务: {task}\n");
    println!("  --- 回调事件序列 ---");

    // broken_tool 失败后 agent 会收到错误信息并继续，最终给出 final_answer
    match agent.execute(task).await {
        Ok(answer) => {
            println!();
            println!("  最终答案: {answer}");
        }
        Err(e) => {
            println!();
            println!("  Agent 执行失败（符合预期）: {e}");
        }
    }

    let errors = error_counter.0.load(Ordering::Relaxed);
    println!();
    println!("  共触发 on_tool_error 回调 {errors} 次");

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// 辅助函数
// ══════════════════════════════════════════════════════════════════════════════

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
                let val = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}={val}")
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
    println!("  本 demo 通过四个场景展示 AgentCallback 的使用：");
    println!("    Part 1  LogCallback    —— 可读事件日志");
    println!("    Part 2  MetricsCallback —— 运行指标采集");
    println!("    Part 3  多回调 + 流式  —— 组合使用");
    println!("    Part 4  ErrorCounter   —— 错误感知回调");
    println!();
    println!("  AgentCallback 回调列表：");
    println!("    on_iteration(agent, iter)");
    println!("    on_think_start(agent, messages)");
    println!("    on_think_end(agent, steps)");
    println!("    on_tool_start(agent, tool, args)");
    println!("    on_tool_end(agent, tool, result)");
    println!("    on_tool_error(agent, tool, err)");
    println!("    on_final_answer(agent, answer)");
    println!();
}
