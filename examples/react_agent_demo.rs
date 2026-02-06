use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{ReactAgent, ReactConfig};
use echo_agent::tools::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};
use echo_agent::tools::weather::WeatherTool;

/// ReAct 智能体完整演示
///
/// 展示如何使用 ReAct 智能体完成任务

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧠 ReAct 智能体完整演示\n");

    let system_prompt = r#"你是一个使用 ReAct 框架的智能助手。

**核心规则：**
1. 在调用任何操作工具之前，必须先调用 think 工具
2. 当你需要向用户提问或确认信息时，必须使用 human_in_loop 工具，绝不要直接输出文字来提问
3. 最终答案必须通过 final_answer 工具输出

标准流程：
1. 调用 think 分析问题
2. 如果信息不足 → 调用 human_in_loop 向用户提问
3. 信息充足 → 调用操作工具
4. 得到结果后调用 think 分析
5. 完成后调用 final_answer 输出最终答案
"#;

    let config = ReactConfig::new("math_agent", "middle", system_prompt).verbose(true);

    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(WeatherTool));

    agent.add_danger_tool(Box::new(DivideTool));

    let result = agent.execute("后天天气如何？温度多少度？").await?;
    println!("\n📋 最终结果:\n{}", result);

    Ok(())
}
