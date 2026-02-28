//! demo15_structured_output — 结构化输出示例
//!
//! 演示三种结构化输出用法：
//!   1. `extract_json()`   — 一次性提取，返回 `serde_json::Value`
//!   2. `extract::<T>()`   — 一次性提取，自动反序列化为 Rust 结构体
//!   3. `AgentConfig::response_format()` — 在整个 ReAct 循环中强制 JSON 格式
//!
//! 运行：
//!   cargo run --example demo15_structured_output

use echo_agent::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ── 目标类型 ──────────────────────────────────────────────────────────────────

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
    sentiment: String, // "positive" | "negative" | "neutral"
    confidence: f64,
    keywords: Vec<String>,
    summary: String,
}

// ── 演示函数 ──────────────────────────────────────────────────────────────────

/// 演示 1：extract_json() — 返回 serde_json::Value
async fn demo_extract_json(agent: &ReactAgent) -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 1：extract_json() → serde_json::Value");
    println!("══════════════════════════════════════════════════════");

    let schema = ResponseFormat::json_schema(
        "person",
        json!({
            "type": "object",
            "properties": {
                "name":       { "type": "string",  "description": "人物姓名" },
                "age":        { "type": "integer", "description": "年龄（数字）" },
                "occupation": { "type": "string",  "description": "职业" }
            },
            "required": ["name", "age", "occupation"],
            "additionalProperties": false
        }),
    );

    let text = "李明，34岁，是一名软件工程师，就职于北京某互联网公司。";
    println!("  输入文本: {text}");

    let value = agent.extract_json(text, schema).await?;

    println!("  提取结果 (Value):");
    println!("    name       = {}", value["name"]);
    println!("    age        = {}", value["age"]);
    println!("    occupation = {}", value["occupation"]);
    println!(
        "  完整 JSON: {}",
        serde_json::to_string_pretty(&value).unwrap()
    );

    Ok(())
}

/// 演示 2：extract::<T>() — 自动反序列化为 Rust 结构体
async fn demo_extract_typed(agent: &ReactAgent) -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 2：extract::<T>() → 强类型 Rust 结构体");
    println!("══════════════════════════════════════════════════════");

    let schema = ResponseFormat::json_schema(
        "sentiment_result",
        json!({
            "type": "object",
            "properties": {
                "sentiment":  {
                    "type": "string",
                    "enum": ["positive", "negative", "neutral"],
                    "description": "情感倾向"
                },
                "confidence": {
                    "type": "number",
                    "minimum": 0.0,
                    "maximum": 1.0,
                    "description": "置信度，0~1 之间"
                },
                "keywords": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "关键词列表"
                },
                "summary": {
                    "type": "string",
                    "description": "一句话情感摘要"
                }
            },
            "required": ["sentiment", "confidence", "keywords", "summary"],
            "additionalProperties": false
        }),
    );

    let review = "这款手机真的让我失望透了！电池续航只有官网说的一半，\
                  散热也差得要命，打游戏没几分钟就开始发烫。\
                  客服态度还非常恶劣，完全不负责任。强烈不推荐购买！";

    println!("  评论文本: {review}");

    let result: SentimentResult = agent.extract(review, schema).await?;

    println!("  分析结果:");
    println!("    情感倾向:  {}", result.sentiment);
    println!("    置信度:    {:.0}%", result.confidence * 100.0);
    println!("    关键词:    {:?}", result.keywords);
    println!("    摘要:      {}", result.summary);

    Ok(())
}

/// 演示 3：批量提取 — 从长文本中提取多条记录
async fn demo_batch_extract(agent: &ReactAgent) -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 3：批量提取 → 数组结构");
    println!("══════════════════════════════════════════════════════");

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
                            "year":         { "type": "integer" },
                            "description":  { "type": "string" },
                            "significance": { "type": "string" }
                        },
                        "required": ["year", "description", "significance"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["events"],
            "additionalProperties": false
        }),
    );

    let prompt = "从以下文本中提取重要历史事件（包含年份、描述、历史意义）：\n\
                  计算机发展史中，1945年冯·诺依曼提出了存储程序概念，奠定了现代计算机的架构基础。\
                  1969年ARPANET诞生，成为互联网的前身。\
                  1991年蒂姆·伯纳斯-李发明了万维网（WWW），彻底改变了信息传播方式。\
                  2007年苹果发布第一代iPhone，开创了移动互联网时代。";

    let preview: String = prompt.chars().take(30).collect();
    println!("  提取文本（截断显示）: {preview}...");

    let result: EventList = agent.extract(prompt, schema).await?;

    println!("  提取到 {} 个历史事件:", result.events.len());
    for event in &result.events {
        println!(
            "    [{:4}] {} — {}",
            event.year, event.description, event.significance
        );
    }

    Ok(())
}

/// 演示 4：通过 AgentConfig 全局设置 response_format
async fn demo_config_level() -> echo_agent::error::Result<()> {
    println!("\n══════════════════════════════════════════════════════");
    println!("  演示 4：AgentConfig::response_format() — 全局设置");
    println!("══════════════════════════════════════════════════════");

    // 构建一个"始终输出 JSON"的 Agent
    let schema = ResponseFormat::json_schema(
        "translation_result",
        json!({
            "type": "object",
            "properties": {
                "original":    { "type": "string", "description": "原文" },
                "translation": { "type": "string", "description": "翻译结果" },
                "language":    { "type": "string", "description": "目标语言" }
            },
            "required": ["original", "translation", "language"],
            "additionalProperties": false
        }),
    );

    let config = AgentConfig::new(
        "qwen3-max",
        "translator",
        "你是一个专业翻译助手，将用户输入翻译成英文，并严格按照指定格式输出。",
    )
    .response_format(schema)
    .enable_cot(false); // 关闭 CoT，避免干扰纯 JSON 输出

    let mut agent = ReactAgent::new(config);

    let input = "人工智能正在改变整个世界的面貌，带来了前所未有的机遇与挑战。";
    println!("  输入: {input}");

    // 直接调用 execute()，返回的是 JSON 字符串
    let raw_output = agent.execute(input).await?;
    println!("  LLM 原始输出: {raw_output}");

    // 手动解析
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw_output) {
        println!("  解析结果:");
        println!("    original:    {}", v["original"]);
        println!("    translation: {}", v["translation"]);
        println!("    language:    {}", v["language"]);
    } else {
        println!("  (输出不是纯 JSON，已直接打印)");
    }

    Ok(())
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    // 通用提取 Agent（无工具，低温度）
    let config = AgentConfig::new(
        "qwen3-max",
        "extractor",
        "你是一个精准的信息提取助手，请仅根据用户提供的文本进行提取，不要添加额外信息。",
    )
    .enable_cot(false); // 纯提取无需推理链

    let agent = ReactAgent::new(config);

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║         echo-agent  结构化输出  demo                ║");
    println!("╚══════════════════════════════════════════════════════╝");

    demo_extract_json(&agent).await?;
    demo_extract_typed(&agent).await?;
    demo_batch_extract(&agent).await?;
    demo_config_level().await?;

    println!("\n✅ 所有演示完成");
    Ok(())
}
