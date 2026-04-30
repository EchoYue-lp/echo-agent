//! 综合示例：企业级工作流自动化 Agent
//!
//! 展示 echo-agent 在企业自动化场景中的完整能力：
//!
//! ## 功能清单
//!
//! | 功能模块 | 实现方式 |
//! |---------|---------|
//! | File-based Skills | `discover_skills()` 动态加载外部技能 |
//! | Plan-and-Execute | `PlanExecuteAgent` 复杂任务编排 |
//! | Dynamic Tools | `add_tool/remove_tool` 运行时工具切换 |
//! | Workflow | `DAG + Conditional + Parallel` 流程编排 |
//! | Topology Tracking | `TopologyCallback` 调用关系追踪 |
//! | Stream Workflow | `run_stream()` 实时进度 |
//! | A2A Handoff | `HandoffManager` Agent 间切换 |
//!
//! ## 运行方式
//!
//! ```bash
//! # 基础运行
//! QWEN_API_KEY=your_key cargo run --example comprehensive_enterprise
//! ```

use echo_agent::advanced::*;
use echo_agent::prelude::*;
use echo_agent::skills::external::loader::DiscoveryScope;
use echo_agent::testing::MockLlmClient;
use echo_agent::workflow::{GraphBuilder, SharedState, WorkflowEvent};
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 自定义工具
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;

/// 分析代码质量工具
struct CodeQualityTool {
    call_count: Arc<AtomicUsize>,
}

impl CodeQualityTool {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn counter(&self) -> Arc<AtomicUsize> {
        self.call_count.clone()
    }
}

impl Tool for CodeQualityTool {
    fn name(&self) -> &str {
        "code_quality"
    }

    fn description(&self) -> &str {
        "分析代码质量，给出评分和改进建议"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "要分析的代码"
                },
                "language": {
                    "type": "string",
                    "description": "编程语言"
                }
            },
            "required": ["code", "language"]
        })
    }

    fn execute(
        &self,
        params: ToolParameters,
    ) -> BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        Box::pin(async move {
            let code = params.get("code").and_then(|v| v.as_str()).unwrap_or("");
            let lang = params
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            // 简单模拟代码分析
            let score: i32 = if code.len() > 50 { 85 } else { 70 };
            let issues = if code.contains("TODO") || code.contains("FIXME") {
                vec!["存在未完成的 TODO 项".to_string()]
            } else {
                vec!["无明显问题".to_string()]
            };

            let result = json!({
                "analysis_id": format!("QA-{:03}", count),
                "language": lang,
                "code_length": code.len(),
                "quality_score": score,
                "issues": issues,
                "suggestions": if score > 80 {
                    vec!["代码质量良好，建议增加单元测试".to_string()]
                } else {
                    vec![]
                }
            });

            Ok(ToolResult::success(result.to_string()))
        })
    }
}

/// CI/CD 状态检查工具
struct CiCheckTool {
    call_count: Arc<AtomicUsize>,
}

impl CiCheckTool {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn counter(&self) -> Arc<AtomicUsize> {
        self.call_count.clone()
    }
}

impl Tool for CiCheckTool {
    fn name(&self) -> &str {
        "ci_check"
    }

    fn description(&self) -> &str {
        "检查 CI/CD 管道状态"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pipeline": {
                    "type": "string",
                    "description": "管道名称"
                }
            },
            "required": ["pipeline"]
        })
    }

    fn execute(
        &self,
        params: ToolParameters,
    ) -> BoxFuture<'_, echo_agent::error::Result<ToolResult>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let pipeline = params
                .get("pipeline")
                .and_then(|v| v.as_str())
                .unwrap_or("main");

            // 模拟 CI 检查
            let result = json!({
                "pipeline": pipeline,
                "status": "success",
                "last_run": "2024-01-15 14:30:00",
                "duration_seconds": 245,
                "stages": [
                    {"name": "build", "status": "passed", "duration": 120},
                    {"name": "test", "status": "passed", "duration": 95},
                    {"name": "deploy", "status": "passed", "duration": 30}
                ]
            });

            Ok(ToolResult::success(result.to_string()))
        })
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Main
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=info,enterprise=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1: 外部技能系统（File-based Skills）─────────────────────────────
    demo_external_skills().await?;

    // ── Part 2: Plan-and-Execute 复杂任务编排 ────────────────────────────────
    demo_plan_execute().await?;

    // ── Part 3: 动态工具切换 ─────────────────────────────────────────────────
    demo_dynamic_tools().await?;

    // ── Part 4: Workflow 流式执行 ─────────────────────────────────────────────
    demo_workflow_stream().await?;

    // ── Part 5: 拓扑追踪 ───────────────────────────────────────────────────────
    demo_topology_tracking().await?;

    // ── Part 6: Agent Handoff ────────────────────────────────────────────────────
    demo_agent_handoff().await?;

    println!("\n═══════════════════════════════════════════════════════");
    println!("              综合示例演示完成！");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 1: 外部技能系统
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_external_skills() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 1: 外部技能系统 (File-based Skills)");
    println!("═══════════════════════════════════════════════════════\n");

    let skills_dir = std::path::Path::new("skills");
    if !skills_dir.exists() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：缺少 skills/ 目录，无法验证外部技能系统".to_string(),
        ));
    }

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("skill-agent")
        .system_prompt("你是一个全能助手，可以根据任务激活和使用不同的技能。")
        .enable_tools()
        .max_iterations(15)
        .build()?;

    // 发现并加载技能
    let discovered = agent
        .discover_skills(&[DiscoveryScope::Custom(skills_dir.into())])
        .await?;

    if discovered.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：未发现任何外部技能".to_string(),
        ));
    }

    println!("  ✓ 发现 {} 个技能:", discovered.len());
    println!("  ✓ 总技能数: {}", agent.skill_count());

    // 列出已注册的工具
    let tools = agent.list_tools();
    let registered_skill_tools = tools
        .iter()
        .filter(|tool| {
            tool.starts_with("activate_") || tool.starts_with("read_") || tool.starts_with("run_")
        })
        .count();
    if registered_skill_tools == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：发现了技能但未注册任何 skill 相关工具".to_string(),
        ));
    }
    println!("  ✓ 自动注册工具: {} 个", tools.len());

    for tool in &tools {
        if tool.starts_with("activate_") || tool.starts_with("read_") || tool.starts_with("run_") {
            println!("    • {}", tool);
        }
    }
    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 2: Plan-and-Execute
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_plan_execute() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 2: Plan-and-Execute 任务编排");
    println!("═══════════════════════════════════════════════════════\n");

    use echo_agent::agent::plan_execute::{Executor, StaticPlanner};

    struct VerboseExecutor;

    impl Executor for VerboseExecutor {
        fn execute_step<'a>(
            &'a mut self,
            description: &'a str,
            _context: &'a str,
        ) -> BoxFuture<'a, echo_agent::error::Result<String>> {
            Box::pin(async move {
                println!("  ▶ 执行步骤: {}", description);
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                Ok(format!("✓ 完成: {}", description))
            })
        }
    }

    let planner = StaticPlanner::new(vec![
        "检查项目状态和代码质量",
        "运行 CI/CD 管道检查",
        "生成部署报告",
        "总结项目健康状态",
    ]);

    let executor = VerboseExecutor;

    let agent = PlanExecuteAgent::new("devops_agent", planner, executor).max_replans(1);

    let task = "评估我的项目是否可以部署到生产环境";

    println!("  任务: {}\n", task);

    let result = agent.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：Plan-and-Execute 返回空结果".to_string(),
        ));
    }
    println!("\n  ✓ 最终结果:\n  {}\n", result);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 3: 动态工具切换
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_dynamic_tools() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 3: 动态工具切换");
    println!("═══════════════════════════════════════════════════════\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("dynamic-tools-agent")
        .system_prompt("你是一个助手，会根据任务阶段使用不同的工具。")
        .enable_tools()
        .max_iterations(10)
        .build()?;

    // Phase 1: 开发阶段工具
    let quality_tool = CodeQualityTool::new();
    let quality_counter = quality_tool.counter();
    agent.add_tool(Box::new(quality_tool));

    println!("  Phase 1: 开发阶段");
    println!("  可用工具: {:?}\n", agent.tool_names());

    let task1 = "分析这段代码的质量: fn main() { println!(\"Hello\"); }";

    println!("  任务: {}\n", task1);

    let result1 = agent.execute(task1).await?;
    if result1.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：开发阶段任务返回空结果".to_string(),
        ));
    }
    if quality_counter.load(Ordering::SeqCst) == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：开发阶段未实际调用 code_quality 工具".to_string(),
        ));
    }
    println!("  ✓ 结果: {}\n", result1);

    // Phase 2: 切换到运维阶段工具
    println!("  ─────────────────────────────────────────────────");
    println!("  Phase 2: 运维阶段（切换工具）\n");

    agent.remove_tool("code_quality");
    let ci_tool = CiCheckTool::new();
    let ci_counter = ci_tool.counter();
    agent.add_tool(Box::new(ci_tool));

    println!("  可用工具: {:?}\n", agent.tool_names());
    if agent.tool_names().contains(&"code_quality") {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：切换工具后 code_quality 仍然存在".to_string(),
        ));
    }
    if !agent.tool_names().contains(&"ci_check") {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：切换工具后 ci_check 未注册".to_string(),
        ));
    }

    let task2 = "检查 main 管道的 CI/CD 状态";

    println!("  任务: {}\n", task2);

    let result2 = agent.execute(task2).await?;
    if result2.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：运维阶段任务返回空结果".to_string(),
        ));
    }
    if ci_counter.load(Ordering::SeqCst) == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：运维阶段未实际调用 ci_check 工具".to_string(),
        ));
    }
    println!("  ✓ 结果: {}\n", result2);

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 4: Workflow 流式执行
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_workflow_stream() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 4: Workflow 流式执行");
    println!("═══════════════════════════════════════════════════════\n");

    let graph = GraphBuilder::new("devops_pipeline")
        .add_function_node("check", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 检查代码库状态...");
                let _ = state.set("status", "checking");
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                Ok(())
            })
        })
        .add_function_node("test", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 运行测试套件...");
                let _ = state.set("test_result", "passed");
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                Ok(())
            })
        })
        .add_function_node("build", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 构建部署包...");
                let _ = state.set("build_artifact", "app-v1.2.3.tar.gz");
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                Ok(())
            })
        })
        .add_function_node("deploy", |state: &SharedState| {
            Box::pin(async move {
                println!("    ▶ 部署到生产环境...");
                let _ = state.set("deployment_url", "https://prod.example.com");
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                Ok(())
            })
        })
        .set_entry("check")
        .add_edge("check", "test")
        .add_edge("test", "build")
        .add_edge("build", "deploy")
        .set_finish("deploy")
        .build()?;

    println!("  执行 CI/CD 流水线:\n");

    let state = SharedState::new();
    let mut stream = graph.run_stream(state).await?;
    let mut completed = false;
    let mut completed_steps = 0usize;

    while let Some(event) = stream.next().await {
        match event? {
            WorkflowEvent::NodeStart {
                node_name,
                step_index,
            } => {
                println!("  [Step {}] 开始: {}", step_index + 1, node_name);
            }
            WorkflowEvent::NodeEnd {
                node_name, elapsed, ..
            } => {
                println!("  [完成] {} (耗时: {:?})", node_name, elapsed);
            }
            WorkflowEvent::Completed {
                total_steps,
                elapsed,
                ..
            } => {
                completed = true;
                completed_steps = total_steps;
                println!("\n  ✓ 流水线执行完成");
                println!("  总步骤数: {}", total_steps);
                println!("  总耗时: {:?}", elapsed);
            }
            _ => {}
        }
    }

    if !completed || completed_steps != 4 {
        return Err(echo_agent::error::ReactError::Other(format!(
            "综合验收失败：Workflow 未按预期完成（completed={completed}, steps={completed_steps}）"
        )));
    }

    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 5: 拓扑追踪
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_topology_tracking() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 5: 拓扑追踪");
    println!("═══════════════════════════════════════════════════════\n");

    let tracker = Arc::new(TopologyTracker::new());
    let callback = Arc::new(TopologyCallback::new(tracker.clone()));

    let mock_llm = Arc::new(
        MockLlmClient::new()
            .with_model_name("topology-mock")
            .then_tool_call(
                "call_code_quality",
                "code_quality",
                r#"{"code":"fn main() { println!(\"hi\"); }","language":"rust"}"#,
            )
            .with_response("这段 Rust 代码结构简单、质量良好，适合作为拓扑追踪演示输入。"),
    );

    let mut agent = ReactAgentBuilder::new()
        .name("tracked-agent")
        .llm_client(mock_llm)
        .system_prompt("你是助手，必须先调用 code_quality 工具，再总结结果。")
        .enable_tools()
        .callback(callback)
        .build()?;

    agent.add_tool(Box::new(CodeQualityTool::new()));

    println!("  Agent 执行任务时的工具调用将被自动追踪...\n");

    let task = "检查这段代码的质量，并给出一句总结。";
    println!("  任务: {}\n", task);

    let result = agent.execute(task).await?;
    if result.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：拓扑追踪任务返回空结果".to_string(),
        ));
    }

    // 显示拓扑图
    println!("  拓扑图 (Mermaid 格式):\n");
    println!("{}", tracker.to_mermaid());

    // 显示统计
    let stats = tracker.stats();
    println!("\n  统计信息:");
    println!("    节点数: {}", stats.node_count);
    println!("    边数: {}", stats.edge_count);
    println!("    总调用: {}", stats.total_calls);
    if stats.total_calls == 0 || stats.node_count == 0 {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：TopologyTracker 未记录任何调用".to_string(),
        ));
    }

    println!();

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Part 6: Agent Handoff
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

async fn demo_agent_handoff() -> Result<()> {
    println!("═══════════════════════════════════════════════════════");
    println!("Part 6: Agent Handoff");
    println!("═══════════════════════════════════════════════════════\n");

    // 创建专业 Agent
    let developer = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("developer")
        .system_prompt("你是开发专家，擅长代码编写和技术分析。")
        .build()?;

    let analyst = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("analyst")
        .system_prompt("你是业务分析师，擅长需求分析和数据处理。")
        .build()?;

    // 创建 Handoff 管理器
    let mut manager = HandoffManager::new();
    manager.register("developer", developer);
    manager.register("analyst", analyst);

    println!("  已注册 Agent: {:?}\n", manager.registered_agents());
    if manager.registered_agents().len() != 2 {
        return Err(echo_agent::error::ReactError::Other(
            "综合验收失败：HandoffManager 注册的 Agent 数量不正确".to_string(),
        ));
    }

    // 场景：技术问题转给开发者，业务问题转给分析师
    let scenarios = vec![
        ("分析用户增长数据趋势", "analyst", "正在分析用户增长数据..."),
        ("修复这段代码的 bug", "developer", "正在分析代码问题..."),
    ];

    for (task, expected_agent, _expected_prefix) in scenarios {
        println!("  场景: \"{}\"", task);

        let target = HandoffTarget::new(expected_agent).with_message(task);
        let context = HandoffContext::new().with_source("user");

        let result = manager.handoff(target, context).await?;
        if result.target_agent != expected_agent {
            return Err(echo_agent::error::ReactError::Other(format!(
                "综合验收失败：handoff 目标错误，预期 `{expected_agent}`，实际 `{}`",
                result.target_agent
            )));
        }
        if result.output.trim().is_empty() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "综合验收失败：handoff 到 `{expected_agent}` 返回空结果"
            )));
        }
        println!("    → 转发给: {}", result.target_agent);
        println!("    → 结果: {}", result.output);
        println!();
    }

    Ok(())
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

fn print_banner() {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║        Echo Agent 企业级工作流自动化 - 综合示例          ║");
    println!("║                                                                ║");
    println!("║  展示核心能力：                                                 ║");
    println!("║  • 外部技能 • Plan-Execute • 动态工具 • Workflow 流式           ║");
    println!("║  • 拓扑追踪 • Agent Handoff • SQLite 记忆 • 语义检索           ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}
