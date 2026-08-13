//! demo05 - 综合能力演示
//!
//! 本示例将框架的四项核心能力整合到一个真实场景中：
//!
//! - **工具调用**：数学运算工具（加减乘除）
//! - **任务规划**：将复杂费用计算拆分为并行子任务
//! - **Human-in-Loop**：通过受审批保护的 `divide` 工具验证人工确认链路
//! - **上下文压缩**：滑动窗口自动管理长对话的 token 用量

use echo_agent::compression::compressor::SlidingWindowCompressor;
use echo_agent::human_loop::{
    HumanLoopEvent, HumanLoopManager, PermissionService, SessionApprovalCache,
};
use echo_agent::prelude::*;
use echo_agent::tool;
use echo_core::tools::permission::{PermissionRule, RuleMatcher, RuleSource};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🧪 demo05 - 综合能力演示（工具 + 任务规划 + 上下文压缩 + Human-in-Loop）\n");

    let manager = Arc::new(HumanLoopManager::new());
    let approval_count = Arc::new(AtomicUsize::new(0));
    let confirmation_count = Arc::new(AtomicUsize::new(0));
    let approval_tools = Arc::new(Mutex::new(Vec::<String>::new()));
    let confirmation_prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let mgr = manager.clone();
    let approval_counter = approval_count.clone();
    let confirmation_counter = confirmation_count.clone();
    let approval_tool_list = approval_tools.clone();
    let confirmation_prompt_list = confirmation_prompts.clone();
    tokio::spawn(async move {
        while let Some(event) = mgr.recv_event().await {
            match event {
                HumanLoopEvent::ApprovalRequest {
                    tool_name,
                    responder,
                    ..
                } => {
                    approval_counter.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut tools) = approval_tool_list.lock() {
                        tools.push(tool_name.clone());
                    }
                    println!("🔔 审批请求（demo05）: tool={tool_name}");
                    responder.approve();
                }
                HumanLoopEvent::InputRequest { prompt, responder } => {
                    confirmation_counter.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut prompts) = confirmation_prompt_list.lock() {
                        prompts.push(prompt.clone());
                    }
                    println!("💬 确认请求（demo05）: {prompt}");
                    responder.respond("自动审批输入".to_string())
                }
                _ => {
                    // SelectionRequest and other variants — not used by this demo.
                }
            }
        }
    });
    let _cache = SessionApprovalCache::with_ttl(Duration::from_secs(30 * 60));
    let permission_service = Arc::new(PermissionService::from_provider(
        manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>
    ));
    permission_service
        .add_rules(vec![
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "plan".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "create_task".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "list_tasks".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "visualize_dependencies".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "get_execution_order".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "agent_dispatch".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "add".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "subtract".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "multiply".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "final_answer".to_string(),
                },
                RuleSource::Session,
            ),
        ])
        .await;

    let system_prompt = r#"你是一个出差费用核算助手，需要综合运用以下所有能力完成任务。

**工作流程**：
1. 先分析费用结构，再用 plan + create_task 将各费用类别拆成并行子任务
2. 独立的费用计算任务不设依赖，可以并行执行；汇总类任务依赖所有费用任务
3. 使用 add / subtract / multiply 工具完成计算
4. 所有费用算出后，使用 divide 工具计算人均分摊金额
5. 最后用 final_answer 给出完整的费用报告

**重要规则**：
- 每次回复尽可能并行调用多个工具
- 不要手动调用 update_task 修改任务状态，执行阶段由系统自动推进
- divide 工具执行前需要人工确认（这是设计行为，请正常调用，系统会自动弹出确认）
- 报告中需包含：各类总额、费用总计、人均分摊、与预算的差额
"#;

    // 使用 AgentBuilder 创建 Agent（全功能配置）
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("expense_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .enable_tasks()
        .enable_human_in_loop()
        .token_limit(3000)
        .max_iterations(40)
        .approval_provider(manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>)
        .permission_service(permission_service)
        .build()?;

    // 普通数学工具（无需审批）
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(SubtractTool));
    agent.add_tool(Box::new(MultiplyTool));

    // divide 工具标记为“需要人工审批”：主流程完成后会追加一次聚焦验收，
    // 强制验证 add_need_appeal_tool 的审批链路确实可用。
    agent.add_need_appeal_tool(Box::new(DivideTool));

    // 滑动窗口压缩：超限后保留最近 20 条消息
    agent.set_compressor(SlidingWindowCompressor::new(20)).await;

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

    let result = agent.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo05 验收失败：综合能力示例返回空结果".to_string(),
        ));
    }
    let approval_validation = agent
        .execute(
            "现在只做一件事：使用 divide 工具计算 15720 / 5，\
不要心算，不要改用其他工具，也不要跳过审批。\
拿到 divide 的结果后，再用 final_answer 简短返回该结果。",
        )
        .await?;
    if approval_validation.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo05 验收失败：审批链路验证步骤返回空结果".to_string(),
        ));
    }
    if confirmation_count.load(Ordering::Relaxed) == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "demo05 验收失败：divide 未触发任何人工审批事件".to_string(),
        ));
    }
    let approval_tools = approval_tools
        .lock()
        .map(|tools| tools.clone())
        .unwrap_or_default();
    if approval_tools.iter().any(|name| name != "divide") {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo05 验收失败：出现了非 divide 的审批事件: {:?}",
            approval_tools
        )));
    }
    let confirmation_prompts = confirmation_prompts
        .lock()
        .map(|prompts| prompts.clone())
        .unwrap_or_default();
    if !confirmation_prompts
        .iter()
        .any(|prompt| prompt.contains("divide"))
    {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo05 验收失败：未观察到 divide 的确认请求，实际 prompts: {:?}",
            confirmation_prompts
        )));
    }

    println!("\n✅ 最终结果:\n{}", result);
    println!("\n✅ 审批链路验证:\n{}", approval_validation);
    Ok(())
}
