# 流式输出（Streaming）

## 是什么

流式输出（Streaming）让 Agent 在 LLM 生成内容的同时，将 Token 片段实时推送给调用方，而不是等待完整响应后一次性返回。用户可以看到 Agent "打字"的过程，大幅改善交互体验。

---

## 解决什么问题

阻塞式调用的问题：
- **长等待**：复杂任务推理时间长，用户界面冻结数十秒
- **无反馈**：用户不知道 Agent 在"想什么"
- **体验割裂**：与现代 AI 对话产品（ChatGPT、Claude）的流畅体验差距明显

流式输出解决了：
- 响应第一个 Token 的延迟（TTFT，Time to First Token）从几秒降至毫秒级
- 用户可以实时看到思考过程（CoT 推理文本）和工具调用
- 可以在生成过程中提前终止

---

## 事件类型

`execute_stream()` 返回 `BoxStream<'_, Result<AgentEvent>>`，包含以下事件：

```rust
pub enum AgentEvent {
    Token(String),                              // LLM 输出的 Token 片段（推理过程 / 最终回答）
    ToolCall { name: String, args: Value },     // LLM 决定调用某个工具
    ToolResult { name: String, output: String },// 工具执行完毕，返回结果
    FinalAnswer(String),                        // 最终答案已生成，流结束
}
```

---

## 使用方式

```rust
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个助手")
        .enable_tool(true);

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(CalculatorTool));

    // 流式执行
    let mut stream = agent.execute_stream("计算 (3 + 4) * 5 并解释步骤").await?;

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Token(token) => {
                print!("{}", token);          // 实时打印推理/回答文本
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            AgentEvent::ToolCall { name, args } => {
                println!("\n[调用工具] {} {:?}", name, args);
            }
            AgentEvent::ToolResult { name, output } => {
                println!("[工具结果] {} -> {}", name, output);
            }
            AgentEvent::FinalAnswer(answer) => {
                println!("\n[最终答案] {}", answer);
                break;
            }
        }
    }
    Ok(())
}
```

---

## 流式输出与 CoT 的配合

当 `enable_cot=true`（默认启用）时，系统提示词末尾追加引导语，要求 LLM 在工具调用前先输出思考文本。这个文本会作为 `Token` 事件实时流出：

```
用户: "计算 42 * 7"

流事件序列：
  Token("让我先分析一下这道计算题...")  ← CoT 推理文本（实时）
  Token("42 乘以 7，需要调用计算工具")
  ToolCall { name: "multiply", args: {"a": 42, "b": 7} }
  ToolResult { name: "multiply", output: "294" }
  Token("计算完成，结果是 294。")        ← 最终回答（实时）
  FinalAnswer("42 × 7 = 294")
```

---

## 阻塞式 vs 流式对比

```rust
// 阻塞式：等待完整响应
let answer: String = agent.execute("你好").await?;

// 流式：实时接收事件
let mut stream = agent.execute_stream("你好").await?;
while let Some(event) = stream.next().await {
    // 处理 Token/ToolCall/ToolResult/FinalAnswer
}
```

两种方式的执行逻辑完全相同，仅输出方式不同。`execute()` 内部实际上是将流式事件聚合后返回最终 `FinalAnswer`。

---

## 在 Web 服务中使用（Server-Sent Events）

```rust
use axum::{Router, response::Sse};
use futures::stream::StreamExt;
use echo_agent::prelude::*;

async fn chat_stream(
    task: String,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let mut agent = ReactAgent::new(/* config */);

    let event_stream = async_stream::stream! {
        if let Ok(mut agent_stream) = agent.execute_stream(&task).await {
            while let Some(event) = agent_stream.next().await {
                let data = match event {
                    Ok(AgentEvent::Token(t))            => format!("{{\"type\":\"token\",\"data\":\"{}\"}}", t),
                    Ok(AgentEvent::ToolCall { name, .. }) => format!("{{\"type\":\"tool_call\",\"name\":\"{}\"}}", name),
                    Ok(AgentEvent::FinalAnswer(a))       => format!("{{\"type\":\"done\",\"data\":\"{}\"}}", a),
                    _ => continue,
                };
                yield Ok(axum::response::sse::Event::default().data(data));
            }
        }
    };

    Sse::new(event_stream)
}
```

---

## 注意事项

1. **工具执行不是流式的**：工具（`execute_tool()`）仍然是阻塞完成后才返回 `ToolResult` 事件，因为工具执行本身不产生增量输出
2. **`FinalAnswer` 是信号**：收到 `FinalAnswer` 事件后，流理论上已结束，建议 `break` 退出循环
3. **错误处理**：流中的每个事件都是 `Result<AgentEvent>`，需要处理中途发生的 LLM 或工具错误

对应示例：`echo-agent-learning/examples/demo10_streaming.rs`

---

## 可跟踪的同轮 Steering

`steer_input()` 只证明活动 turn 的 mailbox 已接收输入。需要可靠持久化的应用应使用
`steer_input_tracked()`，明确区分 mailbox 接收、写入模型上下文和 root turn 终态：

```rust
use echo_agent::prelude::{AgentSteerState, Message};

let mut receipt = agent_handle
    .steer_input_tracked(
        Some("turn-42"),
        Message::user("同时检查生成的文件。".to_string()),
    )
    .await?;

match receipt.wait_for_drained().await {
    AgentSteerState::Drained
    | AgentSteerState::TurnSettled { drained: true, .. } => {
        // 输入已经进入活动模型上下文，durable inbox 可以确认消费。
    }
    AgentSteerState::TurnSettled { drained: false, .. } => {
        // turn 在消费前结束，应保留输入等待重放。
    }
    AgentSteerState::Accepted => {}
}

let _terminal = receipt.wait_for_turn_settled().await;
```

`Drained` 不是成功终态。所属 turn 之后仍可能完成、失败、取消或被 drop。receipt 的
转换由 framework 的真实 mailbox drain 和 active-turn lease 触发；调用方不应根据
渲染 token 或 transcript 写入时机自行推断。Hook block 结算为 `Failed`。生命周期
信号异常关闭时，所有 receipt clone 会收敛为 `Dropped`，同时保留最后确认的 drain
事实。

### 可跟踪的初始输入

冷启动 turn 应在把请求交给 `AgentTurnDriver` 前调用
`TurnRequest::with_input_receipt()`。driver 在 request/identity 校验完成、调用
Agent stream API 之前发布 `Accepted`；`ReactAgent` 只在初始消息成功写入
`ContextManager`、且 provider 调用之前发布 `Drained`；同一个 driver 负责发布
typed terminal。没有 input lifecycle publisher 的 Agent 即使产生输出，也保持
`drained = false`；不能用输出事件或 EOF 替代真实 drain。

### 规范化 turn receipt

`AgentTurnDriver` 返回 framework 对通用 turn 事实的唯一权威
`TurnReceipt`。除 typed terminal 外，它还携带 final answer 及其 message
identity、provider 报告的输入/输出 token 合计、报告 usage 的调用次数、显式
context compaction 次数、最后一个 envelope sequence 和耗时。产品 sink 可以持久化
或渲染同一批 envelope，但必须从 receipt 投影这些字段，不能再从事件折叠第二份
turn summary。workspace routing、UI retention pin、webhook delivery 等产品事实仍留在
应用 adapter。

---

## LLM 超时

`LlmTimeouts` 是完整请求与流式请求共用的唯一 provider-neutral 超时合同。
`LlmConfig` 保存 client 默认值，`ChatRequest` 可用同一种类型覆盖单次调用：

```rust
use echo_agent::prelude::*;
use std::time::Duration;

let timeouts = LlmTimeouts::default()
    .with_request_timeout(Duration::from_secs(90))
    .with_first_chunk_timeout(Duration::from_secs(20))
    .with_idle_timeout(Duration::from_secs(45))
    .without_overall_timeout();

let config = LlmConfig::for_provider(
    "compatible",
    "https://api.example.com/v1",
    "token",
    "model",
    LlmApiProtocol::ChatCompletions,
)?
.with_timeouts(timeouts);

let request = ChatRequest::new(vec![Message::user("你好".to_string())])
    .with_timeouts(timeouts.with_first_chunk_timeout(Duration::from_secs(10)));
```

| 边界 | 默认值 | 范围 |
| --- | ---: | --- |
| `request` | 120s | 完整的非流式请求及 response body |
| `first_chunk` | 30s | 从请求开始到收到第一批 response bytes |
| `idle` | 60s | 流式 byte chunk 之间允许的最大间隔 |
| `overall` | 关闭 | 包含请求启动在内的完整 stream |

对应的 `without_*_timeout()` 方法关闭该边界；序列化配置中的 null 或 0 毫秒也按关闭
处理。Chat Completions、
Responses 和 Anthropic Messages 共用同一条 SSE transport，统一负责请求启动、取消、
UTF-8 安全解码、first/idle/overall 超时以及截断 event 拒绝。provider adapter 只把
语义 JSON event 翻译为 `ChatChunk`。

完整请求和 stream timeout 分开是有意设计：健康的长 stream 可以超过非流式请求
超时，而停滞的 stream 仍会在 first chunk 或 idle 边界失败。超时继续作为 typed LLM
network error 进入既有 retry policy。

### 工具输出落盘与截断

所有工具结果统一经过输出预算阶段。达到 1 MiB 的输出会先写入受管 spill 文件，
模型只接收预览、完整文件路径和 `read_file` 回读提示。即使没有配置
`max_tool_output_tokens`，超大输出仍会落盘。

配置 working directory 时，spill 文件写到
`<working_dir>/.echo-agent/spill`，确保受工作目录约束的文件工具可以回读；未配置时
回退系统临时目录。每次创建新 spill 时会尽力清理一小时前的旧文件。

未达到 spill 阈值但超过 `max_tool_output_tokens` 的结果使用 UTF-8 安全的 head+tail
视图，按 70/30 分配保留字符：

```
完整输出: [────────────────────────────────────────] 10KB
截断后:   [────── head (70%) ──────][── tail (30%) ──]
                              ↑ 省略中间部分
```

spill 创建失败时，框架使用保守的 fallback token 预算截断，不会把完整超大结果直接
塞回模型。`ToolResult.truncated` 和 metadata 记录输出处理方式；spill 成功时，完整引用由
类型化的 `ToolResult.artifact` descriptor 承载。
