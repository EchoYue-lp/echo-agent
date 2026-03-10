use echo_agent::prelude::*;
use echo_agent::tools::others::weather::WeatherTool;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let system_prompt = r#"你是一个天气助手，本示例用于测试 human-in-loop 交互流程。

核心规则：
1. 先调用 human_in_loop 向用户确认城市和日期，再调用 query_weather。
2. 最终答案必须通过 final_answer 工具输出。
"#;

    // 使用 AgentBuilder 创建 Agent（启用人工介入）
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("human_loop_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .enable_human_in_loop()
        .max_iterations(50)
        .build()?;

    // 只保留天气工具，聚焦 human-in-loop 能力
    agent.add_tool(Box::new(WeatherTool));

    // human-in-loop 示例：用户故意不给完整参数，要求 agent 主动追问
    let result = agent.execute("帮我查天气，并告诉我要不要带伞。").await;

    println!("\n✅ 最终结果:\n{:?}", result);
    Ok(())
}
