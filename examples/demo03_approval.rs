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

use echo_agent::human_loop::{
    HumanLoopEvent, HumanLoopManager, PermissionService, SessionApprovalCache,
};
use echo_agent::prelude::*;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // ── 1. 创建事件驱动的审批管理器 ──────────────────────────────────────
    let manager = Arc::new(HumanLoopManager::new());

    // 后台事件处理：自动批准所有请求（生产环境应接入真实 UI）
    let mgr = manager.clone();
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
                    println!("🔔 审批请求: {}", prompt);
                    println!("   工具: {} | 参数: {:?}", tool_name, args);
                    println!("   风险: {:?} → 自动批准", risk_level);
                    responder.approve();
                }
                HumanLoopEvent::InputRequest { prompt, responder } => {
                    println!("💬 输入请求: {}", prompt);
                    responder.respond("示例输入".to_string());
                }
            }
        }
    });

    // ── 2. 创建带 TTL 的审批缓存 ────────────────────────────────────────
    let _cache = SessionApprovalCache::with_ttl(Duration::from_secs(30 * 60));

    // ── 3. 从 Provider 创建 PermissionService ──────────────────────────
    let permission_service = Arc::new(
        PermissionService::from_provider(manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>)
    );

    // ── 4. 构建 Agent ──────────────────────────────────────────────────
    let system_prompt = r#"你是一个助手，可以使用工具完成任务。
请直接调用工具，不要询问用户。"#;

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("approval_demo_agent")
        .system_prompt(system_prompt)
        .enable_tools()
        .enable_human_in_loop()
        .max_iterations(10)
        .permission_service(permission_service)
        .build()?;

    // ── 5. 执行任务 ────────────────────────────────────────────────────
    let result = agent.chat("获取当前时间").await;
    match result {
        Ok(answer) => println!("\n✅ 结果: {}", answer),
        Err(e) => println!("\n❌ 错误: {:?}", e),
    }

    Ok(())
}
