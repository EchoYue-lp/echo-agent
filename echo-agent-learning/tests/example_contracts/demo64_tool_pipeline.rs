//! demo64_tool_pipeline —— ToolExecutionPipeline 综合演示
//!
//! 展示工具执行管线的完整架构：
//!
//! 阶段总览由真实工具调用的生产 tracing 事件生成，不维护第二份静态顺序。
//!
//! Contract test: `contract_demo64_tool_pipeline`.

use echo_agent::prelude::*;
use echo_agent::testing::MockLlmClient;
use echo_agent::tool;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

#[derive(Default)]
struct StageVisitor {
    stage: Option<String>,
}

impl Visit for StageVisitor {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "stage" {
            self.stage = Some(value.to_string());
        }
    }
}

struct StageTraceLayer {
    stages: Arc<Mutex<Vec<String>>>,
}

impl<S: Subscriber> Layer<S> for StageTraceLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "echo_agent::agent::react::run::pipeline" {
            return;
        }
        let mut visitor = StageVisitor::default();
        event.record(&mut visitor);
        if let Some(stage) = visitor.stage
            && let Ok(mut stages) = self.stages.lock()
        {
            stages.push(stage);
        }
    }
}

async fn observed_default_stages() -> echo_agent::error::Result<Vec<String>> {
    let stages = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(StageTraceLayer {
        stages: stages.clone(),
    });
    let llm = Arc::new(
        MockLlmClient::new()
            .then_tool_call("call_add", "add", r#"{"a":15.0,"b":27.0}"#)
            .with_response("42"),
    );
    let mut agent = ReactAgent::new(AgentConfig::new(
        "qwen3-max",
        "stage_observer",
        "Calculate.",
    ))
    .with_llm_client(llm);
    agent.add_tool(Box::new(AddTool));
    let guard = tracing::subscriber::set_default(subscriber);
    let answer = agent.execute("15 + 27").await?;
    drop(guard);
    assert_eq!(answer.trim(), "42");
    let observed = stages.lock().map_err(|_| {
        echo_agent::error::ReactError::Other("stage trace lock poisoned".to_string())
    })?;
    Ok(observed.clone())
}

#[tokio::test(flavor = "current_thread")]
async fn contract_demo64_stage_inventory_matches_overview() -> echo_agent::error::Result<()> {
    let observed = observed_default_stages().await?;
    verify_stage_contract(&observed)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn contract_demo64_rejects_duplicate_production_stage() -> echo_agent::error::Result<()> {
    let mut observed = observed_default_stages().await?;
    observed.push("execute".to_string());
    assert!(verify_stage_contract(&observed).is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn contract_demo64_rejects_invalid_pre_execution_order() -> echo_agent::error::Result<()> {
    let observed = observed_default_stages().await?;
    verify_stage_contract(&observed)?;
    for (before, after) in [
        ("intervention", "tool_visibility"),
        ("tool_visibility", "plan_mode"),
        ("plan_mode", "pre_tool_use_hook"),
        ("intervention", "plan_mode"),
        ("intervention", "pre_tool_use_hook"),
        ("pre_tool_use_hook", "permission"),
        ("permission", "read_before_edit"),
    ] {
        let mut invalid = observed.clone();
        let before_position = stage_position(&invalid, before)?;
        let after_position = stage_position(&invalid, after)?;
        invalid.swap(before_position, after_position);
        assert!(
            verify_stage_contract(&invalid).is_err(),
            "{before} and {after} may not change places"
        );
    }
    Ok(())
}

// Description catalog, alphabetically keyed; the runtime trace alone defines order.
const STAGE_INFO: [(&str, &str, &str); 17] = [
    ("audit", "AuditStage", "审计结算后的工具终态"),
    (
        "callback_end",
        "CallbackStage(End)",
        "按真实终态分流 on_tool_end / on_tool_error",
    ),
    (
        "callback_start",
        "CallbackStage(Start)",
        "通知观察者工具即将执行",
    ),
    ("execute", "ExecuteStage", "ToolManager 校验并执行工具"),
    (
        "intervention",
        "InterventionStage",
        "干预回调：block / cancel / redirect / modify_args",
    ),
    (
        "invocation",
        "InvocationStage",
        "发布 requested/effective canonical invocation",
    ),
    ("output_guard", "OutputGuardStage", "守卫工具结果及错误投影"),
    ("permission", "PermissionStage", "检查本次工具调用的权限"),
    ("plan_mode", "PlanModeStage", "计划模式下阻止写操作"),
    (
        "post_tool_use_hook",
        "PostToolUseHookStage",
        "执行后 Hook 检查结果",
    ),
    (
        "pre_tool_use_hook",
        "PreToolUseHookStage",
        "执行前 Hook 可修改输入或阻止调用",
    ),
    (
        "read_before_edit",
        "ReadBeforeEditStage",
        "按配置要求先读后改",
    ),
    (
        "skill_permission",
        "SkillPermissionStage",
        "检查激活技能的工具许可",
    ),
    (
        "tool_input_guard",
        "ToolInputGuardStage",
        "在有效调用被发布前检查最终输入",
    ),
    (
        "tool_visibility",
        "ToolVisibilityStage",
        "检查工具是否在本次调用中可见",
    ),
    (
        "trace_recording",
        "TraceRecordingStage",
        "记录结算后的 ToolResult / ToolError",
    ),
    ("truncation", "TruncationStage", "输出预算及 artifact 处理"),
];

fn stage_info(name: &str) -> Option<(&'static str, &'static str)> {
    STAGE_INFO
        .iter()
        .find(|(key, _, _)| *key == name)
        .map(|(_, display, description)| (*display, *description))
}

fn stage_position(stages: &[String], name: &str) -> echo_agent::error::Result<usize> {
    stages
        .iter()
        .position(|stage| stage == name)
        .ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!("production pipeline omitted {name}"))
        })
}

fn verify_stage_contract(stages: &[String]) -> echo_agent::error::Result<()> {
    for (name, _, _) in STAGE_INFO {
        let count = stages.iter().filter(|stage| stage.as_str() == name).count();
        if count != 1 {
            return Err(echo_agent::error::ReactError::Other(format!(
                "production pipeline stage {name} occurred {count} times"
            )));
        }
    }
    for stage in stages {
        if stage_info(stage).is_none() {
            return Err(echo_agent::error::ReactError::Other(format!(
                "document the new production pipeline stage: {stage}"
            )));
        }
    }
    for (before, after) in [
        ("intervention", "tool_visibility"),
        ("tool_visibility", "plan_mode"),
        ("plan_mode", "pre_tool_use_hook"),
        ("pre_tool_use_hook", "permission"),
        ("permission", "read_before_edit"),
        ("tool_visibility", "invocation"),
        ("plan_mode", "invocation"),
        ("permission", "tool_input_guard"),
        ("skill_permission", "invocation"),
        ("tool_input_guard", "invocation"),
        ("invocation", "callback_start"),
        ("callback_start", "execute"),
        ("execute", "post_tool_use_hook"),
        ("post_tool_use_hook", "output_guard"),
        ("output_guard", "truncation"),
        ("truncation", "trace_recording"),
        ("trace_recording", "audit"),
        ("audit", "callback_end"),
    ] {
        if stage_position(stages, before)? >= stage_position(stages, after)? {
            return Err(echo_agent::error::ReactError::Other(format!(
                "production pipeline order violated: {before} must precede {after}"
            )));
        }
    }
    Ok(())
}

// ── 工具定义 ──────────────────────────────────────────────────────────────────

#[tool(name = "add", description = "两数相加")]
async fn add(
    /// 第一个数
    a: f64,
    /// 第二个数
    b: f64,
) -> Result<ToolResult> {
    Ok(ToolResult::success(format!("{}", a + b)))
}

// ── 入口 ──────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn contract_demo64_tool_pipeline() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=warn,demo64_tool_pipeline=info".into()),
        )
        .try_init();

    print_banner();

    // ── Part 1: Pipeline 阶段总览 ────────────────────────────────────────────
    let stages = observed_default_stages().await?;
    verify_stage_contract(&stages)?;
    separator(&format!(
        "Part 1: ToolExecutionPipeline {} 阶段总览",
        stages.len()
    ));
    demo_pipeline_stages(&stages)?;

    // ── Part 2: InterventionCallback — 第一阶段拦截 ──────────────────────────
    separator("Part 2: InterventionCallback — 干预回调");
    demo_intervention_callback().await?;

    // ── Part 3: AgentCallback — 管线观测回调 ──────────────────────────────────
    separator("Part 3: AgentCallback — 管线阶段回调");
    demo_agent_callback().await?;

    // ── Part 4: ToolExecutionConfig — 执行策略配置 ────────────────────────────
    separator("Part 4: ToolExecutionConfig — 执行策略配置");
    demo_execution_config()?;

    println!("\n{}", "═".repeat(64));
    println!("  demo64 完成 ✓");
    println!("{}", "═".repeat(64));
    Ok(())
}

// ── Part 1: Pipeline 阶段总览 ─────────────────────────────────────────────────

fn demo_pipeline_stages(stages: &[String]) -> echo_agent::error::Result<()> {
    println!("  从真实 ToolExecutionPipeline::default_pipeline() 调用观测到的阶段:\n");
    for (index, stage) in stages.iter().enumerate() {
        let (name, desc) = stage_info(stage).ok_or_else(|| {
            echo_agent::error::ReactError::Other(format!("unknown pipeline stage: {stage}"))
        })?;
        println!("  [{:>2}] {name:<24} — {desc}", index + 1);
    }

    println!("\n  关键设计:");
    println!("    • 执行前 block 短路；执行后 block 仍完成输出处理、trace、audit 与终态回调");
    println!("    • InterventionStage 是最高优先级决策点（在工具执行之前）");
    println!("    • ExecuteStage 的错误被转化为 ToolResult {{ success: false }}");
    println!("      而非 Err，确保 Trace / Callback 等后续阶段仍然执行");
    println!("    • PlanModeStage 阻止 shell / apply_patch 等写工具");
    println!("    • ReadBeforeEditStage 仅在 force_read_before_edit = true 时生效");

    println!("  → Pipeline 阶段总览 ✓");
    Ok(())
}

// ── Part 2: InterventionCallback ──────────────────────────────────────────────

/// 自定义干预回调：阻止 shell 工具，为 web_search 注入额外上下文
struct DemoIntervention {
    call_count: AtomicUsize,
}

impl DemoIntervention {
    fn new() -> Self {
        Self {
            call_count: AtomicUsize::new(0),
        }
    }
}

impl InterventionCallback for DemoIntervention {
    fn on_tool_call<'a>(
        &'a self,
        _agent: &'a str,
        tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, InterventionResult> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let result = match tool {
            "shell" => InterventionResult::block("Demo: shell commands are restricted"),
            "web_search" => InterventionResult::inject("Note: prefer academic sources"),
            _ => InterventionResult::allow(),
        };
        Box::pin(async move { result })
    }
}

async fn demo_intervention_callback() -> echo_agent::error::Result<()> {
    // ── 2a. InterventionResult 各构造方法 ──
    let allow = InterventionResult::allow();
    println!(
        "  InterventionResult::allow()      → block={}, cancel={}",
        allow.block, allow.cancel
    );
    assert!(!allow.block && !allow.cancel);

    let block = InterventionResult::block("sensitive operation");
    println!(
        "  InterventionResult::block(...)    → block=true, reason=\"{}\"",
        block.block_reason.as_deref().unwrap_or("")
    );
    assert!(block.block);

    let inject = InterventionResult::inject("extra context here");
    println!("  InterventionResult::inject(...)   → injected_context=Some(...)");
    assert!(inject.injected_context.is_some());

    let cancel = InterventionResult::cancel();
    println!("  InterventionResult::cancel()      → cancel=true");
    assert!(cancel.cancel);

    let modify = InterventionResult::modify_args(json!({"path": "/safe/path"}));
    println!("  InterventionResult::modify_args() → modified_args=Some(...)");
    assert!(modify.modified_args.is_some());

    // ── 2b. 自定义干预回调对不同工具的响应 ──
    let intervention = Arc::new(DemoIntervention::new());

    println!("\n  模拟 InterventionCallback 对不同工具的响应:");
    let r1 = intervention
        .on_tool_call("agent", "shell", &json!({}))
        .await;
    println!("    shell      → block={}", r1.block);
    assert!(r1.block);

    let r2 = intervention
        .on_tool_call("agent", "web_search", &json!({}))
        .await;
    println!("    web_search → inject={}", r2.injected_context.is_some());
    assert!(r2.injected_context.is_some());

    let r3 = intervention
        .on_tool_call("agent", "read_file", &json!({}))
        .await;
    println!(
        "    read_file  → allow (block={}, cancel={})",
        r3.block, r3.cancel
    );
    assert!(!r3.block && !r3.cancel);

    let total = intervention.call_count.load(Ordering::SeqCst);
    println!("  总调用次数: {total}");
    assert_eq!(total, 3);

    // ── 2c. 通过 ReactAgentBuilder 注入干预回调 ──
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("intervention_demo")
        .system_prompt("你是一个助手")
        .intervention_callback(intervention.clone())
        .build()?;

    println!("\n  ReactAgentBuilder::add_intervention_callback() → 注册成功");
    assert!(agent.tool_names().contains(&"final_answer".to_string()));

    println!("  → InterventionCallback ✓");
    Ok(())
}

// ── Part 3: AgentCallback — 管线观测回调 ──────────────────────────────────────

/// 追踪管线事件的回调
struct PipelineTracker {
    tool_starts: AtomicUsize,
    tool_ends: AtomicUsize,
    add_starts: AtomicUsize,
    add_ends: AtomicUsize,
    iterations: AtomicUsize,
}

impl PipelineTracker {
    fn new() -> Self {
        Self {
            tool_starts: AtomicUsize::new(0),
            tool_ends: AtomicUsize::new(0),
            add_starts: AtomicUsize::new(0),
            add_ends: AtomicUsize::new(0),
            iterations: AtomicUsize::new(0),
        }
    }
}

impl AgentCallback for PipelineTracker {
    fn on_tool_start<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        _args: &'a Value,
    ) -> BoxFuture<'a, ()> {
        self.tool_starts.fetch_add(1, Ordering::SeqCst);
        if tool == "add" {
            self.add_starts.fetch_add(1, Ordering::SeqCst);
        }
        println!("    📍 CallbackStage(Start) [{agent}] → {tool}");
        Box::pin(async {})
    }

    fn on_tool_end<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        output: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.tool_ends.fetch_add(1, Ordering::SeqCst);
        if tool == "add" {
            self.add_ends.fetch_add(1, Ordering::SeqCst);
        }
        let preview: String = output.chars().take(40).collect();
        println!("    📍 CallbackStage(End)   [{agent}] → {tool}: \"{preview}\"");
        Box::pin(async {})
    }

    fn on_tool_error<'a>(
        &'a self,
        agent: &'a str,
        tool: &'a str,
        err: &'a echo_agent::error::ReactError,
    ) -> BoxFuture<'a, ()> {
        println!("    📍 ToolError [{agent}] → {tool}: {err}");
        Box::pin(async {})
    }

    fn on_final_answer<'a>(&'a self, _agent: &'a str, answer: &'a str) -> BoxFuture<'a, ()> {
        let preview: String = answer.chars().take(50).collect();
        println!("    📍 FinalAnswer → \"{preview}\"");
        Box::pin(async {})
    }

    fn on_iteration<'a>(&'a self, _agent: &'a str, iteration: usize) -> BoxFuture<'a, ()> {
        self.iterations.fetch_add(1, Ordering::SeqCst);
        println!("    📍 Iteration #{iteration}");
        Box::pin(async {})
    }
}

async fn demo_agent_callback() -> echo_agent::error::Result<()> {
    println!("  创建带 AgentCallback 的 Agent，观察管线回调事件\n");

    let tracker = Arc::new(PipelineTracker::new());
    let mock_client = Arc::new(
        MockLlmClient::new()
            .with_model_name("pipeline-mock")
            .then_tool_call("call_add", "add", r#"{"a":15.0,"b":27.0}"#)
            .then_tool_call(
                "call_final_answer",
                "final_answer",
                r#"{"answer":"15 + 27 = 42"}"#,
            ),
    );

    let config = AgentConfig::new(
        "qwen3-max",
        "pipeline_tracker_agent",
        "你是一个计算助手。使用 add 工具完成计算。",
    )
    .tool_execution(ToolExecutionConfig {
        timeout_ms: 10_000,
        ..ToolExecutionConfig::default()
    })
    .with_callback(tracker.clone());

    let mut agent = ReactAgent::new(config).with_llm_client(mock_client);
    agent.add_tool(Box::new(AddTool));

    println!("  执行: \"计算 15 + 27\"");
    let answer = agent.execute("计算 15 + 27").await?;
    println!("\n  最终答案: {answer}");

    let starts = tracker.tool_starts.load(Ordering::SeqCst);
    let ends = tracker.tool_ends.load(Ordering::SeqCst);
    let add_starts = tracker.add_starts.load(Ordering::SeqCst);
    let add_ends = tracker.add_ends.load(Ordering::SeqCst);
    let iters = tracker.iterations.load(Ordering::SeqCst);
    println!("\n  管线回调统计:");
    println!("    on_tool_start: {starts} 次 (对应 CallbackStage(Start))");
    println!("    on_tool_end:   {ends} 次 (对应 CallbackStage(End))");
    println!("    on_iteration:  {iters} 次");

    assert_eq!(answer.trim(), "15 + 27 = 42");
    assert_eq!(add_starts, 1, "add should trigger one start callback");
    assert_eq!(add_ends, 1, "add should trigger one end callback");

    println!("\n  回调触发的管线阶段:");
    println!("    on_tool_start → CallbackStage(Start)");
    println!("    on_tool_end / on_tool_error → CallbackStage(End)");
    println!("    on_final_answer → 最终答案输出时触发");
    println!("    on_iteration → 每轮 ReAct 循环结束时触发");

    println!("  → AgentCallback ✓");
    Ok(())
}

// ── Part 4: ToolExecutionConfig ───────────────────────────────────────────────

fn demo_execution_config() -> echo_agent::error::Result<()> {
    println!("  ToolExecutionConfig 的各配置项与管线阶段的关系\n");

    // ── 6a. 默认配置 ──
    let default_config = ToolExecutionConfig::default();
    println!("  默认配置:");
    println!("    timeout_ms:      {} (30 秒)", default_config.timeout_ms);
    println!("    retry_on_fail:   {}", default_config.retry_on_fail);
    println!("    max_retries:     {}", default_config.max_retries);
    println!("    retry_delay_ms:  {}", default_config.retry_delay_ms);
    println!("    max_concurrency: {:?}", default_config.max_concurrency);
    println!(
        "    max_read_concurrency: {:?}",
        default_config.max_read_concurrency
    );

    // ── 6b. 自定义配置 ──
    let custom = ToolExecutionConfig {
        timeout_ms: 5_000,
        retry_on_fail: true,
        max_retries: 3,
        retry_delay_ms: 200,
        max_concurrency: Some(4),
        max_read_concurrency: Some(16),
    };

    println!("\n  自定义配置:");
    println!("    timeout_ms:      {} (5 秒超时)", custom.timeout_ms);
    println!("    retry_on_fail:   {} (启用重试)", custom.retry_on_fail);
    println!("    max_retries:     {} (最多 3 次)", custom.max_retries);
    println!(
        "    retry_delay_ms:  {} (200ms 间隔)",
        custom.retry_delay_ms
    );
    println!(
        "    max_concurrency: {:?} (最多 4 并发)",
        custom.max_concurrency
    );
    println!(
        "    max_read_concurrency: {:?} (最多 16 读并发)",
        custom.max_read_concurrency
    );

    // ── 6c. 验证 AgentConfig 中的配置 ──
    let agent_config = AgentConfig::new("qwen3-max", "pipeline_config_agent", "你是一个助手。")
        .tool_execution(custom);

    let exec = agent_config.get_tool_execution();
    assert_eq!(exec.timeout_ms, 5_000);
    assert!(exec.retry_on_fail);
    assert_eq!(exec.max_retries, 3);
    assert_eq!(exec.retry_delay_ms, 200);
    assert_eq!(exec.max_concurrency, Some(4));
    assert_eq!(exec.max_read_concurrency, Some(16));
    println!("\n  AgentConfig::get_tool_execution() 验证通过 ✓");

    // ── 6d. 配置项与管线阶段的映射 ──
    println!("\n  配置项 → 管线阶段映射:");
    println!("    ┌─────────────────┬──────────────────────┬────────────────────────┐");
    println!("    │ 配置项           │ 管线阶段              │ 作用                   │");
    println!("    ├─────────────────┼──────────────────────┼────────────────────────┤");
    println!("    │ timeout_ms      │ ExecuteStage         │ 工具执行超时            │");
    println!("    │ retry_on_fail   │ ExecuteStage         │ 失败自动重试            │");
    println!("    │ max_retries     │ ExecuteStage         │ 最大重试次数            │");
    println!("    │ retry_delay_ms  │ ExecuteStage         │ 重试间隔                │");
    println!("    │ max_concurrency │ ExecuteStage (外层)   │ 并行工具调用限流        │");
    println!("    │ max_read_concurrency │ ExecuteStage     │ 只读工具并发限流        │");
    println!("    └─────────────────┴──────────────────────┴────────────────────────┘");

    // ── 6e. 管线短路场景总结 ──
    println!("\n  管线短路场景（ctx.blocked = true）:");
    println!("    • InterventionStage  → 干预回调 block=true");
    println!("    • ToolVisibilityStage→ 工具不可见或不在当前执行面");
    println!("    • PlanModeStage      → 计划模式下调用写操作工具");
    println!("    • PreToolUseHookStage→ Hook 阻止执行");
    println!("    • ReadBeforeEditStage→ 编辑文件前未先读取");
    println!("    • PostToolUseHookStage 的执行后 block 仍完成输出处理与终态观察");

    println!("  → ToolExecutionConfig ✓");
    Ok(())
}

// ── 辅助 ──────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × Tool Pipeline (demo64)");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
