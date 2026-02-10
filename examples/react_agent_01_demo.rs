use echo_agent::agent::Agent;
use echo_agent::agent::react_agent::{ReactAgent, ReactConfig};
use echo_agent::tools::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

/// ReAct 智能体完整演示
///
/// 展示如何使用 ReAct 智能体完成任务

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧠 ReAct 智能体完整演示\n");

    let system_prompt = r#"你是一个使用 ReAct 框架的智能助手。

**核心规则：在调用任何操作工具之前，必须先调用 think 工具！**

可用工具：
- think: 记录你的推理过程（必须首先调用）
- add/subtract/multiply/divide: 执行计算

标准流程：
1. 调用 think(reasoning="我的分析...") 记录思考
2. 调用实际的操作工具
3. 得到结果后，再次调用 think 分析结果
4. 重复直到问题解决

"#;
    let model = "qwen3-max";
    let agent_name = "my_math_agent";

    let config = ReactConfig::new(model, agent_name, system_prompt).verbose(true);

    let mut agent = ReactAgent::new(config);

    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    let result = agent
        .execute("计算 12 除以 3 + 2 +2 * 8 + 2 + 6 乘以 4 等于多少？")
        .await;
    println!("\n📋 最终结果:\n{:?}", result);

    Ok(())
}
