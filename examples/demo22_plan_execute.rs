//! demo22_plan_execute —— 面向真实使用的 Plan-and-Execute 示例
//!
//! 这个示例优先演示推荐写法：
//! - `LlmPlanner` 负责把用户任务拆成结构化步骤
//! - `ReactExecutor` 负责逐步执行与汇总
//! - 仅演示真实 LLM 路径，不做 mock/offline 回退
//!
//! ```bash
//! cargo run --example demo22_plan_execute
//! ```

use echo_agent::agent::plan_execute::{LlmPlanner, Plan, PlanExecuteAgent, Planner, ReactExecutor};
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter("echo_agent::plan_execute=info,demo22=info")
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("      Plan-and-Execute 实战示例 (demo22)");
    println!("═══════════════════════════════════════════════════════\n");

    demo_recommended_shape();
    demo_practical_plan_execute().await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("      demo22 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}

fn demo_recommended_shape() {
    println!("─────────────────────────────────────────────");
    println!("Part 1: 推荐使用方式");
    println!("─────────────────────────────────────────────\n");

    println!("  推荐组合:");
    println!("    1. 用 `LlmPlanner` 生成结构化计划");
    println!("    2. 用 `ReactExecutor` 包装一个执行 Agent");
    println!("    3. 直接调用 `PlanExecuteAgent::execute(task)`");
    println!("    4. 保持真实模型、真实规划、真实执行，不做假回退\n");
}

async fn demo_practical_plan_execute() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 2: 端到端任务编排");
    println!("─────────────────────────────────────────────\n");

    let task = "为一个有 Python 基础、每天可投入 1 小时的人，制定一份 3 天的 Rust 入门学习行动计划，要求每天都有学习重点、练习方式和交付结果。";
    println!("  任务:\n  {task}\n");

    run_live_plan_execute(task).await
}

async fn run_live_plan_execute(task: &str) -> echo_agent::error::Result<()> {
    println!("  模式: 真实 LlmPlanner + ReactExecutor\n");

    let preview_plan = build_live_planner().plan(task).await?;
    print_plan_preview(&preview_plan);

    let executor_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("plan-executor")
        .system_prompt(
            "你是计划执行助手。你每次只执行当前这一个步骤，并基于已有上下文输出简洁、可复用的中间结果或最终总结。",
        )
        .max_iterations(6)
        .build()?;

    let mut agent = PlanExecuteAgent::new(
        "practical-plan-execute",
        build_live_planner(),
        ReactExecutor::new(executor_agent),
    )
    .disable_replan();

    let result = agent.execute(task).await?;
    ensure_non_empty_result("demo22", &result)?;

    println!("  ✓ 最终结果:\n{result}\n");
    Ok(())
}

fn build_live_planner() -> LlmPlanner {
    LlmPlanner::new("qwen3-max").with_system_prompt(
        "你是一个任务规划专家。请把用户目标拆成 3-5 个真正可执行的步骤。\
每一步都要足够具体，并尽量体现顺序依赖。\
不要输出空泛口号，不要把“继续思考”写成步骤。",
    )
}

fn print_plan_preview(plan: &Plan) {
    println!("  计划预览:");
    for (idx, step) in plan.steps.iter().enumerate() {
        if step.dependencies.is_empty() {
            println!("    {}. {}", idx + 1, step.description);
        } else {
            println!(
                "    {}. {}  (依赖: {:?})",
                idx + 1,
                step.description,
                step.dependencies
            );
        }
    }
    println!();
}

fn ensure_non_empty_result(label: &str, result: &str) -> echo_agent::error::Result<()> {
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "{label} 验收失败：最终结果为空"
        ))
        .into());
    }
    Ok(())
}
