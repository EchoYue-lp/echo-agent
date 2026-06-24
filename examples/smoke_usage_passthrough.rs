//! 端到端冒烟测试：验证阶段 A 真实 usage 透传链路。
//!
//! 用真实 DeepSeek provider 跑一次 subagent delegate，断言：
//! 1. `delegate_to_agent_with_parent_and_cancel` 返回 `SubagentResult`
//! 2. `result.usage` 非 None（A2 捕获了 LlmUsage）
//! 3. `usage.prompt_tokens + completion_tokens > 0`（真实数据，非假占位）
//! 4. `usage.model` 非 "unknown"（真实模型名）
//! 5. `usage.usage_reported == true`（provider 真实上报）
//!
//! 运行：`cargo run --example smoke_usage_passthrough --features "subagent,tasks" --release`
//! 依赖 `~/.echo-agent/config.yaml` 里配置了 deepseek provider。

use std::sync::Arc;

use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    println!("🧪 阶段 A 端到端冒烟：真实 usage 透传\n");

    // 从 ~/.echo-agent/config.yaml 读取 auth_token，避免依赖环境变量。
    // 这是 echo-agent-cli GUI 应用的注入方式（见 infra.rs:188 build_llm_config）。
    let config_path = {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(home)
            .join(".echo-agent")
            .join("config.yaml")
    };
    let config_text = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("读取 {} 失败: {e}", config_path.display()))?;
    let config_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&config_text)?;
    let auth_token = config_yaml["model"]["auth_token"]
        .as_str()
        .ok_or("config.yaml 缺少 model.auth_token")?;
    let model_name = config_yaml["model"]["name"]
        .as_str()
        .unwrap_or("deepseek-v4-flash");
    println!("→ 已从 config.yaml 加载 auth_token (model={model_name})");

    // 用 LlmConfig::deepseek 直接构造 client（绕过 from_env 的环境变量依赖）
    let llm_config =
        echo_integration::providers::config::LlmConfig::deepseek(auth_token, model_name);
    let worker_llm = Arc::new(echo_integration::providers::OpenAiClient::new(llm_config)?);

    let worker = ReactAgentBuilder::new()
        .name("smoke-worker")
        .model(model_name)
        .system_prompt("You are a minimal echo worker. Reply with exactly one short sentence.")
        .llm_client(worker_llm)
        .max_iterations(1)
        .build()?;

    let mut main_agent = ReactAgentBuilder::new()
        .name("smoke-main")
        .model(model_name)
        .system_prompt("You are a smoke test orchestrator.")
        .enable_tools()
        .enable_subagent()
        .max_iterations(1)
        .build()?;

    main_agent.register_agent(Box::new(worker));

    println!("→ 调用 delegate_to_agent_with_parent_and_cancel ...");
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = main_agent
        .delegate_to_agent_with_parent_and_cancel(
            "smoke-worker",
            "Reply with: hello from smoke test",
            "smoke",
            cancel,
            0,
        )
        .await?;

    println!("\n📋 SubagentResult:");
    println!("  output: {} bytes", result.output.len());
    println!(
        "  output preview: {:?}",
        result.output.chars().take(120).collect::<String>()
    );

    // === 断言 A2/A3/A4 链路 ===
    println!("\n🔍 验证 usage 透传:");
    let mut all_pass = true;

    let usage = result.usage;
    let pass1 = usage.is_some();
    println!(
        "  [{}] usage 非 None（A2 捕获 LlmUsage）: {:?}",
        if pass1 { "✓" } else { "✗" },
        usage.is_some()
    );
    all_pass &= pass1;

    if let Some(ref stats) = usage {
        let pass2 = stats.prompt_tokens + stats.completion_tokens > 0;
        println!(
            "  [{}] tokens > 0（真实数据非假占位）: prompt={} completion={}",
            if pass2 { "✓" } else { "✗" },
            stats.prompt_tokens,
            stats.completion_tokens
        );
        all_pass &= pass2;

        let pass3 = stats.model != "unknown";
        println!(
            "  [{}] model 非 unknown: {}",
            if pass3 { "✓" } else { "✗" },
            stats.model
        );
        all_pass &= pass3;

        let pass4 = stats.usage_reported;
        println!(
            "  [{}] usage_reported == true: {}",
            if pass4 { "✓" } else { "✗" },
            stats.usage_reported
        );
        all_pass &= pass4;

        let pass5 = stats.call_count >= 1;
        println!(
            "  [{}] call_count >= 1: {}",
            if pass5 { "✓" } else { "✗" },
            stats.call_count
        );
        all_pass &= pass5;

        println!("\n  完整 usage stats: {:#?}", stats);
    }

    println!(
        "\n{}",
        if all_pass {
            "✅ 冒烟通过：阶段 A 真实 usage 透传链路验证成功"
        } else {
            "❌ 冒烟失败：usage 透传链路存在问题"
        }
    );

    if !all_pass {
        std::process::exit(1);
    }
    Ok(())
}
