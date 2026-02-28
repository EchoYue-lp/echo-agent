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
    agent.reset();

    Ok(())
}
```

### 流式多轮对话

```rust
use echo_agent::prelude::*;
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手");
    let mut agent = ReactAgent::new(config);

    let messages = [
        "我在学习 Rust 的异步编程。",
        "能给我一个 async/await 的简单例子吗？",
        "基于我刚才的问题，你觉得我下一步应该学什么？",
    ];

    for msg in &messages {
        println!("用户: {msg}");
        print!("Agent: ");
        std::io::stdout().flush().ok();

        let mut stream = agent.chat_stream(msg).await?;

        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::Token(token) => {
                    print!("{token}");
                    std::io::stdout().flush().ok();
                }
                AgentEvent::FinalAnswer(_) => break,
                _ => {}
            }
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
let (msg_count, token_est) = agent.context_stats();
println!("当前上下文：{msg_count} 条消息，估算 ~{token_est} tokens");
```

### 重置对话（开启新会话）

`reset()` 是 `Agent` trait 的方法，对所有实现类均可用（包括通过 `dyn Agent` 持有的实例）：

```rust
// 直接调用（具体类型）
agent.reset();

// 通过 trait 对象调用
let mut agent: Box<dyn Agent> = Box::new(ReactAgent::new(config));
agent.chat("第一轮：你好，我叫张三").await?;
agent.reset();                               // ← trait 方法，清除上下文
agent.chat("第二轮：我是谁？").await?;       // Agent 不再记得"张三"
```

### 结合 Checkpointer 跨进程续接

`chat()` 的多轮历史可以配合 Checkpointer 持久化，在重启后恢复：

```rust
use echo_agent::prelude::*;
use std::sync::Arc;

let cp = FileCheckpointer::new("~/.echo-agent/checkpoints.json")?;
let mut agent = ReactAgent::new(config);
agent.set_checkpointer(Arc::new(cp), "user-alice-session".to_string());

// 首次启动：会从 Checkpointer 恢复已有历史（如有）
agent.chat("继续我们上次的对话…").await?;
// 每轮 chat() 结束后自动保存 Checkpoint
```

---

## 结合上下文压缩

对话轮次增多后，上下文会持续增长。可以配置自动压缩，防止超出模型 token 限制：

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "assistant", "你是一个助手")
    .token_limit(8192); // 超过此限制时触发压缩

let mut agent = ReactAgent::new(config);

// 配置滑动窗口压缩（仅保留最近 20 条消息）
agent.set_compressor(SlidingWindowCompressor::new(20));

// 此后 chat() 调用会在 token 超限时自动触发压缩
agent.chat("第一条消息").await?;
// ...更多轮次
```

---

## Agent Trait 设计

`Agent` trait 包含完整的对话生命周期接口：

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn system_prompt(&self) -> &str;

    /// 阻塞执行，每次调用重置上下文（单轮模式）。连续对话请用 `chat()`。
    async fn execute(&mut self, task: &str) -> Result<String>;

    /// 流式执行，每次调用重置上下文（单轮模式）。连续对话请用 `chat_stream()`。
    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>;

    /// 多轮对话（阻塞）。追加到现有上下文，历史跨轮保留。
    /// 用 `reset()` 开启新会话；默认回退到 `execute()`。
    async fn chat(&mut self, message: &str) -> Result<String> {
        self.execute(message).await
    }

    /// 多轮对话（流式）。追加到现有上下文，历史跨轮保留。
    /// 用 `reset()` 开启新会话；默认回退到 `execute_stream()`。
    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        self.execute_stream(message).await
    }

    /// 清除对话历史，开启新会话。不影响 `execute()`（它自行重置）。
    /// 默认 no-op；维护对话状态的实现类应覆盖此方法。
    fn reset(&mut self) {}
}
```

**各实现类的行为对比：**

| 实现 | `chat()` | `reset()` |
|------|---------|---------|
| `ReactAgent` | 保留完整上下文 | 清空历史，仅保留 system prompt |
| `MockAgent` | 记录调用、消费响应队列 | 清空调用历史 |
| `FailingMockAgent` | 总是返回错误 | 清空调用历史 |
| 其他自定义 Agent | 默认回退到 `execute()` | 默认 no-op |

---

## 在 Web 服务中使用（Chatbot API）

```rust
use axum::{Json, extract::State};
use echo_agent::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;

// 共享 Agent 状态（单用户场景示例）
type AgentState = Arc<Mutex<ReactAgent>>;

async fn chat_handler(
    State(agent): State<AgentState>,
    Json(req): Json<ChatRequest>,
) -> Json<ChatResponse> {
    let mut agent = agent.lock().await;
    let answer = agent.chat(&req.message).await.unwrap_or_default();
    Json(ChatResponse { answer })
}

// 流式版本（SSE）
async fn chat_stream_handler(
    State(agent): State<AgentState>,
    Json(req): Json<ChatRequest>,
) -> axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let event_stream = async_stream::stream! {
        let mut agent = agent.lock().await;
        if let Ok(mut stream) = agent.chat_stream(&req.message).await {
            while let Some(event) = stream.next().await {
                let data = match event {
                    Ok(AgentEvent::Token(t))      => format!("{{\"type\":\"token\",\"data\":\"{t}\"}}"),
                    Ok(AgentEvent::FinalAnswer(a)) => format!("{{\"type\":\"done\",\"data\":\"{a}\"}}"),
                    _ => continue,
                };
                yield Ok(axum::response::sse::Event::default().data(data));
            }
        }
    };
    axum::response::Sse::new(event_stream)
}
```

---

## 注意事项

1. **多用户场景下需要独立 Agent 实例**：`ReactAgent` 不是线程安全的，每个用户会话应有独立实例（或用 `Arc<Mutex<ReactAgent>>`）
2. **reset() 清除的是内存中的历史**：如果配置了 Checkpointer，已持久化的会话不受影响，下次调用 `execute()` 时仍会恢复
3. **上下文增长**：长时间对话会累积大量 token，建议配合 `set_compressor()` 使用
4. **`execute()` 不影响 `chat()` 的历史**：混用 `execute()` 和 `chat()` 时，`execute()` 每次调用都会重置历史，之前 `chat()` 积累的上下文会丢失

对应示例：`examples/demo17_chat.rs`
