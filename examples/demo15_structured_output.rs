//! demo15_structured_output — 结构化输出示例

use echo_agent::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize)]
struct EventList {
    events: Vec<HistoryEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HistoryEvent {
    year: i32,
    description: String,
    significance: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SentimentResult {
    sentiment: String,
    confidence: f64,
    keywords: Vec<String>,
    summary: String,
}

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    // 使用 AgentBuilder 创建通用提取 Agent
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("extractor")
        .system_prompt("你是一个精准的信息提取助手，请仅根据用户提供的文本进行提取。")
        .disable_cot()
        .build()?;

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║         echo-agent  结构化输出  demo                ║");
    println!("╚══════════════════════════════════════════════════════╝");

    // 演示 1：extract_json()
    demo_extract_json(&agent).await?;

    // 演示 2：extract::<T>()
    demo_extract_typed(&agent).await?;

    // 演示 3：批量提取
    demo_batch_extract(&agent).await?;

    // 演示 4：AgentConfig 全局设置
    demo_config_level().await?;

    println!("\n✅ 所有演示完成");
    Ok(())
}

async fn demo_extract_json(agent: &ReactAgent) -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 1：extract_json() → serde_json::Value");

    let schema = ResponseFormat::json_schema(
        "person",
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "人物姓名" },
                "age": { "type": "integer", "description": "年龄" },
                "occupation": { "type": "string", "description": "职业" }
            },
            "required": ["name", "age", "occupation"]
        }),
    );

    let text = "李明，34岁，是一名软件工程师。";
    println!("  输入: {text}");

    let value = agent.extract_json(text, schema).await?;
    println!(
        "  结果: name={}, age={}, occupation={}",
        value["name"], value["age"], value["occupation"]
    );
    Ok(())
}

async fn demo_extract_typed(agent: &ReactAgent) -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 2：extract::<T>() → 强类型 Rust 结构体");

    let schema = ResponseFormat::json_schema(
        "sentiment_result",
        json!({
            "type": "object",
            "properties": {
                "sentiment": { "type": "string", "enum": ["positive", "negative", "neutral"] },
                "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                "keywords": { "type": "array", "items": { "type": "string" } },
                "summary": { "type": "string" }
            },
            "required": ["sentiment", "confidence", "keywords", "summary"]
        }),
    );

    let review = "这款手机真的很棒！电池续航好，屏幕清晰。";
    println!("  输入: {review}");

    let result: SentimentResult = agent.extract(review, schema).await?;
    println!(
        "  结果: sentiment={}, confidence={:.0}%",
        result.sentiment,
        result.confidence * 100.0
    );
    Ok(())
}

async fn demo_batch_extract(agent: &ReactAgent) -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 3：批量提取 → 数组结构");

    let schema = ResponseFormat::json_schema(
        "event_list",
        json!({
            "type": "object",
            "properties": {
                "events": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "year": { "type": "integer" },
                            "description": { "type": "string" },
                            "significance": { "type": "string" }
                        },
                        "required": ["year", "description", "significance"]
                    }
                }
            },
            "required": ["events"]
        }),
    );

    let prompt = "从以下文本中提取历史事件：1945年冯·诺依曼提出了存储程序概念。1969年ARPANET诞生。";
    let result: EventList = agent.extract(prompt, schema).await?;
    println!("  提取到 {} 个事件", result.events.len());
    Ok(())
}

async fn demo_config_level() -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 4：AgentBuilder 全局设置 response_format");

    let _schema = ResponseFormat::json_schema(
        "translation_result",
        json!({
            "type": "object",
            "properties": {
                "original": { "type": "string" },
                "translation": { "type": "string" },
                "language": { "type": "string" }
            },
            "required": ["original", "translation", "language"]
        }),
    );

    // 使用 AgentBuilder 创建翻译 Agent
    let mut agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("translator")
        .system_prompt("你是一个专业翻译助手，将用户输入翻译成英文。")
        .disable_cot()
        .build()?;

    let input = "人工智能正在改变世界。";
    println!("  输入: {input}");

    let raw_output = agent.execute(input).await?;
    println!("  输出: {raw_output}");
    Ok(())
}
