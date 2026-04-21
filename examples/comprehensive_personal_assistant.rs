//! 综合示例：个人智能助手
//!
//! 展示 echo-agent 在个人助手场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | 长期记忆 | `SqliteStore` 持久化用户偏好和对话历史 |
//! | 多模态支持 | `execute_with_image()` 处理图片输入 |
//! | Agent 编排 | 主协调 Agent + 专业化子 Agent |
//! | 任务管理 | `TaskManager` 创建和追踪任务 |
//! | 流式输出 | `chat_stream()` 实时对话体验 |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行（需要 LLM API Key）
//! QWEN_API_KEY=your_key cargo run --example comprehensive_personal_assistant --features sqlite
//! ```

use echo_agent::memory::SqliteStore;
use echo_agent::prelude::*;
use echo_agent::tasks::{Task, TaskManager, TaskStatus};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

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
        )
        .into());
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
        )
        .into());
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

    // 创建专业化子 Agent
    let planner = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("planner")
        .system_prompt("你是规划专家，擅长制定计划和分解任务。")
        .max_iterations(5)
        .build()?;

    let researcher = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("researcher")
        .system_prompt("你是研究专家，擅长收集和分析信息。")
        .max_iterations(5)
        .build()?;

    let writer = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("writer")
        .system_prompt("你是写作专家，擅长整理和表达内容。")
        .max_iterations(5)
        .build()?;

    // 创建主编排 Agent
    let mut coordinator = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("coordinator")
        .system_prompt(
            "你是主编排者，负责协调各个子 Agent 完成复杂任务。
你可以调度的子 Agent：
- planner: 制定计划
- researcher: 收集信息
- writer: 整理输出

请根据任务需求合理分配工作。",
        )
        .role(echo_agent::agent::AgentRole::Orchestrator)
        .enable_subagent()
        .max_iterations(15)
        .build()?;

    coordinator.register_agent(Box::new(planner));
    coordinator.register_agent(Box::new(researcher));
    coordinator.register_agent(Box::new(writer));

    println!("  ✓ 已创建 1 个主编排 Agent + 3 个专业化子 Agent\n");

    println!("  📋 协作任务: 制定「学习 Rust」计划\n");

    let task = r#"请帮我制定一个学习 Rust 的计划：
1. 使用 planner 制定学习大纲
2. 使用 researcher 收集学习资源
3. 使用 writer 整理成完整的学习计划

请协调子 Agent 完成这个任务。"#;

    println!("  执行中...\n");

    let result = coordinator.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：个人助手编排结果为空".to_string(),
        )
        .into());
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

    let manager = TaskManager::new();

    // 创建任务
    let tasks = vec![
        Task::new("task-001", "学习 Rust 基础语法"),
        Task::new("task-002", "完成第一个 Rust 项目"),
        Task::new("task-003", "阅读 Rust 官方文档"),
    ];

    for task in &tasks {
        manager.add_task(task.clone());
    }

    println!("  ✓ 已创建 {} 个任务\n", tasks.len());

    // 列出所有任务
    let all_tasks = manager.get_all_tasks();
    if all_tasks.len() != tasks.len() {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：任务数不匹配，预期 {} 实际 {}",
            tasks.len(),
            all_tasks.len()
        ))
        .into());
    }
    println!("  任务列表:\n");
    for task in &all_tasks {
        let status_icon = match &task.status {
            TaskStatus::Pending => "⏳",
            TaskStatus::InProgress => "🔄",
            TaskStatus::Completed => "✅",
            TaskStatus::Cancelled => "🚫",
            TaskStatus::Failed(_) => "❌",
            TaskStatus::Blocked(_) => "🔒",
            TaskStatus::TimedOut { .. } => "⏰",
            TaskStatus::Retrying { .. } => "🔄",
        };
        println!("    {} {} - {}", status_icon, task.id, task.description);
    }
    println!();

    // 更新任务状态
    let _ = manager.update_task_status("task-001", TaskStatus::Completed);
    let _ = manager.update_task_status("task-002", TaskStatus::InProgress);

    println!("  ✓ 更新了任务状态\n");

    // 获取特定任务
    let Some(task) = manager.get_task("task-003") else {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：无法读取 task-003".to_string(),
        )
        .into());
    };
    println!("  当前进行中: {} ({})\n", task.description, task.id);

    // 进度统计
    let (completed, total) = manager.get_progress();
    if completed != 1 || total != 3 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：任务进度不符合预期（completed={completed}, total={total}）"
        ))
        .into());
    }
    println!("  进度: {}/{} 任务已完成\n", completed, total);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 4: 多模态支持
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_multimodal_support() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 4: 多模态支持");
    println!("═══════════════════════════════════════════════════════\n");

    println!("  📝 演示多模态消息类型:\n");

    // 创建支持多模态的 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
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
        )
        .into());
    }
    let preview: String = response.chars().take(100).collect();
    println!("    回复: {}...\n", preview);

    println!("  注意: 实际图片分析需要支持视觉的 LLM 模型");
    println!("    支持的模型包括: qwen3-max, gpt-4o, gpt-4o-mini 等\n");

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
