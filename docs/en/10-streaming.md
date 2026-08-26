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
    let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant")
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

### Tracked initial input

For a cold turn, call `TurnRequest::with_input_receipt()` before passing the
request to `AgentTurnDriver`. The driver publishes `Accepted` after validating
the request and immediately before invoking the Agent. `ReactAgent` publishes
`Drained` only after the initial message has entered `ContextManager`, before
the provider call. The same driver publishes the typed terminal outcome. Agents
that do not expose an input lifecycle publisher leave the terminal receipt at
`drained = false`; output events and EOF are never used as a substitute.

---

## Stream Timeout Mechanism (Planned)

> **Note:** The API below is planned but not yet implemented in the current version. Stream timeouts are currently controlled via the LLM client's request timeout.

The SSE client plans to include three-level timeout protection:

| Timeout Type | Planned Value | Purpose |
|-------------|---------|---------|
| **first_chunk** | 30s | Maximum time to wait for the first chunk |
| **idle** | 60s | Maximum idle time between chunks |
| **overall** | 300s | Total stream timeout |

Currently, LLM request timeouts can be controlled via `AgentConfig`'s `request_timeout`.

### Stream Loop Architecture

v0.2.1 splits the monolithic `stream_loop.rs` into a modular subdirectory:

```
src/agent/react/run/stream_loop/
├── mod.rs           # Entry point and main orchestration
├── llm_stream.rs    # LLM streaming request and SSE parsing
└── processor.rs     # Event processing and dispatch
```

### Tool Output Spill and Truncation

All tool results pass through one output-budget stage. Outputs of at least 1 MiB are
written to a managed spill file before token truncation, and the model receives a
preview plus the exact path for `read_file`. Spill is active even when
`max_tool_output_tokens` is not configured.

When a working directory is configured, spill files live under
`<working_dir>/.echo-agent/spill` so the confined file tools can read them. Without a
working directory, echo-agent falls back to the system temporary directory. Files older
than one hour are removed on a best-effort basis when a new spill is created.

Smaller output that exceeds `max_tool_output_tokens` uses a UTF-8-safe head+tail view
with a 70/30 character allocation:

```
Full output:    [────────────────────────────────────────] 10KB
After truncate: [────── head (70%) ──────][── tail (30%) ──]
                                      ↑ middle omitted
```

If spill creation fails, echo-agent applies a conservative fallback token budget instead
of returning the full oversized result to the model. `ToolResult.truncated` and metadata
record whether the result was spilled, truncated, or truncated after a spill failure.
