//! demo36_multimodal —— 多模态支持（Image / File 输入）完整演示
//!
//! 演示真实 LLM 多模态调用能力：
//! - 单图分析
//! - 图文多轮对话
//! - 多图对比分析
//!
//! # 前置条件
//!
//! 在 `application configuration` 中把 `model.name` 指向一个支持视觉的模型，并在
//! `models.*` 中声明对应 provider 配置。密钥仍可通过 YAML 里的 `${ENV}` 注入。
//!
//! # 运行方式
//!
//! ```bash
//! # 使用默认搜索路径中的 application configuration
//! cargo run --example demo36_multimodal
//!
//! # 或者显式指定配置文件路径
//! ECHO_AGENT_CONFIG=/path/to/application configuration cargo run --example demo36_multimodal
//! ```
use echo_agent::prelude::*;
mod support;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=warn,demo36=info".into()),
        )
        .init();

    println!("═══ Multi-Modal Message Demo ═══\n");

    let llm_config = require_yaml_model()?;

    // ── Part 1: LLM 图片分析（需要 API Key）────────────────────────────────────
    demo_live_image_analysis(&llm_config).await?;

    // ── Part 2: Chat 模式连续对话 ─────────────────────────────────────────────
    demo_live_chat_mode(&llm_config).await?;

    // ── Part 3: 多图分析示例 ───────────────────────────────────────────────────
    demo_live_multiple_images(&llm_config).await?;

    println!("\n═══ Demo Complete ═══");
    Ok(())
}

// ── Part 3: LLM 图片分析 ─────────────────────────────────────────────────────────

async fn demo_live_image_analysis(config: &LlmConfig) -> echo_agent::error::Result<()> {
    let model_name = config.model.as_str();
    println!("─────────────────────────────────────────────");
    println!("Part 1: 真实图片分析");
    println!("─────────────────────────────────────────────\n");

    println!("  使用模型: {}", model_name);
    println!("  图片 URL: {}\n", live_image_url());

    let agent = build_live_agent(config)?;
    let result = agent
        .chat_with_image_url("请用一句话描述这张图片的主体和场景。", live_image_url())
        .await
        .map_err(|err| multimodal_call_error(model_name, err))
        .and_then(|result| ensure_vision_response(model_name, result))?;
    println!("  ✓ 分析结果:\n  {}\n", result);

    Ok(())
}

// ── Part 4: Chat 模式连续对话 ───────────────────────────────────────────────────

async fn demo_live_chat_mode(config: &LlmConfig) -> echo_agent::error::Result<()> {
    let model_name = config.model.as_str();
    println!("─────────────────────────────────────────────");
    println!("Part 2: 真实 Chat 模式多轮对话");
    println!("─────────────────────────────────────────────\n");

    let agent = build_live_agent(config)?;

    println!("  Q: 什么是多模态 AI？\n");
    match agent.chat("什么是多模态 AI？请用一句话简要说明。").await {
        Ok(result) => {
            println!("  A: {}\n", result);
        }
        Err(e) => {
            println!("  ✗ 错误: {}\n", e);
        }
    }

    println!("  Q: 结合这张图片再补充一句说明。\n");
    let result = agent
        .chat_with_image_url("结合这张图片，再补充一句说明。", live_image_url())
        .await
        .map_err(|err| multimodal_call_error(model_name, err))
        .and_then(|result| ensure_vision_response(model_name, result))?;
    println!("  A: {}\n", result);

    Ok(())
}

// ── Part 5: 多图分析示例 ─────────────────────────────────────────────────────────

async fn demo_live_multiple_images(config: &LlmConfig) -> echo_agent::error::Result<()> {
    let model_name = config.model.as_str();
    println!("─────────────────────────────────────────────");
    println!("Part 3: 真实多图分析");
    println!("─────────────────────────────────────────────\n");

    let agent = build_live_agent(config)?;

    let message = Message::user_multimodal(vec![
        ContentPart::Text {
            text: "请比较这两张图片的主体、颜色和场景差异。".to_string(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: live_image_url().to_string(),
                detail: Some("low".to_string()),
            },
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: live_second_image_url().to_string(),
                detail: Some("low".to_string()),
            },
        },
    ]);
    let result = agent
        .chat_multimodal(message)
        .await
        .map_err(|err| multimodal_call_error(model_name, err))
        .and_then(|result| ensure_vision_response(model_name, result))?;
    println!("  A: {}\n", result);

    Ok(())
}

fn require_yaml_model() -> echo_agent::error::Result<LlmConfig> {
    let config = support::llm_config(None).map_err(|error| {
        echo_agent::error::ReactError::Other(format!(
            "demo36 需要显式配置 provider 和视觉模型：{error}"
        ))
    })?;
    if !config.input_modalities.contains(&ModelInputModality::Image) {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo36 模型 '{}' 未启用图片输入能力",
            config.model
        )));
    }
    Ok(config)
}

fn build_live_agent(config: &LlmConfig) -> echo_agent::error::Result<ReactAgent> {
    ReactAgentBuilder::new()
        .name("multimodal-live-agent")
        .system_prompt("你是一个多模态智能助手，可以分析图片并回答相关问题。")
        .llm_config(config.clone())
        .build()
}

fn multimodal_call_error(
    model_name: &str,
    err: impl std::fmt::Display,
) -> echo_agent::error::ReactError {
    echo_agent::error::ReactError::Other(format!(
        "demo36 当前 `model.name = {model_name}` 的多模态请求失败：{err}。这通常表示该模型不支持视觉输入，或 provider 不接受当前图片 URL。"
    ))
}

fn ensure_vision_response(model_name: &str, response: String) -> echo_agent::error::Result<String> {
    let normalized = response.trim();
    let looks_like_text_only_fallback = normalized.is_empty()
        || normalized.contains("无法看到")
        || normalized.contains("没有看到")
        || normalized.contains("未看到")
        || normalized.contains("请您上传图片")
        || normalized.contains("请上传图片");

    if looks_like_text_only_fallback {
        return Err(echo_agent::error::ReactError::Other(format!(
            "demo36 当前 `model.name = {model_name}` 没有真正处理图片输入，而是返回了文本模型式的兜底回复：{normalized:?}。请在 YAML 中改用支持视觉的模型。"
        )));
    }

    Ok(response)
}

fn live_image_url() -> &'static str {
    std::env::var("ECHO_MULTIMODAL_IMAGE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| Box::leak(value.into_boxed_str()) as &'static str)
        .unwrap_or(
            "https://img1.baidu.com/it/u=2746626622,1108715953&fm=253&app=138&f=JPEG?w=800&h=1200",
        )
}

fn live_second_image_url() -> &'static str {
    std::env::var("ECHO_MULTIMODAL_IMAGE_URL_2")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| Box::leak(value.into_boxed_str()) as &'static str)
        .unwrap_or("https://pics4.baidu.com/feed/cf1b9d16fdfaaf51c78c7323aa7afbfef11f7af9.jpeg@f_auto?token=26bf5a00ddf08277b603191645316dba")
}
