//! demo16_testing — Mock 测试基础设施示例

use echo_agent::compression::compressor::{
    DefaultSummaryPrompt, SlidingWindowCompressor, SummaryCompressor,
};
use echo_agent::compression::{CompressionInput, ContextCompressor};
use echo_agent::llm::types::Message;
use echo_agent::memory::checkpointer::{Checkpointer, InMemoryCheckpointer};
use echo_agent::memory::store::{InMemoryStore, Store};
use echo_agent::prelude::*;
use echo_agent::tools::Tool;
use std::collections::HashMap;
use std::sync::Arc;

macro_rules! pass {
    ($msg:expr) => {
        println!("  ✅  {}", $msg)
    };
}

macro_rules! section {
    ($n:expr, $title:expr) => {
        println!("\n══════════════════════════════════════════════════");
        println!("  场景 {} ：{}", $n, $title);
        println!("══════════════════════════════════════════════════");
    };
}

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║      echo-agent  Mock 测试基础设施 demo          ║");
    println!("║  （全程无真实 LLM 调用 / 无网络请求）             ║");
    println!("╚══════════════════════════════════════════════════╝");

    test_mock_tool().await;
    test_sliding_window().await;
    test_summary_compressor().await;
    test_mock_agent().await;
    test_failing_agent().await;
    test_memory_no_io().await;

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║  全部 6 个场景通过 ✅                             ║");
    println!("╚══════════════════════════════════════════════════╝");
}

async fn test_mock_tool() {
    section!(1, "MockTool — 工具级单元测试");

    let tool = MockTool::new("weather")
        .with_description("查询城市天气")
        .with_response(r#"{"city":"Beijing","temp":25}"#)
        .with_failure("API 服务暂时不可用");

    let mut params = HashMap::new();
    params.insert("city".to_string(), serde_json::json!("Beijing"));

    let r1 = tool.execute(params.clone()).await.unwrap();
    assert!(r1.success);
    pass!("第 1 次调用：成功");

    let r2 = tool.execute(params.clone()).await.unwrap();
    assert!(!r2.success);
    pass!("第 2 次调用：失败");

    assert_eq!(tool.call_count(), 2);
    pass!("call_count() == 2");
}

async fn test_sliding_window() {
    section!(2, "SlidingWindowCompressor — 无 LLM 的压缩测试");

    let compressor = SlidingWindowCompressor::new(3);

    let messages = vec![
        Message::user("消息 1".to_string()),
        Message::assistant("回复 1".to_string()),
        Message::user("消息 2".to_string()),
        Message::assistant("回复 2".to_string()),
        Message::user("消息 3".to_string()),
    ];

    let input = CompressionInput {
        messages,
        token_limit: 50,
        current_query: None,
    };
    let output = compressor.compress(input).await.unwrap();

    assert!(output.messages.len() <= 3);
    pass!(format!("压缩后保留 {} 条", output.messages.len()));
}

async fn test_summary_compressor() {
    section!(3, "SummaryCompressor + MockLlmClient");

    let mock_llm = Arc::new(MockLlmClient::new().with_response("【摘要】用户询问了天气。"));

    let compressor = SummaryCompressor::new(mock_llm.clone(), DefaultSummaryPrompt, 2);

    let messages = vec![
        Message::user("问题 1".to_string()),
        Message::assistant("回答 1".to_string()),
        Message::user("问题 2".to_string()),
    ];

    let input = CompressionInput {
        messages,
        token_limit: 100,
        current_query: None,
    };
    let _output = compressor.compress(input).await.unwrap();

    assert_eq!(mock_llm.call_count(), 1);
    pass!("MockLlmClient 被调用了 1 次");
}

async fn test_mock_agent() {
    section!(4, "MockAgent — SubAgent 行为验证");

    let mut math = MockAgent::new("math_agent")
        .with_response("6 × 7 = 42")
        .with_response("√144 = 12");

    let r1 = math.execute("计算 6 * 7").await.unwrap();
    assert_eq!(r1, "6 × 7 = 42");
    pass!(format!("math_agent 返回: {r1}"));

    assert_eq!(math.call_count(), 1);
    pass!("call_count 记录准确");
}

async fn test_failing_agent() {
    section!(5, "FailingMockAgent — 编排容错路径验证");

    let mut failing = FailingMockAgent::new("broken_agent", "下游服务不可用");

    let result = failing.execute("执行任务").await;
    assert!(result.is_err());
    pass!("错误被正确传播");

    assert_eq!(failing.call_count(), 1);
    pass!("call_count 记录了失败的调用");
}

async fn test_memory_no_io() {
    section!(6, "InMemoryStore / InMemoryCheckpointer");

    let store = InMemoryStore::new();
    let ns = vec!["test", "ns"];

    store
        .put(&ns, "key1", serde_json::json!("value1"))
        .await
        .unwrap();
    let item = store.get(&ns, "key1").await.unwrap().unwrap();
    assert_eq!(item.value, serde_json::json!("value1"));
    pass!("InMemoryStore put + get 正常");

    let cp = InMemoryCheckpointer::new();
    let messages = vec![Message::user("你好".to_string())];

    cp.put("session-1", messages.clone()).await.unwrap();
    let snapshot = cp.get("session-1").await.unwrap().unwrap();
    assert_eq!(snapshot.messages.len(), 1);
    pass!("InMemoryCheckpointer put + get 正常");
}
