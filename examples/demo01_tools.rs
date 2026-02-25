use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{AgentConfig, ReactAgent};
use echo_agent::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

/// demo01: 工具调用能力演示（不包含规划 / human-in-loop / subagent）

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("🧪 demo01 - 工具调用演示\n");

    let system_prompt = r#"你是一个计算助手，本示例只用于测试工具调用。

**核心规则：在调用任何操作工具之前，必须先调用 think 工具！**

可用工具：
- think: 记录你的推理过程（必须首先调用）
- add/subtract/multiply/divide: 执行计算

标准流程（本 demo 仅验证工具链路）：
1. 调用 think(reasoning="我的分析...") 记录思考
2. 调用实际的操作工具
3. 得到结果后，再次调用 think 分析结果
4. 重复直到问题解决

"#;
    let config = AgentConfig::new("qwen3-max", "my_math_agent", system_prompt)
        .enable_tool(true)
        .enable_task(false)
        .enable_human_in_loop(false)
        .enable_subagent(false)
        .verbose(true);

    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    let result = agent.execute("计算 (12 / 3) + (2 * 8) + (6 * 4) + 2").await;
    println!("\n📋 最终结果:\n{:?}", result);

    Ok(())
}
