//! 工具审批（Human-in-the-Loop）示例
//!
//! 演示 PermissionService 7 阶段管线的完整审批流程：
//! - 受保护路径检查（.git/.env）
//! - 审批缓存（带 TTL）
//! - 事件驱动审批（HumanLoopManager）
//!
//! ```bash
//! RUST_LOG=info cargo run --example demo03_approval
//! ```

use echo_agent::error::ReactError;
use echo_agent::human_loop::{
    HumanLoopEvent, HumanLoopManager, PermissionService, SessionApprovalCache,
};
use echo_agent::prelude::*;
use echo_agent::tool;
use echo_core::tools::permission::{PermissionRule, RuleMatcher, RuleSource};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tool(name = "current_time", description = "获取当前系统时间")]
async fn current_time() -> Result<ToolResult> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| ReactError::Other(format!("读取系统时间失败: {e}")))?;
    Ok(ToolResult::success(format!(
        "当前 UNIX 时间戳为 {} 秒",
        now.as_secs()
    )))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // ── 1. 创建事件驱动的审批管理器 ──────────────────────────────────────
    let manager = Arc::new(HumanLoopManager::new());
    let approval_count = Arc::new(AtomicUsize::new(0));
    let input_count = Arc::new(AtomicUsize::new(0));

    // 后台事件处理：自动批准所有请求（生产环境应接入真实 UI）
    let mgr = manager.clone();
    let approval_counter = approval_count.clone();
    let input_counter = input_count.clone();
    tokio::spawn(async move {
        while let Some(event) = mgr.recv_event().await {
            match event {
                HumanLoopEvent::ApprovalRequest {
                    tool_name,
                    args,
                    prompt,
                    risk_level,
                    responder,
                } => {
                    approval_counter.fetch_add(1, Ordering::Relaxed);
                    println!("🔔 审批请求: {}", prompt);
                    println!("   工具: {} | 参数: {:?}", tool_name, args);
                    println!("   风险: {:?} → 自动批准", risk_level);
                    responder.approve();
                }
                HumanLoopEvent::InputRequest { prompt, responder } => {
                    input_counter.fetch_add(1, Ordering::Relaxed);
                    println!("💬 输入请求: {}", prompt);
                    responder.respond("提醒对象是产品团队，时间是下周三下午三点".to_string());
                }
            }
        }
    });

    // ── 2. 创建带 TTL 的审批缓存 ────────────────────────────────────────
    let _cache = SessionApprovalCache::with_ttl(Duration::from_secs(30 * 60));

    // ── 3. 从 Provider 创建 PermissionService ──────────────────────────
    let permission_service = Arc::new(PermissionService::from_provider(
        manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>
    ));
    permission_service
        .add_rule(PermissionRule::ask(
            RuleMatcher::Pattern {
                pattern: "current_time".to_string(),
            },
            vec!["允许".to_string(), "拒绝".to_string()],
            RuleSource::Session,
        ))
        .await;

    // ── 4. 构建 Agent ──────────────────────────────────────────────────
    let system_prompt = r#"你是一个助手，可以使用工具完成任务。

规则：
1. 当用户意图不清楚、缺少关键参数、或者你需要额外信息时，必须调用 `human_in_loop` 工具，不要直接猜测，也不要直接在回答里反问用户。
2. 当任务需要调用敏感工具时，正常调用该工具，系统会自动触发人工审批。
3. 完成后用简洁中文总结结果。"#;

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("approval_demo_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .enable_human_in_loop()
        .max_iterations(10)
        .approval_provider(manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>)
        .permission_service(permission_service)
        .build()?;
    agent.add_tool(Box::new(CurrentTimeTool));

    // ── 5. 执行任务 ────────────────────────────────────────────────────
    println!("\n--- Part 1: 意图不清楚 → human_in_loop 输入请求 ---");
    let clarification = agent
        .chat("帮我安排一个会议提醒，但我还没说清楚提醒谁、什么时候提醒。")
        .await?;
    if clarification.trim().is_empty() {
        return Err(ReactError::Other("demo03 验收失败：澄清链路返回空答案".to_string()).into());
    }
    if input_count.load(Ordering::Relaxed) == 0 {
        return Err(ReactError::Other(
            "demo03 验收失败：未触发 human_in_loop 输入请求".to_string(),
        )
        .into());
    }
    println!("\n✅ 澄清结果: {}", clarification);

    println!("\n--- Part 2: 敏感工具调用 → 人工审批 ---");
    let approval_answer = agent.chat("请调用工具获取当前时间。").await?;
    if approval_answer.trim().is_empty() {
        return Err(
            ReactError::Other("demo03 验收失败：审批链路完成后返回空答案".to_string()).into(),
        );
    }
    if approval_count.load(Ordering::Relaxed) == 0 {
        return Err(ReactError::Other("demo03 验收失败：未触发工具审批事件".to_string()).into());
    }
    println!("\n✅ 审批结果: {}", approval_answer);

    Ok(())
}
