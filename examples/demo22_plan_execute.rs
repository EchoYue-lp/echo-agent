//! demo22_plan_execute —— Plan-and-Execute 完整演示
//!
//! 演示 Plan-and-Execute 引擎的完整能力：
//! - **Plan 阶段**：将任务拆解为结构化步骤
//! - **Execute 阶段**：按步骤顺序执行
//! - **Replan 阶段**：失败后重新规划
//!
//! # 运行方式
//!
//! ```bash
//! # 无需 LLM，完整演示整个流程
//! cargo run --example demo22_plan_execute
//! ```

use echo_agent::agents::plan_execute::{Executor, PlanExecuteAgent, StaticPlanner};
use echo_agent::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Import Timelike for hour() method on NaiveTime
use chrono::Timelike;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    // 设置日志级别以查看 Plan-and-Execute 的内部流程
    tracing_subscriber::fmt()
        .with_env_filter("echo_agent::plan_execute=info,demo22=info")
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("      Plan-and-Execute 完整演示 (demo22)");
    println!("═══════════════════════════════════════════════════════\n");

    // ── Part 1: 静态计划演示（无需 LLM）──────────────────────────────────────
    demo_static_plan().await?;

    // ── Part 2: 自定义 Executor 演示──────────────────────────────────────────
    demo_custom_executor().await?;

    // ── Part 3: 失败重规划演示──────────────────────────────────────────────────
    demo_replan_on_failure().await?;

    // ── Part 4: 计划结构演示────────────────────────────────────────────────────
    demo_plan_structure();

    println!("═══════════════════════════════════════════════════════\n");

    Ok(())
}

// ── Part 1: 静态计划演示 ─────────────────────────────────────────────────────

async fn demo_static_plan() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 1: 静态计划（无需 LLM）");
    println!("─────────────────────────────────────────────\n");

    // 使用 StaticPlanner 创建预定义计划
    let planner = StaticPlanner::new(vec![
        "搜索 Rust 所有权权机制的官方文档",
        "总结所有权的核心概念（所有权、借用、生命周期）",
        "列举 3 个实际代码示例",
        "生成一份学习指南",
    ]);

    // 创建可打印计划的 Executor
    let executor = VerboseExecutor::new();

    let mut agent = PlanExecuteAgent::new("static_demo_agent", planner, executor).disable_replan();

    let task = "学习 Rust 的所有权机制";

    println!("┌─────────────────────────────────────────────┐");
    println!("│ 任务: {}", task);
    println!("└─────────────────────────────────────────────┘\n");

    let result = agent.execute(task).await?;

    println!("┌─────────────────────────────────────────────┐");
    println!("│ 最终结果                                     │");
    println!("└─────────────────────────────────────────────┘");
    println!("{}\n", result);

    Ok(())
}

// ── Part 2: 自定义 Executor 演示 ───────────────────────────────────────────────

async fn demo_custom_executor() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 2: 自定义 Executor");
    println!("─────────────────────────────────────────────\n");

    println!("演示：创建一个会显示步骤详情的 Executor\n");

    // 创建一个可以注入上下文的 Executor
    let executor = ContextAwareExecutor::new();

    let planner = StaticPlanner::new(vec!["读取当前时间", "根据时间判断问候语", "生成问候消息"]);

    let mut agent = PlanExecuteAgent::new("custom_executor_agent", planner, executor);

    println!("┌─────────────────────────────────────────────┐");
    println!("│ 任务: 生成带时间问候的消息                     │");
    println!("└─────────────────────────────────────────────┘\n");

    let result = agent.execute("生成问候消息").await?;

    println!("✓ 最终结果:\n{}\n", result);

    Ok(())
}

// ── Part 3: 失败重规划演示 ─────────────────────────────────────────────────────

async fn demo_replan_on_failure() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 3: 失败重规划演示");
    println!("─────────────────────────────────────────────\n");

    println!("演示：当某个步骤失败时，系统如何重新规划下游任务\n");

    // 创建一个会前两次失败、第三次成功的 Executor
    let executor = FlakyExecutor::new(vec![false, false, true]);

    let planner = StaticPlanner::new(vec![
        "连接数据库",
        "查询用户数据",
        "生成统计报告",
        "发送邮件通知",
    ]);

    let mut agent = PlanExecuteAgent::new("replan_demo_agent", planner, executor).max_replans(2);

    println!("┌─────────────────────────────────────────────┐");
    println!("│ 任务: 数据库查询 + 报告生成                  │");
    println!("│ 说明: 前两次执行会失败，触发重规划         │");
    println!("└─────────────────────────────────────────────┘\n");

    match agent.execute("执行数据查询任务").await {
        Ok(result) => {
            println!("✓ 最终结果:\n{}\n", result);
        }
        Err(e) => {
            println!("✗ 执行失败: {}\n", e);
        }
    }

    Ok(())
}

// ── Part 4: 计划结构演示 ─────────────────────────────────────────────────────

fn demo_plan_structure() {
    println!("─────────────────────────────────────────────");
    println!("Part 4: Plan 结构说明");
    println!("─────────────────────────────────────────────\n");

    println!("Plan 结构:");
    println!("┌─────────────────────────────────────────────┐");
    println!("│ struct Plan {{                                 │");
    println!("│     id: Option<String>,        // 计划 ID    │");
    println!("│     version: u32,              // 版本号    │");
    println!("│     steps: Vec<PlanStep>,      // 步骤列表  │");
    println!("│     goal: Option<String>,       // 目标描述  │");
    println!("│     parent_plan_id: Option<String>, // 父计划ID│");
    println!("│     created_at: u64,           // 创建时间  │");
    println!("│     updated_at: u64,           // 更新时间  │");
    println!("│ }}                                               │");
    println!("└─────────────────────────────────────────────┘\n");

    println!("PlanStep 结构:");
    println!("┌─────────────────────────────────────────────┐");
    println!("│ struct PlanStep {{                              │");
    println!("│     id: String,                  // 步骤 ID   │");
    println!("│     description: String,        // 步骤描述  │");
    println!("│     dependencies: Vec<String>, // 依赖的步骤│");
    println!("│     expected_output: Option<String>, // 期望输出 │");
    println!("│ }}                                               │");
    println!("└─────────────────────────────────────────────┘\n");

    println!("执行流程:");
    println!("  1. Planner.plan(task) → Plan");
    println!("  2. Plan → TaskManager (转换为 DAG)");
    println!("  3. TaskExecutor.execute_all() (并行或顺序)");
    println!("  4. 汇总结果 → 最终答案\n");
}

// ── 自定义 Executor 实现 ─────────────────────────────────────────────────────

/// 详细输出 Executor - 显示每个步骤的执行情况
struct VerboseExecutor;

impl VerboseExecutor {
    fn new() -> Self {
        Self
    }
}

impl Executor for VerboseExecutor {
    fn execute_step<'a>(
        &'a mut self,
        description: &'a str,
        context: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            println!("┌─────────────────────────────────────────────┐");
            println!("│ ⚡ 执行步骤                                   │");
            println!("├─────────────────────────────────────────────┤");
            println!("│ 描述: {}", description);
            println!(
                "│ 上下文: {}",
                if context.is_empty() { "(无)" } else { context }
            );
            println!("├─────────────────────────────────────────────┤");
            println!("│ 状态: 执行中...                               │");
            println!("└─────────────────────────────────────────────┘");

            // 模拟执行
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let result = format!("已完成: {}", description);

            println!("┌─────────────────────────────────────────────┐");
            println!("│ ✓ 步骤完成                                  │");
            println!("│ 结果: {}", result);
            println!("└─────────────────────────────────────────────┘\n");

            Ok(result)
        })
    }
}

/// 带上下文的 Executor - 可以使用之前步骤的结果
struct ContextAwareExecutor {
    results: Arc<std::sync::Mutex<Vec<String>>>,
}

impl ContextAwareExecutor {
    fn new() -> Self {
        Self {
            results: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

impl Executor for ContextAwareExecutor {
    fn execute_step<'a>(
        &'a mut self,
        description: &'a str,
        _context: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            println!("  ⚡ 执行: {}", description);

            // 获取当前时间
            if description.contains("读取当前时间") {
                let now = chrono::Local::now().time().format("%H:%M").to_string();
                let result = format!("当前时间: {}", now);

                // 保存结果供后续步骤使用
                if let Ok(mut results) = self.results.lock() {
                    results.push(result.clone());
                }

                return Ok(result);
            }

            // 判断问候语
            if description.contains("根据时间判断问候语") {
                let now = chrono::Local::now().time();
                let greeting = if now.hour() < 12 {
                    "早上好"
                } else if now.hour() < 18 {
                    "下午好"
                } else {
                    "晚上好"
                };

                let result = format!("时间判断结果: {}", greeting);

                if let Ok(mut results) = self.results.lock() {
                    results.push(result.clone());
                }

                return Ok(result);
            }

            // 生成问候
            if description.contains("生成问候消息") {
                let results = self.results.lock().unwrap();
                let time_info = results.get(0).map(|s| s.as_str()).unwrap_or("未知时间");
                let greeting = results.get(1).map(|s| s.as_str()).unwrap_or("你好");

                let result = format!("{}，{}！很高兴为您服务。", greeting, time_info);

                return Ok(result);
            }

            Ok(format!("已处理: {}", description))
        })
    }
}

/// 会失败几次后成功的 Executor（演示重规划）
struct FlakyExecutor {
    /// 记录每次调用是否应该成功
    should_succeed: Vec<bool>,
    call_count: Arc<AtomicUsize>,
}

impl FlakyExecutor {
    fn new(should_succeed: Vec<bool>) -> Self {
        Self {
            should_succeed,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Executor for FlakyExecutor {
    fn execute_step<'a>(
        &'a mut self,
        description: &'a str,
        _context: &'a str,
    ) -> futures::future::BoxFuture<'a, echo_agent::error::Result<String>> {
        Box::pin(async move {
            let call_num = self.call_count.fetch_add(1, Ordering::SeqCst);
            let should_succeed = self.should_succeed.get(call_num).copied().unwrap_or(true);

            println!("  ⚡ [调用 #{:?}] 执行: {}", call_num + 1, description);

            if should_succeed {
                println!("  ✓ 执行成功\n");
                Ok(format!("{}: 成功完成", description))
            } else {
                println!("  ✗ 执行失败 (将触发重规划)\n");
                Err(echo_agent::error::ReactError::Agent(
                    echo_agent::error::AgentError::NoResponse,
                ))
            }
        })
    }
}
