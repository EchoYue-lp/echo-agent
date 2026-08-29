//! Workflow / Pipeline 抽象示例
//!
//! 演示三种工作流编排模式：
//! 1. SequentialWorkflow — 顺序管道
//! 2. ConcurrentWorkflow — 并发管道
//! 3. DagWorkflow — DAG 管道
//!
//! ```bash
//! cargo run -p echo-agent-learning --example demo28_workflow
//! ```

use echo_agent::prelude::*;
use echo_agent::workflow::{ConcurrentWorkflow, DagWorkflow, SequentialWorkflow, Workflow};

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,echo_agent::workflow=debug")
        .init();

    // ── 示例 1：顺序工作流 ────────────────────────────────────────────────
    println!("=== 示例 1：SequentialWorkflow ===\n");
    {
        let translator = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是翻译，将用户输入翻译成英文，只输出翻译结果",
        )?;
        let reviewer = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是校对，检查英文翻译的准确性并给出修改建议，输出最终翻译",
        )?;

        let mut wf = SequentialWorkflow::builder()
            .step(translator)
            .step(reviewer)
            .build();

        let output = wf.run("人工智能正在改变世界").await?;
        println!("最终结果: {}", output.result);
        println!("总耗时: {:?}", output.elapsed);
        for (i, s) in output.steps.iter().enumerate() {
            println!(
                "  步骤 {} [{}] ({:?}): {}",
                i + 1,
                s.agent_name,
                s.elapsed,
                s.output.chars().take(120).collect::<String>()
            );
        }
    }

    println!("\n");

    // ── 示例 2：并发工作流 ────────────────────────────────────────────────
    println!("=== 示例 2：ConcurrentWorkflow ===\n");
    {
        let tech_analyst = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是技术分析师，从技术可行性角度简要分析（3句话以内）",
        )?;
        let biz_analyst = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是商业分析师，从市场与商业模式角度简要分析（3句话以内）",
        )?;

        let mut wf = ConcurrentWorkflow::builder()
            .agent(tech_analyst)
            .agent(biz_analyst)
            .merge(|results| {
                let mut merged = String::from("=== 综合分析报告 ===\n");
                for (i, r) in results.iter().enumerate() {
                    merged.push_str(&format!("\n--- 视角 {} ---\n{}\n", i + 1, r));
                }
                merged
            })
            .build();

        let output = wf.run("评估 AI Agent 开发框架的发展前景").await?;
        println!("合并结果:\n{}", output.result);
        println!("总耗时: {:?}", output.elapsed);
    }

    println!("\n");

    // ── 示例 3：DAG 工作流 ────────────────────────────────────────────────
    println!("=== 示例 3：DagWorkflow ===\n");
    {
        let researcher = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是调研员，针对用户话题收集关键信息要点（5个要点以内）",
        )?;
        let analyst = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是数据分析师，对用户话题提供数据驱动的洞察（3个洞察以内）",
        )?;
        let writer = ReactAgentBuilder::simple(
            "qwen3-max",
            "你是报告撰写者，根据输入的调研和分析结果撰写一份简短的综合报告",
        )?;

        let mut wf = DagWorkflow::builder()
            .node("research", researcher)
            .node("analyze", analyst)
            .node("write", writer)
            .edge("research", "write")
            .edge("analyze", "write")
            .build()?;

        let output = wf.run("Rust 在 AI 领域的应用前景").await?;
        println!("DAG 最终报告:\n{}", output.result);
        println!("总耗时: {:?}", output.elapsed);
        for s in &output.steps {
            println!("  节点 [{}] ({:?})", s.agent_name, s.elapsed);
        }
    }

    Ok(())
}
