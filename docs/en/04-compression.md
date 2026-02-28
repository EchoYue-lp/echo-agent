# Context Compression

## What It Is

An LLM's context window is finite. As conversation history accumulates, sending everything verbatim will eventually exceed the token limit (causing request failures) or drive up cost and latency.

The context compression system automatically checks token usage before each LLM call and, when over the configured limit, compresses the message history according to the chosen strategy — while keeping the most valuable information intact.

---

## Problem It Solves

- **Long conversation support**: Handle dozens of turns without crashing due to context overflow
- **Cost control**: Fewer tokens = lower API bills
- **Speed optimization**: Shorter context = faster inference
- **Transparent automation**: Compression is invisible to Agent execution logic — no manual intervention needed

---

## Three Compression Strategies

### 1. SlidingWindowCompressor

**Principle**: Keep the most recent N messages and discard the oldest ones.

**Pros**: No LLM call required — instant, zero cost.

**Cons**: Early conversation content is completely lost with no summary.

```rust
use echo_agent::prelude::*;

SlidingWindowCompressor::new(20) // keep the 20 most recent messages
```

Best for: High-volume conversations where history is unimportant, or cost-sensitive workloads.

---

### 2. SummaryCompressor

**Principle**: Send older messages (beyond the retention window) to the LLM to generate a summary, then insert the summary as a new system message.

**Pros**: Historical information is preserved in condensed form.

**Cons**: Compression requires an additional LLM call (has cost).

```rust
use echo_agent::prelude::*;
use echo_agent::llm::DefaultLlmClient;
use reqwest::Client;
use std::sync::Arc;

let llm = Arc::new(DefaultLlmClient::new(Arc::new(Client::new()), "gpt-4o-mini"));

// Built-in summary prompt
SummaryCompressor::new(llm.clone(), DefaultSummaryPrompt, 6)
//                                                         ↑
//                                         keep latest 6 messages unsummarized

// Custom summary prompt
SummaryCompressor::new(
    llm.clone(),
    FnSummaryPrompt(|messages| {
        format!("Summarize the following {} messages in 3 sentences:", messages.len())
    }),
    6,
)
```

---

### 3. HybridCompressor

**Principle**: Chain multiple strategies into a pipeline where each stage's output feeds the next.

**Typical pattern**: Fast sliding-window trim first, then precision LLM summary on the remainder.

```rust
use echo_agent::prelude::*;

let compressor = HybridCompressor::builder()
    .stage(SlidingWindowCompressor::new(30))         // stage 1: keep last 30
    .stage(SummaryCompressor::new(llm, DefaultSummaryPrompt, 8)) // stage 2: summarize
    .build();
```

---

## Integration with Agent

### Automatic Compression (recommended)

Set `AgentConfig::token_limit` and install a compressor — the framework automatically checks and compresses before every LLM call:

```rust
let config = AgentConfig::new("gpt-4o", "agent", "You are an assistant")
    .token_limit(4096); // compress when estimated tokens exceed 4096

let mut agent = ReactAgent::new(config);

// Install the compressor (none by default — must be set explicitly)
agent.set_compressor(SlidingWindowCompressor::new(20));

// All subsequent execute() calls are protected by auto-compression
let answer = agent.execute("...").await?;
```

### Manual Compression

```rust
// Force-compress with a specific strategy (without replacing the installed compressor)
let stats = agent.force_compress_with(
    &SlidingWindowCompressor::new(10)
).await?;

println!(
    "Before: {} msgs / {} tokens → After: {} msgs / {} tokens (evicted {})",
    stats.before_count, stats.before_tokens,
    stats.after_count,  stats.after_tokens,
    stats.evicted
);
```

---

## Using ContextManager Directly

Use `ContextManager` independently without an Agent:

```rust
use echo_agent::prelude::*;
use echo_agent::llm::types::Message;

let mut ctx = ContextManager::builder(2000) // token limit 2000
    .compressor(SlidingWindowCompressor::new(10))
    .build();

ctx.push(Message::system("You are an assistant".to_string()));
for i in 0..30 {
    ctx.push(Message::user(format!("Question {}", i)));
    ctx.push(Message::assistant(format!("Answer {}", i)));
}

println!("Tokens before: {}", ctx.token_estimate());

// prepare() triggers auto-compression and returns the list to send to the LLM
let messages = ctx.prepare(None).await?;

println!("Messages after: {}", messages.len());
```

---

## When Compression Fires

```
ctx.prepare() is called:
    │
    ├─ Estimate current tokens (chars / 4, rough estimate)
    │
    ├─ estimate ≤ token_limit → return as-is, no compression
    │
    └─ estimate > token_limit → call compressor.compress()
           ├─ SlidingWindow: truncate in-memory (nanoseconds)
           └─ Summary: call LLM to summarize (seconds, has cost)
```

---

## Recommendations

| Scenario | Recommended Strategy |
|----------|---------------------|
| Chatbot (history unimportant) | `SlidingWindowCompressor(20~50)` |
| Task-execution Agent (history matters) | `SummaryCompressor` or `Hybrid` |
| High-frequency, cost-sensitive | `SlidingWindowCompressor` |
| Long document analysis | `HybridCompressor` (slide then summarize) |
| Test environment | `SlidingWindowCompressor(5)` + `token_limit: 100` |

See: `examples/demo05_compressor.rs`
