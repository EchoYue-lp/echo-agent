# Mock 测试基础设施

## 是什么

`echo_agent::testing` 模块提供了一套在**不发起任何真实 LLM 调用或网络请求**的情况下，测试各层组件的工具集。

| 类型 | 替代对象 | 典型用途 |
|------|---------|---------|
| `MockLlmClient` | 真实 LLM（OpenAI 等） | 测试 `SummaryCompressor` 等依赖 `LlmClient` 的组件 |
| `MockTool` | 真实工具（数据库、HTTP API 等） | 测试工具参数解析、错误处理 |
| `MockAgent` | 真实 SubAgent | 测试多 Agent 编排逻辑 |
| `FailingMockAgent` | 总是失败的 SubAgent | 测试编排容错路径 |

配合框架内置的 `InMemoryStore` 和 `InMemoryCheckpointer`，可以覆盖绝大多数场景的单元 / 集成测试。

---

## 为什么需要

### 测试 LLM 相关代码的挑战

真实 LLM 调用存在以下问题：

- **不稳定**：网络波动、API 限流会导致测试失败，但这与被测代码无关
- **不可预测**：相同输入每次输出不同，断言困难
- **耗时**：一次 API 调用通常需要数秒
- **有成本**：按 Token 计费，CI/CD 中频繁运行成本高昂
- **需要密钥**：测试环境配置复杂，不适合开源项目

### Mock 方案解决的问题

- **零网络请求**：测试完全在内存中运行，毫秒级完成
- **完全可控**：精确预设每次调用的返回值
- **可观测**：验证组件确实发起了正确的调用（次数、参数）
- **错误注入**：轻松模拟网络错误、限流、服务不可用等异常情况

---

## MockLlmClient

实现 `LlmClient` trait，用于测试通过 `Arc<dyn LlmClient>` 接受 LLM 依赖的组件（如 `SummaryCompressor`）。

### 基本用法

```rust
use echo_agent::testing::MockLlmClient;
use echo_agent::compression::compressor::{SummaryCompressor, DefaultSummaryPrompt};
use std::sync::Arc;

// 创建 Mock，预设响应队列
let mock_llm = Arc::new(
    MockLlmClient::new()
        .with_response("第一次摘要：用户询问了天气情况。")
        .with_response("第二次摘要：用户继续追问详情。")
);

// 注入到压缩器
let compressor = SummaryCompressor::new(mock_llm.clone(), DefaultSummaryPrompt, 2);

// ... 执行压缩 ...

// 事后验证
assert_eq!(mock_llm.call_count(), 1);  // 确认 LLM 被调用了一次
let sent_messages = mock_llm.last_messages().unwrap();
println!("LLM 收到了 {} 条消息", sent_messages.len());
```

### 错误注入

```rust
use echo_agent::testing::MockLlmClient;
use echo_agent::error::{ReactError, LlmError};

let mock = MockLlmClient::new()
    .with_response("正常响应")
    .with_network_error("模拟网络超时")  // 便捷方法
    .with_rate_limit_error()            // 429 限流
    .with_error(ReactError::Llm(LlmError::EmptyResponse)); // 自定义错误

// 第 1 次调用 → "正常响应"
// 第 2 次调用 → Err(NetworkError)
// 第 3 次调用 → Err(ApiError { status: 429 })
// 第 4 次调用 → Err(EmptyResponse)
```

### API 参考

| 方法 | 说明 |
|------|------|
| `with_response(text)` | 追加一条成功响应 |
| `with_responses(iter)` | 批量追加多条成功响应 |
| `with_error(err)` | 追加一条错误响应 |
| `with_network_error(msg)` | 追加网络错误（便捷方法） |
| `with_rate_limit_error()` | 追加 429 限流错误 |
| `call_count()` | 已发生的调用次数 |
| `last_messages()` | 最后一次调用的消息列表 |
| `all_calls()` | 所有调用的消息列表（按时序） |
| `remaining()` | 队列中剩余的预设响应数 |
| `reset_calls()` | 清空调用历史 |

---

## MockTool

实现 `Tool` trait，用于在不依赖外部服务的情况下测试 Agent 的工具调用行为。

### 基本用法

```rust
use echo_agent::testing::MockTool;
use echo_agent::tools::Tool;
use std::collections::HashMap;

let tool = MockTool::new("database_query")
    .with_description("查询数据库")
    .with_response(r#"[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]"#)
    .with_failure("数据库连接超时");

// 第 1 次执行 → 成功，返回 JSON
let r1 = tool.execute(HashMap::new()).await?;
assert!(r1.success);

// 第 2 次执行 → 失败
let r2 = tool.execute(HashMap::new()).await?;
assert!(!r2.success);

// 验证调用次数
assert_eq!(tool.call_count(), 2);
```

### 验证传入参数

```rust
let mut params = HashMap::new();
params.insert("city".to_string(), serde_json::json!("Beijing"));

tool.execute(params).await?;

let last = tool.last_args().unwrap();
assert_eq!(last["city"], "Beijing");
```

### API 参考

| 方法 | 说明 |
|------|------|
| `new(name)` | 创建具名 Mock Tool |
| `with_description(desc)` | 设置工具描述 |
| `with_parameters(schema)` | 设置参数 JSON Schema |
| `with_response(text)` | 追加成功响应 |
| `with_responses(iter)` | 批量追加成功响应 |
| `with_failure(msg)` | 追加失败响应 |
| `call_count()` | 已执行次数 |
| `last_args()` | 最后一次调用的参数 |
| `all_calls()` | 所有调用的参数列表 |
| `reset_calls()` | 清空调用历史 |

---

## MockAgent

实现 `Agent` trait，用于在测试编排逻辑时替换真实的 SubAgent。

### 基本用法

```rust
use echo_agent::testing::MockAgent;
use echo_agent::agent::Agent;

let mut math_agent = MockAgent::new("math_agent")
    .with_response("6 × 7 = 42")
    .with_response("√144 = 12");

// 模拟编排者调用 SubAgent
let r1 = math_agent.execute("计算 6 * 7").await?;
assert_eq!(r1, "6 × 7 = 42");

let r2 = math_agent.execute("计算 √144").await?;
assert_eq!(r2, "√144 = 12");

// 验证 SubAgent 被正确调用
assert_eq!(math_agent.call_count(), 2);
assert_eq!(math_agent.calls()[0], "计算 6 * 7");
```

### 与真实编排器组合

```rust
use echo_agent::prelude::*;
use echo_agent::testing::MockAgent;

// 创建 Mock SubAgent
let math = MockAgent::new("math_agent").with_response("结果是 42");
let writer = MockAgent::new("writer_agent").with_response("报告已生成");

// 注入到真实编排 Agent
let config = AgentConfig::new("gpt-4o", "orchestrator", "...")
    .role(AgentRole::Orchestrator)
    .enable_subagent(true);

let mut orchestrator = ReactAgent::new(config);
orchestrator.register_agent(Box::new(math));
orchestrator.register_agent(Box::new(writer));

// 编排器使用真实 LLM，SubAgent 使用 Mock
let result = orchestrator.execute("完成任务").await?;
```

### `FailingMockAgent` — 测试容错路径

```rust
use echo_agent::testing::FailingMockAgent;

let mut broken = FailingMockAgent::new("broken_agent", "下游服务不可用");
let result = broken.execute("任务").await;
assert!(result.is_err());
assert_eq!(broken.call_count(), 1); // 失败的调用也被记录
```

### API 参考（MockAgent）

| 方法 | 说明 |
|------|------|
| `new(name)` | 创建具名 Mock Agent |
| `with_model(model)` | 设置模型名称 |
| `with_system_prompt(prompt)` | 设置系统提示词 |
| `with_response(text)` | 追加预设响应 |
| `with_responses(iter)` | 批量追加预设响应 |
| `call_count()` | 已调用次数 |
| `calls()` | 所有调用的任务字符串列表 |
| `last_task()` | 最后一次调用的任务字符串 |
| `reset_calls()` | 清空调用历史 |

---

## 配合 InMemoryStore / InMemoryCheckpointer

对于涉及记忆系统的测试，使用内置的内存实现（无文件 I/O）：

```rust
use echo_agent::memory::checkpointer::{Checkpointer, InMemoryCheckpointer};
use echo_agent::memory::store::{InMemoryStore, Store};
use echo_agent::llm::types::Message;

// ── Store 测试 ─────────────────────────────────────────────────
let store = InMemoryStore::new();
let ns = vec!["test_agent", "memories"];

store.put(&ns, "key1", serde_json::json!("内容1")).await?;

let item = store.get(&ns, "key1").await?.unwrap();
assert_eq!(item.value, serde_json::json!("内容1"));

let results = store.search(&ns, "内容", 10).await?;
assert_eq!(results.len(), 1);

// ── Checkpointer 测试 ─────────────────────────────────────────
let cp = InMemoryCheckpointer::new();
let messages = vec![
    Message::user("你好".to_string()),
    Message::assistant("你好！".to_string()),
];

cp.put("session-1", messages).await?;
let snapshot = cp.get("session-1").await?.unwrap();
assert_eq!(snapshot.messages.len(), 2);

cp.delete_session("session-1").await?;
assert!(cp.get("session-1").await?.is_none());
```

---

## 在 #[tokio::test] 中使用

将上述 Mock 直接嵌入标准 Rust 测试：

```rust
#[cfg(test)]
mod tests {
    use echo_agent::compression::compressor::{SummaryCompressor, DefaultSummaryPrompt};
    use echo_agent::compression::{CompressionInput, ContextCompressor};
    use echo_agent::llm::types::Message;
    use echo_agent::testing::MockLlmClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_summary_compressor_calls_llm_once() {
        let mock = Arc::new(MockLlmClient::new().with_response("摘要文本"));
        let compressor = SummaryCompressor::new(mock.clone(), DefaultSummaryPrompt, 2);

        let input = CompressionInput {
            messages: (0..6).flat_map(|i| vec![
                Message::user(format!("问题{i}")),
                Message::assistant(format!("回答{i}")),
            ]).collect(),
            token_limit: 50,
            current_query: None,
        };

        let output = compressor.compress(input).await.unwrap();
        assert_eq!(mock.call_count(), 1);   // LLM 只被调用一次
        assert!(!output.messages.is_empty());
    }

    #[tokio::test]
    async fn test_summary_compressor_propagates_llm_error() {
        let mock = Arc::new(MockLlmClient::new().with_network_error("超时"));
        let compressor = SummaryCompressor::new(mock, DefaultSummaryPrompt, 2);

        let input = CompressionInput {
            messages: vec![
                Message::user("hi".to_string()),
                Message::assistant("hello".to_string()),
                Message::user("bye".to_string()),
            ],
            token_limit: 10,
            current_query: None,
        };

        assert!(compressor.compress(input).await.is_err());
    }
}
```

---

## 覆盖范围说明

| 测试场景 | 推荐工具 | 是否需要真实 LLM |
|---------|---------|----------------|
| 工具参数解析 | `MockTool` | 否 |
| 工具错误处理 | `MockTool::with_failure()` | 否 |
| 滑动窗口压缩 | 直接测试 `SlidingWindowCompressor` | 否 |
| LLM 摘要压缩 | `MockLlmClient` + `SummaryCompressor` | 否 |
| SubAgent 编排逻辑 | `MockAgent` + 真实编排器 | 是（编排器本身） |
| 编排容错 | `FailingMockAgent` | 是（编排器本身） |
| 记忆存储 | `InMemoryStore` | 否 |
| 会话恢复 | `InMemoryCheckpointer` | 否 |
| 端到端 Agent 行为 | 真实 LLM | 是 |

---

## 完整示例

对应示例：`examples/demo16_testing.rs`

```bash
cargo run --example demo16_testing
```
