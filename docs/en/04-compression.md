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

## Compression Strategies

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
use echo_agent::llm::OpenAiClient;
use std::sync::Arc;

let llm = Arc::new(OpenAiClient::from_env("qwen3.6-plus")?);

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

### 3. IncrementalSummaryCompressor

> **New in v0.2.2.** Incremental LLM summarization that reuses previous summaries.

Unlike `SummaryCompressor` which re-summarizes ALL old messages every time, `IncrementalSummaryCompressor` maintains the previous summary and only sends `[previous summary] + [new messages]` to the LLM on subsequent compressions. This dramatically reduces LLM cost and latency for long conversations.

```rust
use echo_agent::compression::compressor::IncrementalSummaryCompressor;
use std::sync::Arc;

let compressor = IncrementalSummaryCompressor::new(llm, 6);
// First compression: summarizes all old messages (like SummaryCompressor)
// Second compression: sends previous summary + new messages only
// Third compression: same pattern, even cheaper

// Inspect or reset the stored summary:
println!("Current summary: {:?}", compressor.current_summary());
compressor.reset();
```

**Pros**: Much cheaper for long conversations with repeated compression.

**Cons**: Requires mutable internal state (wrapped in `Mutex`); slightly more complex.

---

### 4. HybridCompressor

**Principle**: Chain multiple strategies into a pipeline where each stage's output feeds the next.

**Typical pattern**: Fast sliding-window trim first, then precision LLM summary on the remainder.

```rust
use echo_agent::prelude::*;

let compressor = HybridCompressor::builder()
    .stage(SlidingWindowCompressor::new(30))         // stage 1: keep last 30
    .stage(SummaryCompressor::new(llm, 8))           // stage 2: summarize
    .build();
```

**Short-circuit optimization** (new in v0.2.2): When enabled (default), the pipeline skips remaining stages once the estimated token count drops to or below `token_limit`. This avoids unnecessary LLM calls in later stages.

```rust
// Disable short-circuit (always run all stages)
let compressor = HybridCompressor::builder()
    .stage(SlidingWindowCompressor::new(30))
    .stage(SummaryCompressor::new(llm, 8))
    .short_circuit(false)
    .build();
```

---

### 5. AdaptiveCompressor

> **New in v0.2.1, enhanced in v0.2.2.** Automatically selects compression level based on context length.

`AdaptiveCompressor` implements a multi-level progressive compression strategy, automatically escalating compression intensity as context exceeds token thresholds:

| Level | Name | Strategy | Trigger Threshold | LLM? |
|-------|------|----------|-------------------|------|
| L1 | **Snip** | Remove tool outputs exceeding token limit | `l1_snip_threshold_tokens` (80k) | No |
| L1 | **Fold** | Collapse consecutive tool results, keep latest N | Runs after Snip | No |
| L2 | **Micro** | Truncate tool outputs, keep first/last N lines | `l2_micro_threshold_tokens` (100k) | No |
| L3 | **Collapse** | Drop older messages, keep system prompt + last N recent | `l3_collapse_threshold_tokens` (120k) | No |
| L4 | **Auto Compact** | Full LLM summarization | `l4_compact_threshold_tokens` (150k) | Yes (optional) |
| L5 | **Reactive** | Emergency: keep only system prompt + last 3 messages | Beyond L4 threshold + 2×target | No |

**v0.2.2 changes:**
- L4 is now **built-in** via `.with_llm()` — no external integration needed
- L1 now includes **tool folding** (`l1_fold_consecutive_tools`) to collapse long runs of tool messages
- `AdaptiveCompressor` now implements `ContextCompressor` — can be used via `ContextManager::builder().compressor()` directly

### Configuration

```rust
use echo_agent::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};

let config = AdaptiveCompressionConfig {
    l1_snip_threshold_tokens: 80_000,
    l1_max_output_tokens: 4_000,        // Max tokens per output for Snip
    l1_fold_consecutive_tools: true,     // Fold consecutive tool results (new)
    l1_fold_keep_latest: 2,             // Keep latest N tool results per run (new)
    l2_micro_threshold_tokens: 100_000,
    l2_keep_lines: 50,                   // Lines to keep at head/tail for Micro
    l3_collapse_threshold_tokens: 120_000,
    l3_keep_recent: 10,                  // Recent messages to keep for Collapse
    l4_compact_threshold_tokens: 150_000,
    l4_keep_recent: 6,                   // Recent messages for Compact (LLM)
};

// Without LLM: L4 is skipped, falls through to L5
let compressor = AdaptiveCompressor::new(config.clone());

// With LLM: L4 auto-compact is enabled
let compressor = AdaptiveCompressor::new(config).with_llm(llm);
```

### How It Works

```
Tokens:    0 ──── 80k ──── 100k ──── 120k ──── 150k ──── ∞
            │       │        │         │         │        │
            │ None  │  Snip  │  Micro  │Collapse │Compact │
            │       │+Fold   │Truncate │Drop old │LLM sum │
            │       │Trim    │outputs  │messages │        │
            │       │long    │         │         │        │
            │       │outputs │         │         │        │
```

### Integration with ContextManager

`AdaptiveCompressor` implements `ContextCompressor` and integrates via `ContextManager::builder()` (new in v0.2.2):

```rust
use echo_agent::compression::ContextManager;
use echo_agent::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};

let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default())
    .with_llm(llm); // optional: enable L4

let mut ctx = ContextManager::builder(token_limit)
    .compressor(compressor) // works directly now (was Box::new() before)
    .with_system("System prompt".to_string())
    .build();

// prepare() auto-detects token count and triggers compression
let result = ctx.prepare(None).await?;
// result.messages — compressed message list
// result.compressed — compression stats (if any)
```

### Low-level API (direct use without ContextManager)

For advanced use cases, `compress_in_place()` mutates messages directly:

```rust
let mut messages = vec![/* ... */];
let current_tokens = 50_000;
let target_tokens = 30_000;
let result = compressor.compress_in_place(&mut messages, current_tokens, target_tokens);
println!("Levels applied: {:?}", result.levels_applied);
```

See [demo53_adaptive_compression.rs](../examples/demo53_adaptive_compression.rs).

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
let result = ctx.prepare(None).await?;

println!("Messages after: {}", result.messages.len());
```

---

## Compression Metrics

> **New in v0.2.2.** Cumulative observability for compression events.

`ContextManager` tracks compression statistics across its lifetime:

```rust
let metrics = ctx.compression_metrics();

println!("Total compressions: {}", metrics.total_compressions);
println!("Tokens saved: {}", metrics.total_tokens_saved());
println!("Compression ratio: {:.1}%", metrics.compression_ratio() * 100.0);
println!("Strategies used: {:?}", metrics.strategies_used);

// Human-readable report:
println!("{}", metrics.report());
// → "CompressionMetrics: 5 compressions, 12340 tokens saved (35.2%), 48 messages evicted, strategies: [SlidingWindow(3), Adaptive(2)]"

// Reset metrics:
ctx.reset_compression_metrics();
```

Metrics are automatically recorded in:
- `prepare()` (auto-compression)
- `force_compress()`
- `force_compress_with()`

Each compression event also emits `tracing` log events at `info` level with fields: `compressor`, `before_messages`, `after_messages`, `before_tokens`, `after_tokens`, `evicted`, `saved_tokens`, `elapsed_ms`.

---

## Token Estimation

### Built-in Tokenizers

| Type | Algorithm | Accuracy |
|------|----------|----------|
| `HeuristicTokenizer` | ASCII weight 1, CJK weight 2, total / 4 | Medium (recommended for mixed CJK/English) |
| `SimpleTokenizer` | `byte_count / 4 + 1` | Low (backward compatible) |

### CalibratedTokenizer (new in v0.2.2)

`CalibratedTokenizer` wraps any base tokenizer and improves accuracy over time by learning from actual API response data:

```rust
use echo_agent::tokenizer::{CalibratedTokenizer, HeuristicTokenizer, Tokenizer};
use std::sync::Arc;

let base = Arc::new(HeuristicTokenizer);
let calibrated = CalibratedTokenizer::new(base);

// Use like any other tokenizer
let tokens = calibrated.count_tokens("some text");

// After the LLM API returns actual token counts, feed them back:
calibrated.calibrate(tokens, api_usage.prompt_tokens);

// The calibration factor converges via exponential moving average (EMA)
println!("Factor: {:.3}", calibrated.calibration_factor());
println!("Samples: {}", calibrated.sample_count());

// Use with ContextManager:
let ctx = ContextManager::builder(4096)
    .tokenizer(Arc::new(calibrated))
    .build();
```

---

## When Compression Fires

```
ctx.prepare() is called:
    │
    ├─ Estimate current tokens (via configured Tokenizer)
    │
    ├─ estimate ≤ token_limit → return as-is, no compression
    │
    └─ estimate > token_limit → call compressor.compress()
           ├─ SlidingWindow: truncate in-memory (nanoseconds)
           ├─ Summary/IncrementalSummary: call LLM to summarize (seconds, has cost)
           ├─ Hybrid: run pipeline stages (short-circuits when below limit)
           └─ Adaptive: escalate L1→L2→L3→L4→L5 as needed
    │
    └─ Record metrics + emit tracing event
```

---

## Recommendations

| Scenario | Recommended Strategy |
|----------|---------------------|
| Chatbot (history unimportant) | `SlidingWindowCompressor(20~50)` |
| Task-execution Agent (history matters) | `SummaryCompressor` or `Hybrid` |
| High-frequency, cost-sensitive | `SlidingWindowCompressor` |
| Long conversations with repeated compression | `IncrementalSummaryCompressor` |
| Long document analysis | `HybridCompressor` (slide then summarize) |
| Tool-heavy workflows | `AdaptiveCompressor` (auto-escalation with L1 tool folding) |
| Test environment | `SlidingWindowCompressor(5)` + `token_limit: 100` |

See: `examples/demo05_compressor.rs`, `examples/demo53_adaptive_compression.rs`

---

## Custom Compression Strategies

`ContextCompressor` is the sole extension point. The framework provides two paths around it:

```text
What do you want to do?                          How
──────────────────────────────────────────────────────────────
Change summary prompt wording/language/focus     →  SummaryCompressor::with_prompt(llm, n, |msgs| ...)
Reduce LLM cost for repeated compressions        →  IncrementalSummaryCompressor
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
use echo_agent::error::Result;
use echo_agent::llm::types::Message;
use futures::future::BoxFuture;

/// Keep only user messages (example)
struct UserOnlyCompressor { keep: usize }

impl ContextCompressor for UserOnlyCompressor {
    fn name(&self) -> &'static str { "UserOnly" } // optional: for metrics tracking

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
use echo_agent::error::Result;
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
 ├── SlidingWindowCompressor       (standalone, no dependencies)
 ├── SummaryCompressor             (LLM summarization with fallback)
 │     ├── new()                   (uses default_summary_prompt)
 │     └── with_prompt()           (uses custom closure)
 ├── IncrementalSummaryCompressor  (incremental LLM summarization, reuses previous)
 ├── HybridCompressor              (pipeline with short-circuit optimization)
 └── AdaptiveCompressor            (5-level auto-escalation, optional LLM for L4)
       ├── L1: Snip + Fold         (truncate/fold tool outputs)
       ├── L2: Micro               (truncate to first/last N lines)
       ├── L3: Collapse            (drop old messages, keep recent)
       ├── L4: Compact             (LLM summarization via .with_llm())
       └── L5: Reactive            (emergency: system prompt + last 3)
```
