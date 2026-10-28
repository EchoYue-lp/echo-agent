//! demo61: Agent Factory — 工厂模式创建 Agent
//!
//! 演示 Agent Factory 抽象：
//!
//! 1. AgentFactoryConfig — 声明式配置：model + prompt + tools
//! 2. DefaultAgentFactory — 根据配置创建 Agent
//!
//! ```bash
//! cargo run -p echo-agent-learning --example demo61_agent_factory --features testing
//! ```

use echo_agent::agent::factory::{AgentFactory, AgentFactoryConfig, DefaultAgentFactory};
use echo_agent::agent::{Agent, AgentEvent, AgentInvocationContext, CancellationToken};
use echo_agent::error::Result;
use echo_agent::prelude::ReactAgentBuilder;
use echo_agent::testing::MockLlmClient;
use futures::StreamExt;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("═══════════════════════════════════════════════════════");
    println!("    demo61: Agent Factory");
    println!("═══════════════════════════════════════════════════════\n");

    // ── Part 1：AgentFactoryConfig — 声明式配置 ───────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 1：AgentFactoryConfig — 声明式 Agent 配置");
    println!("───────────────────────────────────────────────────────\n");

    let config = AgentFactoryConfig::new()
        .model("qwen3-max")
        .name("code-reviewer")
        .with_system_prompt(
            "You are a code review expert. Analyze code for bugs, security issues, and style.",
        );

    println!("  完整构建示例：");
    println!("    model      : {}", config.model_name());
    println!("    name       : {}", config.agent_name());
    let prompt_preview: String = config.system_prompt().chars().take(60).collect();
    println!("    prompt     : {}…", prompt_preview);
    println!();

    // ── Part 2：DefaultAgentFactory — 工厂创建 Agent ──────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 2：DefaultAgentFactory — 工厂创建 Agent");
    println!("───────────────────────────────────────────────────────\n");

    let factory = DefaultAgentFactory;

    let factory_config = AgentFactoryConfig::new()
        .model("mock-factory")
        .name("factory-react-agent")
        .with_system_prompt("You are a helpful coding assistant.");

    let agent = factory.create_agent(factory_config)?;
    println!("  ✓ 工厂创建 Agent 成功");
    println!("    name  : {}", agent.name());
    println!("    model : {}", agent.model_name());
    println!();

    // ── Part 3：手动 Builder + Mock 执行 ─────────────────────────────────
    println!("───────────────────────────────────────────────────────");
    println!("Part 3：手动 Builder + Mock 执行");
    println!("───────────────────────────────────────────────────────\n");

    let mock_client = Arc::new(
        MockLlmClient::new()
            .with_model_name("mock-factory")
            .with_response(r###"Factory-created agent is working!"###)
            .with_response(r###"Invocation uses its original hard tool scope."###),
    );

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

    // Carry the positive scope until the Agent acquires its execution lock.
    // Later registrations cannot widen it; initial visibility is independent.
    let mut scoped = manual_agent
        .execute_stream_with_invocation_context(
            "Only report the scoped result.",
            CancellationToken::new(),
            AgentInvocationContext {
                // 同一调用同时声明只读与工具集合上界，不修改共享 Agent 的配置。
                tool_access: Some(echo_agent::tools::ToolAccess::ReadOnly),
                allowed_tools: Some(std::collections::HashSet::from(["task_list".to_string()])),
                ..Default::default()
            },
        )
        .await?;
    while let Some(event) = scoped.next().await {
        if let AgentEvent::FinalAnswer(answer) = event? {
            println!("  ✓ Invocation 范围：{}", answer.trim());
        }
    }

    println!("\n═══════════════════════════════════════════════════════");
    println!("    demo61 完成");
    println!("═══════════════════════════════════════════════════════");

    Ok(())
}
