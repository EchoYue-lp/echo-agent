/// ReAct 智能体完整演示
///
/// 展示如何使用 ReAct 智能体完成任务
use echo_ai::react::{ReactAgent, ReactAgentConfig};
use echo_ai::tools::files::ReadFileTool;
use echo_ai::tools::shell::ShellTool;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧠 ReAct 智能体完整演示\n");

    // 1. 创建配置
    let config = ReactAgentConfig {
        max_iterations: 100,
        model: "high".to_string(),
        system_prompt: "You are a helpful coding assistant. \
                        You can read files and execute safe shell commands."
            .to_string(),
        verbose: false, // 启用详细日志
    };

    // 2. 创建智能体
    let mut agent = ReactAgent::new(config);

    // 3. 注册工具
    agent.register_tool(Box::new(ReadFileTool));
    agent.register_tool(Box::new(ShellTool::new()));

    println!("✅ ReAct 智能体已创建");
    println!("✅ 可用工具: {:?}\n", agent.available_tools());

    // 4. 执行任务（示例）
    // 注意：需要真实的 LLM API 才能运行

    let result = agent.run("读取 README.md 文件并总结主要内容").await?;
    println!("\n📋 最终结果:\n{}", result);

    Ok(())
}
