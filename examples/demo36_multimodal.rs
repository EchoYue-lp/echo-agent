//! demo36_multimodal —— 多模态支持（Image / File 输入）完整演示
//!
//! 演示 `Message` 类型对多模态内容的支持，以及与 LLM 的实际交互：
//! - `ContentPart::Text` / `ContentPart::ImageUrl` / `ContentPart::File`
//! - `Message::user_with_image()` / `Message::user_with_image_url()`
//! - 序列化兼容性：纯文本 → `"string"`，多模态 → `[{...}]`
//! - 实际 LLM 多模态调用：图片分析、停车缴费单识别
//!
//! # 前置条件
//!
//! 设置 LLM API 密钥（支持视觉的模型）：
//! - QWEN_API_KEY（推荐，qwen3.5-plus 支持视觉）
//! - OPENAI_API_KEY（gpt-4o / gpt-4o-mini）
//!
//! # 运行方式
//!
//! ```bash
//! # Part 1-2: 类型系统演示（无需 LLM）
//! cargo run --example demo36_multimodal
//!
//! # Part 3-5: LLM 多模态调用（需要 API Key）
//! QWEN_API_KEY=your_key cargo run --example demo36_multimodal
//! ```

use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "echo_agent=warn,demo36=info".into()),
        )
        .init();

    println!("═══ Multi-Modal Message Demo ═══\n");

    // ── Part 1: 类型系统演示（无需 LLM）──────────────────────────────────────
    demo_type_system();

    // ── Part 2: 序列化兼容性验证 ─────────────────────────────────────────────
    demo_serialization();

    // ── Part 3: LLM 图片分析（需要 API Key）────────────────────────────────────
    if has_llm_config() {
        demo_llm_image_analysis().await?;
    } else {
        println!("\n[跳过 Part 3-5] 未检测到 LLM API 密钥");
        println!("设置 QWEN_API_KEY 或 OPENAI_API_KEY 后可体验完整功能\n");
        return Ok(());
    }

    // ── Part 4: Chat 模式连续对话 ─────────────────────────────────────────────
    demo_chat_mode().await?;

    // ── Part 5: 多图分析示例 ───────────────────────────────────────────────────
    demo_multiple_images().await?;

    println!("\n═══ Demo Complete ═══");
    Ok(())
}

// ── Part 1: 类型系统演示 ─────────────────────────────────────────────────────

fn demo_type_system() {
    println!("─────────────────────────────────────────────");
    println!("Part 1: Message 类型系统");
    println!("─────────────────────────────────────────────\n");

    // 1.1 传统纯文本消息（完全向后兼容）
    println!("  [1.1] 纯文本消息（向后兼容）");
    let text_msg = Message::user("你好，请帮我分析数据".to_string());
    assert!(!text_msg.is_multimodal());

    let json = serde_json::to_value(&text_msg).unwrap();
    println!("    role: {}", json["role"]);
    println!("    content: {}", json["content"]);
    println!("    is_multimodal: {}\n", text_msg.is_multimodal());

    // 1.2 Base64 图片消息
    println!("  [1.2] Base64 图片消息");
    let img_msg = Message::user_with_image(
        "请描述这张图片中的内容",
        "image/png",
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ...",
    );
    assert!(img_msg.is_multimodal());

    let json = serde_json::to_value(&img_msg).unwrap();
    let content = json["content"].as_array().unwrap();
    println!("    parts count: {}", content.len());
    println!(
        "    part[0]: type={}, text={}",
        content[0]["type"], content[0]["text"]
    );
    println!(
        "    part[1]: type={}, url={}...",
        content[1]["type"],
        &content[1]["image_url"]["url"].as_str().unwrap()[..30]
    );
    println!();

    // 1.3 URL 图片消息
    println!("  [1.3] URL 图片消息");
    let url_msg = Message::user_with_image_url(
        "这张照片拍摄于什么地方？",
        "https://vcg00.cfp.cn/creative/vcg/800/new/VCG211572711860.jpg",
    );
    println!("    text_content: {:?}", url_msg.text_content());
    println!("    is_multimodal: {}\n", url_msg.is_multimodal());

    // 1.4 混合内容消息（文本 + 图片 + 文件）
    println!("  [1.4] 混合内容消息");
    let mixed = Message::user_multimodal(vec![
        ContentPart::Text {
            text: "请分析以下图表和数据：".to_string(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://vcg00.cfp.cn/creative/vcg/800/new/VCG211572711860.jpg".to_string(),
                detail: Some("high".to_string()),
            },
        },
        ContentPart::File {
            name: "sales_data.csv".to_string(),
            content: "bmFtZSxhbW91bnQKQWxpY2UsMTAwCkJvYiwxNTAK".to_string(),
        },
    ]);

    let parts = match &mixed.content {
        MessageContent::Parts(parts) => parts,
        _ => unreachable!("mixed message should be multimodal"),
    };
    println!("    parts: {} 个", parts.len());
    for (i, part) in parts.iter().enumerate() {
        match part {
            ContentPart::Text { text } => println!("    [{}] Text: \"{}\"", i, text),
            ContentPart::ImageUrl { image_url } => {
                println!(
                    "    [{}] ImageUrl: {} (detail={:?})",
                    i, image_url.url, image_url.detail
                );
            }
            ContentPart::File { name, .. } => println!("    [{}] File: {}", i, name),
        }
    }
    println!();
}

// ── Part 2: 序列化兼容性验证 ─────────────────────────────────────────────────────

fn demo_serialization() {
    println!("─────────────────────────────────────────────");
    println!("Part 2: Serde 兼容性验证");
    println!("─────────────────────────────────────────────\n");

    // 2a. 纯文本 → JSON string
    let text_json = serde_json::to_string(&Message::user("hello".to_string())).unwrap();
    println!("  [2a] 纯文本 JSON: {}", text_json);
    assert!(text_json.contains("\"content\":\"hello\""));

    // 2b. 多模态 → JSON array
    let mm_json =
        serde_json::to_string(&Message::user_with_image_url("desc", "https://img.png")).unwrap();
    println!(
        "  [2b] 多模态 JSON: {}...",
        &mm_json[..80.min(mm_json.len())]
    );

    // 2c. 反序列化纯文本（旧格式）
    let legacy: Message = serde_json::from_str(r#"{"role":"assistant","content":"回复"}"#).unwrap();
    assert!(!legacy.is_multimodal());
    assert_eq!(legacy.content.as_text_ref(), Some("回复"));
    println!("  [2c] 反序列化旧格式 ✓");

    // 2d. 反序列化多模态（新格式）
    let mm_str = r#"{
        "role": "user",
        "content": [
            {"type": "text", "text": "看图"},
            {"type": "image_url", "image_url": {"url": "https://img.png"}}
        ]
    }"#;
    let mm: Message = serde_json::from_str(mm_str).unwrap();
    assert!(mm.is_multimodal());
    match &mm.content {
        MessageContent::Parts(parts) => assert_eq!(parts.len(), 2),
        _ => panic!("expected multimodal content"),
    }
    println!("  [2d] 反序列化多模态格式 ✓");

    // 2e. text_content() 提取
    let multi = Message::user_multimodal(vec![
        ContentPart::Text {
            text: "第一段".to_string(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://img.png".into(),
                detail: None,
            },
        },
        ContentPart::Text {
            text: "第二段".to_string(),
        },
    ]);
    assert_eq!(multi.text_content(), Some("第一段第二段".to_string()));
    println!("  [2e] text_content() 合并多段文本 ✓\n");
}

// ── Part 3: LLM 图片分析 ─────────────────────────────────────────────────────────

async fn demo_llm_image_analysis() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 3: LLM 图片分析（execute_with_image_url）");
    println!("─────────────────────────────────────────────\n");

    let system_prompt = r#"你是一个多模态智能助手，具备图片分析能力。
当用户提供图片时，请详细分析并回答相关问题。
专注于提取图片中的关键信息，如文字、数字、物体等。"#;

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3.5-plus") // 使用支持视觉的模型
        .name("multimodal-agent")
        .system_prompt(system_prompt)
        .build()?;

    // 3.1 GitHub 公开图片分析（更稳定）
    println!("  [3.1] GitHub 图片分析");
    let image_url =
        "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/25.png";
    let prompt = "这是哪只宝可梦？请描述它的外观特征。";

    println!("    图片 URL: {}", image_url);
    println!("    提示词: {}\n", prompt);

    match agent.execute_with_image_url(prompt, image_url).await {
        Ok(result) => {
            println!("    ✓ 分析结果:\n    {}\n", result);
        }
        Err(e) => {
            println!("    ✗ 错误: {}\n", e);
        }
    }

    Ok(())
}

// ── Part 4: Chat 模式连续对话 ───────────────────────────────────────────────────

async fn demo_chat_mode() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 4: Chat 模式多轮对话");
    println!("─────────────────────────────────────────────\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3.5-plus")
        .name("multimodal-agent")
        .system_prompt("你是一个多模态智能助手，可以分析图片并回答相关问题。")
        .build()?;

    println!("  Q: 什么是多模态 AI？\n");
    match agent.chat("什么是多模态 AI？请用一句话简要说明。").await {
        Ok(result) => {
            println!("  A: {}\n", result);
        }
        Err(e) => {
            println!("  ✗ 错误: {}\n", e);
        }
    }

    Ok(())
}

// ── Part 5: 多图分析示例 ─────────────────────────────────────────────────────────

async fn demo_multiple_images() -> echo_agent::error::Result<()> {
    println!("─────────────────────────────────────────────");
    println!("Part 5: 多图分析（连续调用 chat_with_image_url）");
    println!("─────────────────────────────────────────────\n");

    let mut agent = ReactAgentBuilder::new()
        .model("qwen3.5-plus")
        .name("multimodal-agent")
        .system_prompt("你是一个多模态智能助手，可以分析图片并回答相关问题。")
        .build()?;

    // 5.1 分析另一张图片
    println!("  [5.1] 宝可梦识别");
    let image_url =
        "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/6.png";

    println!("    图片 URL: {}\n", image_url);
    println!("    Q: 这又是哪只宝可梦？\n");

    match agent
        .chat_with_image_url("这只宝可梦的名字和特征是什么？", image_url)
        .await
    {
        Ok(result) => {
            println!("  A: {}\n", result);
        }
        Err(e) => {
            println!("  ✗ 错误: {}\n", e);
        }
    }

    // 5.2 停车缴费单分析（实际应用场景）
    println!("  [5.2] 停车缴费单信息提取");
    let parking_url = "https://xdt-prod.oss-cn-hangzhou.aliyuncs.com/resource/o_appeal_main/f2e3256df8d04b60adbc9e59a7f3db51/4161b69134744ee4b5c9a70dbaae4d8f?Expires=1776147283&OSSAccessKeyId=LTAI5tEV5vfK7k8cDLCHwsqa&Signature=r7l0C%2FntCkMzgvOjFxULCPBhd8w%3D&objectName=resource/o_appeal_main/f2e3256df8d04b60adbc9e59a7f3db51/4161b69134744ee4b5c9a70dbaae4d8f";

    println!("    图片 URL: {}\n", parking_url);
    println!("    Q: 分析图片里面的车牌、停车费、支付时间？\n");

    match agent
        .chat_with_image_url("分析图片里面的车牌、停车费、支付时间？", parking_url)
        .await
    {
        Ok(result) => {
            println!("  A: {}\n", result);
        }
        Err(e) => {
            println!("  ✗ 错误: {}\n", e);
        }
    }

    Ok(())
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────────────

fn has_llm_config() -> bool {
    std::env::var("QWEN_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
}
