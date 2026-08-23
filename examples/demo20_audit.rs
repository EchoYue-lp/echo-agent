//! 审计追踪 + 权限服务示例
//!
//! 演示 PermissionService 管线的审计记录：
//! - InMemoryPermissionAuditSink：内存环形缓冲区
//! - LoggingPermissionAuditSink：tracing 日志
//! - CompositePermissionAuditSink：组合多个 Sink
//! - 查询审计记录：recent() / query()
//!
//! ```bash
//! RUST_LOG=info cargo run --example demo20_audit
//! ```

use echo_agent::human_loop::{
    CompositePermissionAuditSink, HumanLoopEvent, HumanLoopManager, InMemoryPermissionAuditSink,
    LoggingPermissionAuditSink, PermissionService,
};
use echo_agent::prelude::*;
use echo_agent::tools::ToolResult;
use echo_agent::tools::permission::{PermissionRule, RuleMatcher, RuleSource};
use futures::future::BoxFuture;
use std::sync::Arc;

/// 一个声明了 Execute 权限的工具（模拟系统命令执行）
struct RunCommandTool;

impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "执行系统命令（需要 Execute 权限）"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "要执行的命令"
                }
            },
            "required": ["command"]
        })
    }

    fn execute(
        &self,
        params: ToolParameters,
    ) -> BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        Box::pin(async move {
            let cmd = params
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("echo hello");
            Ok(ToolResult::success(format!("模拟执行: {cmd}")))
        })
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Execute]
    }
}

/// 一个无需特殊权限的安全工具
struct SafeTool;

impl Tool for SafeTool {
    fn name(&self) -> &str {
        "get_time"
    }

    fn description(&self) -> &str {
        "获取当前时间（无特殊权限要求）"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    fn execute(
        &self,
        _params: ToolParameters,
    ) -> BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        Box::pin(async { Ok(ToolResult::success("2026-04-05 12:00:00".to_string())) })
    }
}

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // ── 1. 创建审计 Sink ───────────────────────────────────────────────

    // 内存 Sink：保留最近 100 条审计记录
    let mem_sink = Arc::new(InMemoryPermissionAuditSink::new(100));

    // 日志 Sink：通过 tracing 输出
    let log_sink = Arc::new(LoggingPermissionAuditSink::info());

    // 组合 Sink：同时写入内存和日志
    let composite = Arc::new(CompositePermissionAuditSink::new(vec![
        Box::new(mem_sink.clone()) as _,
        Box::new(log_sink) as _,
    ]));

    // ── 2.1 创建自动批准的 HumanLoop Provider ─────────────────────────────
    let manager = Arc::new(HumanLoopManager::new());
    let mgr = manager.clone();
    tokio::spawn(async move {
        while let Some(event) = mgr.recv_event().await {
            match event {
                HumanLoopEvent::ApprovalRequest {
                    tool_name,
                    responder,
                    ..
                } => {
                    println!("自动批准敏感工具审批: {tool_name}");
                    responder.approve();
                }
                HumanLoopEvent::InputRequest { responder, .. } => {
                    responder.respond("无需额外输入".to_string());
                }
                _ => {
                    // SelectionRequest and other variants — not used by this demo.
                }
            }
        }
    });

    // ── 2. 创建带审计的 PermissionService ──────────────────────────────
    let service = Arc::new(
        PermissionService::from_provider(
            manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>
        )
        .with_audit_sink(composite),
    );
    service
        .add_rules(vec![
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "get_time".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "final_answer".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::ask(
                RuleMatcher::Pattern {
                    pattern: "run_command".to_string(),
                },
                vec!["允许".to_string(), "拒绝".to_string()],
                RuleSource::Session,
            ),
        ])
        .await;

    // ── 3. 构建 Agent ──────────────────────────────────────────────────
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("你是一个助手，可以使用工具完成任务。请直接调用工具，不要询问用户。")
        .enable_tools()
        .approval_provider(manager)
        .permission_service(service)
        .tool(Box::new(RunCommandTool))
        .tool(Box::new(SafeTool))
        .build()?;

    // 执行对话：一个安全工具 + 一个敏感工具
    let answer = agent.chat("请获取当前时间").await?;
    println!("Agent: {answer}\n");

    let command_answer = agent
        .chat("请调用 run_command 执行 `echo hello-from-demo20`，并告诉我结果。")
        .await?;
    println!("Agent: {command_answer}\n");

    // ── 4. 查询审计记录 ────────────────────────────────────────────────
    println!("=== 审计记录 ({} 条) ===", mem_sink.count());

    let recent = mem_sink.recent(10);
    for (i, entry) in recent.iter().enumerate() {
        println!(
            "  [{}] {} {} → {} (原因: {}, 来源: {}, {}us)",
            i + 1,
            entry.tool_name,
            entry.args_hash,
            entry.decision,
            entry.reason,
            entry.source,
            entry.duration_us
        );
    }

    // 按条件查询
    let allows = mem_sink.query(|e| e.decision == "allow");
    println!("\n允许的操作: {} 条", allows.len());
    let approval_requests = mem_sink.query(|e| e.decision == "ask");
    println!("需要审批的操作: {} 条", approval_requests.len());

    if allows.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo20 验收失败：未记录到任何 allow 审计事件".to_string(),
        ));
    }
    if approval_requests.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo20 验收失败：未记录到任何 ask 审计事件".to_string(),
        ));
    }

    Ok(())
}
