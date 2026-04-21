//! demo26_provider_factory.rs —— ProviderFactory 动态 Provider 演示
//!
//! 展示三种方式通过配置字符串自动实例化 LLM 客户端：
//! 1. `provider:model` 简写格式（如 `"ollama:llama3"`）
//! 2. 从配置文件/环境变量加载模型名称
//! 3. 从 `LlmConfig` 手动构建
//!
//! 运行方式：
//! ```bash
//! cargo run --example demo26_provider_factory
//! ```

use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo26_provider_factory=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1: provider:model 简写格式 ─────────────────────────────────────
    separator("Part 1: provider:model 简写格式");
    demo_provider_model_shorthand()?;

    // ── Part 2: 从配置文件加载 ───────────────────────────────────────────────
    separator("Part 2: 从配置文件/环境变量加载");
    demo_from_config_file()?;

    // ── Part 3: LlmConfig 手动构建 + build_client ────────────────────────────
    separator("Part 3: LlmConfig 手动构建");
    demo_from_llm_config()?;

    // ── Part 4: 自动 Provider 检测 ──────────────────────────────────────────
    separator("Part 4: 自动 Provider 类型检测");
    demo_auto_provider_detection()?;

    // ── Part 5: 实际对话测试 ─────────────────────────────────────────────────
    separator("Part 5: 通过 ProviderFactory 创建 Agent 并对话");
    demo_agent_with_factory().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo26 完成");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1 ──────────────────────────────────────────────────────────────────

fn demo_provider_model_shorthand() -> echo_agent::error::Result<()> {
    println!("  使用 ProviderFactory::create(\"provider:model\") 快速创建客户端\n");

    // 列出支持的 Provider
    println!(
        "  支持的 Provider: {:?}\n",
        ProviderFactory::supported_providers()
    );

    // 演示各种 provider:model 组合（仅构造，不实际发请求）
    let configs = [
        ("ollama:llama3", "Ollama 本地推理"),
        ("anthropic:claude-sonnet-4-6", "Anthropic Claude"),
        ("openai:gpt-4o", "OpenAI GPT-4o"),
        ("deepseek:deepseek-chat", "DeepSeek（OpenAI 兼容）"),
        ("dashscope:qwen3-max", "通义千问（OpenAI 兼容）"),
    ];

    for (config_str, desc) in &configs {
        let client = ProviderFactory::create(config_str)?;
        println!("  ✅ \"{config_str}\" → {} ({})", client.model_name(), desc);
    }

    println!();
    Ok(())
}

// ── Part 2 ──────────────────────────────────────────────────────────────────

fn demo_from_config_file() -> echo_agent::error::Result<()> {
    println!("  使用 ProviderFactory::create(\"model_name\") 从配置文件加载\n");

    // 尝试从配置文件加载（可能失败，取决于用户是否有配置文件）
    println!("  配置文件查找顺序：");
    println!("    1. $ECHO_AGENT_CONFIG 环境变量");
    println!("    2. ./echo-agent.yaml");
    println!("    3. ~/.echo-agent/config.yaml\n");

    // 列出已配置的模型
    let models = echo_agent::llm::config::Config::list_models();
    if models.is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo26 验收失败：未找到可用模型配置".to_string(),
        )
        .into());
    } else {
        println!("  已配置的模型: {:?}\n", models);
        // 尝试用第一个模型创建客户端
        let model = &models[0];
        let client = ProviderFactory::create(model)?;
        println!("  ✅ \"{}\" → {}", model, client.model_name());
    }

    println!();
    Ok(())
}

// ── Part 3 ──────────────────────────────────────────────────────────────────

fn demo_from_llm_config() -> echo_agent::error::Result<()> {
    println!("  使用 LlmConfig 手动构建后调用 ProviderFactory::from_config()\n");

    // 各种 LlmConfig 快捷构造器
    let configs = [
        (
            LlmConfig::openai("sk-demo-key", "gpt-4o"),
            "LlmConfig::openai()",
        ),
        (
            LlmConfig::anthropic("sk-ant-demo", "claude-sonnet-4-6"),
            "LlmConfig::anthropic()",
        ),
        (LlmConfig::ollama("llama3"), "LlmConfig::ollama()"),
        (
            LlmConfig::deepseek("sk-ds-demo", "deepseek-chat"),
            "LlmConfig::deepseek()",
        ),
        (
            LlmConfig::dashscope("sk-dash-demo", "qwen3-max"),
            "LlmConfig::dashscope()",
        ),
    ];

    for (config, constructor) in &configs {
        let client = ProviderFactory::from_config(config)?;
        println!(
            "  ✅ {constructor:<30} → model={}, provider={:?}",
            client.model_name(),
            config.provider,
        );
    }

    println!();
    Ok(())
}

// ── Part 4 ──────────────────────────────────────────────────────────────────

fn demo_auto_provider_detection() -> echo_agent::error::Result<()> {
    println!("  LlmConfig::from_model() 自动检测 provider 类型\n");
    println!("  当配置文件中指定了 provider 字段时直接使用；");
    println!("  否则根据 base_url 自动推断：\n");

    let url_examples = [
        ("https://api.anthropic.com/v1/messages", "→ Anthropic"),
        ("http://localhost:11434/api/chat", "→ Ollama"),
        ("https://api.openai.com/v1/chat/completions", "→ OpenAI"),
        (
            "https://api.deepseek.com/chat/completions",
            "→ OpenAI (兼容)",
        ),
    ];

    for (url, expected) in &url_examples {
        let config = LlmConfig::new(*url, "sk-test", "test-model");
        // build_client 会根据 provider 字段分发
        println!("  📡 {url}");
        println!("     provider = {:?}  {expected}\n", config.provider);
    }

    // 演示 Anthropic 配置的 provider 自动设置
    let anthropic_config = LlmConfig::anthropic("sk-ant-test", "claude-sonnet-4-6");
    if anthropic_config.provider != LlmProvider::Anthropic {
        return Err(echo_agent::error::ReactError::Other(
            "demo26 验收失败：anthropic provider 自动检测错误".to_string(),
        )
        .into());
    }
    println!(
        "  LlmConfig::anthropic() → provider = {:?}",
        anthropic_config.provider
    );

    let ollama_config = LlmConfig::ollama("llama3");
    if ollama_config.provider != LlmProvider::Ollama {
        return Err(echo_agent::error::ReactError::Other(
            "demo26 验收失败：ollama provider 自动检测错误".to_string(),
        )
        .into());
    }
    println!(
        "  LlmConfig::ollama()    → provider = {:?}",
        ollama_config.provider
    );

    println!();
    Ok(())
}

// ── Part 5 ──────────────────────────────────────────────────────────────────

async fn demo_agent_with_factory() -> echo_agent::error::Result<()> {
    println!("  使用 ProviderFactory 创建客户端，注入 ReactAgent 执行对话\n");

    // 尝试找到可用的模型
    let models = echo_agent::llm::config::Config::list_models();
    let model_name = if !models.is_empty() {
        models[0].clone()
    } else {
        return Err(echo_agent::error::ReactError::Other(
            "demo26 验收失败：无配置文件，无法验证 ProviderFactory + Agent 对话".to_string(),
        )
        .into());
    };

    println!("  使用模型: {model_name}\n");

    // 通过 ProviderFactory 创建客户端
    let client = ProviderFactory::create(&model_name)?;
    println!("  ✅ 客户端创建成功: {}\n", client.model_name());

    // 构建 Agent，注入 LlmConfig
    let llm_config = LlmConfig::from_model(&model_name)?;
    let mut agent = ReactAgentBuilder::new()
        .llm_config(llm_config)
        .name("factory_demo_agent")
        .system_prompt("你是一个简洁的助手，用一句话回答问题。")
        .max_iterations(3)
        .build()?;

    println!("  👤 用户: 用一句话介绍 Rust 语言");
    let answer = agent.execute("用一句话介绍 Rust 语言").await?;
    if answer.trim().is_empty() {
        return Err(echo_agent::error::ReactError::Other(
            "demo26 验收失败：ProviderFactory Agent 对话返回空答案".to_string(),
        )
        .into());
    }
    println!("  🤖 Agent: {answer}");

    Ok(())
}

// ── 辅助 ────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × ProviderFactory 动态 Provider (demo26)");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
