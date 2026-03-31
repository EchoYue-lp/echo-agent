//! Plan-and-Execute 示例
//!
//! 演示 Plan-and-Execute 引擎的使用，与 ReAct 对比。
//!
//! ```bash
//! cargo run --example demo22_plan_execute
//! ```

use echo_agent::plan_execute::{LlmPlanner, PlanExecuteAgent, ReactExecutor};
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("=== Plan-and-Execute 示例 ===\n");

    // 创建 Planner（使用 LLM 生成计划）
    let planner = LlmPlanner::new("qwen3-max");

    // 创建 Executor（使用 ReactAgent 执行每个步骤）
    let executor_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("executor")
        .system_prompt("你是一个任务执行助手。请直接执行给定的步骤，返回结果。")
        .build()?;
    let executor = ReactExecutor::new(executor_agent);

    // 组装 Plan-and-Execute Agent
    let mut agent = PlanExecuteAgent::new("plan_execute_agent", planner, executor).max_replans(2);

    println!("--- 执行复杂任务 ---\n");
    let result = agent
        .execute("请分析'人工智能'这个主题，列出三个关键发展方向，并为每个方向给出一句话总结。")
        .await?;

    println!("\n最终结果:\n{}", result);
    println!("\n=== Plan-and-Execute 示例完成 ===");
    Ok(())
}
