# Multi-Turn Chat Mode

## What It Is

`chat()` / `chat_stream()` are interfaces designed specifically for **continuous multi-turn conversations**. Unlike `execute()`, which resets the context on every call, `chat()` appends messages to the existing conversation history, allowing the Agent to remember all previous turns.

---

## Problem It Solves

`execute()` has "single-task" semantics — each call internally resets the message history, making it ideal for independent batch tasks. However, in Chatbot and interactive assistant scenarios, users expect the Agent to remember conversation context:

```
// The problem with using execute() for continuous dialogue
agent.execute("My name is Alice").await?;
agent.execute("Do you remember my name?").await?;
// Agent: "I don't know — we just met." ← context was reset
```

`chat()` fixes this:

```
// Correct behavior with chat()
agent.chat("My name is Alice").await?;
agent.chat("Do you remember my name?").await?;
// Agent: "Your name is Alice." ← history fully preserved
```

---

## Core Differences

| | `execute()` / `execute_stream()` | `chat()` / `chat_stream()` |
|---|---|---|
| Resets context on call | ✅ Yes | ❌ No |
| Cross-turn memory | ❌ None | ✅ Full |
| Tool call support | ✅ | ✅ |
| Long-term memory (Store) injection | ✅ | ✅ |
| Automatic Checkpoint saving | ✅ | ✅ |
| Ideal for | Independent batch tasks | Continuous conversations / Chatbots |

---

## Basic Usage

### Blocking multi-turn chat

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant");
    let mut agent = ReactAgent::new(config);

    // Turn 1
    let r1 = agent.chat("Hi, I'm Alice and I'm a Rust developer.").await?;
    println!("Agent: {r1}");

    // Turn 2 — Agent remembers "Alice" and "Rust developer"
    let r2 = agent.chat("Do you remember my name and occupation?").await?;
    println!("Agent: {r2}");

    // Turn 3 — personalized advice based on prior context
    let r3 = agent.chat("Given my background, what should I learn next?").await?;
    println!("Agent: {r3}");

    // Clear history, start a fresh session
    agent.reset();

    Ok(())
}
```

### Streaming multi-turn chat

```rust
use echo_agent::prelude::*;
use futures::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant");
    let mut agent = ReactAgent::new(config);

    let messages = [
        "I'm learning Rust async programming.",
        "Can you give me a simple async/await example?",
        "Based on what I asked, what should I study next?",
    ];

    for msg in &messages {
        println!("User: {msg}");
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

## Multi-Turn Chat with Tools

`chat()` fully supports tool calls. Across turns, the Agent can reference results from previous tool calls:

```rust
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new(
        "gpt-4o",
        "math_agent",
        "You are a calculator assistant. Use tools for all calculations and remember results.",
    )
    .enable_tool(true);

    let mut agent = ReactAgent::new(config);
    agent.add_tool(Box::new(AddTool));
    agent.add_tool(Box::new(MultiplyTool));

    // Turn 1: initial calculation
    let r1 = agent.chat("Calculate 15 + 27 and remember the result.").await?;
    println!("Turn 1 result: {r1}");

    // Turn 2: references the previous result (Agent knows from context that turn 1 = 42)
    let r2 = agent.chat("Multiply the previous result by 3.").await?;
    println!("Turn 2 result: {r2}");

    Ok(())
}
```

---

## Managing Conversation History

### Check context state

```rust
// Returns (message count, estimated token count); ReactAgent-specific method
let (msg_count, token_est) = agent.context_stats();
println!("Context: {msg_count} messages, ~{token_est} tokens");
```

### Reset conversation (start a new session)

`reset()` is an `Agent` trait method — available on all implementations, including via `dyn Agent`:

```rust
// Direct call on a concrete type
agent.reset();

// Via a trait object
let mut agent: Box<dyn Agent> = Box::new(ReactAgent::new(config));
agent.chat("Turn 1: Hi, I'm Alice").await?;
agent.reset();                                    // ← trait method, clears context
agent.chat("Turn 2: Who am I?").await?;           // Agent no longer knows "Alice"
```

### Cross-process session restoration with Checkpointer

The multi-turn history from `chat()` can be persisted with a Checkpointer and restored after a restart:

```rust
use echo_agent::prelude::*;
use std::sync::Arc;

let cp = FileCheckpointer::new("~/.echo-agent/checkpoints.json")?;
let mut agent = ReactAgent::new(config);
agent.set_checkpointer(Arc::new(cp), "user-alice-session".to_string());

// On first launch, the Checkpointer restores any existing history
agent.chat("Let's continue where we left off…").await?;
// Each chat() turn auto-saves a Checkpoint
```

---

## Combined with Context Compression

As turns accumulate, the context grows continuously. Configure auto-compression to prevent exceeding the model's token limit:

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("gpt-4o", "assistant", "You are a helpful assistant")
    .token_limit(8192); // compression triggers when this limit is approached

let mut agent = ReactAgent::new(config);

// Sliding window: keep only the most recent 20 messages
agent.set_compressor(SlidingWindowCompressor::new(20));

// chat() will auto-compress when tokens exceed the limit
agent.chat("First message").await?;
// ... more turns
```

---

## Agent Trait Design

The `Agent` trait defines the full conversation lifecycle:

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn system_prompt(&self) -> &str;

    /// Blocking execution; resets context on every call (single-task mode).
    /// For continuous dialogue use `chat()`.
    async fn execute(&mut self, task: &str) -> Result<String>;

    /// Streaming execution; resets context on every call (single-task mode).
    /// For continuous dialogue use `chat_stream()`.
    async fn execute_stream(&mut self, task: &str) -> Result<BoxStream<'_, Result<AgentEvent>>>;

    /// Multi-turn chat (blocking). Appends to existing context; history persists across turns.
    /// Call `reset()` to start a new session. Defaults to `execute()`.
    async fn chat(&mut self, message: &str) -> Result<String> {
        self.execute(message).await
    }

    /// Multi-turn chat (streaming). Appends to existing context; history persists across turns.
    /// Call `reset()` to start a new session. Defaults to `execute_stream()`.
    async fn chat_stream(&mut self, message: &str) -> Result<BoxStream<'_, Result<AgentEvent>>> {
        self.execute_stream(message).await
    }

    /// Clears conversation history to start a new session. Does not affect `execute()`.
    /// Default is a no-op; stateful implementations should override.
    fn reset(&mut self) {}
}
```

**Behavior summary by implementation:**

| Implementation | `chat()` | `reset()` |
|---|---|---|
| `ReactAgent` | Preserves full context | Clears history, keeps system prompt |
| `MockAgent` | Records call, consumes response queue | Clears call history |
| `FailingMockAgent` | Always returns error | Clears call history |
| Custom Agent | Falls back to `execute()` by default | No-op by default |

---

## Using in a Web Service (Chatbot API)

```rust
use axum::{Json, extract::State};
use echo_agent::prelude::*;
use std::sync::Arc;
use tokio::sync::Mutex;

// Shared Agent state (single-user session example)
type AgentState = Arc<Mutex<ReactAgent>>;

async fn chat_handler(
    State(agent): State<AgentState>,
    Json(req): Json<ChatRequest>,
) -> Json<ChatResponse> {
    let mut agent = agent.lock().await;
    let answer = agent.chat(&req.message).await.unwrap_or_default();
    Json(ChatResponse { answer })
}

// Streaming version (SSE)
async fn chat_stream_handler(
    State(agent): State<AgentState>,
    Json(req): Json<ChatRequest>,
) -> axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let event_stream = async_stream::stream! {
        let mut agent = agent.lock().await;
        if let Ok(mut stream) = agent.chat_stream(&req.message).await {
            while let Some(event) = stream.next().await {
                let data = match event {
                    Ok(AgentEvent::Token(t))       => format!("{{\"type\":\"token\",\"data\":\"{t}\"}}"),
                    Ok(AgentEvent::FinalAnswer(a))  => format!("{{\"type\":\"done\",\"data\":\"{a}\"}}"),
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

## Notes

1. **Each user session needs its own Agent instance**: `ReactAgent` is not `Sync` — use a separate instance per user, or wrap with `Arc<Mutex<ReactAgent>>`
2. **`reset()` only clears in-memory history**: Persisted Checkpointer data is unaffected; `execute()` will still restore it on the next call
3. **Context growth**: Long conversations accumulate tokens — use `set_compressor()` to prevent context overflow
4. **Mixing `execute()` and `chat()`**: `execute()` always resets history on each call, discarding any context accumulated by prior `chat()` calls

See: `examples/demo17_chat.rs`
