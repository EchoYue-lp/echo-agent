//! demo36_multimodal —— 多模态支持（Image / File 输入）
//!
//! 演示 `Message` 类型对多模态内容的支持：
//! - `ContentPart::Text` / `ContentPart::ImageUrl` / `ContentPart::File`
//! - `Message::user_with_image()` / `Message::user_with_image_url()`
//! - 序列化兼容性：纯文本 → `"string"`，多模态 → `[{...}]`
//!
//! ```bash
//! cargo run --example demo36_multimodal
//! ```

use echo_agent::prelude::*;

fn main() {
    println!("═══ Multi-Modal Message Demo ═══\n");

    // ── 1. 传统纯文本消息（完全向后兼容）────────────────────────────────
    println!("[1] 纯文本消息（向后兼容）");
    let text_msg = Message::user("你好，请帮我分析数据".to_string());
    assert!(!text_msg.is_multimodal());

    let json = serde_json::to_value(&text_msg).unwrap();
    println!("    role: {}", json["role"]);
    println!("    content: {}", json["content"]);
    println!("    is_multimodal: {}", text_msg.is_multimodal());

    // ── 2. Base64 图片消息 ────────────────────────────────────────────────
    println!("\n[2] Base64 图片消息");
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

    // ── 3. URL 图片消息 ──────────────────────────────────────────────────
    println!("\n[3] URL 图片消息");
    let url_msg =
        Message::user_with_image_url("这张照片拍摄于什么地方？", "https://example.com/sunset.jpg");
    println!("    text_content: {:?}", url_msg.text_content());
    println!("    is_multimodal: {}", url_msg.is_multimodal());

    // ── 4. 混合内容消息（文本 + 图片 + 文件）────────────────────────────
    println!("\n[4] 混合内容消息");
    let mixed = Message::user_multimodal(vec![
        ContentPart::Text {
            text: "请分析以下图表和数据：".to_string(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.com/chart.png".to_string(),
                detail: Some("high".to_string()),
            },
        },
        ContentPart::File {
            name: "sales_data.csv".to_string(),
            content: "bmFtZSxhbW91bnQKQWxpY2UsMTAwCkJvYiwxNTAK".to_string(),
        },
    ]);

    let parts = mixed.content_parts.as_ref().unwrap();
    println!("    parts: {} 个", parts.len());
    for (i, part) in parts.iter().enumerate() {
        match part {
            ContentPart::Text { text } => println!("    [{i}] Text: \"{text}\""),
            ContentPart::ImageUrl { image_url } => {
                println!(
                    "    [{i}] ImageUrl: {} (detail={:?})",
                    image_url.url, image_url.detail
                );
            }
            ContentPart::File { name, .. } => println!("    [{i}] File: {name}"),
        }
    }

    // ── 5. 序列化 / 反序列化兼容性 ──────────────────────────────────────
    println!("\n[5] Serde 兼容性验证");

    // 5a. 纯文本 → JSON string
    let text_json = serde_json::to_string(&Message::user("hello".to_string())).unwrap();
    println!("    纯文本 JSON: {text_json}");
    assert!(text_json.contains("\"content\":\"hello\""));

    // 5b. 多模态 → JSON array
    let mm_json =
        serde_json::to_string(&Message::user_with_image_url("desc", "https://img.png")).unwrap();
    println!(
        "    多模态 JSON (truncated): {}...",
        &mm_json[..80.min(mm_json.len())]
    );

    // 5c. 反序列化纯文本（旧格式）
    let legacy: Message = serde_json::from_str(r#"{"role":"assistant","content":"回复"}"#).unwrap();
    assert!(!legacy.is_multimodal());
    assert_eq!(legacy.content, Some("回复".to_string()));
    println!("    反序列化旧格式 ✓");

    // 5d. 反序列化多模态（新格式）
    let mm_str = r#"{
        "role": "user",
        "content": [
            {"type": "text", "text": "看图"},
            {"type": "image_url", "image_url": {"url": "https://img.png"}}
        ]
    }"#;
    let mm: Message = serde_json::from_str(mm_str).unwrap();
    assert!(mm.is_multimodal());
    assert_eq!(mm.content_parts.as_ref().unwrap().len(), 2);
    println!("    反序列化多模态格式 ✓");

    // ── 6. text_content() 提取 ──────────────────────────────────────────
    println!("\n[6] text_content() 辅助方法");
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
    println!("    text_content() = {:?}", multi.text_content());
    assert_eq!(multi.text_content(), Some("第一段第二段".to_string()));

    println!("\n═══ Demo Complete ═══");
}
