# Tracing System — Execution Traces and Observability

## Overview

The tracing system records every agent execution as a structured `Run` trace — capturing LLM calls, tool invocations, phase transitions, errors, and timing breakdowns. Traces are opt-in and feed into the eval and self-improvement pipelines.

```
Agent.execute("task")
  │
  ├── start_trace_run()     → Run { status: Running }
  ├── record_trace_event()  → LlmCall, ToolCall, ToolResult, PhaseTransition, ...
  ├── record_trace_event()  → ToolCall, ToolResult, FileEdit, ...
  └── finalize_trace_run()  → Run { status: Completed, final_output, timings }
                                   │
                                   ▼
                            RunStore (InMemory / Jsonl)
                                   │
                          ┌────────┼────────┐
                          ▼                 ▼
                    EvalRunner         Analyzer
                    (replay)           (self-improve)
```

---

## Core Types

### Run

The top-level execution record for a single agent invocation:

```rust
pub struct Run {
    pub run_id: String,                    // e.g. "run_<uuid>"
    pub parent_run_id: Option<String>,     // set for subagent runs
    pub session_id: String,                // session this run belongs to
    pub status: RunStatus,                 // Pending → Running → Completed/Failed/Cancelled
    pub input: String,                     // user input that triggered this run
    pub events: Vec<RunEvent>,             // chronological execution events
    pub final_output: Option<String>,      // final output text (set on Completed)
    pub error: Option<String>,             // error message (set on Failed)
    pub token_usage: TokenUsage,           // token breakdown
    pub timings: RunTimings,               // timing breakdown
    pub started_at: DateTime<Utc>,         // when the run started
    pub finished_at: Option<DateTime<Utc>>,// when the run finished
}
```

### RunStatus

```rust
pub enum RunStatus {
    Pending,    // created but not started
    Running,    // execution in progress
    Completed,  // finished successfully
    Failed,     // finished with error
    Cancelled,  // cancelled by user or system
}
```

### TokenUsage

```rust
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
```

### RunTimings

```rust
pub struct RunTimings {
    pub total_duration_ms: u64,   // wall-clock time
    pub llm_duration_ms: u64,     // time spent in LLM calls
    pub tool_duration_ms: u64,    // time spent in tool execution
}
```

### RunSummary

Lightweight summary for listing runs (without full event history):

```rust
pub struct RunSummary {
    pub run_id: String,
    pub session_id: String,
    pub status: RunStatus,
    pub input_preview: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub token_usage: TokenUsage,
    pub total_duration_ms: u64,
}
```

---

## RunEvent — 11 Event Types

`RunEvent` is a tagged union with 11 variants, each capturing a specific execution moment:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    LlmCall { messages, prompt_tokens, completion_tokens, duration_ms },
    ToolCall { call_id, name, args, risk, duration_ms },
    ToolResult { call_id, name, success, output_preview, output_truncated, duration_ms },
    ToolError { call_id, name, message },
    Error { message },
    Checkpoint { id },
    PermissionDecision { tool, decision, reason },
    FileEdit { tool, path },
    TestRun { command, passed, failure_count },
    PhaseTransition { phase, iteration },
    SubagentRun { agent_name, task, outcome },
}
```

### When Each Event Is Emitted

| Event | Phase | Description |
|-------|-------|-------------|
| `LlmCall` | Think | After each LLM API call — records token counts and latency |
| `ToolCall` | Act | Before tool execution — records name, args (secrets redacted), risk level |
| `ToolResult` | Act | After tool succeeds — records success flag, output preview (first 200 chars) |
| `ToolError` | Act | After tool fails — records error message |
| `Error` | Any | Run-level errors |
| `Checkpoint` | Any | When a checkpoint is saved |
| `PermissionDecision` | Act | After permission policy evaluation — "allow", "deny", or "ask" |
| `FileEdit` | Act | After a write tool edits a file |
| `TestRun` | Act | After a test command runs |
| `PhaseTransition` | Loop | At each ReAct phase: "recall", "think", "act", "finalize" |
| `SubagentRun` | Dispatch | When a subagent completes — "completed", "failed", "cancelled" |

### Secret Redaction

`RunEvent::new_tool_call()` automatically applies `redact_secrets()` to tool arguments before constructing the event. This ensures API keys, passwords, and tokens in tool arguments are never stored in traces.

---

## RunStore — Persistence Trait

```rust
#[async_trait]
pub trait RunStore: Send + Sync {
    async fn save(&self, run: Run) -> Result<()>;
    async fn load(&self, run_id: &str) -> Result<Option<Run>>;
    async fn list_by_session(&self, session_id: &str) -> Result<Vec<RunSummary>>;
    async fn list_all(&self, limit: usize) -> Result<Vec<RunSummary>>;

    // Default implementation: load → push event → save
    async fn append_event(&self, run_id: &str, event: RunEvent) -> Result<()>;
}
```

### Built-in Implementations

| Implementation | Storage | Use Case |
|---------------|---------|----------|
| `InMemoryRunStore` | `RwLock<HashMap>` | Testing, short-lived sessions |
| `JsonlRunStore` | Append-only `.jsonl` files | Production, persistent traces |

#### InMemoryRunStore

Backed by `RwLock<HashMap<String, Run>>`. Extra helpers: `len()`, `is_empty()`.

```rust
let store = InMemoryRunStore::new();
```

#### JsonlRunStore

File-based persistence. Each run stored as `{dir}/{run_id}.jsonl` (append-only; latest line is current state). Has an in-memory cache populated on construction.

```rust
let store = JsonlRunStore::new(PathBuf::from("./traces"))?;
```

---

## Agent Integration

The trace system is **opt-in**. Wire it through the builder:

```rust
use echo_agent::prelude::*;
use echo_agent::trace::JsonlRunStore;

let store = Arc::new(JsonlRunStore::new(PathBuf::from("./traces"))?);

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are helpful")
    .with_run_store(store.clone())  // opt-in tracing
    .build()?;
```

Or via `AgentRunner`:

```rust
let runner = AgentRunner::new(agent)
    .with_run_store(store);
```

### Run Lifecycle

```
1. start_trace_run(input)
   → Creates Run { status: Running, run_id: "run_<uuid>" }
   → Saves to store

2. record_trace_event(event)   (called multiple times)
   → Appends event to Run via store.append_event()
   → Fire-and-forget (errors silently discarded)

3. finalize_trace_run(status, output, error)
   → Sets status, final_output, finished_at
   → Saves final state to store
   → Clears current_run_id
```

### Where Events Are Emitted

| Source File | Events |
|------------|--------|
| `react_loop.rs` | `LlmCall`, `PhaseTransition`, finalize |
| `execution.rs` | `PermissionDecision`, `ToolCall`, `ToolError`, `ToolResult`, `FileEdit` |
| `pipeline.rs` | `ToolCall`, `ToolResult`, `ToolError` |
| `stream_channel.rs` | `ToolCall`, `ToolError`, `ToolResult` |
| `approval.rs` | `PermissionDecision` |

---

## Consumers

The tracing system feeds two downstream subsystems:

### Eval System

The eval runner uses traces for:
- **TrajectoryReplay**: Offline analysis of tool usage patterns, constraint violations
- **RegressionSuite**: Building regression test cases from past successful runs
- **Metrics**: Extracting token usage, timing, and tool call counts from traces

### Self-Improvement Pipeline

The improvement system uses traces for:
- **Analyzer**: Detecting failure patterns (write-without-read, excessive retries)
- **BackgroundReviewer**: Extracting memory and skill signals from conversation traces
- **TrajectorySaver**: Converting traces to ShareGPT format for model fine-tuning
- **ChangeLog**: Records memory/skill/rule mutations (self-evolution audit log)

```
┌──────────┐     ┌──────────────┐     ┌─────────────────┐
│ RunStore │────▶│ TrajectoryReplay  │────▶│ Eval Report     │
│ (traces) │     └──────────────┘     └─────────────────┘
│          │
│          │     ┌──────────────┐     ┌─────────────────┐
│          │────▶│ Analyzer     │────▶│ ImprovementLoop │
│          │     └──────────────┘     └─────────────────┘
│          │
│          │     ┌──────────────┐     ┌─────────────────┐
│          │────▶│TrajectorySaver│───▶│ ShareGPT JSONL  │
└──────────┘     └──────────────┘     └─────────────────┘
```

---

## JSON Output Format

Each trace event serializes to JSON with a `type` discriminator:

```json
{
  "run_id": "run_abc123",
  "status": "completed",
  "input": "Read src/main.rs",
  "events": [
    {
      "type": "phase_transition",
      "phase": "recall",
      "iteration": 0
    },
    {
      "type": "llm_call",
      "messages": 3,
      "prompt_tokens": 150,
      "completion_tokens": 45,
      "duration_ms": 320
    },
    {
      "type": "tool_call",
      "call_id": "call_1",
      "name": "read_file",
      "args": {"path": "src/main.rs"},
      "risk": null,
      "duration_ms": 5
    },
    {
      "type": "tool_result",
      "call_id": "call_1",
      "name": "read_file",
      "success": true,
      "output_preview": "fn main() { ...",
      "output_truncated": false,
      "duration_ms": 5
    },
    {
      "type": "phase_transition",
      "phase": "finalize",
      "iteration": 1
    }
  ],
  "token_usage": {
    "prompt_tokens": 150,
    "completion_tokens": 45,
    "total_tokens": 195
  },
  "timings": {
    "total_duration_ms": 850,
    "llm_duration_ms": 320,
    "tool_duration_ms": 5
  }
}
```

---

## Feature Gate

The tracing system has **no feature gate** — it is always compiled and always available. All types are re-exported through the `prelude` module unconditionally.

The downstream consumers (`eval`, `improve`) are behind their own feature flags, but the tracing infrastructure they depend on is always present.

```toml
[dependencies]
echo_agent = { version = "0.2" }  # tracing always included
echo_agent = { version = "0.2", features = ["eval"] }  # + eval replay
echo_agent = { version = "0.2", features = ["improve"] }  # + trajectory and lifecycle helpers
echo_agent = { version = "0.2", features = ["improve", "eval"] }  # + eval-driven analysis
```
