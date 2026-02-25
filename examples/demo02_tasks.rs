use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    let system_prompt = r#"你是一个具有规划能力的智能助手，本示例用于测试任务规划与任务状态流转。

对于复杂任务，你应该：
1. 使用 plan 工具分析问题，制定整体策略
2. 使用 create_task 创建子任务列表：
   - 互相独立的任务不设依赖，让它们并行执行
   - 只有真正需要其他任务结果时才设置 dependencies
   - 尽量构建宽而浅的任务 DAG，而非线性链
3. 执行任务时，在一次回复中尽量并行调用多个工具
4. 完成后用 update_task 标记
5. 所有任务完成后，用 final_answer 给出答案
"#;
    let config = AgentConfig::new("qwen3-max", "planning_agent", system_prompt)
        .enable_tool(true)
        .enable_task(true)
        .enable_human_in_loop(false)
        .enable_subagent(false)
        .verbose(true)
        .max_iterations(30);

    let mut agent = ReactAgent::new(config);

    // 添加领域工具（保持纯数学，避免 human-in-loop/天气干扰）
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(DivideTool));

    // 任务规划示例
    let result = agent
        .execute(
            "我有 1200 元。买了 8 个 18 元的本子、12 支 9 元的笔、3 个 120 元的玩具、1 件 400 元外套。\
            先计算原价总和，再对总价打 95 折，最后算剩余金额。",
        )
        .await;

    println!("\n✅ 最终结果:\n{:?}", result);
    Ok(())
}
