use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{ReactAgent, ReactConfig};
use echo_agent::tools::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    let system_prompt = r#"你是一个具有规划能力的智能助手，擅长将复杂问题拆分为可并行执行的子任务。

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
    let model = "qwen3-max";
    let agent_name = "my_math_agent";

    let config = ReactConfig::new(model, agent_name, system_prompt)
        .verbose(true)
        .max_iterations(30);

    let mut agent = ReactAgent::new(config);

    // 添加领域工具
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    // 复杂任务示例
    let result = agent
        .execute_with_planning("我有1000元，买了10个15元的本子,16个8元的笔,2个98元的玩具，一个500快的衣服。商场决定单品价格满500打八折，总价满800打9折，还剩多少？")
        .await;

    println!("\n✅ 最终结果:\n{:?}", result);
    Ok(())
}
