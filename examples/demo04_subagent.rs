use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use echo_agent::human_loop::{HumanLoopEvent, HumanLoopManager};
use echo_agent::prelude::*;
use echo_agent::testing::MockLlmClient;
use echo_agent::tool;

#[tool(name = "add", description = "两数相加")]
async fn add(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} + {} = {}", a, b, a + b)))
}

#[tool(name = "subtract", description = "两数相减")]
async fn subtract(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} - {} = {}", a, b, a - b)))
}

#[tool(name = "multiply", description = "两数相乘")]
async fn multiply(a: f64, b: f64) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{} * {} = {}", a, b, a * b)))
}

#[tool(name = "divide", description = "两数相除")]
async fn divide(a: f64, b: f64) -> Result<ToolResult> {
    if b == 0.0 {
        return Ok(ToolResult::error("除数不能为 0".to_string()));
    }
    Ok(ToolResult::success(format!("{} / {} = {}", a, b, a / b)))
}

fn math_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(AddTool),
        Box::new(SubtractTool),
        Box::new(MultiplyTool),
        Box::new(DivideTool),
    ]
}

fn spawn_human_loop_worker(
    manager: Arc<HumanLoopManager>,
    input_count: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = manager.recv_event().await {
            match event {
                HumanLoopEvent::ApprovalRequest {
                    tool_name,
                    prompt,
                    responder,
                    ..
                } => {
                    println!("🔔 审批请求: {tool_name} -> {prompt}");
                    responder.approve();
                }
                HumanLoopEvent::InputRequest { prompt, responder } => {
                    input_count.fetch_add(1, Ordering::Relaxed);
                    println!("💬 Human-in-the-loop: {prompt}");
                    responder.respond("用户补充：今天下雨，请按雨天折扣计算。".to_string());
                }
                _ => {
                    // SelectionRequest and other variants — not used by this demo.
                }
            }
        }
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🧪 demo04 - SubAgent 编排 + Human-in-the-Loop 演示\n");

    let human_loop = Arc::new(HumanLoopManager::new());
    let input_count = Arc::new(AtomicUsize::new(0));
    let worker = spawn_human_loop_worker(human_loop.clone(), input_count.clone());

    let weather_llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("weather-agent-mock")
            .then_tool_call(
                "weather-hitl-1",
                "human_in_loop",
                r#"{
                    "reasoning":"用户没有说明购物当天是否下雨，无法决定是否触发雨天 85 折。",
                    "approval_type":"LLM",
                    "tool":"weather-context"
                }"#,
            )
            .then_tool_call(
                "weather-final-1",
                "final_answer",
                r#"{"answer":"已通过 human_in_loop 向用户确认：今天下雨，因此后续计算需要叠加雨天 85 折。"}"#,
            ),
    );

    let math_llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("math-agent-mock")
            .then_tool_call("math-1", "multiply", r#"{"a":10,"b":15}"#)
            .then_tool_call("math-2", "multiply", r#"{"a":16,"b":8}"#)
            .then_tool_call("math-3", "multiply", r#"{"a":2,"b":98}"#)
            .then_tool_call("math-4", "multiply", r#"{"a":500,"b":0.8}"#)
            .then_tool_call("math-5", "add", r#"{"a":150,"b":128}"#)
            .then_tool_call("math-6", "add", r#"{"a":278,"b":196}"#)
            .then_tool_call("math-7", "add", r#"{"a":474,"b":400}"#)
            .then_tool_call("math-8", "multiply", r#"{"a":874,"b":0.9}"#)
            .then_tool_call("math-9", "multiply", r#"{"a":786.6,"b":0.85}"#)
            .then_tool_call("math-10", "subtract", r#"{"a":1000,"b":668.61}"#)
            .then_tool_call(
                "math-final-1",
                "final_answer",
                r#"{"answer":"计算结果：本子 150 元，笔 128 元，玩具 196 元，衣服满 500 打八折后为 400 元；小计 874 元，满 800 打九折后为 786.6 元；已确认下雨，再打 85 折，实付 668.61 元，最终剩余 331.39 元。"}"#,
            ),
    );

    let orchestrator_llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("orchestrator-mock")
            .then_tool_call(
                "main-1",
                "agent_tool",
                r#"{
                    "agent_name":"weather-agent",
                    "task":"先向用户确认购物当天是否下雨，并只返回与折扣判断相关的结论。"
                }"#,
            )
            .then_tool_call(
                "main-2",
                "agent_tool",
                r#"{
                    "agent_name":"math-agent",
                    "task":"预算是 1000 元。已确认今天下雨。请按先单品折扣、再满减折扣、最后雨天折扣的顺序计算剩余金额，并返回清晰的计算结论。"
                }"#,
            )
            .then_tool_call(
                "main-final-1",
                "final_answer",
                r#"{"answer":"weather-agent 先通过 human_in_loop 完成了天气澄清，确认今天下雨；math-agent 随后完成折扣计算，最终实付 668.61 元，还剩 331.39 元。"}"#,
            ),
    );

    let weather_agent = ReactAgentBuilder::new()
        .name("weather-agent")
        .model("weather-agent-mock")
        .system_prompt(
            "你负责识别是否缺少天气上下文。若用户未说明是否下雨，必须先调用 human_in_loop 获取补充信息，再给出简洁结论。",
        )
        .enable_tools()
        .enable_human_in_loop()
        .approval_provider(human_loop.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>)
        .llm_client(weather_llm)
        .max_iterations(6)
        .build()?;

    let mut math_agent = ReactAgentBuilder::new()
        .name("math-agent")
        .model("math-agent-mock")
        .system_prompt("你是折扣计算专家，只使用数学工具逐步计算，再给出最终金额。")
        .enable_tools()
        .llm_client(math_llm)
        .max_iterations(16)
        .build()?;
    math_agent.add_tools(math_tools());

    let mut main_agent = ReactAgentBuilder::new()
        .model("orchestrator-mock")
        .name("main_agent")
        .system_prompt(
            "你是主编排 Agent。遇到缺失上下文的专业问题先分派给对应 subagent，再汇总结果，不要自己直接计算。",
        )
        .role(AgentRole::Orchestrator)
        .enable_subagent()
        .llm_client(orchestrator_llm)
        .max_iterations(8)
        .enable_tools()
        .build()?;

    main_agent.register_agent(Box::new(weather_agent));
    main_agent.register_agent(Box::new(math_agent));

    let result = main_agent
        .execute(
            "我有1000元，要买 10 个 15 元的本子、16 个 8 元的笔、2 个 98 元的玩具和一件 500 元的衣服。单品满 500 打八折，总价满 800 打九折；如果当天下雨，总价再打 85 折。请问我还剩多少？",
        )
        .await?;

    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo04 验收失败：SubAgent 编排示例返回空结果".to_string(),
        ));
    }
    if input_count.load(Ordering::Relaxed) == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "demo04 验收失败：未触发 human_in_loop 输入请求".to_string(),
        ));
    }
    if !result.contains("331.39") {
        return Err(echo_agent::error::ReactError::Other(
            "demo04 验收失败：最终结果未包含预期剩余金额 331.39".to_string(),
        ));
    }

    println!("\n✅ 最终结果:\n{}", result);

    worker.abort();
    Ok(())
}
