//! 综合示例：个人智能助手
//!
//! 展示 echo-agent 在个人助手场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | 长期记忆 | `SqliteStore` 持久化用户偏好和对话历史 |
//! | 多模态支持 | `chat_with_image_url()` / `execute_with_image_url()` 处理图片输入 |
//! | Agent 编排 | 主协调 Agent + 专业化子 Agent |
//! | 任务管理 | `TaskRevisionService` 原子维护版本化任务图 |
//! | 流式输出 | `chat_stream()` 实时对话体验 |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行（需要 LLM API Key）
//! QWEN_API_KEY=your_key cargo run --example demo48_personal_assistant --features sqlite,tasks,subagent
//! ```

use echo_agent::memory::SqliteStore;
use echo_agent::prelude::*;
use echo_agent::tasks::{
    DefaultTaskToolPolicy, InMemoryRevisionedTaskStore, TaskCreateInput, TaskDraft,
    TaskGraphExecutionMode, TaskKind, TaskPlanPatchInputOp, TaskRevisionService, TaskStatus,
    TaskUpdateInput,
};
use echo_agent::tools::ToolContext;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SUBAGENT_TOOL_TIMEOUT_MS: u64 = 180_000;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,personal_assistant=info".into()),
        )
        .init();

    print_banner();

    println!("🤖 正在初始化个人智能助手...\n");

    let db_path = personal_assistant_db_path();
    cleanup_sqlite_files(&db_path);

    // ── Part 1: 长期记忆系统 ───────────────────────────────────────────────────
    demo_long_term_memory(&db_path).await?;

    // ── Part 2: Agent 编排协作 ─────────────────────────────────────────────────
    demo_agent_orchestration().await?;

    // ── Part 3: 任务管理系统 ───────────────────────────────────────────────────
    demo_task_management().await?;

    // ── Part 4: 多模态支持 ─────────────────────────────────────────────────────
    demo_multimodal_support().await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    cleanup_sqlite_files(&db_path);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 1: 长期记忆系统
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_long_term_memory(db_path: &Path) -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 1: 长期记忆系统");
    println!("═══════════════════════════════════════════════════════\n");

    let store = Arc::new(SqliteStore::new(db_path)?);
    let ns = &["personal_assistant", "profile"];

    println!("  📁 数据库路径: {}\n", db_path.display());

    // 存储用户偏好
    let user_profile = json!({
        "name": "用户",
        "preferences": {
            "theme": "dark",
            "language": "zh-CN",
            "timezone": "Asia/Shanghai"
        },
        "interests": ["编程", "AI", "科技", "阅读"],
        "goals": ["学习 Rust", "完成项目", "提升技能"]
    });

    store.put(ns, "user_profile", user_profile).await?;

    // 存储对话历史
    let conversations = vec![
        (
            "conv_001",
            json!({
                "date": "2024-01-15",
                "topic": "Rust 学习计划",
                "summary": "制定了为期3个月的 Rust 学习计划"
            }),
        ),
        (
            "conv_002",
            json!({
                "date": "2024-01-20",
                "topic": "项目架构讨论",
                "summary": "讨论了微服务架构的设计方案"
            }),
        ),
        (
            "conv_003",
            json!({
                "date": "2024-02-01",
                "topic": "技术选型",
                "summary": "比较了 Go 和 Rust 在后端开发中的优劣"
            }),
        ),
    ];

    for (key, value) in &conversations {
        store.put(ns, key, value.clone()).await?;
    }

    println!("  ✓ 已存储用户资料和 {} 条对话历史\n", conversations.len());

    // 演示记忆检索
    println!("  🔍 记忆检索测试:\n");

    // 获取用户资料
    let Some(item) = store.get(ns, "user_profile").await? else {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：无法读取 user_profile".to_string(),
        ));
    };
    println!("    用户资料:");
    println!("      兴趣: {:?}", item.value["interests"]);
    println!("      目标: {:?}", item.value["goals"]);
    println!();

    // 搜索相关对话
    let search_results = store.search(ns, "Rust", 3).await?;
    if search_results.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：长期记忆检索没有命中 Rust 对话".to_string(),
        ));
    }
    println!("    关于「Rust」的对话:");
    for item in &search_results {
        let summary = item.value["summary"].as_str().unwrap_or("");
        println!("      • {}", summary);
    }
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 2: Agent 编排协作
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_agent_orchestration() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 2: Agent 编排协作");
    println!("═══════════════════════════════════════════════════════\n");

    let model_name = require_configured_model(None)?;

    // 创建专业化子 Agent
    let planner = ReactAgentBuilder::new()
        .model(&model_name)
        .name("planner")
        .system_prompt("你是规划专家，擅长制定计划和分解任务。请用简洁要点输出，不要写冗长铺垫。")
        .max_iterations(5)
        .build()?;

    let researcher = ReactAgentBuilder::new()
        .model(&model_name)
        .name("researcher")
        .system_prompt("你是研究专家，擅长收集和分析信息。请只返回高价值学习资源和简短说明。")
        .max_iterations(5)
        .build()?;

    let writer = ReactAgentBuilder::new()
        .model(&model_name)
        .name("writer")
        .system_prompt("你是写作专家，擅长整理和表达内容。请输出结构清晰、篇幅适中的最终方案。")
        .max_iterations(5)
        .build()?;

    // 创建主编排 Agent
    let mut coordinator = ReactAgentBuilder::new()
        .model(&model_name)
        .name("coordinator")
        .system_prompt(
            "你是主编排者，负责协调各个子 Agent 完成复杂任务。
你可以调度的子 Agent：
- planner: 制定计划
- researcher: 收集信息
- writer: 整理输出

请根据任务需求合理分配工作。",
        )
        .enable_subagent()
        .tool_execution(ToolExecutionConfig {
            timeout_ms: SUBAGENT_TOOL_TIMEOUT_MS,
            ..ToolExecutionConfig::default()
        })
        .max_iterations(15)
        .build()?;

    coordinator.register_agent(Box::new(planner));
    coordinator.register_agent(Box::new(researcher));
    coordinator.register_agent(Box::new(writer));

    println!("  ✓ 已创建 1 个主编排 Agent + 3 个专业化子 Agent\n");
    println!(
        "  子 Agent 工具超时: {} 秒\n",
        SUBAGENT_TOOL_TIMEOUT_MS / 1000
    );

    println!("  📋 协作任务: 制定「学习 Rust」计划\n");

    let task = r#"请帮我制定一个学习 Rust 的计划，并尽量高效完成：
1. 使用 planner 制定学习大纲
2. 使用 researcher 收集学习资源
3. 使用 writer 整理成完整的学习计划

要求：
- 每个子 Agent 输出尽量精炼，控制在要点级别
- 最终结果包含阶段安排、推荐资源、实践建议
- 不要重复调用同一个子 Agent 做同一件事"#;

    println!("  执行中...\n");

    let result = coordinator.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：个人助手编排结果为空".to_string(),
        ));
    }
    let preview: String = result.chars().take(300).collect();
    println!("  ✓ 协作完成:\n");
    println!("  {}\n... (内容已截断)", preview);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 3: 任务管理系统
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_task_management() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 3: 任务管理系统");
    println!("═══════════════════════════════════════════════════════\n");

    let service = TaskRevisionService::new(
        Arc::new(InMemoryRevisionedTaskStore::new()),
        Arc::new(DefaultTaskToolPolicy::new("personal-assistant-demo")),
    );
    let context = ToolContext {
        run_id: Some("personal-assistant-demo".to_string()),
        ..ToolContext::default()
    };
    let drafts = vec![
        task_draft("task-001", "学习 Rust 基础语法"),
        task_draft("task-002", "完成第一个 Rust 项目"),
        task_draft("task-003", "阅读 Rust 官方文档"),
    ];
    let created = service
        .create_from_tool(
            TaskCreateInput {
                tasks: drafts,
                base_revision: None,
                reason: Some("创建学习计划".to_string()),
                assumptions: Vec::new(),
                risks: Vec::new(),
                execution_mode: TaskGraphExecutionMode::Sequential,
            },
            &context,
        )
        .await
        .map_err(|error| echo_agent::error::ReactError::Other(error.to_string()))?;

    let expected_count = 3;
    println!("  ✓ 已创建 {} 个任务\n", created.created_count);
    if created.graph.snapshot.tasks.len() != expected_count {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：任务数不匹配，预期 {} 实际 {}",
            expected_count,
            created.graph.snapshot.tasks.len()
        )));
    }
    println!("  任务列表:\n");
    for task in &created.graph.snapshot.tasks {
        let status_icon = match &task.execution.status {
            TaskStatus::Pending => "⏳",
            TaskStatus::Running => "🔄",
            TaskStatus::Completed => "✅",
            TaskStatus::Cancelled => "🚫",
            TaskStatus::Failed(_) => "❌",
            TaskStatus::Blocked(_) => "🔒",
            TaskStatus::TimedOut { .. } => "⏰",
            TaskStatus::Retrying { .. } => "🔄",
            TaskStatus::Skipped => "⏭️",
            TaskStatus::Paused(_) => "⏸️",
        };
        println!(
            "    {} {} - {}",
            status_icon, task.spec.id, task.spec.description
        );
    }
    println!();

    let running = service
        .update_from_tool(
            TaskUpdateInput {
                base_revision: created.graph.snapshot.revision,
                reason: "开始第一个任务".to_string(),
                operations: vec![TaskPlanPatchInputOp::SetStatus {
                    task_id: "task-001".to_string(),
                    status: TaskStatus::Running,
                }],
            },
            &context,
        )
        .await
        .map_err(|error| echo_agent::error::ReactError::Other(error.to_string()))?;
    let updated = service
        .update_from_tool(
            TaskUpdateInput {
                base_revision: running.snapshot.revision,
                reason: "推进学习计划".to_string(),
                operations: vec![
                    TaskPlanPatchInputOp::SetStatus {
                        task_id: "task-001".to_string(),
                        status: TaskStatus::Completed,
                    },
                    TaskPlanPatchInputOp::SetStatus {
                        task_id: "task-002".to_string(),
                        status: TaskStatus::Running,
                    },
                ],
            },
            &context,
        )
        .await
        .map_err(|error| echo_agent::error::ReactError::Other(error.to_string()))?;

    println!("  ✓ 更新了任务状态\n");

    let Some(task) = updated
        .snapshot
        .tasks
        .iter()
        .find(|task| task.spec.id == "task-002")
    else {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：无法读取 task-002".to_string(),
        ));
    };
    println!(
        "  当前进行中: {} ({})\n",
        task.spec.description, task.spec.id
    );

    let completed = updated
        .snapshot
        .tasks
        .iter()
        .filter(|task| task.execution.status == TaskStatus::Completed)
        .count();
    let total = updated.snapshot.tasks.len();
    if completed != 1 || total != 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：任务进度不符合预期（completed={completed}, total={total}）"
        )));
    }
    println!("  进度: {}/{} 任务已完成\n", completed, total);

    Ok(())
}

fn task_draft(id: &str, description: &str) -> TaskDraft {
    TaskDraft {
        id: id.to_string(),
        title: description.to_string(),
        description: description.to_string(),
        kind: TaskKind::Implementation,
        subagent: None,
        depends_on: Vec::new(),
        files: Vec::new(),
        allowed_tools: Vec::new(),
        required_artifacts: Vec::new(),
        execution_checks: Vec::new(),
        acceptance_criteria: Vec::new(),
        max_retries: 0,
        extensions: serde_json::Value::Null,
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 4: 多模态支持
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_multimodal_support() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 4: 多模态支持");
    println!("═══════════════════════════════════════════════════════\n");

    println!("  📝 演示多模态消息类型:\n");

    let model_name = require_configured_model(None)?;

    // 创建支持多模态的 Agent
    let agent = ReactAgentBuilder::new()
        .model(&model_name)
        .name("multimodal-assistant")
        .system_prompt("你是一个多模态助手，可以理解文字和图片。")
        .max_iterations(5)
        .build()?;

    println!("  ✓ 已创建支持多模态的 Agent\n");

    // 纯文本对话
    println!("  [纯文本对话]");
    let response = agent.chat("你好，请介绍一下自己").await?;
    if response.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：多模态 Agent 的文本对话返回空结果".to_string(),
        ));
    }
    let preview: String = response.chars().take(100).collect();
    println!("    回复: {}...\n", preview);

    println!("  注意: 实际图片分析需要在 echo-agent.yaml 中把 model.name 设为视觉模型");
    println!("    例如 `qwen3.7-plus` 或 `gpt-5.5`，并确保它已在 models 中声明\n");

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          Echo Agent 个人智能助手 - 综合示例                 ║");
    println!("║                                                                ║");
    println!("║  展示核心能力：                                                 ║");
    println!("║  • 长期记忆 • Agent 编排 • 任务管理 • 多模态支持              ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}

fn personal_assistant_db_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "echo_agent_personal_assistant_{}.db",
        std::process::id()
    ))
}

fn cleanup_sqlite_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

fn require_configured_model(preferred: Option<&str>) -> echo_agent::error::Result<String> {
    let app_config = echo_agent::config::load_config(None);
    let configured = app_config.model.name.trim();

    if !configured.is_empty() {
        return echo_agent::llm::config::LlmConfig::from_model(configured)
            .map(|_| configured.to_string())
            .map_err(|e| {
                echo_agent::error::ReactError::Other(format!(
                    "综合验收失败：当前 `model.name = {configured}` 配置无效：{e}"
                ))
            });
    }

    if let Some(preferred) = preferred
        && echo_agent::llm::config::Config::has_model(preferred)
    {
        return Ok(preferred.to_string());
    }

    if let Some(first) = echo_agent::llm::config::Config::list_models()
        .into_iter()
        .next()
    {
        return Ok(first);
    }

    let load_err = echo_agent::llm::config::Config::load_cached()
        .err()
        .map(|e| format!("配置加载失败：{e}"))
        .unwrap_or_else(|| {
            "请在 echo-agent.yaml 的 `models:` 中声明至少一个模型，并让 `model.name` 指向它。"
                .to_string()
        });
    Err(echo_agent::error::ReactError::Other(format!(
        "综合验收失败：未找到可用模型配置。{load_err}"
    )))
}
