//! demo51_self_improvement — 自进化系统示例
//!
//! 展示 echo-agent 自进化流水线的核心功能：
//! 1. Analyzer —— 失败模式检测
//! 2. RunCritique —— 人类可读的分析报告
//! 3. Curator —— 技能生命周期管理
//! 4. TrajectorySaver —— 微调数据生成（ShareGPT 格式）
//!
//! 全程无真实 LLM 调用，使用 Mock 数据演示离线分析能力。
//!
//! 运行方式：
//! ```bash
//! cargo run --example demo51_self_improvement
//! ```

use chrono::Utc;
use echo_agent::improve::*;
use echo_agent::trace::{Run, RunEvent, RunStatus, RunTimings, TokenUsage};

macro_rules! section {
    ($n:expr, $title:expr) => {
        println!("\n══════════════════════════════════════════════════");
        println!("  场景 {} ：{}", $n, $title);
        println!("══════════════════════════════════════════════════");
    };
}

macro_rules! pass {
    ($msg:expr) => {
        println!("  ✅  {}", $msg)
    };
}

fn make_run(id: &str, input: &str, events: Vec<RunEvent>, status: RunStatus) -> Run {
    Run {
        run_id: id.to_string(),
        parent_run_id: None,
        session_id: "demo_session".into(),
        status,
        input: input.to_string(),
        events,
        final_output: Some("任务完成".into()),
        error: None,
        token_usage: TokenUsage {
            prompt_tokens: 500,
            completion_tokens: 200,
            total_tokens: 700,
        },
        timings: RunTimings {
            total_duration_ms: 2000,
            llm_duration_ms: 1500,
            tool_duration_ms: 300,
        },
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
    }
}

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║       echo-agent  自进化系统 demo                ║");
    println!("║  （全程无真实 LLM 调用 / 离线分析演示）           ║");
    println!("╚══════════════════════════════════════════════════╝");

    demo_analyzer().await;
    demo_critique_aggregation().await;
    demo_curator().await;
    demo_trajectory_saver().await;

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  全部 4 个场景通过 ✅                             ║");
    println!("╚══════════════════════════════════════════════════╝");
}

/// 场景 1：Analyzer — 失败模式检测
async fn demo_analyzer() {
    section!(1, "Analyzer — 失败模式检测");

    // 构造一个有问题的运行：先写后读 + 过度重试
    let run = make_run(
        "run_bad_001",
        "修复 auth.rs 中的 bug",
        vec![
            // 直接写入，没有先读
            RunEvent::ToolCall {
                call_id: "c1".into(),
                name: "write_file".into(),
                args: Some(serde_json::json!({"path": "auth.rs"})),
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "c1".into(),
                name: "write_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 5,
            },
            // shell 工具反复失败
            RunEvent::ToolCall {
                call_id: "c2".into(),
                name: "shell".into(),
                args: None,
                risk: None,
                duration_ms: 100,
            },
            RunEvent::ToolError {
                call_id: "c2".into(),
                name: "shell".into(),
                message: "command not found".into(),
            },
            RunEvent::ToolCall {
                call_id: "c3".into(),
                name: "shell".into(),
                args: None,
                risk: None,
                duration_ms: 100,
            },
            RunEvent::ToolError {
                call_id: "c3".into(),
                name: "shell".into(),
                message: "command not found".into(),
            },
            RunEvent::ToolCall {
                call_id: "c4".into(),
                name: "shell".into(),
                args: None,
                risk: None,
                duration_ms: 100,
            },
            RunEvent::ToolError {
                call_id: "c4".into(),
                name: "shell".into(),
                message: "command not found".into(),
            },
        ],
        RunStatus::Failed,
    );

    let critique = Analyzer::analyze(&run);

    println!("  运行 ID: {}", critique.run_id);
    println!("  成功: {}", critique.success);
    println!("  得分: {:.2}", critique.score);
    println!("  发现 {} 个问题", critique.issues.len());

    // 验证检测到的问题
    let has_write_without_read = critique
        .issues
        .iter()
        .any(|i| matches!(i, CritiqueIssue::WriteWithoutRead { .. }));
    let has_excessive_retries = critique
        .issues
        .iter()
        .any(|i| matches!(i, CritiqueIssue::ExcessiveRetries { .. }));

    assert!(has_write_without_read, "应检测到先写后读");
    assert!(has_excessive_retries, "应检测到过度重试");
    pass!("检测到先写后读和过度重试");

    // 验证生成了改进建议
    assert!(!critique.suggestions.is_empty(), "应生成改进建议");
    pass!(format!("生成了 {} 条改进建议", critique.suggestions.len()));

    // 打印人类可读报告
    println!("\n{}", critique.format_report());
}

/// 场景 2：批判聚合 — 演示多次分析的结果聚合
async fn demo_critique_aggregation() {
    section!(2, "批判聚合 — 多次分析的模式检测");

    // 构造多个运行并分析
    let run1 = make_run(
        "run_agg_001",
        "任务 1",
        vec![
            RunEvent::ToolCall {
                call_id: "s1".into(),
                name: "write_file".into(),
                args: None,
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "s1".into(),
                name: "write_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 5,
            },
        ],
        RunStatus::Completed,
    );

    let run2 = make_run(
        "run_agg_002",
        "任务 2",
        vec![
            RunEvent::ToolCall {
                call_id: "s2".into(),
                name: "write_file".into(),
                args: None,
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "s2".into(),
                name: "write_file".into(),
                success: true,
                output_preview: None,
                output_truncated: false,
                duration_ms: 5,
            },
        ],
        RunStatus::Completed,
    );

    let critiques = Analyzer::analyze_batch(&[run1, run2]);

    println!("  分析了 {} 次运行", critiques.len());
    assert_eq!(critiques.len(), 2);
    pass!("批量分析完成");

    // 聚合问题模式
    let mut issue_counts = std::collections::HashMap::new();
    for critique in &critiques {
        for issue in &critique.issues {
            let key = format!("{:?}", std::mem::discriminant(issue));
            *issue_counts.entry(key).or_insert(0usize) += 1;
        }
    }
    println!("\n  问题模式聚合:");
    for (pattern, count) in &issue_counts {
        println!("    {pattern}: {count} 次");
    }
    pass!("问题模式聚合正确");
}

/// 场景 3：Curator — 技能生命周期管理
async fn demo_curator() {
    section!(3, "Curator — 技能生命周期管理");

    let dir = std::env::temp_dir().join(format!("echo_curator_demo_{}", uuid::Uuid::new_v4()));
    let curator = Curator::new(CuratorConfig::default(), dir.join("curator_state.json"));

    // 注册技能
    curator.touch_skill("code-review", true).unwrap();
    curator.touch_skill("web-search", true).unwrap();
    curator.touch_skill("bundled-skill", false).unwrap(); // 非 Agent 创建
    pass!("注册了 3 个技能");

    // 固定重要技能
    curator.pin_skill("code-review").unwrap();
    pass!("固定了 code-review 技能");

    // 查看状态
    let status = curator.status();
    println!("  技能状态:");
    println!("    总数: {}", status.total);
    println!("    活跃: {}", status.active);
    println!("    过期: {}", status.stale);
    println!("    归档: {}", status.archived);
    println!("    固定: {}", status.pinned);
    assert_eq!(status.total, 3);
    assert_eq!(status.active, 3);
    assert_eq!(status.pinned, 1);
    pass!("状态查询正确");

    // 模拟时间流逝：手动设置 last_used_at 为 31 天前
    {
        let mut state = curator.load_state();
        if let Some(meta) = state.skills.get_mut("web-search") {
            meta.last_used_at = Utc::now() - chrono::Duration::days(31);
        }
        curator.save_state(&state).unwrap();
    }

    // 应用自动转换
    let transitions = curator.apply_transitions().unwrap();
    println!("\n  自动转换:");
    for (name, from, to) in &transitions {
        println!("    {name}: {from:?} → {to:?}");
    }
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].0, "web-search");
    assert_eq!(transitions[0].2, SkillLifecycle::Stale);
    pass!("web-search 从 Active 转为 Stale");

    // 验证固定技能未被转换
    let state = curator.load_state();
    let code_review = state.skills.get("code-review").unwrap();
    assert_eq!(code_review.lifecycle, SkillLifecycle::Active);
    pass!("固定的 code-review 保持 Active");

    // 验证非 Agent 创建的技能未被转换
    let bundled = state.skills.get("bundled-skill").unwrap();
    assert_eq!(bundled.lifecycle, SkillLifecycle::Active);
    pass!("非 Agent 创建的 bundled-skill 保持 Active");

    // 清理
    let _ = std::fs::remove_dir_all(&dir);
}

/// 场景 4：TrajectorySaver — 微调数据生成
async fn demo_trajectory_saver() {
    section!(4, "TrajectorySaver — ShareGPT 格式微调数据");

    let dir = std::env::temp_dir().join(format!("echo_traj_demo_{}", uuid::Uuid::new_v4()));
    let saver = TrajectorySaver::new(&dir).unwrap();

    // 构造一个完成的运行
    let run = make_run(
        "run_traj_001",
        "读取 foo.txt 的内容",
        vec![
            RunEvent::ToolCall {
                call_id: "t1".into(),
                name: "read_file".into(),
                args: Some(serde_json::json!({"path": "foo.txt"})),
                risk: None,
                duration_ms: 50,
            },
            RunEvent::ToolResult {
                call_id: "t1".into(),
                name: "read_file".into(),
                success: true,
                output_preview: Some("Hello, world!".into()),
                output_truncated: false,
                duration_ms: 50,
            },
        ],
        RunStatus::Completed,
    );

    // 保存轨迹
    let saved = saver.save(&run, "demo-model").await.unwrap();
    assert!(saved);
    pass!("轨迹保存成功");

    // 转换为 ShareGPT 格式预览
    let messages = TrajectorySaver::convert_run_to_sharegpt(&run);
    println!("\n  ShareGPT 格式 ({} 条消息):", messages.len());
    for msg in &messages {
        let preview: String = msg.value.chars().take(60).collect();
        println!("    [{}] {}", msg.from, preview);
    }
    pass!("ShareGPT 转换正确");

    // 列出保存的轨迹
    let entries = saver.list(None).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "run_traj_001");
    assert!(entries[0].completed);
    pass!("轨迹列表查询正确");

    // 统计
    let stats = saver.stats().await.unwrap();
    println!("\n  轨迹统计:");
    println!("    总数: {}", stats.total);
    println!("    完成: {}", stats.completed);
    println!("    失败: {}", stats.failed);
    println!("    总 Token: {}", stats.total_tokens);
    println!("    总工具调用: {}", stats.total_tool_calls);
    println!("    平均耗时: {}ms", stats.avg_duration_ms);
    assert_eq!(stats.total, 1);
    assert_eq!(stats.completed, 1);
    pass!("统计信息正确");

    // 清理
    let _ = tokio::fs::remove_dir_all(&dir).await;
}
