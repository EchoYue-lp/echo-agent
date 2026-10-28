# 流式输出（Streaming）

## 是什么

流式输出（Streaming）让 Agent 在 LLM 生成内容的同时，以typed消息块、真实工具和运行状态向调用方传递事实。明确进度和工具事实能持续展示；最终候选在配置校验与结算前按合同缓冲。

---

## 解决什么问题

阻塞式调用的问题：
- **长等待**：复杂任务推理时间长，用户界面冻结数十秒
- **无反馈**：用户不知道 Agent 在"想什么"
- **体验割裂**：与现代 AI 对话产品（ChatGPT、Claude）的流畅体验差距明显

流式输出解决了：
- 及时展示已经允许公开的消息块、真实工具与等待状态；不承诺固定首字延迟
- provider reasoning/公开摘要独立于普通正文与工具显示
- 可以在生成过程中提前终止

---

## 事件类型

以下仅简化展示主要事件 payload，省略身份 metadata 和其它生命周期变体。
`execute_stream()` 返回 `BoxStream<'_, Result<AgentEvent>>`，包含以下事件：

```rust
pub enum AgentEvent {
    AssistantMessage { block: AssistantMessageBlock }, // 原生身份/阶段/增量
    ExecutionStatus(ExecutionStatus),              // 最后runtime结算事实
    ToolCall { invocation: ToolInvocation },    // LLM 决定调用某个工具
    ToolResult { name: String, result: ToolResult }, // 工具执行完毕，返回结果
    FinalAnswer(String),                        // 受检最终正文确认；最后status仍须消费
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

    let mut first_error = None;
    while let Some(event) = stream.next().await {
        match event {
            Ok(AgentEvent::AssistantMessage { block }) => {
                print!("[{:?}] {}", block.phase, block.delta); // 教学追踪事件；业务正文按批准投影呈现
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            Ok(AgentEvent::ToolCall { invocation, .. }) => {
                println!("\n[调用工具] {} {:?}", invocation.name, invocation.args);
            }
            Ok(AgentEvent::ToolResult { name, result, .. }) => {
                println!("[工具结果] {} -> {}", name, result.output);
            }
            Ok(AgentEvent::FinalAnswer(answer)) => {
                println!("\n[最终答案] {}", answer);
            }
            Err(error) => {
                // 展示每条观察，保留首个错误原值；继续消费同一 source 的清理与 EOF。
                eprintln!("[流错误 / 观察] {error}");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Ok(event) => println!("[生命周期] {event:?}"),
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
```

---

## 原生块与执行终态

新执行仅以 `AssistantMessage` 的明确 phase/身份/增量区分 reasoning、普通进度与正文，不再双发 Token/ThinkStart/ThinkEnd 副本。块 complete 只结束块；最终候选经 Guard/Critic/Stop、持久化、原 EOF 和 owner Join 后才发布，最后单次 `ExecutionStatus(Settled)` 才提供运行时完成事实。旧变体保留给历史读取与合理公开 custom Agent。应用不能把确认事件再次拼进已经呈现的正文。

详细字段、恢复呈现资格与 TaskRun 边界见[原生消息合同](../zh/43-native-message-execution.md)。

## 阻塞式 vs 流式对比

```rust
// 阻塞式：等待完整响应
let answer: String = agent.execute("你好").await?;

// 流式：实时接收事件
let mut stream = agent.execute_stream("你好").await?;
while let Some(event) = stream.next().await {
    match event {
        Ok(event) => println!("{event:?}"),
        Err(error) => eprintln!("[流错误 / 观察] {error}"),
    }
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
        match agent.execute_stream(&task).await {
            Ok(mut agent_stream) => {
                while let Some(event) = agent_stream.next().await {
                    let data = match event {
                        Ok(AgentEvent::Token(text)) => serde_json::json!({
                            "type": "token", "data": text,
                        }),
                        Ok(AgentEvent::ToolCall { invocation, .. }) => serde_json::json!({
                            "type": "tool_call", "name": invocation.name,
                        }),
                        Ok(AgentEvent::FinalAnswer(answer)) => serde_json::json!({
                            "type": "answer", "data": answer,
                        }),
                        Err(error) => {
                            // 发送错误给接收方，同时继续消费同一原始 producer。
                            serde_json::json!({
                                "type": "stream_error_or_observation", "message": error.to_string(),
                            })
                        }
                        Ok(event) => serde_json::json!({
                            "type": "lifecycle", "event": event,
                        }),
                    };
                    yield Ok(axum::response::sse::Event::default().data(data.to_string()));
                }
            }
            Err(error) => {
                let data = serde_json::json!({
                    "type": "setup_error_or_observation", "message": error.to_string(),
                });
                yield Ok(axum::response::sse::Event::default().data(data.to_string()));
            }
        }
    };

    Sse::new(event_stream)
}
```

---

## 注意事项

1. **工具执行不是流式的**：工具（`execute_tool()`）仍然是阻塞完成后才返回 `ToolResult` 事件，因为工具执行本身不产生增量输出
2. **`FinalAnswer` 是业务结果**：继续消费同一原始 source 到物理 EOF；答案之后的生产者清理仍可能失败。默认取消和 invocation 适配器保留首个普通清理错误，同时继续持有并 drain 原始 source。
3. **错误处理**：每个事件都是 `Result<AgentEvent>`。drain 时保留最早的失败或取消首因，后续清理错误不能覆盖它。`ExecutionNotSettled` 保留原始 run/execution 身份，是独立的非终态观察；仅有 EOF 不能清除该观察。

以上示例展示每个流错误或观察，并继续消费同一 source 到 EOF。返回首个错误时保留其
原始值，不把 `ExecutionNotSettled` 变成终态失败，也不据此证明物理结算。需要持久化
执行状态的应用应消费规范化 `AgentTurnDriver` receipt，而非从这些 UI 事件推断。

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

### 未启动工具之前的纠正

流式与非流式共享的 ReAct 循环会在模型返回工具提议后，以及原有每个工具执行波次
获准启动前检查同一 mailbox。在检查前已经接收的纠正会先交给原模型重新考虑，
该批尚未启动的操作不会继续执行。检查与 tracked receipt 使用相同的 turn ID 和
私有 incarnation，不按当前界面焦点寻找目标。

尚未写入 transcript 的工具提议直接丢弃，不凭空生成工具结果。如果 assistant 的
call ID 已经写入 transcript，则为余下每个未启动调用写入明确的“未执行、无副作用”
结果；所有 call/result 配对闭合后，纠正才进入原上下文并变为 `Drained`。此前真实
完成的结果仍供模型参考，此前的最终答案候选则需要重新考虑。

已经获准启动的波次继续持有原执行 future、并行调用、结果、stream reader 和资源，
直到真实结算。波次检查后才收到的 Steer 等到下一未启动边界处理，不抢占活动模型
或工具，不回答 HITL，也不代表 Stop。显式 Stop、超时继续沿原取消与物理结算路径
处理，不自动重放效果未知的操作。

这些提前检查是本框架选定的响应性增强。已核对的 Pi 官方循环在整批工具完成后
才消费 Steer，不能据此声称其已有逐工具纠正保证。取舍和参考见
[ADR 0003](../adr/0003-tracked-steer-lifecycle.md)。草稿确认和持久输入仍由应用
负责，`Accepted`、`Drained` 不替代这些产品事实。

### 可跟踪的初始输入

冷启动 turn 应在把请求交给 `AgentTurnDriver` 前调用
`TurnRequest::with_input_receipt()`。driver 在 request/identity 校验完成、调用
Agent stream API 之前发布 `Accepted`；`ReactAgent` 只在初始消息成功写入
`ContextManager`、且 provider 调用之前发布 `Drained`；同一个 driver 负责发布
typed terminal。没有 input lifecycle publisher 的 Agent 即使产生输出，也保持
`drained = false`；不能用输出事件或 EOF 替代真实 drain。

### 规范化 turn receipt

`AgentTurnDriver` 返回 framework 对通用 turn 事实的唯一权威
`TurnReceipt`。除 typed execution terminal 外，它还携带正交的 delivery result、
final answer 及其 message identity、provider 报告的输入/输出 token 合计、报告 usage
的调用次数、显式 context compaction 次数、最后一个 envelope sequence 和耗时。产品
sink 可以持久化或渲染同一批 envelope，但必须从 receipt 投影这些字段，不能再从事件
折叠第二份 turn summary。一个 turn 可以 execution 已完成但 delivery 失败；调用方只有
同时检查两条结果后，才能声称结果已交付。workspace routing、UI retention pin、
webhook delivery 等产品事实仍留在应用 adapter。

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

各 provider 还会在发布成功的 finish reason 和 usage 前验证自身的语义终态：Chat
Completions 要求成功的 choice finish reason 后再收到 `[DONE]`；Responses 要求
`response.completed`；Anthropic Messages 要求成功的 `message_delta` 后再收到
`message_stop`。这些信号之前的 EOF、非成功 stop reason 或损坏的最终事件都会返回
typed `InvalidResponse`。调用者仍可看到之前的部分 delta，但不能把它们当作已完成响应。

完整请求和 stream timeout 分开是有意设计：健康的长 stream 可以超过非流式请求
超时，而停滞的 stream 仍会在 first chunk 或 idle 边界失败。超时继续作为 typed LLM
network error 进入既有 retry policy。

### 工具输出落盘与截断

ReAct 工具结果统一经过输出预算阶段。启用 artifact 存储时，达到配置的字节阈值
（框架默认 1 MiB）或超过 `max_tool_output_tokens` 的输出会先持久化。模型只接收预览、
精确 artifact 路径、SHA-256 和 `read_artifact` 分页读取提示。即使没有配置
`max_tool_output_tokens`，按字节阈值触发的 spill 仍然生效。

artifact 根目录和保留策略由 `tool_output_artifacts` 配置拥有。框架默认使用系统
临时目录下的 `echo_agent_artifacts/tool-logs`，最大保留一小时；应用可以提供自己的
受管根和保留策略。传入 `None` 禁用 artifact 持久化；仅配置 working directory
不会将它变成 artifact 根或完整输出的读取权威。

持久化禁用或失败时，超预算输出使用 UTF-8 安全的 head+tail 视图，按 70/30 分配
保留字符。原 `HeuristicTokenizer` 对“头部＋截断通知＋尾部”整体计数，ASCII、中文、
emoji 混合输出也不能超过同一个 estimated-token 上限：

```
完整输出: [────────────────────────────────────────] 10KB
截断后:   [────── head (70%) ──────][── tail (30%) ──]
                              ↑ 省略中间部分
```

预算很小时使用短通知或省略号；零 token 预算返回空的模型文本，但仍通过
`ToolResult.truncated` 和 metadata 记录截断。这是估算 token 上限，不是 provider
计费 token 保证。spill 失败且没有显式 token 配置时，继续采用既有的 8,000-token
fallback。失败的 spill 不产生可恢复 artifact，被省略内容不能声称可通过
`read_artifact` 恢复。spill 成功时，完整引用由类型化的 `ToolResult.artifact` descriptor
承载；消费者不得从展示文字或 metadata 重建该引用。
