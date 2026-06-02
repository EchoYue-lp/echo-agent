//! demo61: Agent Factory + Mode Engine — 工厂模式创建 Agent + 多语言模式引擎
//!
//! 演示 Agent Factory 和 Mode Engine 两大核心抽象：
//!
//! **Agent Factory（工厂模式）**
//! 1. AgentParadigm — 四种 Agent 范式：React / PlanExecute / SelfReflection / Structured
//! 2. AgentFactoryConfig — 声明式配置：paradigm + mode + model + prompt + tools
//! 3. DefaultAgentFactory — 根据配置创建 Agent
//!
//! **Mode Engine（模式引擎）**
//! 4. AgentMode — 五种工作模式：General / Coding / Research / Data / Writing
//! 5. DefaultModeEngine — 英文默认 prompt
//! 6. LocalizedModeEngine — 中文/多语言 prompt 覆盖
//!
//! ```bash
//! cargo run --example demo61_agent_factory --features testing
//! ```

use echo_agent::agent::factory::{
    AgentFactory, AgentFactoryConfig, AgentParadigm, DefaultAgentFactory,
};
use echo_agent::agent::{Agent, AgentMode, DefaultModeEngine, LocalizedModeEngine, ModeEngine};
use echo_agent::error::Result;
use echo_agent::prelude::ReactAgentBuilder;
use echo_agent::testing::MockLlmClient;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    demo61: Agent Factory + Mode Engine");
    println!("═══════════════════════════════════════════════════════\n");

    // ── Part 1：AgentParadigm — 四种 Agent 范式 ───────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1：AgentParadigm — Agent 执行范式");
    println!("───────────────────────────────────────────────────────\n");

    for paradigm in AgentParadigm::all() {
        println!("  {paradigm}");
    }
    println!();

    // Parse from string
    println!("  从字符串解析：");
    for name in &[
        "react",
        "plan-execute",
        "self-reflection",
        "structured",
        "unknown",
    ] {
        let result = AgentParadigm::from_name(name);
        println!("    \"{name}\" → {result:?}");
    }
    println!();

    // ── Part 2：AgentFactoryConfig — 声明式配置 ───────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2：AgentFactoryConfig — 声明式 Agent 配置");
    println!("───────────────────────────────────────────────────────\n");

    // Shorthand constructors
    let configs = vec![
        ("React", AgentFactoryConfig::react()),
        ("PlanExecute", AgentFactoryConfig::plan_execute()),
        ("SelfReflection", AgentFactoryConfig::self_reflection()),
        ("Structured", AgentFactoryConfig::structured()),
    ];

    for (label, config) in &configs {
        println!("  {label}:");
        println!("    paradigm   : {}", config.paradigm());
        println!("    model      : '{}'", config.model_name());
        println!("    name       : '{}'", config.agent_name());
        println!("    tool_count : {}", config.tool_count());
    }
    println!();

    // Full builder pattern
    let config = AgentFactoryConfig::react()
        .model("qwen3-max")
        .name("code-reviewer")
        .with_system_prompt(
            "You are a code review expert. Analyze code for bugs, security issues, and style.",
        )
        .with_mode(AgentMode::Coding);

    println!("  完整构建示例：");
    println!("    paradigm   : {}", config.paradigm());
    println!("    model      : {}", config.model_name());
    println!("    name       : {}", config.agent_name());
    println!("    mode       : {:?}", config.mode());
    println!(
        "    prompt     : {}…",
        &config.system_prompt()[..60.min(config.system_prompt().len())]
    );
    println!();

    // ── Part 3：DefaultAgentFactory — 工厂创建 Agent ──────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3：DefaultAgentFactory — 工厂创建 Agent");
    println!("───────────────────────────────────────────────────────\n");

    let factory = DefaultAgentFactory;

    // Create a React agent through the factory with a mock LLM client.
    // Note: DefaultAgentFactory uses ReactAgentBuilder internally.
    // For a fully working agent, we inject a MockLlmClient via the builder path.
    let mock_client = Arc::new(
        MockLlmClient::new()
            .with_model_name("mock-factory")
            .then_tool_call(
                "call_1",
                "final_answer",
                r#"{"answer":"Factory-created agent is working!"}"#,
            ),
    );

    // Method A: Direct factory (paradigm → builder mapping)
    let factory_config = AgentFactoryConfig::react()
        .model("mock-factory")
        .name("factory-react-agent")
        .with_system_prompt("You are a helpful coding assistant.");

    let agent = factory.create_agent(factory_config)?;
    println!("  ✓ 工厂创建 Agent 成功");
    println!("    name  : {}", agent.name());
    println!("    model : {}", agent.model_name());
    println!();

    // Method B: Manual builder (more control, e.g. injecting mock client)
    let manual_agent = ReactAgentBuilder::new()
        .llm_client(mock_client)
        .name("manual-react-agent")
        .model("mock-factory")
        .system_prompt("You are a helpful assistant.")
        .enable_tools()
        .build()?;

    let result = manual_agent.execute("Hello, factory agent!").await?;
    println!("  ✓ 手动构建 + Mock 执行：");
    println!("    result: {}", result.trim());
    println!();

    // ── Part 4：AgentMode — 五种工作模式 ──────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 4：AgentMode — 五种工作模式");
    println!("───────────────────────────────────────────────────────\n");

    for mode in AgentMode::all() {
        println!("  {mode}");
    }
    println!();

    // Parse from string
    println!("  从字符串解析：");
    for name in &[
        "general", "coding", "code", "research", "data", "writing", "unknown",
    ] {
        let result = AgentMode::from_name(name);
        println!("    \"{name}\" → {result:?}");
    }
    println!();

    // ── Part 5：DefaultModeEngine — 英文默认 prompt ──────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 5：DefaultModeEngine — 英文默认配置");
    println!("───────────────────────────────────────────────────────\n");

    let engine = DefaultModeEngine;
    for mode in engine.all_modes() {
        let config = engine.mode_config(&mode);
        let prompt_preview = if config.system_prompt_template.len() > 80 {
            format!("{}…", &config.system_prompt_template[..80])
        } else {
            config.system_prompt_template.clone()
        };
        println!("  {} {} ({})", config.icon, config.display_name, mode);
        println!("    prompt : {prompt_preview}");
        println!(
            "    tools  : [{}]",
            if config.recommended_tools.is_empty() {
                "all".to_string()
            } else {
                config.recommended_tools.join(", ")
            }
        );
        println!();
    }

    // ── Part 6：LocalizedModeEngine — 中文 prompt 覆盖 ────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 6：LocalizedModeEngine — 中文模式引擎");
    println!("───────────────────────────────────────────────────────\n");

    let zh_engine = LocalizedModeEngine::with_chinese();
    for mode in zh_engine.all_modes() {
        let config = zh_engine.mode_config(&mode);
        let prompt_preview = if config.system_prompt_template.len() > 60 {
            format!("{}…", &config.system_prompt_template[..60])
        } else {
            config.system_prompt_template.clone()
        };
        println!("  {} {} ({})", config.icon, config.display_name, mode);
        println!("    prompt : {prompt_preview}");
        println!();
    }

    // Parse Chinese mode names
    println!("  中文模式名解析：");
    for name in &["编程", "代码", "研究", "数据", "写作", "通用"] {
        let result = LocalizedModeEngine::from_str(name);
        println!("    \"{name}\" → {result:?}");
    }
    println!();

    // ── Part 7：自定义 override ───────────────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 7：自定义 Mode Override");
    println!("───────────────────────────────────────────────────────\n");

    let custom_engine = LocalizedModeEngine::new()
        .with_override(
            AgentMode::Coding,
            "You are a Rust specialist. Always prefer zero-cost abstractions.".into(),
        )
        .with_display_name(AgentMode::Coding, "Rust Expert".into());

    let config = custom_engine.mode_config(&AgentMode::Coding);
    println!("  自定义 Coding 模式：");
    println!("    display_name : {}", config.display_name);
    println!("    prompt       : {}", config.system_prompt_template);
    println!(
        "    tools        : [{}] (still from defaults)",
        config.recommended_tools.join(", ")
    );

    println!("\n═══════════════════════════════════════════════════════");
    println!("    demo61 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
