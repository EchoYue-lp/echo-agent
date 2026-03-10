//! demo05 - 综合能力演示
//!
//! 本示例将框架的四项核心能力整合到一个真实场景中：
//!
//! - **工具调用**：数学运算工具（加减乘除）
//! - **任务规划**：将复杂费用计算拆分为并行子任务
//! - **Human-in-Loop**：每人分摊金额（divide）执行前需人工确认
//! - **上下文压缩**：滑动窗口自动管理长对话的 token 用量

use echo_agent::compression::compressor::SlidingWindowCompressor;
use echo_agent::prelude::*;
use echo_agent::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🧪 demo05 - 综合能力演示（工具 + 任务规划 + 上下文压缩 + Human-in-Loop）\n");

    let system_prompt = r#"你是一个出差费用核算助手，需要综合运用以下所有能力完成任务。

**工作流程**：
1. 先分析费用结构，再用 plan + create_task 将各费用类别拆成并行子任务
2. 独立的费用计算任务不设依赖，可以并行执行；汇总类任务依赖所有费用任务
3. 使用 add / subtract / multiply 工具完成计算，用 update_task 记录结果
4. 所有费用算出后，使用 divide 工具计算人均分摊金额
5. 最后用 final_answer 给出完整的费用报告

**重要规则**：
- 每次回复尽可能并行调用多个工具
- divide 工具执行前需要人工确认（这是设计行为，请正常调用，系统会自动弹出确认）
- 报告中需包含：各类总额、费用总计、人均分摊、与预算的差额
"#;

    // 使用 AgentBuilder 创建 Agent（全功能配置）
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("expense_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .enable_planning()
        .enable_human_in_loop()
        .token_limit(3000)
        .max_iterations(40)
        .build()?;

    // 普通数学工具（无需审批）
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    // divide 工具标记为"需要人工审批"：每次调用前终端会弹出 y/n 确认
    agent.add_need_appeal_tool(Box::new(DivideTool));

    // 滑动窗口压缩：超限后保留最近 20 条消息
    agent.set_compressor(SlidingWindowCompressor::new(20));

    let task = r#"我们团队 5 人去北京出差 3 天，请帮我核算本次出差费用：

【住宿】每晚每人 380 元，共 3 晚
【餐饮】每人每天 120 元，共 3 天
【交通】往返机票每人 1350 元；市内交通全团总计 420 元
【会议】会议室租金 800 元，设备租金 250 元
【公司预算】本次出差审批总预算为 25000 元

请计算：
1. 住宿、餐饮、交通、会议各类费用的团队总额
2. 本次出差总费用
3. 每人平均分摊金额（调用 divide 时系统会请你确认）
4. 与预算的差额（超支或结余）"#;

    let result = agent.execute(task).await;

    println!("\n✅ 最终结果:\n{:?}", result);
    Ok(())
}
