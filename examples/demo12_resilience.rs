//! demo12_resilience.rs —— 韧性特性开关对比演示
//!
//! 通过四个场景直观对比两个韧性开关打开 / 关闭时的行为差异：
//!
//! ```text
//! Part 1: tool_error_feedback = false（旧行为）
//!         BrokenTool 失败 → execute_tool 立即返回 Err
//!         → Agent 中断，任务失败
//!
//! Part 2: tool_error_feedback = true（新行为，默认）
//!         BrokenTool 失败 → 错误文本作为工具观测值写入上下文
//!         → LLM 读取错误后换用其他工具，任务成功完成
//!
//! Part 3: FlakyTool（前 2 次失败、第 3 次成功）
//!         tool_error_feedback = true
//!         展示 LLM 收到错误观测值后自动重试并最终成功的完整流程
//!
//! Part 4: llm_max_retries 开关对比
//!         = 0：LLM 调用失败后不重试，立即抛出错误
//!         = 3：遇到网络/限流/5xx 时指数退避最多重试 3 次
//!         通过日志回调展示配置差异及重试触发条件
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo12_resilience
//! ```

use async_trait::async_trait;
use echo_agent::agent::react_agent::ReactAgent;
use echo_agent::agent::react_agent::StepType;
use echo_agent::agent::{Agent, AgentCallback, AgentConfig};
use echo_agent::error::ReactError;
use echo_agent::llm::types::Message;
use echo_agent::tools::others::math::AddTool;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ── 入口 ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo12_resilience=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1 ────────────────────────────────────────────────────────────────
    sep("Part 1: tool_error_feedback = false（旧行为）");
    demo_feedback_off().await;

    // ── Part 2 ────────────────────────────────────────────────────────────────
    sep("Part 2: tool_error_feedback = true（新行为，默认）");
    demo_feedback_on().await?;

    // ── Part 3 ────────────────────────────────────────────────────────────────
    sep("Part 3: FlakyTool —— 偶发故障后自动恢复");
    demo_flaky_tool().await?;

    // ── Part 4 ────────────────────────────────────────────────────────────────
    sep("Part 4: llm_max_retries 开关对比");
    demo_llm_retry_config();

    println!();
    println!("{}", "═".repeat(64));
    println!("  demo12 完成");
    println!("{}", "═".repeat(64));
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// 测试工具
// ══════════════════════════════════════════════════════════════════════════════

// ── BrokenTool：永远失败 ───────────────────────────────────────────────────────

struct BrokenTool;

#[async_trait]
impl Tool for BrokenTool {
    fn name(&self) -> &str {
        "broken_tool"
    }
    fn description(&self) -> &str {
        "模拟损坏的工具，总是返回失败，用于测试错误处理路径"
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
    async fn execute(&self, _params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        Ok(ToolResult {
            success: false,
            output: String::new(),
            error: Some("BrokenTool: 工具内部错误，服务不可用".to_string()),
        })
    }
}

// ── FlakyTool：前 N 次失败，之后成功 ─────────────────────────────────────────

struct FlakyTool {
    /// 还需要失败几次
    fail_remaining: Arc<AtomicUsize>,
    /// 已被调用总次数（可观测）
    call_count: Arc<AtomicUsize>,
}

impl FlakyTool {
    fn new(fail_times: usize) -> Self {
        Self {
            fail_remaining: Arc::new(AtomicUsize::new(fail_times)),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl Tool for FlakyTool {
    fn name(&self) -> &str {
        "weather_api"
    }
    fn description(&self) -> &str {
        "查询城市实时天气。服务偶有故障，遇到错误请稍后重试。"
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "城市名称" }
            },
            "required": ["city"]
        })
    }
    async fn execute(&self, params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        let city = params
            .get("city")
            .and_then(|v| v.as_str())
            .unwrap_or("未知城市");

        let call_idx = self.call_count.fetch_add(1, Ordering::Relaxed) + 1;
        let remaining = self.fail_remaining.load(Ordering::Relaxed);

        if remaining > 0 {
            self.fail_remaining.fetch_sub(1, Ordering::Relaxed);
            println!("    [FlakyTool] 第 {call_idx} 次调用（city={city}）→ 模拟瞬时故障");
            Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "weather_api: 服务暂时不可用（瞬时故障），请稍后重试（第 {call_idx} 次尝试）"
                )),
            })
        } else {
            println!("    [FlakyTool] 第 {call_idx} 次调用（city={city}）→ 成功返回数据");
            Ok(ToolResult {
                success: true,
                output: format!("{city}：晴，26°C，东南风 3 级"),
                error: None,
            })
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 简易日志回调（用于所有 Part）
// ══════════════════════════════════════════════════════════════════════════════

struct SimpleLog {
    label: &'static str,
}

#[async_trait]
impl AgentCallback for SimpleLog {
    async fn on_iteration(&self, _agent: &str, iteration: usize) {
        println!("  [{}] 🔄 迭代 {}", self.label, iteration + 1);
    }
    async fn on_tool_start(&self, _agent: &str, tool: &str, args: &Value) {
        println!(
            "  [{}] 🔧 调用工具: {}  args={}",
            self.label,
            tool,
            compact(args)
        );
    }
    async fn on_tool_end(&self, _agent: &str, tool: &str, result: &str) {
        println!(
            "  [{}] ✅ 工具成功: {}  result=\"{}\"",
            self.label,
            tool,
            trunc(result, 60)
        );
    }
    async fn on_tool_error(&self, _agent: &str, tool: &str, err: &ReactError) {
        println!("  [{}] ❌ 工具错误: {}  err={}", self.label, tool, err);
    }
    async fn on_think_start(&self, _agent: &str, _messages: &[Message]) {}
    async fn on_think_end(&self, _agent: &str, steps: &[StepType]) {
        let names: Vec<String> = steps
            .iter()
            .map(|s| match s {
                StepType::Thought(_) => "Thought".into(),
                StepType::Call { function_name, .. } => format!("Call({function_name})"),
            })
            .collect();
        println!("  [{}] 💡 LLM 决策: [{}]", self.label, names.join(", "));
    }
    async fn on_final_answer(&self, _agent: &str, answer: &str) {
        println!("  [{}] 🏁 最终答案: \"{}\"", self.label, trunc(answer, 80));
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 1: tool_error_feedback = false
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_feedback_off() {
    println!("  配置：tool_error_feedback = false");
    println!("  预期：BrokenTool 失败 → execute_tool 返回 Err → Agent 立即中断\n");

    let system = r#"你是一个智能助手。请调用 broken_tool（input="test"）并报告结果。"#;

    let config = AgentConfig::new("qwen3-max", "agent_no_feedback", system)
        .enable_tool(true)
        .max_iterations(4)
        .tool_error_feedback(false) // ← 关闭
        .with_callback(Arc::new(SimpleLog { label: "NO-FB" }));

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(BrokenTool));

    println!("  任务：调用 broken_tool 并报告\n");

    match agent
        .execute("请调用 broken_tool（input=\"test\"）并报告结果。")
        .await
    {
        Ok(answer) => {
            // 通常不会走到这里
            println!("\n  ⚠️  意外成功（不应发生）: {answer}");
        }
        Err(e) => {
            println!("\n  ✅ 符合预期 —— Agent 因工具失败而中断:");
            println!("     错误类型: {e}");
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 2: tool_error_feedback = true
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_feedback_on() -> echo_agent::error::Result<()> {
    println!("  配置：tool_error_feedback = true（默认）");
    println!("  预期：BrokenTool 失败 → 错误回传 LLM → LLM 换用 add 完成任务\n");

    let system = r#"你是一个智能助手。
规则：
1. 先尝试调用 broken_tool（input="test"）
2. 如果工具失败，换用 add 工具完成一次加法计算（3 + 4）
3. 最后用 final_answer 报告完整过程和结果"#;

    let config = AgentConfig::new("qwen3-max", "agent_with_feedback", system)
        .enable_tool(true)
        .max_iterations(6)
        .tool_error_feedback(true) // ← 开启（默认值）
        .with_callback(Arc::new(SimpleLog { label: "FB-ON" }));

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(BrokenTool));
    agent.add_tool(Box::new(AddTool));

    println!("  任务：先调用 broken_tool，失败后换用 add\n");

    let answer = agent
        .execute("先调用 broken_tool（input=\"test\"），失败后换用 add 计算 3+4，报告完整过程。")
        .await?;

    println!("\n  ✅ 任务成功完成:");
    for line in answer.lines().take(6) {
        println!("     {line}");
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 3: FlakyTool —— 偶发故障后自动恢复
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_flaky_tool() -> echo_agent::error::Result<()> {
    println!("  配置：tool_error_feedback = true + FlakyTool（前 2 次失败）");
    println!("  预期：LLM 收到故障观测值 → 重试 weather_api → 第 3 次成功\n");

    let system = r#"你是一个天气查询助手。
规则：
1. 调用 weather_api 查询北京天气
2. 如果工具返回"服务暂时不可用"，请等待并重试（最多重试 3 次）
3. 成功获取天气后，用 final_answer 报告结果"#;

    let flaky = FlakyTool::new(2); // 前 2 次失败，第 3 次成功
    let call_count = flaky.call_count.clone();

    let config = AgentConfig::new("qwen3-max", "agent_flaky", system)
        .enable_tool(true)
        .max_iterations(8)
        .tool_error_feedback(true) // ← 必须开启才能重试
        .with_callback(Arc::new(SimpleLog { label: "FLAKY" }));

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(flaky));

    println!("  任务：查询北京天气（工具前 2 次会故障）\n");

    let answer = agent.execute("查询北京的实时天气。").await?;

    println!(
        "\n  ✅ 任务成功完成（共调用工具 {} 次）:",
        call_count.load(Ordering::Relaxed)
    );
    println!("     {answer}");
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 4: llm_max_retries 开关对比（配置层面展示）
// ══════════════════════════════════════════════════════════════════════════════

fn demo_llm_retry_config() {
    // 不实际发起 LLM 请求，只展示配置参数与触发条件
    println!("  LLM 重试仅在以下错误类型上触发（配置层面对比）：\n");
    println!("  触发条件                    是否重试");
    println!("  ─────────────────────────── ────────");
    println!("  网络超时 / 连接失败          ✅ 是");
    println!("  HTTP 429  Too Many Requests  ✅ 是");
    println!("  HTTP 500 / 502 / 503 / 504   ✅ 是");
    println!("  HTTP 401  未授权             ❌ 否");
    println!("  HTTP 404  模型不存在         ❌ 否");
    println!("  配置错误 / API Key 缺失      ❌ 否");
    println!("  JSON 解析失败                ❌ 否");
    println!();

    let config_off = AgentConfig::new("qwen3-max", "retry_off", "").llm_max_retries(0); // 不重试

    let config_on = AgentConfig::new("qwen3-max", "retry_on", "")
        .llm_max_retries(3) // 最多重试 3 次
        .llm_retry_delay_ms(500); // 首次延迟 500ms，后续指数翻倍

    println!("  ── llm_max_retries = 0（关闭重试）──");
    println!("     LLM 调用失败 → 立即返回 Err，不等待");
    println!("     max_retries = {}", config_off.get_llm_max_retries());
    println!();

    println!("  ── llm_max_retries = 3（开启重试）──");
    println!("     调用失败 → 等 500ms → 重试 1");
    println!("     再失败  → 等 1000ms → 重试 2");
    println!("     再失败  → 等 2000ms → 重试 3");
    println!("     仍失败  → 返回 Err");
    println!("     max_retries = {}", config_on.get_llm_max_retries());
    println!(
        "     retry_delay = {}ms",
        config_on.get_llm_retry_delay_ms()
    );
    println!();
    println!("  （实际触发需要遇到网络故障 / 限流等可重试错误）");
}

// ══════════════════════════════════════════════════════════════════════════════
// 辅助
// ══════════════════════════════════════════════════════════════════════════════

fn trunc(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{out}…")
    } else {
        out
    }
}

fn compact(v: &Value) -> String {
    match v {
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

fn sep(title: &str) {
    println!();
    println!("{}", "─".repeat(64));
    println!("{title}");
    println!();
}

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × 韧性特性开关对比 (demo12)");
    println!("{}", "═".repeat(64));
    println!();
    println!("  对比场景：");
    println!("    Part 1  tool_error_feedback = false  → 工具失败即中断");
    println!("    Part 2  tool_error_feedback = true   → 错误回传 LLM 纠错");
    println!("    Part 3  FlakyTool (前2次失败)        → LLM 重试后恢复");
    println!("    Part 4  llm_max_retries 0 vs 3       → 重试配置对比");
    println!();
}
