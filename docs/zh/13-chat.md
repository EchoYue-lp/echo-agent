# 多轮对话（Chat 模式）

## 是什么

`chat()` / `chat_stream()` 是专为**连续多轮对话**设计的接口。与 `execute()` 每次调用都重置上下文不同，`chat()` 在现有对话历史上追加消息，让 Agent 能记住之前所有轮次的内容。

---

## 解决什么问题

`execute()` 是"单次任务"语义——每次调用内部都会重置消息历史，适合独立的批处理任务。但在 Chatbot、交互式助手等场景中，用户期望 Agent 能记住对话上下文：

```
// 用 execute() 做连续对话的问题
agent.execute("我叫张三").await?;
agent.execute("你记得我的名字吗？").await?;
// Agent 答：「不知道，我们才刚见面。」← 上下文已被重置
```

`chat()` 解决了这个问题：

```
// chat() 的正确行为
agent.chat("我叫张三").await?;
agent.chat("你记得我的名字吗？").await?;
// Agent 答：「你叫张三。」← 历史完整保留
```

---

fresh execute 不追加上一轮 chat；显式传入 history 或恢复运行时上下文时，仍由当前
typed producer 按既有身份与持久化合同处理。多轮可用历史受预算和压缩策略约束。

## 核心差异

| | `execute()` / `execute_stream()` | `chat()` / `chat_stream()` |
|---|---|---|
| 调用时重置上下文 | ✅ 是 | ❌ 否 |
| 跨轮记忆 | ❌ 无 | ✅ 有 |
| 工具调用支持 | ✅ | ✅ |
| 长期记忆（Store）注入 | ✅ | ✅ |
| Checkpoint 自动保存 | ✅ | ✅ |
| 适用场景 | 独立批处理任务 | 连续对话 / Chatbot |

---

## 基本用法

### 阻塞式多轮对话

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);

    // 第一轮
    let r1 = agent.chat("你好，我叫小明，是一名 Rust 程序员。").await?;
    println!("Agent: {r1}");

    // 第二轮 — Agent 记得"小明"和"Rust 程序员"
    let r2 = agent.chat("你还记得我的名字和职业吗？").await?;
    println!("Agent: {r2}");

    // 第三轮 — 基于前两轮的信息做出个性化建议
    let r3 = agent.chat("根据我的背景，有什么学习建议？").await?;
    println!("Agent: {r3}");

    // 清除历史，开启新一轮对话
    agent.reset().await;

    Ok(())
}
```

### 流式多轮对话

```rust,no_run
use echo_agent::agent::{Agent, AgentEvent, AgentOutputProjection, AssistantPhase, ExecutionStatusKind};
use echo_agent::error::{AgentError, AgentTerminalKind, ReactError};
use echo_agent::prelude::*;
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    println!("  使用 chat_stream() 进行流式多轮对话\n");

    // 使用 AgentBuilder 创建 Agent
    let agent = ReactAgentBuilder::new()
        .model("qwen3-max")
        .name("stream_chat_agent")
        .system_prompt("你是一个助手，用中文回答，不超过三句话。")
        .max_iterations(3)
        .build()?;

    let messages = [
        "我在学习 Rust，想了解异步编程。",
        "能给我一个 async/await 的简单例子吗？",
    ];

    for msg in &messages {
        println!("  👤 用户: {msg}");
        print!("  🤖 Agent: ");
        std::io::stdout().flush().ok();

        let mut stream = agent.chat_stream(msg).await?;
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

        // 每轮读取同一 chat source 到真实 EOF，下一轮才复用会话上下文。
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
                "demo17 验收失败：流式对话返回空答案".into(),
            ));
        }
        println!("\n");
    }

    Ok(())
}
```

---

## 携带工具的多轮对话

`chat()` 完整支持工具调用。多轮推理时，Agent 可以引用前几轮工具调用的结果：

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new(
        "qwen3-max",
        "math_agent",
        "你是一个计算助手，需要计算时使用工具，记住每轮的结果。",
    )
    .enable_tool(true);

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));

    // 第一轮：初始计算
    let r1 = agent.chat("计算 15 + 27，记住这个结果。").await?;
    println!("第一轮结果: {r1}");

    // 第二轮：引用上一轮结果（Agent 从上下文中知道第一轮答案是 42）
    let r2 = agent.chat("把上一步的结果再乘以 3。").await?;
    println!("第二轮结果: {r2}");

    Ok(())
}
```

---

## 管理对话历史

### 查看上下文状态

```rust
// 返回 (消息条数, 估算 token 数)；ReactAgent 专有方法
let (msg_count, token_est) = agent.context_stats().await;
println!("当前上下文：{msg_count} 条消息，估算 ~{token_est} tokens");
```

### 重置对话（开启新会话）

`reset()` 是 `Agent` trait 的方法，对所有实现类均可用（包括通过 `dyn Agent` 持有的实例）：

```rust
// 直接调用（具体类型）
agent.reset().await;

// 通过 trait 对象调用
let mut agent: Box<dyn Agent> = Box::new(ReactAgent::new(config));
agent.chat("第一轮：你好，我叫张三").await?;
agent.reset().await;                               // ← trait 方法，清除上下文
agent.chat("第二轮：我是谁？").await?;       // Agent 不再记得"张三"
```

### 结合 RuntimeStateStore 跨进程续接

当前会话可以配置 RuntimeStateStore 保存运行时上下文；恢复仍需严格匹配
checkpoint 身份、transcript generation 与当前 Message/history。不能只靠相同
conversation_id 接受任意旧断点、缺失字段或重放已完成副作用。任务计划仍归版本化
任务图，不从 ReAct checkpoint 恢复。Store 配对与冷恢复边界见
[41-persistence-concepts.md](./41-persistence-concepts.md)，记忆能力见
[03-memory.md](./03-memory.md)。

---

## 结合上下文压缩

对话轮次增多后，上下文会持续增长。可以配置自动压缩，防止超出模型 token 限制：

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "assistant", "你是一个助手")
    .token_limit(8192); // 超过此限制时触发压缩

let mut agent = ReactAgent::new(config);

// 配置滑动窗口压缩（仅保留最近 20 条消息）
agent.set_compressor(SlidingWindowCompressor::new(20)).await;

// 此后 chat() 调用会在 token 超限时自动触发压缩
agent.chat("第一条消息").await?;
// ...更多轮次
```

---

流式例子的循环与当前 demo17 保持一致；本轮只核对源码和文档，未执行片段编译或模型验收。

## 当前 Agent typed 合同

String 与 raw-event 便利入口委托给对应 typed producer；接口使用 `&self` 和
`BoxFuture`，不使用旧的 `async_trait`/`&mut self` 定义。下面仅列签名关系：

```text
execute / execute_stream
  -> execute_stream_message_with_invocation_context(Message, CancellationToken, AgentInvocationContext)
chat / chat_stream
  -> chat_stream_message_with_invocation_context(Message, CancellationToken, AgentInvocationContext)
reset(&self) -> Future<Output = ()>
```

两个 typed 入口独立声明能力，完整 Message、history、invocation 与资源不能先压成
String 再回补。实现未提供该入口时，当前默认明确返回 `ReactError::Other`，消息为
`canonical typed execute invocation` 或 `canonical typed chat invocation`。
它不是成功响应，也不 fallback 到另一个入口。原生 source 自己提供稳定的
Started、真实终态及原 EOF；读取方不能根据 FinalAnswer 文本制造这些事实。

| 实现 | 多轮入口 | reset |
| --- | --- | --- |
| ReactAgent | 原上下文上的 typed chat；预算压缩/恢复仍按当前 owner | 异步清除会话消息，必须 await |
| MockAgent | 显式 typed chat 使用配置响应，保完整调用记录与原生状态 | 清除调用历史 |
| FailingMockAgent | 保留原 typed setup/脚本错误，不合成成功 | 清除调用历史 |
| 自定义 Agent | 实现自己支持的 typed 入口；缺失入口明确拒绝 | 默认空操作；有会话状态时自行实现 |

---

## 在宿主或 Web 接口中使用

每个需要独立上下文的会话持有自己的 Agent，或由宿主串行驱动同一会话。
`Agent: Send + Sync` 不意味着可把同一实例的多轮上下文任意并发写入。
接收完整 Message 与 history 时使用对应 typed chat 入口；调用级工作目录、工具上界、
取消和资源放入 `AgentInvocationContext`，不经旧 mutable setter/source fallback 传递。

SSE 只负责运输事实；若接收端断开，投递结果与原执行结果分开记录，原 source 仍须由
原 owner 收尾。宿主需要持久 turn receipt 时使用既有 `AgentTurnDriver`。
原生字段与投影示例见 [10-streaming.md](./10-streaming.md)，不把 FinalAnswer 通知
重命名为物理关闭，不把 transport flush 当成 producer EOF。

---

## 使用约束

1. `reset().await`、`context_stats().await`、`set_compressor(...).await` 都必须等待完成。
2. Store 恢复、transcript generation 和严格 checkpoint 配对按当前持久化合同处理，
   不能只凭相同 conversation_id 声称任意旧断点都会自动恢复；见 [41-persistence-concepts.md](./41-persistence-concepts.md)。
3. 上下文受 token 预算与压缩策略约束；多轮不代表保留无限历史。
4. fresh execute 与 chat 语义分开；不要用 execute 替代下一轮 chat，也不要在原 stream
   的最终通知处提前关流后立即写入下一轮。
5. String/event 返回形式和 `model_streaming(bool)` 是两个独立选择。显式 false 仍走
   同一 typed 入口、原生校验与 EOF 规则，默认 true 不变。

实际完整示例：[demo17_chat.rs](../../echo-agent-learning/examples/demo17_chat.rs)。
