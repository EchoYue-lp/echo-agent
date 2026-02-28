//! demo13_tool_execution.rs —— ToolExecutionConfig 完整演示
//!
//! 通过四个场景逐一展示 `ToolExecutionConfig` 各字段的效果：
//!
//! ```text
//! Part 1: 默认配置（30s 超时、不重试、不限并发）
//!         4 个数学工具并行调用，观察正常执行流程
//!
//! Part 2: timeout_ms = 1_500
//!         SlowTool 耗时 3 秒 → 超过 1.5 秒限制 → ToolError::Timeout
//!         tool_error_feedback=true：超时错误回传 LLM，LLM 换用正常工具完成任务
//!
//! Part 3: retry_on_fail = true, max_retries = 2
//!         FlakyTool 前 2 次必定失败，第 3 次成功
//!         → ToolManager 自动重试，最终返回正确结果
//!
//! Part 4: max_concurrency = 2
//!         同时注册 4 个 ConcurrentTool（每个耗时 1 秒）
//!         LLM 并行调用全部 4 个 → 实际并发限制为 2
//!         通过峰值并发计数器验证最多 2 个同时运行
//! ```
//!
//! # 运行
//! ```bash
//! cargo run --example demo13_tool_execution
//! ```

use async_trait::async_trait;
use echo_agent::agent::react_agent::ReactAgent;
use echo_agent::agent::{Agent, AgentConfig};
use echo_agent::error::{Result, ToolError};
use echo_agent::tools::others::math::AddTool;
use echo_agent::tools::{Tool, ToolExecutionConfig, ToolParameters, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use tokio::time::{Duration, sleep};

// ── 入口 ──────────────────────────────────────────────────────────────────────

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

    println!();
    println!("{}", "═".repeat(64));
    println!("  demo13 完成");
    println!("{}", "═".repeat(64));
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// 自定义测试工具
// ══════════════════════════════════════════════════════════════════════════════

/// 模拟慢工具：耗时 `delay_secs` 秒后才返回结果
struct SlowTool {
    delay_secs: u64,
}

#[async_trait]
impl Tool for SlowTool {
    fn name(&self) -> &str {
        "slow_add"
    }

    fn description(&self) -> &str {
        "一个故意很慢的加法工具，用于演示超时控制。参数：a（整数）、b（整数）"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer", "description": "第一个加数" },
                "b": { "type": "integer", "description": "第二个加数" }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        println!("  [SlowTool] 开始执行，将等待 {} 秒...", self.delay_secs);
        sleep(Duration::from_secs(self.delay_secs)).await;
        let a = params.get("a").and_then(Value::as_i64).unwrap_or(0);
        let b = params.get("b").and_then(Value::as_i64).unwrap_or(0);
        println!("  [SlowTool] 执行完成，结果：{}", a + b);
        Ok(ToolResult::success(format!("{}", a + b)))
    }
}

/// 模拟不稳定工具：前 `fail_times` 次调用返回错误，之后成功
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
        "一个偶尔出错的乘法工具，用于演示重试机制。参数：a（整数）、b（整数）"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer", "description": "被乘数" },
                "b": { "type": "integer", "description": "乘数" }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        let attempt = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.fail_times {
            println!(
                "  [FlakyTool] 第 {} 次调用 → 故意失败（模拟偶发错误）",
                attempt
            );
            return Err(ToolError::ExecutionFailed {
                tool: "flaky_multiply".into(),
                message: format!("模拟第 {} 次偶发故障", attempt),
            }
            .into());
        }
        let a = params.get("a").and_then(Value::as_i64).unwrap_or(0);
        let b = params.get("b").and_then(Value::as_i64).unwrap_or(0);
        println!(
            "  [FlakyTool] 第 {} 次调用 → 成功，结果：{}",
            attempt,
            a * b
        );
        Ok(ToolResult::success(format!("{}", a * b)))
    }
}

/// 模拟并发感知工具：执行时增加全局并发计数，完成后减少，记录峰值
struct ConcurrentTool {
    id: usize,
    active_count: Arc<AtomicI32>,
    peak_count: Arc<AtomicI32>,
}

impl ConcurrentTool {
    fn new(id: usize, active: Arc<AtomicI32>, peak: Arc<AtomicI32>) -> Self {
        Self {
            id,
            active_count: active,
            peak_count: peak,
        }
    }
}

#[async_trait]
impl Tool for ConcurrentTool {
    fn name(&self) -> &str {
        // 返回固定名称会覆盖，用 leak 生成不同名称
        Box::leak(format!("task_{}", self.id).into_boxed_str())
    }

    fn description(&self) -> &str {
        "并发测试工具，模拟耗时 1 秒的任务"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _params: ToolParameters) -> echo_agent::error::Result<ToolResult> {
        let current = self.active_count.fetch_add(1, Ordering::SeqCst) + 1;
        // 更新峰值
        let mut peak = self.peak_count.load(Ordering::SeqCst);
        while current > peak {
            match self.peak_count.compare_exchange(
                peak,
                current,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => peak = actual,
            }
        }
        println!("  [task_{}] 开始执行（当前并发数：{}）", self.id, current);
        sleep(Duration::from_secs(1)).await;
        self.active_count.fetch_sub(1, Ordering::SeqCst);
        println!("  [task_{}] 执行完成", self.id);
        Ok(ToolResult::success(format!("task_{} 完成", self.id)))
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 1: 默认配置
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_default() -> Result<()> {
    println!("配置：ToolExecutionConfig::default()");
    println!("  timeout_ms     = 30_000（30 秒，不会触发）");
    println!("  retry_on_fail  = false");
    println!("  max_concurrency= None（不限制）");
    println!();

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_default",
        "你是一个计算助手。用数学工具完成计算，通过 final_answer 报告结果。",
    )
    .enable_tool(true)
    // 不设置 tool_execution，使用默认配置
    ;

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));

    println!("任务：计算 15 + 27");
    match agent.execute("计算 15 + 27，给出数字结果").await {
        Ok(ans) => println!("✅ 结果：{}", ans),
        Err(e) => println!("❌ 错误：{}", e),
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 2: 超时控制
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_timeout() -> Result<()> {
    println!("配置：timeout_ms = 1_500");
    println!("  SlowTool 耗时 3 秒 → 超过 1.5 秒 → ToolError::Timeout");
    println!("  tool_error_feedback = true → 错误回传 LLM → LLM 换用 AddTool");
    println!();

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_timeout",
        r#"你是一个计算助手。
- slow_add：执行加法，但非常慢
- add：执行加法，速度正常
优先尝试 slow_add；如果它超时或失败，立刻改用 add。
通过 final_answer 报告最终数字结果。"#,
    )
    .enable_tool(true)
    .tool_execution(ToolExecutionConfig {
        timeout_ms: 1_500, // 1.5 秒超时
        retry_on_fail: false,
        max_retries: 0,
        retry_delay_ms: 0,
        max_concurrency: None,
    });

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(SlowTool { delay_secs: 3 })); // 会超时
    agent.add_tool(Box::new(AddTool)); // 备用

    println!("任务：计算 100 + 200（预期 slow_add 超时后由 add 完成）");
    let start = std::time::Instant::now();
    match agent
        .execute("用 slow_add 计算 100 + 200，给出数字结果")
        .await
    {
        Ok(ans) => println!(
            "✅ 结果：{}（耗时 {:.1}s）",
            ans,
            start.elapsed().as_secs_f32()
        ),
        Err(e) => println!(
            "❌ 错误：{}（耗时 {:.1}s）",
            e,
            start.elapsed().as_secs_f32()
        ),
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 3: 自动重试
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_retry() -> Result<()> {
    println!("配置：retry_on_fail = true, max_retries = 2");
    println!("  FlakyTool 前 2 次失败，第 3 次成功");
    println!("  ToolManager 自动重试，最终返回正确结果（LLM 无感知）");
    println!();

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_retry",
        "你是一个计算助手。用 flaky_multiply 完成乘法计算，通过 final_answer 报告结果。",
    )
    .enable_tool(true)
    .tool_execution(ToolExecutionConfig {
        timeout_ms: 30_000,
        retry_on_fail: true, // 开启自动重试
        max_retries: 2,      // 最多重试 2 次
        retry_delay_ms: 100, // 每次重试前等 100ms
        max_concurrency: None,
    });

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(FlakyTool::new(2))); // 前 2 次失败

    println!("任务：计算 7 × 8（前两次调用会故障，第三次成功）");
    match agent
        .execute("用 flaky_multiply 计算 7 乘以 8，给出数字结果")
        .await
    {
        Ok(ans) => println!("✅ 结果：{}", ans),
        Err(e) => println!("❌ 错误：{}", e),
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Part 4: 并发度控制
// ══════════════════════════════════════════════════════════════════════════════

async fn demo_concurrency() -> Result<()> {
    println!("配置：max_concurrency = 2");
    println!("  注册 4 个任务工具，每个耗时 1 秒");
    println!("  LLM 并行调用全部 4 个 → Semaphore 限流，实际每批最多 2 个同时执行");
    println!("  预期：峰值并发数 ≤ 2，总耗时 ≈ 2 秒（而非限流前的 ≈ 1 秒）");
    println!();

    let active = Arc::new(AtomicI32::new(0));
    let peak = Arc::new(AtomicI32::new(0));

    let config = AgentConfig::new(
        "qwen3-max",
        "agent_concurrency",
        r#"你是一个任务调度助手。
请同时调用全部 4 个工具：task_1、task_2、task_3、task_4，
等所有工具都完成后，通过 final_answer 报告"全部任务完成"。"#,
    )
    .enable_tool(true)
    .tool_execution(ToolExecutionConfig {
        timeout_ms: 30_000,
        retry_on_fail: false,
        max_retries: 0,
        retry_delay_ms: 0,
        max_concurrency: Some(2), // 最多 2 个并发
    });

    let mut agent = ReactAgent::new(config);
    for i in 1..=4 {
        agent.add_tool(Box::new(ConcurrentTool::new(
            i,
            active.clone(),
            peak.clone(),
        )));
    }

    let start = std::time::Instant::now();
    match agent
        .execute("请并行调用 task_1、task_2、task_3、task_4 这 4 个工具")
        .await
    {
        Ok(ans) => {
            let elapsed = start.elapsed().as_secs_f32();
            let peak_val = peak.load(Ordering::SeqCst);
            println!("✅ 结果：{}", ans);
            println!();
            println!("📊 并发统计：");
            println!("  峰值并发数 = {}（上限为 2）", peak_val);
            println!("  总耗时     = {:.1}s", elapsed);
            if peak_val <= 2 {
                println!("  ✅ 并发限流生效：峰值 {} ≤ 2", peak_val);
            } else {
                println!("  ⚠️ 并发超出预期上限");
            }
        }
        Err(e) => println!("❌ 错误：{}", e),
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// 辅助函数
// ══════════════════════════════════════════════════════════════════════════════

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("  demo13 — ToolExecutionConfig 演示");
    println!("{}", "═".repeat(64));
    println!();
    println!("演示 ToolExecutionConfig 四个配置项的实际效果：");
    println!("  timeout_ms      单工具执行超时（毫秒）");
    println!("  retry_on_fail   失败时自动重试");
    println!("  max_retries     最大重试次数");
    println!("  max_concurrency 并行工具调用的最大并发数");
    println!();
}

fn sep(title: &str) {
    println!();
    println!("{}", "─".repeat(64));
    println!("  {}", title);
    println!("{}", "─".repeat(64));
    println!();
}
