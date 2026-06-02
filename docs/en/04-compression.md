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

let llm = Arc::new(DefaultLlmClient::new(Arc::new(Client::new()), "qwen-turbo"));

// Built-in summary prompt
SummaryCompressor::new(llm.clone(), 6)
//                                 ↑ keep latest 6 messages unsummarized

// Custom summary prompt
SummaryCompressor::with_prompt(
    llm.clone(),
    6,
    |messages| format!("Summarize the following {} messages in 3 sentences:", messages.len()),
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
    .stage(SummaryCompressor::new(llm, 8))           // stage 2: summarize
    .build();
```

---

## Integration with Agent

### Automatic Compression (recommended)

Set `AgentConfig::token_limit` and install a compressor — the framework automatically checks and compresses before every LLM call:

```rust
let config = AgentConfig::new("qwen3-max", "agent", "You are an assistant")
    .token_limit(4096); // compress when estimated tokens exceed 4096

let mut agent = ReactAgent::new(config);

// Install the compressor (none by default — must be set explicitly)
agent.set_compressor(SlidingWindowCompressor::new(20)).await;

// All subsequent execute() calls are protected by auto-compression
let answer = agent.execute("...").await?;
```

Or with the builder pattern (recommended):

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .name("agent")
    .system_prompt("You are an assistant")
    .token_limit(4096)
    .build()?;

agent.set_compressor(SlidingWindowCompressor::new(20)).await;
```

### Manual Compression

```rust
// Force-compress with a specific strategy (without replacing the installed compressor)
let compressor = SlidingWindowCompressor::new(10);
let stats = agent.force_compress_with(&compressor).await?;

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

---

## Custom Compression Strategies

`ContextCompressor` is the sole extension point. The framework provides two paths around it:

```text
What do you want to do?                          How
──────────────────────────────────────────────────────────────
Change summary prompt wording/language/focus     →  SummaryCompressor::with_prompt(llm, n, |msgs| ...)
Change compression logic (message filtering,     →  impl ContextCompressor
  fallback strategy, output structure, etc.)
Quickly generate a compressor from an async fn   →  #[compressor] proc macro
```

### Custom Summary Prompt

If you're happy with `SummaryCompressor`'s splitting/fallback/assembly logic and only want to change the prompt sent to the LLM, use `with_prompt`:

```rust
use echo_agent::compression::compressor::SummaryCompressor;

let compressor = SummaryCompressor::with_prompt(
    llm,
    6,
    |messages| format!("Summarize the following {} messages in English", messages.len()),
);
```

### Fully Custom Compression Logic

When `SummaryCompressor`'s behavior doesn't fit (e.g., message filtering, incremental summaries, different summary placement, custom fallback, token-budget-aware splitting), implement `ContextCompressor` directly:

```rust
use echo_agent::compression::{ContextCompressor, CompressionInput, CompressionOutput};
use echo_core::error::Result;
use echo_core::llm::types::Message;
use futures::future::BoxFuture;

/// Keep only user messages (example)
struct UserOnlyCompressor { keep: usize }

impl ContextCompressor for UserOnlyCompressor {
    fn compress(&self, input: CompressionInput) -> BoxFuture<'_, Result<CompressionOutput>> {
        Box::pin(async move {
            let (system, conv): (Vec<_>, Vec<_>) = input.messages
                .into_iter()
                .partition(|m| m.role == "system");
            let user_msgs: Vec<_> = conv.into_iter()
                .filter(|m| m.role == "user")
                .collect();
            let keep = self.keep.min(user_msgs.len());
            let evicted = user_msgs[..user_msgs.len() - keep].to_vec();
            let kept = user_msgs[user_msgs.len() - keep..].to_vec();
            let mut messages = system;
            messages.extend(kept);
            Ok(CompressionOutput { messages, evicted })
        })
    }
}
```

When implementing `ContextCompressor`, you can call `default_summary_prompt(messages)` to reuse the built-in Chinese summary template:

```rust
use echo_agent::compression::compressor::default_summary_prompt;

let prompt = default_summary_prompt(&messages);
// prompt is a complete summary instruction string, ready to send to the LLM
```

### `#[compressor]` Proc Macro

Generate a `ContextCompressor` implementation from an async fn — no manual struct needed:

```rust
use echo_agent::compression::{CompressionInput, CompressionOutput};
use echo_core::error::Result;
use echo_agent_macros::compressor;

#[compressor]
async fn tail_only(input: CompressionInput) -> Result<CompressionOutput> {
    let keep = 10.min(input.messages.len());
    let evicted = input.messages[..input.messages.len() - keep].to_vec();
    let messages = input.messages[input.messages.len() - keep..].to_vec();
    Ok(CompressionOutput { messages, evicted })
}
// Auto-generates: struct TailOnlyCompressor; impl ContextCompressor for TailOnlyCompressor { ... }
```

### Architecture Overview

```text
ContextCompressor (the sole compression strategy extension point)
 ├── SlidingWindowCompressor  (standalone, no dependencies)
 ├── SummaryCompressor        (uses Box<dyn Fn> internally for prompt generation)
 │     ├── new()              (uses default_summary_prompt)
 │     └── with_prompt()      (uses custom closure)
 └── HybridCompressor         (chains multiple ContextCompressors)
```

---

## Adaptive Compression (AdaptiveCompressor)

> **New in v0.2.1.** Automatically selects compression level based on context length.

`AdaptiveCompressor` implements a 5-level progressive compression strategy, automatically escalating compression intensity as context exceeds token thresholds:

| Level | Name | Strategy | Trigger Threshold |
|-------|------|----------|-------------------|
| L1 | **Snip** | Remove tool outputs exceeding token limit | `l1_snip_threshold_tokens` (80k) |
| L2 | **Micro** | Truncate tool outputs, keep first/last N lines | `l2_micro_threshold_tokens` (100k) |
| L3 | **Collapse** | Drop older messages, keep system prompt + last N recent | `l3_collapse_threshold_tokens` (120k) |
| L4 | **Auto Compact** | Full LLM summarization (requires external integration) | `l4_compact_threshold_tokens` (150k) |
| L5 | **Reactive** | Emergency: keep only system prompt + last 3 messages | Beyond L3 threshold |

**Note:** L4 requires an external LLM call. `AdaptiveCompressor` internally applies L1 → L2 → L3 → L5 in order, stopping once `target_tokens` is reached.

### Configuration

```rust
use echo_state::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};

let config = AdaptiveCompressionConfig {
    l1_snip_threshold_tokens: 80_000,
    l1_max_output_tokens: 4_000,        // Max tokens per output for Snip
    l2_micro_threshold_tokens: 100_000,
    l2_keep_lines: 50,                   // Lines to keep at head/tail for Micro
    l3_collapse_threshold_tokens: 120_000,
    l3_keep_recent: 10,                  // Recent messages to keep for Collapse
    l4_compact_threshold_tokens: 150_000,
    l4_keep_recent: 6,                   // Recent messages for Compact (external LLM)
};

let compressor = AdaptiveCompressor::new(config);
```

### How It Works

```
Tokens:    0 ──── 80k ──── 100k ──── 120k ──── 150k ──── ∞
            │       │        │         │         │        │
            │ None  │  Snip  │  Micro  │Collapse │Compact │
            │       │Trim long│Truncate │Drop old │Ext LLM │
            │       │outputs │outputs  │messages │summary │
```

### Integration with ContextManager

`AdaptiveCompressor` is integrated via `ContextManager::builder()`, not `AgentConfig`:

```rust
use echo_state::compression::ContextManager;
use echo_state::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};

let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default());
let mut ctx = ContextManager::builder(token_limit)
    .compressor(Box::new(compressor))
    .with_system("System prompt")
    .build();

// prepare() auto-detects token count and triggers compression
let result = ctx.prepare(None).await?;
// result.messages — compressed message list
// result.compressed — compression stats (if any)
```

See [demo53_adaptive_compression.rs](../examples/demo53_adaptive_compression.rs).
