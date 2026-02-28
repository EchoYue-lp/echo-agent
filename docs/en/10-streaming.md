# Streaming Output

## What It Is

Streaming output allows the Agent to push Token fragments to the caller as the LLM generates them, rather than waiting for the full response before returning. Users see the Agent "typing" in real time, dramatically improving the interactive experience.

---

## Problem It Solves

Blocking calls have serious UX problems:
- **Long wait**: Complex reasoning tasks take tens of seconds — the UI freezes
- **No feedback**: Users have no visibility into what the Agent is "thinking"
- **Experience gap**: Falls far behind modern AI products (ChatGPT, Claude) in fluency

Streaming solves:
- Time-to-first-token (TTFT) drops from seconds to milliseconds
- Users see the reasoning process (CoT text) and tool calls in real time
- Execution can be cancelled mid-generation

---

## Event Types

`execute_stream()` returns `BoxStream<'_, Result<AgentEvent>>` containing:

```rust
pub enum AgentEvent {
    Token(String),                               // LLM token fragment (reasoning / final answer)
    ToolCall { name: String, args: Value },      // LLM decided to call a tool
    ToolResult { name: String, output: String }, // tool finished, returning result
    FinalAnswer(String),                         // final answer generated, stream ends
}
```

---

## Usage

```rust
use echo_agent::prelude::*;
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant")
        .enable_tool(true);

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(CalculatorTool));

    let mut stream = agent.execute_stream("Calculate (3 + 4) * 5 and explain each step").await?;

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Token(token) => {
                print!("{}", token);
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            AgentEvent::ToolCall { name, args } => {
                println!("\n[Tool call] {} {:?}", name, args);
            }
            AgentEvent::ToolResult { name, output } => {
                println!("[Tool result] {} -> {}", name, output);
            }
            AgentEvent::FinalAnswer(answer) => {
                println!("\n[Final answer] {}", answer);
                break;
            }
        }
    }
    Ok(())
}
```

---

## Streaming + CoT

When `enable_cot=true` (default), the framework appends a guidance instruction to the system prompt, asking the LLM to output reasoning text in the `content` field before each tool call. This text streams out as `Token` events in real time:

```
User: "Calculate 42 * 7"

Event stream:
  Token("Let me analyze this calculation...")       ← CoT reasoning (real-time)
  Token("42 times 7 — I should use the multiply tool")
  ToolCall { name: "multiply", args: {"a": 42, "b": 7} }
  ToolResult { name: "multiply", output: "294" }
  Token("The calculation is done. The result is 294.") ← final answer (real-time)
  FinalAnswer("42 × 7 = 294")
```

---

## Blocking vs Streaming

```rust
// Blocking: wait for full response
let answer: String = agent.execute("Hello").await?;

// Streaming: receive events in real time
let mut stream = agent.execute_stream("Hello").await?;
while let Some(event) = stream.next().await {
    // handle Token / ToolCall / ToolResult / FinalAnswer
}
```

Both modes run identical execution logic; only the delivery mechanism differs. `execute()` internally aggregates streaming events and returns the `FinalAnswer` string.

---

## Using in a Web Service (SSE)

```rust
use axum::response::Sse;
use futures::StreamExt;
use echo_agent::prelude::*;

async fn chat_sse(task: String) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let mut agent = ReactAgent::new(/* config */);

    let event_stream = async_stream::stream! {
        if let Ok(mut agent_stream) = agent.execute_stream(&task).await {
            while let Some(event) = agent_stream.next().await {
                let data = match event {
                    Ok(AgentEvent::Token(t))             => format!("{{\"type\":\"token\",\"data\":\"{}\"}}", t),
                    Ok(AgentEvent::ToolCall { name, .. }) => format!("{{\"type\":\"tool_call\",\"name\":\"{}\"}}", name),
                    Ok(AgentEvent::FinalAnswer(a))        => format!("{{\"type\":\"done\",\"data\":\"{}\"}}", a),
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

## Notes

1. **Tool execution is not streamed**: `execute_tool()` is still blocking — a `ToolResult` event fires only after the tool completes, because tools themselves don't produce incremental output
2. **`FinalAnswer` is a sentinel**: Once received, the stream is logically complete — break out of the loop
3. **Error handling**: Every event in the stream is `Result<AgentEvent>` — handle LLM or tool errors that may occur mid-stream

See: `examples/demo10_streaming.rs`
