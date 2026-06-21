//! 综合示例：智能客服助手
//!
//! 展示 echo-agent 在客服场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | 工具调用 | `#[tool]` 宏定义客服工具 |
//! | 长期记忆 | `SqliteStore` 持久化客户偏好和历史 |
//! | 流式输出 | `chat_stream()` 实时打字效果 |
//! | 护栏系统 | `RuleGuard` 敏感词过滤 |
//! | 人工审批 | `PermissionService` 退款需要人工批准 |
//! | 审计日志 | `InMemoryAuditLogger` 记录操作 |
//! | 任务规划 | `plan` + `create_task` 复杂问题拆解 |
//! | 快照回滚 | `SnapshotPolicy` 支持会话恢复 |
//! | 多模态 | `execute_with_image_url()` 处理图片问题 |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行（需要 LLM API Key + sqlite + human-loop feature）
//! QWEN_API_KEY=your_key cargo run --example demo45_customer_service --features sqlite,human-loop
//!
//! # 带流式输出和彩色日志
//! RUST_LOG=info QWEN_API_KEY=your_key cargo run --example demo45_customer_service --features sqlite,human-loop
//!
//! # 若要验证图片输入，请先在 echo-agent.yaml 中把 model.name 设为视觉模型
//! cargo run --example demo45_customer_service --features sqlite,human-loop
//! ```

use echo_agent::human_loop::{
    HumanLoopEvent, HumanLoopManager, InMemoryPermissionAuditSink, PermissionService,
};
use echo_agent::memory::SqliteStore;
use echo_agent::prelude::*;
use echo_agent::tool;
use echo_core::tools::permission::{PermissionRule, RuleMatcher, RuleSource};
use futures::StreamExt;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 工具定义 - 客服业务能力
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

static TICKET_COUNTER: AtomicU32 = AtomicU32::new(1);

#[derive(Default)]
struct ChatRunSummary {
    final_answer: String,
    tool_calls: Vec<String>,
}

#[tool(name = "query_order", description = "查询订单状态")]
async fn query_order(
    /// 订单号，格式如 ORD-2024-001234
    order_id: String,
) -> Result<ToolResult> {
    // 模拟订单查询
    let order_info = json!({
        "order_id": order_id,
        "status": "已发货",
        "shipping_date": "2024-01-15",
        "estimated_delivery": "2024-01-20",
        "items": ["商品A x2", "商品B x1"],
        "total": 299.00
    });
    Ok(ToolResult::success(order_info.to_string()))
}

#[tool(name = "process_refund", description = "处理退款申请（需要人工审批）")]
async fn process_refund(
    /// 订单号
    order_id: String,
    /// 退款原因
    reason: String,
    /// 退款金额
    amount: f64,
) -> Result<ToolResult> {
    let refund_info = json!({
        "refund_id": format!("REF-{}", order_id),
        "order_id": order_id,
        "reason": reason,
        "amount": amount,
        "status": "pending_approval",
        "message": "退款申请已提交，等待客服审核"
    });
    Ok(ToolResult::success(refund_info.to_string()))
}

#[tool(name = "check_inventory", description = "查询商品库存")]
async fn check_inventory(
    /// 商品名称
    product_name: String,
) -> Result<ToolResult> {
    let inventory = json!({
        "product": product_name,
        "stock": 156,
        "warehouse": ["华东仓", "华南仓"],
        "restock_date": "2024-01-25"
    });
    Ok(ToolResult::success(inventory.to_string()))
}

#[tool(name = "create_ticket", description = "创建工单")]
async fn create_ticket(
    /// 工单标题
    title: String,
    /// 问题描述
    description: String,
    /// 优先级
    priority: String,
) -> Result<ToolResult> {
    let ticket_id = TICKET_COUNTER.fetch_add(1, Ordering::SeqCst);
    let ticket = json!({
        "ticket_id": format!("TKT-{:06}", ticket_id),
        "title": title,
        "description": description,
        "priority": priority,
        "status": "已创建",
        "created_at": chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
    });
    Ok(ToolResult::success(ticket.to_string()))
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,customer_service=info".into()),
        )
        .init();

    print_banner();

    let db_path = customer_service_db_path();
    cleanup_sqlite_files(&db_path);

    // ── 1. 创建 SQLite 持久化存储（长期记忆）───────────────────────────────
    let store = Arc::new(SqliteStore::new(&db_path)?);
    let ns = &["customer_service", "memories"];

    // 预填充一些常见知识
    store
        .put(
            ns,
            "policy_return",
            json!({
                "content": "退换货政策：7天无理由退货，15天内质量问题可换货。退货需保持商品原包装完好。",
                "category": "政策"
            }),
        )
        .await?;
    store
        .put(
            ns,
            "policy_shipping",
            json!({
                "content": "配送政策：全国包邮，偏远地区除外。通常48小时内发货，3-5个工作日送达。",
                "category": "政策"
            }),
        )
        .await?;

    // ── 2. 创建审计日志（操作追踪）──────────────────────────────────────────
    let audit_logger = Arc::new(InMemoryAuditLogger::new());

    // ── 3. 创建审批服务（退款需要人工批准）──────────────────────────────────
    let audit_sink = Arc::new(InMemoryPermissionAuditSink::new(100));
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
                    println!("\n   🔐 [自动审批: {}]\n", tool_name);
                    responder.approve();
                }
                HumanLoopEvent::InputRequest { prompt, responder } => {
                    println!("\n   💬 [自动补充信息: {}]\n", prompt);
                    responder
                        .respond("客户未补充更多信息，请基于已知订单信息继续处理。".to_string());
                }
                _ => {
                    // SelectionRequest and other variants — not used by this demo.
                }
            }
        }
    });
    let permission_service = Arc::new(
        PermissionService::from_provider(
            manager.clone() as Arc<dyn echo_agent::human_loop::HumanLoopProvider>
        )
        .with_audit_sink(audit_sink.clone()),
    );
    permission_service
        .add_rules(vec![
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "query_order".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "check_inventory".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "create_ticket".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "remember".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "recall".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "search_memory".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::allow(
                RuleMatcher::Pattern {
                    pattern: "forget".to_string(),
                },
                RuleSource::Session,
            ),
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
                    pattern: "update_task".to_string(),
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
                    pattern: "final_answer".to_string(),
                },
                RuleSource::Session,
            ),
            PermissionRule::ask(
                RuleMatcher::Pattern {
                    pattern: "process_refund".to_string(),
                },
                vec!["允许".to_string(), "拒绝".to_string()],
                RuleSource::Session,
            ),
        ])
        .await;

    // ── 4. 创建护栏（敏感词过滤）────────────────────────────────────────────
    let input_guard = Arc::new(
        RuleGuardBuilder::new("content-filter")
            .blocked_pattern(r"(?i)(假冒|伪劣|高仿|骗|诈骗)")
            .blocked_pattern(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b") // 银行卡
            .max_length(5000)
            .direction(GuardDirection::Input)
            .build(),
    );

    // ── 5. 构建智能客服 Agent ─────────────────────────────────────────────────
    println!("🤖 正在初始化智能客服助手...\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("customer-service-agent")
        .system_prompt(CUSTOMER_SERVICE_PROMPT)
        .enable_tools()
        .enable_planning()
        .enable_human_in_loop()
        .max_iterations(20)
        .snapshot_policy(SnapshotPolicy::EveryIteration)
        .max_snapshots(10)
        .approval_provider(manager)
        .permission_service(permission_service)
        .guard(input_guard)
        .audit_logger(audit_logger.clone())
        .with_memory_tools(store.clone())
        .build()?;

    // 添加业务工具
    agent.add_tool(Box::new(QueryOrderTool));
    agent.add_tool(Box::new(ProcessRefundTool));
    agent.add_tool(Box::new(CheckInventoryTool));
    agent.add_tool(Box::new(CreateTicketTool));

    println!("✓ 助手初始化完成！\n");

    // ── 6. 交互式客服对话 ─────────────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("              智能客服助手已启动");
    println!("      输入 'quit' 或 'exit' 退出，'snap' 查看快照");
    println!("═══════════════════════════════════════════════════════\n");

    // 示例对话 1：普通查询（使用流式输出）
    println!("📞 客户: 我的订单 ORD-2024-001234 到哪了？");
    let order_summary = stream_chat(&mut agent, "我的订单 ORD-2024-001234 到哪了？").await?;
    ensure_tool_called(&order_summary, "query_order", "订单查询")?;
    println!();

    // 示例对话 2：库存查询（使用记忆）
    println!("📞 客户: Rust 编程语言还有货吗？");
    let inventory_summary = stream_chat(&mut agent, "Rust 编程语言还有货吗？").await?;
    ensure_tool_called(&inventory_summary, "check_inventory", "库存查询")?;
    println!();

    // 示例对话 3：复杂问题（触发任务规划）
    println!("📞 客户: 我想退单，但订单已经发货了，怎么办？");
    let shipping_summary = stream_chat(
        &mut agent,
        "我想退单，但订单 ORD-2024-001234 已经发货了，怎么办？",
    )
    .await?;
    if shipping_summary.final_answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：发货退单场景没有生成最终答复".to_string(),
        ));
    }
    println!();

    // 示例对话 4：需要退款（触发人工审批）
    println!("📞 客户: 商品有质量问题，我要退款 100 元");
    let refund_summary = stream_chat(
        &mut agent,
        "我收到的商品有质量问题，订单号 ORD-2024-001234，我要退款 100 元",
    )
    .await?;
    ensure_tool_called(&refund_summary, "process_refund", "退款申请")?;
    println!();

    // ── 7. 展示审计日志 ───────────────────────────────────────────────────────
    println!("\n───────────────────────────────────────────────────────");
    println!("📋 操作审计日志");
    println!("───────────────────────────────────────────────────────\n");

    let audit_events = audit_logger.query(AuditFilter::default()).await?;
    if audit_events.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：审计日志为空".to_string(),
        ));
    }
    for (i, event) in audit_events.iter().take(10).enumerate() {
        println!(
            "  [{}] {:?} - {}",
            i + 1,
            event.event_type,
            match &event.event_type {
                echo_agent::audit::AuditEventType::UserInput { content } => {
                    format!("输入: {}", &content.chars().take(30).collect::<String>())
                }
                echo_agent::audit::AuditEventType::ToolCall { tool, success, .. } => {
                    format!("工具: {} (成功: {})", tool, success)
                }
                echo_agent::audit::AuditEventType::FinalAnswer { .. } => "最终答案".to_string(),
                _ => format!("{:?}", event.event_type),
            }
        );
    }

    // ── 8. 展示快照功能 ───────────────────────────────────────────────────────
    println!("\n───────────────────────────────────────────────────────");
    println!("📸 会话快照");
    println!("───────────────────────────────────────────────────────\n");

    let snapshots = agent.snapshots();
    if snapshots.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：未生成任何会话快照".to_string(),
        ));
    }
    println!("  当前会话快照数: {}", snapshots.len());
    for (i, snap) in snapshots.iter().take(3).enumerate() {
        println!(
            "  [{}] iteration={}, messages={}",
            i + 1,
            snap.iteration,
            snap.messages.len()
        );
    }

    // ── 9. 展示记忆系统 ───────────────────────────────────────────────────────
    println!("\n───────────────────────────────────────────────────────");
    println!("🧠 长期记忆");
    println!("───────────────────────────────────────────────────────\n");

    let memories: Vec<_> = store.search(ns, "政策", 5).await?;
    if memories.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：长期记忆检索没有命中政策内容".to_string(),
        ));
    }
    println!("  相关记忆: {} 条", memories.len());
    for mem in &memories {
        let content = mem.value["content"].as_str().unwrap_or("");
        let preview: String = content.chars().take(80).collect();
        println!("    • {}", preview);
    }

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    if audit_sink.count() == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：权限审计未记录任何事件".to_string(),
        ));
    }

    cleanup_sqlite_files(&db_path);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn stream_chat(agent: &mut ReactAgent, message: &str) -> Result<ChatRunSummary> {
    let mut stream = agent.chat_stream(message).await?;
    let mut summary = ChatRunSummary::default();

    print!("🤖 客服: ");
    std::io::stdout().flush().ok();

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Token(token) => {
                summary.final_answer.push_str(&token);
                print!("{}", token);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolCall { name, args: _ } => {
                summary.tool_calls.push(name.clone());
                print!("\n   🔧 [工具: {}]\n", name);
                std::io::stdout().flush().ok();
            }
            AgentEvent::ToolResult { output, .. } => {
                let preview: String = output.chars().take(100).collect();
                if output.len() > 100 {
                    println!("   ✓ 结果: {}...", preview);
                } else {
                    println!("   ✓ 结果: {}", preview);
                }
                std::io::stdout().flush().ok();
            }
            AgentEvent::FinalAnswer(_) => {
                println!();
            }
            _ => {}
        }
    }

    if summary.final_answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：客服对话没有产生最终答复".to_string(),
        ));
    }

    Ok(summary)
}

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          Echo Agent 智能客服助手 - 综合示例                  ║");
    println!("║                                                                ║");
    println!("║  展示核心能力：                                                   ║");
    println!("║  • 工具调用 • 长期记忆 • 流式输出 • 护栏系统                  ║");
    println!("║  • 人工审批 • 审计日志 • 任务规划 • 快照回滚                  ║");
    println!("║  • 多模态支持                                                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}

fn ensure_tool_called(summary: &ChatRunSummary, tool_name: &str, scenario: &str) -> Result<()> {
    if !summary.tool_calls.iter().any(|name| name == tool_name) {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：{scenario} 场景未调用 `{tool_name}`"
        )));
    }
    Ok(())
}

fn customer_service_db_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "echo_agent_customer_service_{}.db",
        std::process::id()
    ))
}

fn cleanup_sqlite_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 系统提示词
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

const CUSTOMER_SERVICE_PROMPT: &str = r#"你是一个专业、友好的电商客服助手，名为"小艾"。

【工作流程】
1. 使用 remember 工具记录客户偏好和历史问题
2. 使用 recall/search_memory 工具查找相关记忆和政策
3. 使用业务工具（query_order, process_refund, check_inventory, create_ticket）解决问题
4. 对于复杂问题，使用 plan 工具制定处理步骤
5. 完成后使用 final_answer 给出最终答复

【服务原则】
- 保持礼貌和专业
- 主动记住客户偏好
- 准确使用工具获取信息
- 无法解决时及时升级工单

【注意事项】
- 退款申请必须使用 process_refund 工具（会触发人工审批）
- 回复要简洁明了，避免冗余
- 遇到图片问题时仔细分析后回答"#;
