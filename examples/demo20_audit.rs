//! 审计日志 + 权限模型示例
//!
//! 演示 AuditCallback 自动记录工具调用链，以及 ToolPermission 权限控制。
//!
//! ```bash
//! RUST_LOG=info cargo run --example demo20_audit
//! ```

use echo_agent::prelude::*;
use echo_agent::tools::ToolResult;
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
        Box::pin(async { Ok(ToolResult::success("2026-03-31 12:00:00".to_string())) })
    }
}

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    // 创建内存审计日志
    let audit_logger = Arc::new(InMemoryAuditLogger::new());

    // 创建审计回调（自动记录 tool 调用链）
    let audit_cb = Arc::new(AuditCallback::new(audit_logger.clone(), "demo-agent", None));

    // 创建权限策略：只授予 Read 权限，Execute 需要审批
    let policy = Arc::new(
        DefaultPermissionPolicy::new()
            .grant(ToolPermission::Read)
            .grant(ToolPermission::Network),
    );

    println!("=== 权限策略 ===");
    println!("已授权: Read, Network");
    println!("需审批: Execute, Sensitive");
    println!("未授权: Write\n");

    // 构建 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("你是一个助手，可以使用工具完成任务。请直接调用工具，不要询问用户。")
        .enable_tools()
        .tool(Box::new(RunCommandTool))
        .tool(Box::new(SafeTool))
        .callback(audit_cb)
        .permission_policy(policy)
        .audit_logger(audit_logger.clone())
        .build()?;

    // 执行对话
    let answer = agent.chat("请获取当前时间").await?;
    println!("Agent: {answer}\n");

    // 查看审计日志
    println!("=== 审计日志 ({} 条事件) ===", audit_logger.len());
    let events = audit_logger.query(AuditFilter::default()).await?;
    for (i, event) in events.iter().enumerate() {
        println!(
            "  [{}] {} - {:?}",
            i + 1,
            event.timestamp.format("%H:%M:%S%.3f"),
            event.event_type
        );
    }

    Ok(())
}
