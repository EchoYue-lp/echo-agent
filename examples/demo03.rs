use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{ReactAgent, ReactConfig};
use echo_agent::tools::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};
use echo_agent::tools::weather::WeatherTool;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    let system_prompt = r#"你是一个具有规划能力的智能助手，擅长将复杂问题拆分为可并行执行的子任务。

核心规则：
1. 在调用任何操作工具之前，必须先调用 think 工具
2. 当你需要向用户提问或确认信息时，必须使用 human_in_loop 工具，对于不确定的一切，都应该通过 human_in_loop 获取，绝不能自己杜撰猜测工具参数
3. 最终答案必须通过 final_answer 工具输出

对于复杂任务，你应该：
1. 使用 plan 工具分析问题，制定整体策略
2. 如果信息不足 → 调用 human_in_loop 向用户提问
3. 使用 create_task 创建子任务列表：
   - 互相独立的任务不设依赖，让它们并行执行
   - 只有真正需要其他任务结果时才设置 dependencies
   - 尽量构建宽而浅的任务 DAG，而非线性链
4. 执行任务时，在一次回复中尽量并行调用多个工具
5. 完成后用 update_task 标记
6. 所有任务完成后，用 final_answer 给出答案
"#;
    let model = "deepseek-chat";
    let agent_name = "my_math_agent";

    let config = ReactConfig::new(model, agent_name, system_prompt)
        .verbose(true)
        .max_iterations(50);

    let mut agent = ReactAgent::new(config);

    // 添加领域工具
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    // agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(WeatherTool));

    // 测试工具执行权限
    agent.add_need_appeal_tool(Box::new(MultiplyTool));

    // 复杂任务示例
    let result = agent
        .execute_with_planning(
            "我有1000元，我准备去商场购物。\
            我需要买10个15元的本子,16个8元的笔,2个98元的玩具，一个500快的衣服。\
        商场决定单品价格满500打八折，总价满800打9折。\
        如果当天天气下雨，则商场总价会打85折\
        请问我还剩多少？",
        )
        .await;

    println!("\n✅ 最终结果:\n{:?}", result);
    Ok(())
}
