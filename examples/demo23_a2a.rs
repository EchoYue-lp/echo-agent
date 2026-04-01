//! A2A 协议示例
//!
//! 演示 Agent Card 创建、A2A Server 和 Client 的使用，
//! 包括任务状态机和流式事件。
//!
//! ```bash
//! cargo run --example demo23_a2a
//! ```

use echo_agent::a2a::*;
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt().with_env_filter("info").init();

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

    // ── 3. 任务状态机演示 ────────────────────────────────────
    println!("\n--- 3. 任务状态机 ---\n");

    println!("TaskState 状态转换:");
    let transitions = [
        (TaskState::Submitted, TaskState::Working),
        (TaskState::Working, TaskState::Completed),
        (TaskState::Working, TaskState::Failed),
        (TaskState::Working, TaskState::InputRequired),
        (TaskState::InputRequired, TaskState::Working),
        (TaskState::Completed, TaskState::Working),
    ];
    for (from, to) in &transitions {
        let ok = from.can_transition_to(*to);
        println!("  {} → {} : {}", from, to, if ok { "✅" } else { "❌" });
    }

    println!("\n终态检查:");
    for state in [
        TaskState::Submitted,
        TaskState::Working,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Canceled,
    ] {
        println!("  {} : terminal={}", state, state.is_terminal());
    }

    // ── 4. A2A Server 处理请求 ───────────────────────────────
    println!("\n--- 4. A2A Server 处理请求 ---\n");

    let server_agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("a2a_agent")
        .system_prompt("你是一个通过 A2A 协议提供服务的助手")
        .build()?;

    let server = A2AServer::new(card.clone(), server_agent);

    println!("Agent Card: {}\n", server.agent_card().name);

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

    println!(
        "发送请求: {}",
        serde_json::to_string_pretty(&request).unwrap()
    );
    let response = server.handle_request(&request.to_string()).await;
    println!("\n响应: {}\n", response);

    // ── 5. 流式任务演示 ──────────────────────────────────────
    println!("--- 5. 流式任务 (tasks/sendSubscribe) ---\n");

    let stream_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "req-002",
        "method": "tasks/sendSubscribe",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "简单介绍 Rust 语言"}]
            }
        }
    });

    match server
        .handle_request_stream(&stream_request.to_string())
        .await
    {
        Ok(mut event_stream) => {
            while let Some(event) = event_stream.next().await {
                match &event {
                    A2AStreamEvent::StatusUpdate(e) => {
                        println!("[状态] {} (final={})", e.status.state, e.is_final);
                        if e.is_final {
                            break;
                        }
                    }
                    A2AStreamEvent::ArtifactUpdate(e) => {
                        let text: String = e
                            .artifact
                            .parts
                            .iter()
                            .filter_map(|p| match p {
                                A2APart::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect();
                        let label = if e.artifact.append {
                            "追加"
                        } else {
                            "新建"
                        };
                        println!(
                            "[产出 #{} {}] {}",
                            e.artifact.index.unwrap_or(0),
                            label,
                            text
                        );
                    }
                }
            }
        }
        Err(e) => {
            println!("流式请求失败: {}", e);
        }
    }

    // ── 6. A2A Client 使用 ───────────────────────────────────
    println!("\n--- 6. A2A Client（演示 API）---\n");
    let _client = A2AClient::new();
    println!("A2AClient 已创建，可用方法:");
    println!("  - discover(url): 发现远程 Agent");
    println!("  - send_task(url, message): 同步发送任务");
    println!("  - send_task_streaming(url, message): 流式发送任务");
    println!("  - get_task(url, task_id): 查询任务状态");
    println!("  - cancel_task(url, task_id): 取消任务");

    println!("\n=== A2A 协议示例完成 ===");
    Ok(())
}
