use echo_agent::agent::ReactAgentBuilder;
use echo_core::agent::Agent;

#[tokio::main]
async fn main() -> echo_core::error::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("🧪 demo01 - 工具调用演示\n");

    let system_prompt = r#"你是一个助手。

可用工具：add / subtract / multiply / divide - 执行数学计算

完成后通过 final_answer 报告结果。
"#;

    let agent = ReactAgentBuilder::new()
        .model("deepseek-chat")
        .name("my_math_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .max_iterations(10)
        .build()?;

    // #[tool] 宏生成的工具

    let result = agent
        .execute("你是谁？哪家公司出品的？上下文是多少？那个版本？知识库截止日期是什么时候？")
        .await?;
    println!("\n📋 最终结果:\n{:?}", result);

    Ok(())
}
