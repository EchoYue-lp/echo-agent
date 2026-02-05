use demo_react::agent::Agent;
use demo_react::agent::react_agent::{ReactAgent, ReactConfig};
use demo_react::tools::math::{AddTool, MultiplyTool, SubtractTool};

// examples/planning_agent_demo.rs
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let system_prompt = r#"你是一个具有规划能力的智能助手。

对于复杂任务，你应该：
1. 使用 plan 工具分析问题，制定整体策略
2. 使用 create_task 创建子任务列表（设置依赖关系和优先级）
3. 使用 list_tasks 查看任务状态
4. 逐个执行任务，完成后用 update_task 标记
5. 所有任务完成后，用 final_answer 给出答案

然后按顺序执行这些任务。
"#;

    let config = ReactConfig::new("planner", "middle", system_prompt)
        .verbose(true)
        .max_iterations(20);

    let mut agent = ReactAgent::new(config);

    // 添加领域工具
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    // 复杂任务示例
    let result = agent
        .execute_with_planning("我有100元，买了3个15元的本子和2个8元的笔，还剩多少？")
        .await?;

    println!("\n✅ 最终结果:\n{}", result);
    Ok(())
}
