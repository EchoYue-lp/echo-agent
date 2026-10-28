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

The following is a simplified view of the main event payloads; identity metadata
and other lifecycle variants are omitted. `execute_stream()` returns `BoxStream<'_, Result<AgentEvent>>` containing:

```rust
pub enum AgentEvent {
    Token(String),                               // LLM token fragment (reasoning / final answer)
    ToolCall { invocation: ToolInvocation },     // LLM decided to call a tool
    ToolResult { name: String, result: ToolResult }, // tool finished, returning result
    FinalAnswer(String),                         // final answer generated; cleanup can follow
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

    let mut first_error = None;
    while let Some(event) = stream.next().await {
        match event {
            Ok(AgentEvent::Token(token)) => {
                print!("{}", token);
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
            Ok(AgentEvent::ToolCall { invocation, .. }) => {
                println!("\n[Tool call] {} {:?}", invocation.name, invocation.args);
            }
            Ok(AgentEvent::ToolResult { name, result, .. }) => {
                println!("[Tool result] {} -> {}", name, result.output);
            }
            Ok(AgentEvent::FinalAnswer(answer)) => {
                println!("\n[Final answer] {}", answer);
            }
            Err(error) => {
                // Report each observation, but retain the first error value.
                // Continue polling this same source through cleanup and EOF.
                eprintln!("[Stream error / observation] {error}");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Ok(event) => println!("[Lifecycle] {event:?}"),
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

新执行仅以 `AssistantMessage` 的明确 phase/身份/增量区分 reasoning、普通进度与正文，不再双发 Token/ThinkStart/ThinkEnd 副本。块 complete 只结束块；最终候选经 Guard/Critic/Stop、持久化、原 EOF 和 owner Join 后才发布，最后单次 `ExecutionStatus(Settled)` 才提供运行时完成事实。旧变体保留给历史读取与合理公开 custom Agent。

详细字段、恢复呈现资格与 TaskRun 边界见[原生消息合同](../zh/43-native-message-execution.md)。

## Blocking vs Streaming

```rust
// Blocking: wait for full response
let answer: String = agent.execute("Hello").await?;

// Streaming: receive events in real time
let mut stream = agent.execute_stream("Hello").await?;
while let Some(event) = stream.next().await {
    match event {
        Ok(event) => println!("{event:?}"),
        Err(error) => eprintln!("[Stream error / observation] {error}"),
    }
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
                            // Emitting an error does not end or replace the original producer.
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

## Notes

1. **Tool execution is not streamed**: `execute_tool()` is still blocking — a `ToolResult` event fires only after the tool completes, because tools themselves don't produce incremental output
2. **`FinalAnswer` is a business result**: Keep consuming the same source to physical EOF; producer cleanup can still fail after the answer. The default cancellation and invocation adapters preserve its first ordinary cleanup error while retaining the original source.
3. **Error handling**: Every event is `Result<AgentEvent>`. Preserve the first failure or cancellation cause while draining; a later cleanup failure must not replace it. `ExecutionNotSettled` retains its original run/execution identity and remains a separate nonterminal observation. EOF alone does not clear that observation.

The examples above render every stream error or observation and continue the same
source to EOF. Returning the first error preserves its original value; it does
not turn `ExecutionNotSettled` into a terminal failure or certify physical
settlement. Applications that persist execution status should consume the
canonical `AgentTurnDriver` receipt instead of inferring it from these UI events.

See: `echo-agent-learning/examples/demo10_streaming.rs`

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
