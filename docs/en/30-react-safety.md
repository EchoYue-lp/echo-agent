# ReAct Safety Mechanisms — Loop Detection, Adaptive Compression & Git Checkpoints

## What They Are

When an Agent autonomously executes multi-step tasks, three risks arise:

1. **Infinite loops**: The Agent repeatedly calls the same tool with the same arguments, burning tokens without completing the task
2. **Context overflow**: Conversation history exceeds the token limit, causing LLM request failures
3. **File corruption**: The Agent writes or edits files incorrectly with no way to roll back

echo-agent provides three safety layers to address these risks:

| Mechanism | Module | Purpose |
|-----------|--------|---------|
| **Loop Detection** | `LoopDetector` | Detect duplicate calls, consecutive failures, no-progress loops |
| **Adaptive Compression** | `AdaptiveCompressor` | 5-level progressive compression to prevent context overflow |
| **Git Checkpoint** | `git_checkpoint` | Auto-tag before file mutations, support rollback |

---

## 1. Loop Detection

### Problem It Solves

LLM-driven Agents can get stuck in several loop patterns:

- **Exact repetition**: Calling the same tool with identical arguments over and over (e.g., `read_file` on the same path)
- **Consecutive failures**: The same tool keeps failing, but the Agent retries the same ineffective approach
- **No progress**: The Agent runs many iterations without any tangible output (no file writes, no task updates)

`LoopDetector` automatically identifies these patterns and intervenes.

### Three Detection Strategies

#### Strategy 1: Exact Duplicate Detection

Tracks every `(tool_name, args_json)` combination. When the same combination is called more than `exact_threshold` times, the Agent is **force-stopped**.

```
Agent calls read_file({"path": "a.rs"}) → 1st time ✓
Agent calls read_file({"path": "a.rs"}) → 2nd time ✓
Agent calls read_file({"path": "a.rs"}) → 3rd time → Break: "Loop detected"
```

#### Strategy 2: Same-Tool Consecutive Failure

Tracks consecutive failure count per tool. When the same tool fails `failure_threshold` times in a row, a **warning message is injected** to guide the Agent toward a different approach (execution is not force-stopped, since args may differ).

```
shell("bad_cmd_1") → fail → streak=1
shell("bad_cmd_2") → fail → streak=2
shell("bad_cmd_3") → fail → streak=3 → Warn: "failed 3 times, consider a different approach"
```

The failure counter resets automatically when the tool succeeds.

#### Strategy 3: No-Progress Detection

Tracks iterations since the last "progress" event. The following tools count as progress when they succeed:

- `edit_file`, `write_file`, `create_file`, `delete_file`
- `create_task`, `update_task`
- `git_commit`
- `shell`

When `no_progress_threshold` consecutive iterations pass without any progress, a warning is injected.

### LoopDetectorConfig

```rust
use echo_agent::agent::react::loop_detector::LoopDetectorConfig;

let config = LoopDetectorConfig {
    exact_threshold: 3,         // max identical calls before Break (default: 3)
    failure_threshold: 3,       // max consecutive failures before Warn (default: 3)
    no_progress_threshold: 8,   // max no-progress iterations before Warn (default: 8)
};
```

| Field | Default | Description |
|-------|---------|-------------|
| `exact_threshold` | 3 | Number of identical `(tool, args)` calls before triggering Break |
| `failure_threshold` | 3 | Number of consecutive same-tool failures before triggering Warn |
| `no_progress_threshold` | 8 | Number of iterations without progress before triggering Warn |

### Configuration via AgentConfig

```rust
use echo_agent::prelude::*;
use echo_agent::agent::react::loop_detector::LoopDetectorConfig;

let config = AgentConfig::new("qwen3-max", "agent", "You are a helpful assistant")
    .enable_tool(true)
    .loop_detector(LoopDetectorConfig {
        exact_threshold: 5,          // allow up to 5 exact duplicates
        failure_threshold: 4,        // allow up to 4 consecutive failures
        no_progress_threshold: 12,   // allow up to 12 no-progress iterations
    });

let agent = ReactAgent::new(config);
```

### LoopVerdict Outcomes

`LoopDetector::check()` returns one of three verdicts:

```rust
pub enum LoopVerdict {
    /// All clear, continue execution
    Continue,
    /// Inject a warning message into Agent context (does not terminate)
    Warn(String),
    /// Force-stop the Agent loop
    Break(String),
}
```

**Priority order**: Exact Duplicate (Break) > Same-Tool Failure (Warn) > No-Progress (Warn)

### Code Example

```rust
use echo_agent::agent::react::loop_detector::{LoopDetector, LoopDetectorConfig};

let mut detector = LoopDetector::new(LoopDetectorConfig::default());

// Record each tool call
detector.record_tool_call("read_file", r#"{"path":"a.rs"}"#, true);
detector.record_tool_call("read_file", r#"{"path":"b.rs"}"#, true);
detector.record_iteration();

// Check for loops
match detector.check() {
    LoopVerdict::Continue => println!("All clear"),
    LoopVerdict::Warn(msg) => println!("Warning: {}", msg),
    LoopVerdict::Break(msg) => println!("Stopping: {}", msg),
}

// Reset all tracking state when starting a new task
detector.reset();
```

---

## 2. Adaptive Compression

### Problem It Solves

Unlike the `SlidingWindowCompressor` / `SummaryCompressor` described in `04-compression.md`, the `AdaptiveCompressor` (in the `echo-state` crate) provides **progressive multi-level compression**: from lightweight output trimming to aggressive emergency compression, escalating only when cheaper strategies are insufficient.

### Compression Levels

| Level | Name | Trigger Threshold (default) | Strategy | Requires LLM |
|-------|------|----------------------------|----------|-------------|
| L1 Snip | **Snip** | 80,000 tokens | Trim oversized tool outputs (truncate anything above `l1_max_output_tokens`) | No |
| L1 Fold | **Fold** | (runs after Snip) | Collapse consecutive tool results, keep latest N | No |
| L2 | **Micro** | 100,000 tokens | Keep first/last N lines of tool outputs, remove the middle | No |
| L3 | **Collapse** | 120,000 tokens | Remove older messages, keep only recent N + system messages | No |
| L4 | **Compact** | 150,000 tokens | Full LLM summarization (via `.with_llm()`) | Yes (optional) |
| L5 | **Reactive** | Emergency | Keep only system prompt + last 3 messages | No |

### Compression Flow

```
compress(messages, current_tokens, target_tokens):
    │
    ├─ tokens > L1 threshold AND > target? → Trim large tool outputs (Snip)
    │                                      → Fold consecutive tool results (Fold)
    │
    ├─ tokens > L2 threshold AND > target? → Keep head/tail lines (Micro)
    │
    ├─ tokens > L3 threshold AND > target? → Remove old messages (Collapse)
    │
    ├─ tokens > L4 threshold AND > target AND LLM configured? → LLM summary (Compact)
    │
    └─ tokens > L4 threshold AND > 2×target? → Emergency mode (Reactive)

Note: L4 requires an LLM client configured via .with_llm(). Without it,
      AdaptiveCompressor skips L4 and falls through to L5.
      AdaptiveCompressor also implements ContextCompressor and integrates
      directly with ContextManager::builder().compressor().
```

### AdaptiveCompressionConfig

```rust
use echo_state::compression::levels::AdaptiveCompressionConfig;

let config = AdaptiveCompressionConfig {
    l1_snip_threshold_tokens: 80_000,      // L1 Snip trigger threshold
    l1_max_output_tokens: 4_000,           // max tokens per tool output
    l1_fold_consecutive_tools: true,       // L1 Fold: collapse consecutive tool results
    l1_fold_keep_latest: 2,               // L1 Fold: keep latest N per run
    l2_micro_threshold_tokens: 100_000,    // L2 trigger threshold
    l2_keep_lines: 50,                     // lines to keep from start/end
    l3_collapse_threshold_tokens: 120_000, // L3 trigger threshold
    l3_keep_recent: 10,                    // recent messages to keep
    l4_compact_threshold_tokens: 150_000,  // L4/L5 trigger threshold
    l4_keep_recent: 6,                     // messages to keep during compact
};
```

### Code Example

```rust
use echo_state::compression::levels::{AdaptiveCompressor, AdaptiveCompressionConfig};
use echo_core::llm::types::{Message, MessageContent, Role};

// Without LLM (L4 skipped):
let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default());

// With LLM (L4 enabled):
// let compressor = AdaptiveCompressor::new(AdaptiveCompressionConfig::default()).with_llm(llm);

let mut messages: Vec<Message> = vec![
    Message::system("You are a helpful assistant"),
    Message::user("Please analyze this report..."),
    Message::assistant("Sure, let me..."),
    // ... more messages
];

let result = compressor.compress_in_place(
    &mut messages,
    130_000, // current_tokens: estimated token count
    80_000,  // target_tokens: desired token count
);

println!("Before: {} tokens", result.tokens_before);
println!("After: {} tokens", result.tokens_after);
println!("Levels applied: {:?}", result.levels_applied);
// Output: ["L1:Snip", "L1:Fold", "L2:Micro", "L3:Collapse"]
```

### Integration with Context Management

At the Agent level, compression is triggered automatically based on `compress_threshold_ratio`:

```rust
let config = AgentConfig::new("qwen3-max", "agent", "You are a helpful assistant")
    .token_limit(100_000)              // context token limit
    .compress_threshold_ratio(0.2);    // trigger compression when < 20% headroom remains

let agent = ReactAgent::new(config);
```

When the available token ratio falls below `compress_threshold_ratio`, the Agent automatically triggers compression before calling `llm.chat()`.

### Level Details

**L1 Snip** — Truncates Tool messages whose output exceeds `l1_max_output_tokens`. Keeps the first N tokens' worth of characters (using char-boundary-safe slicing to avoid UTF-8 panics) and appends an `[output truncated]` notice.

**L1 Fold** — Collapses consecutive tool result messages in a run. Keeps the latest `l1_fold_keep_latest` messages and replaces older ones with a `[L1 fold: N consecutive tool results collapsed]` user message. Runs after Snip when `l1_fold_consecutive_tools` is true.

**L2 Micro** — Truncates Tool messages by line: keeps the first `l2_keep_lines` and last `l2_keep_lines` lines, replacing the middle with `[N lines truncated]`.

**L3 Collapse** — Preserves all System messages and the most recent `l3_keep_recent` messages. Removes everything in between and inserts a `[Context compressed: N older messages removed]` System message.

**L4 Compact** — Full LLM summarization of older messages. Only active when an LLM client is configured via `AdaptiveCompressor::with_llm(llm)`. On LLM failure, gracefully falls through to L5.

**L5 Reactive** — Emergency mode. Keeps only System messages and the last 3 messages. Inserts an `[Emergency compression: context was critically large]` notice. Only triggered when tokens exceed both `l4_compact_threshold_tokens` and 2x the target.

---

## 3. Git Checkpoint

### Problem It Solves

When an Agent performs file writes, edits, or deletions, the results may be unexpected (e.g., deleting important code, writing incorrect content). If the project is under Git version control, echo-agent can automatically create lightweight tags before each file mutation, providing a rollback safety net.

### How It Works

```
File mutation (create_file / write_file / edit_file / delete_file)
    │
    ├─ 1. Check if the target file is inside a Git repository
    │     └─ Not a Git repo → skip checkpoint (no impact on non-Git projects)
    │
    ├─ 2. Get current HEAD commit hash
    │
    ├─ 3. Create lightweight tag: echo-checkpoint/{timestamp}
    │
    └─ 4. Perform the file mutation
```

### Core API

```rust
use echo_tools::git_checkpoint::{
    create_checkpoint,
    rollback_to_checkpoint,
    cleanup_old_checkpoints,
};
use std::path::Path;

// Create a checkpoint (call before file mutation)
let tag = create_checkpoint(Path::new("src/main.rs"));
// Returns: Some("echo-checkpoint/1717200000") or None (not a Git repo)

// Rollback to a checkpoint
let success = rollback_to_checkpoint(
    Path::new("src/main.rs"),
    "echo-checkpoint/1717200000",
);

// Clean up old checkpoints (keep the most recent N tags)
cleanup_old_checkpoints(Path::new("src/main.rs"), 10);
```

### Key Features

- **Auto-detects Git root**: Walks up from the target file to find `.git`, automatically locating the repository root
- **Lightweight tags**: Uses `git tag` (no annotated tags), zero commit overhead
- **Safe rollback**: Uses `git checkout <tag> -- .` to restore working-tree files (does not alter Git history or branch pointers)
- **Auto-cleanup**: `cleanup_old_checkpoints` sorts by creation date (descending) and deletes tags beyond the retention count

### Important Notes

- Only activates for files inside a Git repository; non-Git projects are unaffected
- Tag format is `echo-checkpoint/{unix_timestamp}` — will not collide with user-defined tags
- Rollback restores working-tree files only — it does not modify branches or HEAD
- Call `cleanup_old_checkpoints` at the end of long-running tasks to prevent tag accumulation

---

## 4. Combined Configuration Example

```rust
use echo_agent::prelude::*;
use echo_agent::agent::react::loop_detector::LoopDetectorConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::new(
        "qwen3-max",
        "safe_agent",
        "You are a safe coding assistant.",
    )
    // Basic config
    .enable_tool(true)
    .max_iterations(30)
    .token_limit(100_000)
    // Loop detection — custom thresholds
    .loop_detector(LoopDetectorConfig {
        exact_threshold: 4,
        failure_threshold: 3,
        no_progress_threshold: 10,
    })
    // Compression — trigger when < 25% headroom remains
    .compress_threshold_ratio(0.25)
    // File safety — require read before edit
    .force_read_before_edit(true);

    let mut agent = ReactAgent::new(config);

    let answer = agent
        .execute("Refactor the parse_date function in src/utils.rs")
        .await?;
    println!("{}", answer);
    Ok(())
}
```

---

## 5. Configuration Reference

### Loop Detection (LoopDetectorConfig)

| Parameter | Type | Default | Set Via | Description |
|-----------|------|---------|---------|-------------|
| `exact_threshold` | `usize` | 3 | `AgentConfig::loop_detector()` | Exact duplicate detection threshold |
| `failure_threshold` | `usize` | 3 | `AgentConfig::loop_detector()` | Consecutive failure detection threshold |
| `no_progress_threshold` | `usize` | 8 | `AgentConfig::loop_detector()` | No-progress iteration threshold |

### Adaptive Compression (AdaptiveCompressionConfig)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `l1_snip_threshold_tokens` | `usize` | 80,000 | L1 Snip trigger threshold |
| `l1_max_output_tokens` | `usize` | 4,000 | Max tokens per tool output |
| `l1_fold_consecutive_tools` | `bool` | true | L1 Fold: collapse consecutive tool results |
| `l1_fold_keep_latest` | `usize` | 2 | L1 Fold: keep latest N tool results per run |
| `l2_micro_threshold_tokens` | `usize` | 100,000 | L2 Micro trigger threshold |
| `l2_keep_lines` | `usize` | 50 | Lines to keep from start/end |
| `l3_collapse_threshold_tokens` | `usize` | 120,000 | L3 Collapse trigger threshold |
| `l3_keep_recent` | `usize` | 10 | Recent messages to keep during collapse |
| `l4_compact_threshold_tokens` | `usize` | 150,000 | L4/L5 trigger threshold |
| `l4_keep_recent` | `usize` | 6 | Recent messages to keep during compact |

### Agent-Level Safety Settings

| Parameter | Type | Default | Set Via | Description |
|-----------|------|---------|---------|-------------|
| `token_limit` | `usize` | `usize::MAX` | `AgentConfig::token_limit()` | Context token limit |
| `compress_threshold_ratio` | `f64` | 0.2 | `AgentConfig::compress_threshold_ratio()` | Trigger compression when headroom drops below this ratio |
| `max_iterations` | `usize` | 10 | `AgentConfig::max_iterations()` | Max iterations (prevents infinite loops) |
| `force_read_before_edit` | `bool` | false | `AgentConfig::force_read_before_edit()` | Require `read_file` before any edit/write/delete |
| `max_tool_output_tokens` | `Option<usize>` | None | `AgentConfig::max_tool_output_tokens()` | Per-tool output token limit; auto-truncated when exceeded |

### Git Checkpoint API

| Function | Parameters | Returns | Description |
|----------|------------|---------|-------------|
| `create_checkpoint` | `file_path: &Path` | `Option<String>` | Create a tag, return the tag name |
| `rollback_to_checkpoint` | `file_path: &Path, tag: &str` | `bool` | Restore files to the tagged state |
| `cleanup_old_checkpoints` | `file_path: &Path, keep: usize` | `()` | Keep the most recent N tags, delete the rest |

---

## Related Documentation

- [ReAct Agent](01-react-agent.md) — Core execution engine and iteration loop
- [Context Compression](04-compression.md) — SlidingWindow / Summary / Hybrid compression strategies
- [Tool System](02-tools.md) — Tool registration, execution, and permissions
- [Configuration Reference](28-config-reference.md) — Complete configuration parameter reference
