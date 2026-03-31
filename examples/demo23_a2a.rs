//! A2A 协议示例
//!
//! 演示 Agent Card 创建、A2A Server 和 Client 的使用。
//!
//! ```bash
//! cargo run --example demo23_a2a
//! ```

use echo_agent::a2a::*;
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    println!("=== A2A 协议示例 ===\n");

    // ── 1. 构建 Agent Card ──────────────────────────────────
    println!("--- 1. 构建 Agent Card ---\n");

    let card = AgentCard::builder("echo-translator", "http://localhost:8080")
        .description("多语言翻译 Agent，支持中英文互译")
        .version("1.0.0")
        .provider(AgentProvider::new("Echo Team").with_url("https://example.com"))
        .skill(
            AgentSkill::new("translate", "翻译文本")
                .with_tags(vec!["nlp", "translation"])
                .with_examples(vec!["翻译'你好'为英文", "Translate 'hello' to Chinese"]),
        )
        .skill(AgentSkill::new("detect_language", "检测文本语言"))
        .streaming()
        .build();

    println!("Agent Card JSON:");
    println!("{}\n", serde_json::to_string_pretty(&card).unwrap());

    // ── 2. 从 Agent 自动生成 Card ────────────────────────────
    println!("--- 2. 从 Agent 自动生成 Card ---\n");

    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("math_agent")
        .system_prompt("你是一个数学计算助手")
        .enable_tools()
        .build()?;

    let auto_card = AgentCard::from_agent(&agent, "http://localhost:9090");
    println!("自动生成的 Agent Card:");
    println!("  名称: {}", auto_card.name);
    println!("  技能数: {}", auto_card.skills.len());
    for skill in &auto_card.skills {
        println!("    - {}: {:?}", skill.name, skill.description);
    }

    // ── 3. A2A Server 使用 ───────────────────────────────────
    println!("\n--- 3. A2A Server 处理请求 ---\n");

    let server_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("a2a_agent")
        .system_prompt("你是一个通过 A2A 协议提供服务的助手")
        .build()?;

    let server = A2AServer::new(card.clone(), server_agent);

    // 获取 Agent Card
    println!("Agent Card: {}\n", server.agent_card().name);

    // 模拟 JSON-RPC 请求
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-001",
        "method": "tasks/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "你好，请介绍一下自己"}]
            }
        }
    });

    println!("发送请求: {}", serde_json::to_string_pretty(&request).unwrap());
    let response = server.handle_request(&request.to_string()).await;
    println!("\n响应: {}\n", response);

    // ── 4. A2A Client 使用（需要真实服务才能运行）──────────────
    println!("--- 4. A2A Client（演示 API）---\n");
    let _client = A2AClient::new();
    println!("A2AClient 已创建，可用方法:");
    println!("  - discover(url): 发现远程 Agent");
    println!("  - send_task(url, message): 发送任务");
    println!("  - get_task(url, task_id): 查询任务状态");
    println!("  - cancel_task(url, task_id): 取消任务");

    println!("\n=== A2A 协议示例完成 ===");
    Ok(())
}
