//! demo50_eval — 评估系统示例
//!
//! 展示 echo-agent 评估框架的核心功能：
//! 1. EvalCase + SuccessCriteria —— 定义测试用例
//! 2. EvalConstraints —— 行为约束
//! 3. TrajectoryReplay —— 离线轨迹分析
//! 4. TriggerAccuracy —— 子 Agent 触发准确率
//! 5. HTML 报告生成
//! 6. A/B 对比（概念演示）
//!
//! 全程无真实 LLM 调用，使用 Mock 数据演示离线分析能力。
//!
//! 运行方式：
//! ```bash
//! cargo run --example demo50_eval
//! ```

use chrono::Utc;
use echo_agent::eval::trigger::{TriggerAccuracy, TriggerTestCase};
use echo_agent::eval::*;
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
    println!("║         echo-agent  评估系统 demo                ║");
    println!("║  （全程无真实 LLM 调用 / 离线分析演示）           ║");
    println!("╚══════════════════════════════════════════════════╝");

    demo_eval_cases().await;
    demo_trajectory_replay().await;
    demo_regression_suite().await;
    demo_trigger_accuracy().await;
    demo_html_report().await;

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  全部 5 个场景通过 ✅                             ║");
    println!("╚══════════════════════════════════════════════════╝");
}

/// 场景 1：定义评估用例
async fn demo_eval_cases() {
    section!(1, "EvalCase + SuccessCriteria — 定义测试用例");

    // 基本用例：检查输出包含特定内容
    let case1 = EvalCase {
        id: "output_check_001".into(),
        name: "输出检查".into(),
        description: "Agent 应输出包含 'hello' 的内容".into(),
        domain: Some("general".into()),
        task: "Say hello world".into(),
        project_fixture: None,
        success_criteria: SuccessCriteria::OutputContains {
            substring: "hello".into(),
        },
        constraints: Default::default(),
    };
    pass!(format!("用例 '{}': OutputContains 标准", case1.name));

    // 组合用例：多个条件同时满足
    let case2 = EvalCase {
        id: "compound_001".into(),
        name: "复合条件".into(),
        description: "Agent 应使用 read_file 且输出包含特定内容".into(),
        domain: Some("coding".into()),
        task: "读取文件并总结内容".into(),
        project_fixture: None,
        success_criteria: SuccessCriteria::AllOf(vec![
            SuccessCriteria::ToolUsed {
                tool_name: "read_file".into(),
            },
            SuccessCriteria::OutputContains {
                substring: "内容".into(),
            },
        ]),
        constraints: EvalConstraints {
            required_read_before_edit: true,
            max_files_changed: Some(1),
            ..Default::default()
        },
    };
    pass!(format!(
        "用例 '{}': AllOf(ToolUsed + OutputContains) + 约束",
        case2.name
    ));

    // 测试命令用例
    let case3 = EvalCase {
        id: "test_pass_001".into(),
        name: "测试通过".into(),
        description: "Agent 修改代码后测试应通过".into(),
        domain: Some("coding".into()),
        task: "修复 bug 并确保测试通过".into(),
        project_fixture: None,
        success_criteria: SuccessCriteria::TestPass {
            command: "cargo test".into(),
        },
        constraints: EvalConstraints {
            forbidden_paths: vec!["Cargo.toml".into(), ".env".into()],
            max_tool_calls: Some(15),
            ..Default::default()
        },
    };
    pass!(format!(
        "用例 '{}': TestPass + 禁止路径 + 最大工具调用",
        case3.name
    ));

    // 展示所有标准类型
    println!("\n  支持的 SuccessCriteria 类型:");
    println!("    - TestPass       : Shell 命令退出码为 0");
    println!("    - OutputContains : 输出包含子串");
    println!("    - ToolUsed       : 使用了指定工具");
    println!("    - ToolNotUsed    : 未使用指定工具");
    println!("    - AllOf          : 所有条件都满足");
    println!("    - AnyOf          : 至少一个条件满足");
    println!("    - LlmGraded      : LLM 评判断言");
    println!("    - SweBench       : 基准测试风格");
}

/// 场景 2：轨迹回放分析
async fn demo_trajectory_replay() {
    section!(2, "TrajectoryReplay — 离线轨迹分析");

    // 构造一个有"先写后读"违规的运行
    let run = make_run(
        "run_demo_001",
        "修复 auth.rs 中的 bug",
        vec![
            // 直接写入，没有先读取
            RunEvent::ToolCall {
                call_id: "c1".into(),
                name: "write_file".into(),
                args: Some(serde_json::json!({"path": "auth.rs", "content": "fixed"})),
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "c1".into(),
                name: "write_file".into(),
                success: true,
                output_preview: Some("写入成功".into()),
                output_truncated: false,
                duration_ms: 5,
            },
            // 读取另一个文件
            RunEvent::ToolCall {
                call_id: "c2".into(),
                name: "read_file".into(),
                args: Some(serde_json::json!({"path": "config.toml"})),
                risk: None,
                duration_ms: 20,
            },
            RunEvent::ToolResult {
                call_id: "c2".into(),
                name: "read_file".into(),
                success: true,
                output_preview: Some("[server] port = 8080".into()),
                output_truncated: false,
                duration_ms: 10,
            },
            // 再次写入
            RunEvent::ToolCall {
                call_id: "c3".into(),
                name: "write_file".into(),
                args: Some(serde_json::json!({"path": "config.toml", "content": "port=9090"})),
                risk: None,
                duration_ms: 10,
            },
            RunEvent::ToolResult {
                call_id: "c3".into(),
                name: "write_file".into(),
                success: true,
                output_preview: Some("写入成功".into()),
                output_truncated: false,
                duration_ms: 5,
            },
        ],
        RunStatus::Completed,
    );

    let replay = TrajectoryReplay::new(run);

    // 工具调用统计
    let counts = replay.tool_call_counts();
    println!("  工具调用统计:");
    for (tool, count) in &counts {
        println!("    {tool}: {count} 次");
    }
    pass!(format!("总工具调用: {}", replay.total_tool_calls()));

    // 检测先写后读违规
    let violations = replay.detect_write_without_read();
    println!("\n  检测到的违规:");
    for v in &violations {
        println!("    ⚠️  {v}");
    }
    assert!(!violations.is_empty(), "应检测到先写后读违规");
    pass!("检测到先写后读违规");

    // 错误统计
    println!("\n  错误数: {}", replay.error_count());

    // 写入的文件
    let files = replay.written_files();
    println!("  写入的文件: {:?}", files);
}

/// 场景 3：回归套件
async fn demo_regression_suite() {
    section!(3, "RegressionSuite — 从历史轨迹构建回归测试");

    // 构造历史成功运行
    let runs = vec![
        make_run(
            "run_hist_001",
            "读取 README.md",
            vec![
                RunEvent::ToolCall {
                    call_id: "h1".into(),
                    name: "read_file".into(),
                    args: Some(serde_json::json!({"path": "README.md"})),
                    risk: None,
                    duration_ms: 10,
                },
                RunEvent::ToolResult {
                    call_id: "h1".into(),
                    name: "read_file".into(),
                    success: true,
                    output_preview: Some("# Project".into()),
                    output_truncated: false,
                    duration_ms: 5,
                },
            ],
            RunStatus::Completed,
        ),
        make_run(
            "run_hist_002",
            "列出目录文件",
            vec![
                RunEvent::ToolCall {
                    call_id: "h2".into(),
                    name: "list_files".into(),
                    args: Some(serde_json::json!({"path": "."})),
                    risk: None,
                    duration_ms: 10,
                },
                RunEvent::ToolResult {
                    call_id: "h2".into(),
                    name: "list_files".into(),
                    success: true,
                    output_preview: Some("src/ docs/ Cargo.toml".into()),
                    output_truncated: false,
                    duration_ms: 5,
                },
            ],
            RunStatus::Completed,
        ),
        // 一个失败的运行（不应被纳入回归套件）
        make_run("run_hist_003", "编译项目", vec![], RunStatus::Failed),
    ];

    let suite = RegressionSuite::from_traces(&runs);
    println!(
        "  从 {} 条历史运行中提取了 {} 个回归用例",
        runs.len(),
        suite.len()
    );
    assert_eq!(suite.len(), 2, "应跳过失败的运行");
    pass!("成功跳过失败运行，提取 2 个回归用例");

    for case in &suite.cases {
        println!("    - {}: {}", case.id, case.name);
    }
}

/// 场景 4：触发准确率评估
async fn demo_trigger_accuracy() {
    section!(4, "TriggerAccuracy — 子 Agent 路由准确率");

    let cases = vec![
        TriggerTestCase {
            query: "读取 src/main.rs 的内容".into(),
            expected_agent: "code-explorer".into(),
            should_trigger: true,
            runs_per_query: 1,
        },
        TriggerTestCase {
            query: "搜索 Rust 所有权相关文档".into(),
            expected_agent: "web-researcher".into(),
            should_trigger: true,
            runs_per_query: 1,
        },
        TriggerTestCase {
            query: "今天天气怎么样".into(),
            expected_agent: "".into(),
            should_trigger: false,
            runs_per_query: 1,
        },
        TriggerTestCase {
            query: "2+2 等于几".into(),
            expected_agent: "".into(),
            should_trigger: false,
            runs_per_query: 1,
        },
    ];

    // 模拟实际触发结果
    let actual_triggers = vec![
        (
            "读取 src/main.rs 的内容".into(),
            Some("code-explorer".into()),
        ),
        (
            "搜索 Rust 所有权相关文档".into(),
            Some("web-researcher".into()),
        ),
        ("今天天气怎么样".into(), None), // 正确：未触发
        ("2+2 等于几".into(), Some("math-agent".into())), // 错误：不应触发但触发了
    ];

    let accuracy = TriggerAccuracy::evaluate(&cases, &actual_triggers);

    println!("  测试用例: {}", accuracy.total);
    println!("  真阳性 (TP): {}", accuracy.true_positives);
    println!("  假阳性 (FP): {}", accuracy.false_positives);
    println!("  真阴性 (TN): {}", accuracy.true_negatives);
    println!("  假阴性 (FN): {}", accuracy.false_negatives);
    println!("  精确率 (Precision): {:.2}", accuracy.precision);
    println!("  召回率 (Recall): {:.2}", accuracy.recall);
    println!("  F1 分数: {:.2}", accuracy.f1);

    assert_eq!(accuracy.true_positives, 2);
    assert_eq!(accuracy.false_positives, 1);
    assert_eq!(accuracy.true_negatives, 1);
    assert_eq!(accuracy.false_negatives, 0);
    pass!("触发准确率计算正确");
}

/// 场景 5：HTML 报告生成
async fn demo_html_report() {
    section!(5, "HTML 报告生成");

    // 构造评估结果
    let results = vec![
        EvalResult::new("case_001", true)
            .with_metric("tool_accuracy", 1.0, "使用了正确的工具")
            .with_metric("constraint_compliance", 1.0, "无违规"),
        EvalResult::new("case_002", false)
            .with_metric("tool_accuracy", 0.5, "使用了错误的工具")
            .with_violation("超出最大文件变更数"),
        EvalResult::new("case_003", true).with_metric("tool_accuracy", 0.9, "工具选择基本正确"),
    ];

    let mut report = EvalReport::new(results);
    report.total_tool_calls = 12;
    report.total_tokens_in = 3000;
    report.total_tokens_out = 1500;

    println!("  评估报告摘要:");
    println!("    总用例: {}", report.total);
    println!("    通过: {}", report.passed);
    println!("    失败: {}", report.failed);
    println!("    平均分: {:.2}", report.avg_score);
    println!("    标准差: {:.3}", report.std_dev);
    println!("    最低分: {:.2}", report.min_score);
    println!("    最高分: {:.2}", report.max_score);
    println!("    总工具调用: {}", report.total_tool_calls);
    println!(
        "    总 Token: {}+{}",
        report.total_tokens_in, report.total_tokens_out
    );

    // 生成静态 HTML
    let html = generate_html(&report, "Demo 评估报告");
    assert!(html.contains("Demo 评估报告"));
    assert!(html.contains("PASS"));
    assert!(html.contains("FAIL"));
    pass!("静态 HTML 报告生成成功");

    // 生成交互式审查 HTML
    let review_html = echo_agent::eval::server::generate_review_html(&report, "审查会话");
    assert!(review_html.contains("审查会话"));
    assert!(review_html.contains("feedback"));
    pass!("交互式审查 HTML 生成成功");

    // 写入文件（可选）
    let report_dir = std::env::temp_dir().join("echo_eval_demo");
    let _ = std::fs::create_dir_all(&report_dir);
    let html_path = report_dir.join("eval_report.html");
    let review_path = report_dir.join("review.html");
    let _ = std::fs::write(&html_path, &html);
    let _ = std::fs::write(&review_path, &review_html);
    println!("\n  报告已写入:");
    println!("    {}", html_path.display());
    println!("    {}", review_path.display());
}
