//! demo01: 工具调用能力演示（不包含规划 / human-in-loop / subagent）

use echo_agent::error::Result;
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("🧪 demo01 - 工具调用演示\n");

    let system_prompt = r#"你是一个计算助手，本示例只用于测试工具调用。

可用工具：add / subtract / multiply / divide - 执行数学计算

完成后通过 final_answer 报告结果。
"#;

    // 使用 AgentBuilder 创建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("my_math_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .max_iterations(10)
        .build()?;

    // 注册工具
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    let result = agent
        .execute("计算 (12 / 3) + (2 * 8) + (6 * 4) + 2")
        .await?;
    println!("\n📋 最终结果:\n{:?}", result);

    Ok(())
}
