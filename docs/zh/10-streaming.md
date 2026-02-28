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

对应示例：`examples/demo10_streaming.rs`
