//! demo16_testing — Mock 测试基础设施示例
//!
//! 演示如何在**不发起任何真实 LLM / 网络请求**的情况下，对各层组件编写测试：
//!
//!   1. `MockTool`   — 独立测试工具实现的参数解析与返回值
//!   2. `MockLlmClient` — 测试 `SlidingWindowCompressor` / `SummaryCompressor` 等压缩器
//!   3. `MockAgent`  — 测试多 Agent 编排逻辑（SubAgent 行为）
//!   4. `FailingMockAgent` — 测试编排容错路径
//!   5. `InMemoryStore` / `InMemoryCheckpointer` — 测试记忆系统（无文件 I/O）
//!
//! 本文件同时展示了如何将相同逻辑写成 `#[tokio::test]` 单元测试（见底部注释）。
//!
//! 运行：
//!   cargo run --example demo16_testing

use echo_agent::agent::Agent;
use echo_agent::compression::compressor::{
    DefaultSummaryPrompt, SlidingWindowCompressor, SummaryCompressor,
};
use echo_agent::compression::{CompressionInput, ContextCompressor};
use echo_agent::llm::types::Message;
use echo_agent::memory::checkpointer::{Checkpointer, InMemoryCheckpointer};
use echo_agent::memory::store::{InMemoryStore, Store};
use echo_agent::testing::{FailingMockAgent, MockAgent, MockLlmClient, MockTool};
use echo_agent::tools::Tool;
use std::collections::HashMap;
use std::sync::Arc;

// ─── 辅助宏 ──────────────────────────────────────────────────────────────────

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

// ─── 场景 1：MockTool 独立测试 ────────────────────────────────────────────────

async fn test_mock_tool() {
    section!(1, "MockTool — 工具级单元测试");

    // 1a. 基本使用
    let tool = MockTool::new("weather")
        .with_description("查询城市天气")
        .with_response(r#"{"city":"Beijing","temp":25,"desc":"晴"}"#)
        .with_response(r#"{"city":"Shanghai","temp":30,"desc":"多云"}"#)
        .with_failure("API 服务暂时不可用");

    let mut params = HashMap::new();
    params.insert("city".to_string(), serde_json::json!("Beijing"));

    let r1 = tool.execute(params.clone()).await.unwrap();
    assert!(r1.success);
    assert!(r1.output.contains("Beijing"));
    pass!("第 1 次调用：成功，返回 Beijing 天气");

    let r2 = tool.execute(params.clone()).await.unwrap();
    assert!(r2.success && r2.output.contains("Shanghai"));
    pass!("第 2 次调用：成功，返回 Shanghai 天气");

    let r3 = tool.execute(params.clone()).await.unwrap();
    assert!(!r3.success);
    pass!("第 3 次调用：失败，返回错误响应");

    assert_eq!(tool.call_count(), 3);
    pass!("call_count() == 3");

    let last = tool.last_args().unwrap();
    assert_eq!(last.get("city").unwrap(), "Beijing");
    pass!("last_args() 记录了最后一次入参");

    // 1b. 队列耗尽后返回默认成功
    let fallback = tool.execute(HashMap::new()).await.unwrap();
    assert!(fallback.success && fallback.output == "mock response");
    pass!("队列耗尽时自动返回默认 mock response");
}

// ─── 场景 2：MockLlmClient + SlidingWindowCompressor ─────────────────────────

async fn test_sliding_window() {
    section!(2, "SlidingWindowCompressor — 无 LLM 的压缩测试");

    let compressor = SlidingWindowCompressor::new(3); // 保留最近 3 条

    let messages = vec![
        Message::user("消息 1".to_string()),
        Message::assistant("回复 1".to_string()),
        Message::user("消息 2".to_string()),
        Message::assistant("回复 2".to_string()),
        Message::user("消息 3".to_string()),
        Message::assistant("回复 3".to_string()),
        Message::user("消息 4".to_string()),
    ];

    let input = CompressionInput {
        messages,
        token_limit: 50, // 强制超限触发压缩
        current_query: None,
    };

    let output = compressor.compress(input).await.unwrap();
    assert!(output.messages.len() <= 3, "压缩后应保留 ≤3 条");
    assert!(!output.evicted.is_empty(), "应有被裁剪的消息");
    pass!(format!(
        "压缩后保留 {} 条，裁剪了 {} 条",
        output.messages.len(),
        output.evicted.len()
    ));
}

// ─── 场景 3：MockLlmClient + SummaryCompressor ────────────────────────────────

async fn test_summary_compressor() {
    section!(3, "SummaryCompressor + MockLlmClient — 验证 LLM 被调用");

    let mock_llm = Arc::new(
        MockLlmClient::new().with_response("【摘要】用户询问了天气，助手回答了北京25度晴天。"),
    );

    let compressor = SummaryCompressor::new(
        mock_llm.clone(),
        DefaultSummaryPrompt,
        2, /* keep_recent */
    );

    let messages = (0..6)
        .flat_map(|i| {
            vec![
                Message::user(format!("问题 {i}")),
                Message::assistant(format!("回答 {i}")),
            ]
        })
        .collect::<Vec<_>>();

    let input = CompressionInput {
        messages,
        token_limit: 100,
        current_query: None,
    };

    let output = compressor.compress(input).await.unwrap();

    assert_eq!(mock_llm.call_count(), 1, "应该恰好调用 LLM 一次");
    pass!("MockLlmClient 被调用了 1 次（SummaryCompressor 请求摘要）");

    // 验证 LLM 收到了消息列表
    let call_msgs = mock_llm.last_messages().unwrap();
    assert!(!call_msgs.is_empty());
    pass!(format!(
        "LLM 收到了 {} 条消息作为摘要上下文",
        call_msgs.len()
    ));

    // 压缩后消息中应包含摘要文本
    let has_summary = output
        .messages
        .iter()
        .any(|m| m.content.as_deref().is_some_and(|c| c.contains("摘要")));
    assert!(has_summary, "压缩结果中应包含摘要消息");
    pass!("压缩结果包含 LLM 生成的摘要消息");

    // 测试错误情况：LLM 响应错误时压缩器应向上传播
    let error_llm = Arc::new(MockLlmClient::new().with_network_error("模拟网络超时"));
    let error_compressor = SummaryCompressor::new(error_llm, DefaultSummaryPrompt, 2);
    let err_input = CompressionInput {
        messages: vec![
            Message::user("hi".to_string()),
            Message::assistant("hello".to_string()),
            Message::user("bye".to_string()),
        ],
        token_limit: 10,
        current_query: None,
    };
    let result = error_compressor.compress(err_input).await;
    assert!(result.is_err(), "LLM 错误应向上传播");
    pass!("LLM 返回网络错误时，压缩器正确向上传播错误");
}

// ─── 场景 4：MockAgent — 多 Agent 编排测试 ────────────────────────────────────

async fn test_mock_agent() {
    section!(4, "MockAgent — SubAgent 行为验证");

    let mut math = MockAgent::new("math_agent")
        .with_response("6 × 7 = 42")
        .with_response("√144 = 12");

    let mut writer =
        MockAgent::new("writer_agent").with_responses(["我已经完成了报告", "摘要已生成"]);

    // 模拟编排者的调用
    let r1 = math.execute("计算 6 * 7").await.unwrap();
    assert_eq!(r1, "6 × 7 = 42");
    pass!(format!("math_agent 返回: {r1}"));

    let r2 = math.execute("计算 √144").await.unwrap();
    assert_eq!(r2, "√144 = 12");
    pass!(format!("math_agent 返回: {r2}"));

    assert_eq!(math.call_count(), 2);
    assert_eq!(math.calls()[0], "计算 6 * 7");
    assert_eq!(math.calls()[1], "计算 √144");
    pass!("call_count 和 calls() 记录准确");

    let r3 = writer.execute("撰写技术报告").await.unwrap();
    assert_eq!(r3, "我已经完成了报告");
    pass!(format!("writer_agent 返回: {r3}"));

    // 队列耗尽后返回默认响应
    writer.execute("任务X").await.unwrap();
    let fallback = writer.execute("任务Y").await.unwrap();
    assert_eq!(fallback, "mock agent response");
    pass!("队列耗尽时返回默认 mock agent response");
}

// ─── 场景 5：FailingMockAgent — 容错测试 ─────────────────────────────────────

async fn test_failing_agent() {
    section!(5, "FailingMockAgent — 编排容错路径验证");

    let mut failing = FailingMockAgent::new("broken_agent", "下游服务不可用");

    let result = failing.execute("执行任务").await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("下游服务不可用") || err_msg.contains("Initialization"));
    pass!(format!("错误被正确传播: {err_msg}"));

    assert_eq!(failing.call_count(), 1);
    pass!("call_count 仍然记录了失败的调用");
}

// ─── 场景 6：InMemoryStore + InMemoryCheckpointer ────────────────────────────

async fn test_memory_no_io() {
    section!(
        6,
        "InMemoryStore / InMemoryCheckpointer — 无文件 I/O 的记忆测试"
    );

    // ── 6a. InMemoryStore ──────────────────────────────────────────────────
    let store = InMemoryStore::new();
    let ns = vec!["test", "ns"];

    store
        .put(&ns, "key1", serde_json::json!("value1"))
        .await
        .unwrap();
    store
        .put(&ns, "key2", serde_json::json!("value2"))
        .await
        .unwrap();

    let item = store.get(&ns, "key1").await.unwrap().unwrap();
    assert_eq!(item.value, serde_json::json!("value1"));
    pass!("InMemoryStore put + get 正常");

    let results = store.search(&ns, "value", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    pass!("InMemoryStore search 返回 2 条匹配");

    store.delete(&ns, "key1").await.unwrap();
    let gone = store.get(&ns, "key1").await.unwrap();
    assert!(gone.is_none());
    pass!("InMemoryStore delete 正常");

    // 不同 namespace 之间隔离
    let ns2 = vec!["other", "ns"];
    store
        .put(&ns2, "key2", serde_json::json!("other_value"))
        .await
        .unwrap();
    let item_ns1 = store.get(&ns, "key2").await.unwrap().unwrap();
    let item_ns2 = store.get(&ns2, "key2").await.unwrap().unwrap();
    assert_eq!(item_ns1.value, serde_json::json!("value2"));
    assert_eq!(item_ns2.value, serde_json::json!("other_value"));
    pass!("namespace 隔离：不同 ns 下同名 key 互不干扰");

    // ── 6b. InMemoryCheckpointer ───────────────────────────────────────────
    let cp = InMemoryCheckpointer::new();

    let messages = vec![
        Message::user("你好".to_string()),
        Message::assistant("你好！有什么可以帮你？".to_string()),
    ];

    // put 保存快照；get 取最新快照
    cp.put("session-1", messages.clone()).await.unwrap();
    let snapshot = cp.get("session-1").await.unwrap().unwrap();
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].role, "user");
    pass!("InMemoryCheckpointer put + get 正常");

    // 不存在的 session 返回 None
    let none = cp.get("non-existent").await.unwrap();
    assert!(none.is_none());
    pass!("加载不存在的 session 返回 None");

    // 列出所有 sessions
    let sessions = cp.list_sessions().await.unwrap();
    assert!(sessions.contains(&"session-1".to_string()));
    pass!("list_sessions 包含 session-1");

    // 删除会话
    cp.delete_session("session-1").await.unwrap();
    let deleted = cp.get("session-1").await.unwrap();
    assert!(deleted.is_none());
    pass!("InMemoryCheckpointer delete_session 正常");
}

// ─── main ──────────────────────────────────────────────────────────────────────

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

// ─── 如何在 #[tokio::test] 中使用这些 Mock ────────────────────────────────────
//
// 在 src/ 或 tests/ 目录下的测试文件中，可以直接复用相同的 Mock：
//
// ```rust
// #[cfg(test)]
// mod tests {
//     use super::*;
//     use echo_agent::prelude::*;
//
//     #[tokio::test]
//     async fn test_weather_tool_success() {
//         let tool = MockTool::new("weather").with_response("sunny, 25°C");
//         let result = tool.execute(std::collections::HashMap::new()).await.unwrap();
//         assert!(result.success);
//         assert_eq!(result.output, "sunny, 25°C");
//     }
//
//     #[tokio::test]
//     async fn test_summary_compressor_calls_llm_once() {
//         let mock_llm = Arc::new(MockLlmClient::new().with_response("summary text"));
//         let compressor = SummaryCompressor::new(mock_llm.clone(), DefaultSummaryPrompt, 2);
//         // ... 执行压缩 ...
//         assert_eq!(mock_llm.call_count(), 1);
//     }
// }
// ```
