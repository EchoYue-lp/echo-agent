# 流式输出与原生执行

## 当前入口

`execute()` / `chat()` 返回 `String`，对应的流式入口返回
`BoxStream<'_, Result<AgentEvent>>`。两种返回形式共用原 typed producer、ReAct、
校验、取消与物理收尾；String 便利方法读取同一原流至 EOF 后投影最终结果。
`chat` 保留多轮上下文，不能因为请求的是 String 就换成 fresh execute。

返回形式不决定模型 HTTP 传输。`model_streaming(true)` 是默认值；显式
`model_streaming(false)` 让同一执行调用 `LlmClient::chat`，完整响应仍保留原生块、
工具来源、reasoning 与实际 usage，随后走同一候选校验和结算路径。它不是第二套执行器。

## 当前事件

| 事件 | 含义 |
| --- | --- |
| `AssistantMessage { block }` | 保留请求、块身份、序号、明确阶段与增量；缺阶段保持 Unknown |
| `ExecutionStatus` | 原 source 的 Started/运行观察及真正终态，不由展示文本推断 |
| `ToolCall` / `ToolResult` | 保留 call_id、真实调用与完整结果/资源引用 |
| `LlmUsage` | 每次模型响应的实际报告；未报告时不能把零当成已知用量 |
| `FinalAnswer(String)` | 当前运行时批准的最终正文通知；不代表原 source 已读到 EOF |

Core 的旧 AgentEvent Token/ThinkStart/ThinkEnd 和旧 String source fallback 已退役。
provider 原生 reasoning wire、签名/加密回放块及真实 `on_think_*` 回调继续保留。
显示推理时只取投影后的可见文字，不直接打印完整 block、签名或密文。

## 读取同一原流到 EOF

下面沿用当前 [demo17_chat.rs](../../echo-agent-learning/examples/demo17_chat.rs) 的
原生投影模式，改为一次 fresh execute。首因和未结算观察保留原值，所有事件都继续
从同一 source 读取。只有投影接受的同 run/execution 新终态才能解除匹配观察，
foreign 状态或正文不能解除它。每段正文只展示一次；EOF 后补同 request 尚未显示的
安全最终正文，FinalAnswer 通知本身不重复打印。

```rust,no_run
use echo_agent::agent::{Agent, AgentEvent, AgentOutputProjection, AssistantPhase, ExecutionStatusKind};
use echo_agent::error::{AgentError, AgentTerminalKind, ReactError};
use echo_agent::prelude::*;
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .system_prompt("你是一个有帮助的助手，请用中文回答。")
        .build()?;
    let mut stream = agent.execute_stream("解释 Rust 的异步编程").await?;
    let owner_run = agent.current_run_id();
    let mut output = AgentOutputProjection::default();
    let mut first_error = None;
    let mut unresolved = None::<ReactError>;
    let same_source = |error: &ReactError, projection: &AgentOutputProjection| {
        matches!(error, ReactError::ExecutionNotSettled { run_id, execution_id }
            if owner_run.as_deref() == Some(run_id.as_str())
                && execution_id.as_deref().is_some()
                && execution_id.as_deref() == projection.execution_id())
    };

    // 持续读取同一原 source 到真实 EOF，再判断是否成功。
    while let Some(event) = stream.next().await {
        let observed = match event {
            Err(error @ ReactError::ExecutionNotSettled { .. }) => {
                // 未结算观察不是终态；foreign 观察不会被这条 source 的完成覆盖。
                if unresolved
                    .as_ref()
                    .is_none_or(|prior| same_source(prior, &output))
                {
                    unresolved = Some(error);
                }
                Ok(())
            }
            Ok(AgentEvent::ExecutionStatus(status)) => {
                output.observe_status(&status).map(|changed| {
                    if changed
                        && matches!(
                            status.status,
                            ExecutionStatusKind::Settled
                                | ExecutionStatusKind::Failed
                                | ExecutionStatusKind::Cancelled
                        )
                        && unresolved
                            .as_ref()
                            .is_some_and(|prior| same_source(prior, &output))
                    {
                        unresolved = None;
                    }
                    if let Some(error) = output.terminal_failure() {
                        first_error.get_or_insert(error);
                    }
                })
            }
            Ok(AgentEvent::AssistantMessage { block }) => {
                output.observe_block(&block).map(|delta| {
                    if let Some(delta) = delta {
                        // 只显示投影后的可见文本；签名、密文和重放载体不直接输出。
                        match delta.block.phase {
                            AssistantPhase::Reasoning if !delta.text.is_empty() => {
                                println!("\n[推理] {}", delta.text)
                            }
                            AssistantPhase::Commentary if !delta.text.is_empty() => {
                                println!("\n[进度] {}", delta.text)
                            }
                            AssistantPhase::Reasoning | AssistantPhase::Commentary => {}
                            _ => {
                                print!("{}", delta.text);
                                std::io::stdout().flush().ok();
                            }
                        }
                    }
                })
            }
            Ok(AgentEvent::LlmUsage {
                model,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                usage_reported,
                ..
            }) => {
                if usage_reported {
                    println!(
                        "\n[LLM 用量] model={model}, prompt={prompt_tokens}, completion={completion_tokens}, total={total_tokens}"
                    );
                } else {
                    println!("\n[LLM 用量] model={model}，上游未报告用量");
                }
                Ok(())
            }
            Ok(AgentEvent::FinalAnswer(answer)) => output.confirm_answer(&answer),
            Ok(AgentEvent::Error { failure, .. }) => Err(match failure.terminal_kind {
                AgentTerminalKind::Cancelled => AgentError::Cancelled(failure.message).into(),
                AgentTerminalKind::TimedOut => AgentError::Timeout(failure.message).into(),
                AgentTerminalKind::PermissionDenied => {
                    AgentError::PermissionDenied(failure.message).into()
                }
                _ => ReactError::Other(failure.message),
            }),
            Ok(AgentEvent::Cancelled) => {
                Err(AgentError::Cancelled("原生 Agent source 已取消".into()).into())
            }
            Err(error) => Err(error),
            _ => Ok(()),
        };
        if let Err(error) = observed {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = unresolved {
        return Err(error);
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    let final_answer = output.finish()?;
    // 原 source EOF 后只补尚未展示的安全最终正文，确认通知本身不重印。
    if let Some(missing) = output.take_final_presentation()? {
        print!("{missing}");
        std::io::stdout().flush().ok();
    }
    if final_answer.trim().is_empty() {
        return Err(ReactError::Other(
            "流式示例返回空答案".into(),
        ));
    }
    Ok(())
}
```

这段代码按当前源码接口整理，本轮没有执行片段编译、模型调用或原生宿主验收。
完整进度、工具和回调示例见 [demo11_callbacks.rs](../../echo-agent-learning/examples/demo11_callbacks.rs)，
多轮入口见 [13-chat.md](./13-chat.md)。

## 自定义 Agent 与完整调用值

自定义实现应接纳 `execute_stream_message_with_invocation_context` 和/或
`chat_stream_message_with_invocation_context`，按实际支持能力实现各自 typed producer。
Message、history、取消、资源、工作目录及工具上界按值进入同一次调用。未实现的入口
在 source admission 前明确拒绝：当前默认错误分别为 `canonical typed execute invocation`
和 `canonical typed chat invocation`；不会调用旧 String 方法补正文、猜 Started 或补成功。
只有 fresh execute 的自定义 Agent 不会自动获得多轮 chat 能力。

## 在 SSE 或其它事件接收端中使用

接收端可序列化原生消息块、工具事实和运行观察，或显示同一投影产生的可见文字。
`FinalAnswer` 是确认通知，不能被命名为“已关闭”或追加到已经显示的正文。
投递失败/接收端断开是 delivery 结果，不能代替 producer 的执行/物理结算。
需要规范化 turn receipt 的宿主使用现有 `AgentTurnDriver`，保留原 source 到 EOF；
执行结果与投递结果分别判断，不为 UI 新建另一份执行 owner 或 turn summary。

## 完成、失败与取消

- 单块 `complete`、provider 的 `phase=final_answer` 和普通文字均不能证明执行完成。
- 最终候选继续经过配置的 Guard/Critic、Stop、持久化和原 owner 的实际收尾；
  只有真实 native status 与原 EOF 满足后才返回成功，尾部错误仍可阻止成功。
- `ExecutionNotSettled` 是非终态观察；原 run/execution 不能被新身份或 foreign 状态替换。
- 普通工具还可以发布进度与资源事件，不能把某个工具返回或工具日志当作 Agent 终态。

完整字段与持久化约束见[原生消息与执行结算](../zh/43-native-message-execution.md)。

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
