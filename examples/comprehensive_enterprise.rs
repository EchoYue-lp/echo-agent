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
struct CiCheckTool;

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
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=info,enterprise=info".into()),
        )
        .init();

    print_banner();

    if !has_llm_config() {
        println!("⚠️  未检测到 LLM API 密钥\n");
        println!("请设置环境变量：");
        println!("  - QWEN_API_KEY");
        println!("  - OPENAI_API_KEY");
        println!("  - DEEPSEEK_API_KEY\n");
        return Ok(());
    }

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
        println!("  [跳过] ./skills/ 目录不存在");
        println!("  提示: 创建 skills/ 目录并添加 SKILL.md 文件以启用此功能\n");
        return Ok(());
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

    println!("  ✓ 发现 {} 个技能:", discovered.len());
    println!("  ✓ 总技能数: {}", agent.skill_count());

    // 列出已注册的工具
    let tools = agent.list_tools();
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

    use echo_agent::agents::plan_execute::{Executor, StaticPlanner};

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

    let mut agent = PlanExecuteAgent::new("devops_agent", planner, executor).max_replans(1);

    let task = "评估我的项目是否可以部署到生产环境";

    println!("  任务: {}\n", task);

    match agent.execute(task).await {
        Ok(result) => {
            println!("\n  ✓ 最终结果:\n  {}\n", result);
        }
        Err(e) => {
            println!("\n  ✗ 执行失败: {}\n", e);
        }
    }

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
    agent.add_tool(Box::new(CodeQualityTool::new()));

    println!("  Phase 1: 开发阶段");
    println!("  可用工具: {:?}\n", agent.tool_names());

    let task1 = "分析这段代码的质量: fn main() { println!(\"Hello\"); }";

    println!("  任务: {}\n", task1);

    match agent.execute(task1).await {
        Ok(result) => println!("  ✓ 结果: {}\n", result),
        Err(e) => println!("  ✗ 失败: {}\n", e),
    }

    // Phase 2: 切换到运维阶段工具
    println!("  ─────────────────────────────────────────────────");
    println!("  Phase 2: 运维阶段（切换工具）\n");

    agent.remove_tool("code_quality");
    agent.add_tool(Box::new(CiCheckTool));

    println!("  可用工具: {:?}\n", agent.tool_names());

    let task2 = "检查 main 管道的 CI/CD 状态";

    println!("  任务: {}\n", task2);

    match agent.execute(task2).await {
        Ok(result) => println!("  ✓ 结果: {}\n", result),
        Err(e) => println!("  ✗ 失败: {}\n", e),
    }

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
                println!("\n  ✓ 流水线执行完成");
                println!("  总步骤数: {}", total_steps);
                println!("  总耗时: {:?}", elapsed);
            }
            _ => {}
        }
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

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("tracked-agent")
        .system_prompt("你是助手")
        .enable_tools()
        .callback(callback)
        .build()?;

    agent.add_tool(Box::new(CodeQualityTool::new()));

    println!("  Agent 执行任务时的工具调用将被自动追踪...\n");

    let task = "检查这段代码的质量";
    println!("  任务: {}\n", task);

    let _ = agent.execute(task).await;

    // 显示拓扑图
    println!("  拓扑图 (Mermaid 格式):\n");
    println!("{}", tracker.to_mermaid());

    // 显示统计
    let stats = tracker.stats();
    println!("\n  统计信息:");
    println!("    节点数: {}", stats.node_count);
    println!("    边数: {}", stats.edge_count);
    println!("    总调用: {}", stats.total_calls);

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

    // 场景：技术问题转给开发者，业务问题转给分析师
    let scenarios = vec![
        ("分析用户增长数据趋势", "analyst", "正在分析用户增长数据..."),
        ("修复这段代码的 bug", "developer", "正在分析代码问题..."),
    ];

    for (task, expected_agent, expected_prefix) in scenarios {
        println!("  场景: \"{}\"", task);

        let target = HandoffTarget::new(expected_agent).with_message(task);
        let context = HandoffContext::new().with_source("user");

        match manager.handoff(target, context).await {
            Ok(result) => {
                println!("    → 转发给: {}", result.target_agent);
                assert!(result.output.starts_with(expected_prefix));
                println!("    → 结果: {}", result.output);
            }
            Err(e) => {
                println!("    → 错误: {}", e);
            }
        }
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

fn has_llm_config() -> bool {
    std::env::var("QWEN_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
}
