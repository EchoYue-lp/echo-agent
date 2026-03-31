//! demo01: 工具调用能力演示（不包含规划 / human-in-loop / subagent）
//!
//! 同时展示两种工具定义方式：
//! - `#[tool]` 宏（推荐）
//! - 手动 `impl Tool`（原有方式，仍然可用）

use echo_agent::error::Result;
use echo_agent::prelude::*;
use echo_agent::tool;
use echo_agent::tools::others::math::{MultiplyTool, SubtractTool};

#[tool(name = "add", description = "两数相加")]
async fn add(
    /// 第一个数
    a: f64,
    /// 第二个数
    b: f64,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} + {} = {}", a, b, a + b)))
}

#[tool(name = "divide", description = "两数相除")]
async fn divide(
    /// 被除数
    a: f64,
    /// 除数（不能为 0）
    b: f64,
) -> Result<ToolResult> {
    if b == 0.0 {
        return Ok(ToolResult::error("除数不能为 0".to_string()));
    }
    Ok(ToolResult::success(format!("{} / {} = {}", a, b, a / b)))
}

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

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("my_math_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .max_iterations(10)
        .build()?;

    // #[tool] 宏生成的工具
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(DivideTool));
    // 原有手动 impl Tool 的工具
    agent.add_tool(Box::new(MultiplyTool));
    agent.add_tool(Box::new(SubtractTool));

    let result = agent
        .execute("计算 (12 / 3) + (2 * 8) + (6 * 4) + 2")
        .await?;
    println!("\n📋 最终结果:\n{:?}", result);

    Ok(())
}
