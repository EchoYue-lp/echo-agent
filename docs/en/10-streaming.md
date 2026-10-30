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

## Tracked Same-Turn Steering

`steer_input()` confirms only that the active turn mailbox accepted the input.
Durable applications should use `steer_input_tracked()` when they must distinguish
mailbox acceptance, insertion into model context, and root-turn settlement:

```rust
use echo_agent::prelude::{AgentSteerState, Message};

let mut receipt = agent_handle
    .steer_input_tracked(
        Some("turn-42"),
        Message::user("Also verify the generated files.".to_string()),
    )
    .await?;

match receipt.wait_for_drained().await {
    AgentSteerState::Drained
    | AgentSteerState::TurnSettled { drained: true, .. } => {
        // The input reached the active model context. A durable inbox may ack it.
    }
    AgentSteerState::TurnSettled { drained: false, .. } => {
        // The turn ended before consumption. Retain the input for replay.
    }
    AgentSteerState::Accepted => {}
}

let _terminal = receipt.wait_for_turn_settled().await;
```

`Drained` is not a success terminal. The owning turn may still complete, fail,
be cancelled, or be dropped. Receipt transitions come from the framework's real
mailbox drain and active-turn lease; callers should not infer them from rendered
tokens or transcript timing. Hook blocks settle as `Failed`. If the lifecycle
signal closes abnormally, all receipt clones converge on `Dropped` while
preserving the last known drain fact.

### Steering before unstarted tools

The shared streaming/non-streaming ReAct loop checks the original mailbox when
the model returns a tool proposal, and before admitting each existing tool
execution wave. A correction accepted before the check returns to the model
before that unstarted work runs. This uses the same turn ID and private
incarnation as the tracked receipt.

An unpublished tool proposal is discarded without inventing tool results. If
the assistant's call IDs are already in the transcript, every remaining
unstarted call receives an explicit not-executed result with no side effects,
and all call/result pairs are closed before the correction enters context and
becomes `Drained`. Earlier physical results remain available to the model;
earlier final-answer candidates are reconsidered.

An admitted wave keeps its original execution futures, parallel peers, results,
stream readers and resources until actual settlement. Steering received after
that wave's check waits for the next unstarted boundary. It does not interrupt
an active model/tool, answer HITL or request Stop. Explicit Stop and timeout
continue to use their original cancellation and physical-settlement paths;
uncertain effects are never automatically replayed.

These earlier checks are the behavior selected by this framework. The reviewed
official Pi loop polls steering after a whole tool batch; it is not evidence of
a per-tool steering guarantee. See [ADR 0003](../adr/0003-tracked-steer-lifecycle.md)
for the decision and source comparison. Product draft acknowledgment and durable
ingress remain application responsibilities; `Accepted` and `Drained` do not
replace those product facts.

### Tracked initial input

For a cold turn, call `TurnRequest::with_input_receipt()` before passing the
request to `AgentTurnDriver`. The driver publishes `Accepted` after validating
the request and immediately before invoking the Agent. `ReactAgent` publishes
`Drained` only after the initial message has entered `ContextManager`, before
the provider call. The same driver publishes the typed terminal outcome. Agents
that do not expose an input lifecycle publisher leave the terminal receipt at
`drained = false`; output events and EOF are never used as a substitute.

### Canonical turn receipt

`AgentTurnDriver` returns the sole framework-owned `TurnReceipt` for generic
turn facts. In addition to the typed execution terminal, it carries an
orthogonal delivery result, the final answer and message identity,
provider-reported input/output totals, reported-call count, explicit
context-compaction count, final envelope sequence, and elapsed time. Product
sinks may persist or render the same envelopes, but must project these fields
from the receipt instead of folding a second turn summary from events. A turn
can be execution-completed while delivery failed; callers must inspect both
results before claiming the result was delivered.
Product-only facts such as workspace routing, UI retention pins, and webhook
delivery remain in the application adapter.

---

## LLM Timeouts

`LlmTimeouts` is the single provider-neutral timeout contract for complete and
streaming requests. `LlmConfig` owns the client default, while `ChatRequest`
may override the same value for one call:

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

let request = ChatRequest::new(vec![Message::user("Hello".to_string())])
    .with_timeouts(timeouts.with_first_chunk_timeout(Duration::from_secs(10)));
```

| Boundary | Default | Scope |
| --- | ---: | --- |
| `request` | 120s | Complete non-streaming request and response body |
| `first_chunk` | 30s | Request start through the first response bytes |
| `idle` | 60s | Maximum gap between streaming byte chunks |
| `overall` | disabled | Complete stream, including request startup |

The matching `without_*_timeout()` method disables a boundary; serialized null
or zero milliseconds also deserialize as disabled. Chat Completions, Responses,
and Anthropic Messages use the same SSE
transport for request startup, cancellation, UTF-8-safe decoding, first/idle/
overall timeouts, and truncated-event rejection. Provider adapters only
translate semantic JSON events into `ChatChunk` values.

Each provider also validates its own completion signal before publishing a
successful finish reason or usage: Chat Completions requires a successful choice
finish reason followed by `[DONE]`; Responses requires `response.completed`;
Anthropic Messages requires a successful `message_delta` followed by
`message_stop`. An EOF before these signals, a non-success stop reason, or a
malformed final event returns a typed `InvalidResponse`. Partial deltas remain
available to a live caller, but they are not a completed model response.

This separation is intentional: a healthy long stream can exceed the
non-streaming request timeout, while a stalled stream still fails at its first
chunk or idle boundary. Timeout failures remain typed LLM network errors and
flow through the existing retry policy.

### Tool Output Spill and Truncation

ReAct tool results pass through one output-budget stage. With artifact storage enabled,
outputs at its configured byte threshold (1 MiB by default), or above the configured
`max_tool_output_tokens`, are persisted before truncation. The model receives a preview
and the exact artifact path and SHA-256 for bounded `read_artifact` pages. Byte-threshold
spill remains active when `max_tool_output_tokens` is not configured.

`tool_output_artifacts` owns the artifact root and retention policy. The framework default
uses the system temporary directory under `echo_agent_artifacts/tool-logs`, with a
one-hour maximum age; applications can supply their own managed root and retention.
Passing `None` disables artifact persistence. Working directory alone is not an artifact
root or a complete-output reading authority.

When persistence is disabled or fails, oversized output uses a UTF-8-safe head+tail view
with a 70/30 allocation of retained characters. The existing `HeuristicTokenizer` counts
the entire rendered text, including the truncation notice; mixed ASCII/CJK/emoji output
stays within the same estimated-token cap:

```
Full output:    [────────────────────────────────────────] 10KB
After truncate: [────── head (70%) ──────][── tail (30%) ──]
                                      ↑ middle omitted
```

Small budgets use a shorter notice or an ellipsis; a zero-token cap returns empty model
text while `ToolResult.truncated` and metadata still record the truncation. This is an
estimated-token bound, not a provider billing-token guarantee. If spill creation fails
without an explicit token cap, the existing 8,000-token fallback applies. Failed spill
does not create a recoverable artifact: the omitted content is unavailable through
`read_artifact`. A successful spill is carried by the typed `ToolResult.artifact`
descriptor; consumers must not reconstruct it from display text or metadata.
