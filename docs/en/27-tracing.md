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

## RunEvent — 17 Event Types

`RunEvent` is a tagged union with 17 variants and a snake-case `type`
discriminator. The enum in `src/trace/mod.rs` is the serialization contract;
the matrix below is the authoritative producer map. A variant is not evidence
merely because a caller can construct it in a fixture.

### Authoritative Producer Matrix

| Variant | Authoritative producer | Settlement rule |
|---------|------------------------|------------------|
| `BudgetDecision` | `src/agent/react/run/stream_channel.rs` | Records an emitted wind-down or final-only budget decision; it is not emitted for every run. |
| `LlmCall` | `src/agent/react/run/phases/think.rs` | Appended after the provider response, including the usage and timing facts known for that call. |
| `ContextCompression` | `src/agent/react/capabilities.rs`, `src/agent/react/run/phases/compact.rs` | Appended after manual or automatic compression completes. |
| `ToolCall` | `src/agent/react/run/pipeline.rs`; synthetic unstarted calls in `src/agent/snapshot.rs` | Normal calls are recorded at `ExecuteStage` entry. A synthetic call closes an invocation that never entered that stage. |
| `ToolExecutionSkipped` | `src/agent/snapshot.rs` | Paired with a synthetic terminal for an invocation that never entered `ExecuteStage`; it is not a tool failure inference. |
| `ToolResult` | `src/agent/react/run/pipeline.rs`, `src/agent/snapshot.rs` | Records the actual result, or the one synthetic failed result for an unstarted invocation. |
| `ToolError` | `src/agent/react/run/pipeline.rs`, `src/agent/snapshot.rs` | Records a failed result or an interrupted synthetic terminal, preserving the typed failure when known. |
| `Error` | `src/trace/mod.rs::apply_run_finalization` | The run-store finalizer adds it only when a failed run has no existing run-level error event. |
| `Checkpoint` | `src/agent/snapshot.rs` | Appended after the runtime checkpoint compare-and-save settles. |
| `CheckpointResumed` | `src/agent/react/mod.rs` (called by `src/agent/react/run/stream_channel.rs`) | Records hydration of a persisted runtime checkpoint before execution continues. |
| `TranscriptProjectionSettlement` | `src/agent/snapshot.rs` | Records the typed conversation projection settlement or reconciliation result. |
| `PermissionDecision` | `src/agent/react/run/pipeline.rs` (`PermissionStage` and hook paths) | Records each observed hook, protected-path, or permission decision; it is not a final authorization receipt. |
| `FileRead` | `src/agent/snapshot.rs::record_tool_effect` | Projects a confirmed `ToolEffect::FileRead` with the resolved path. Failed or guessed reads emit nothing. |
| `FileEdit` | `src/agent/snapshot.rs::record_tool_effect` | Projects a confirmed `ToolEffect::FileEdit` after mutation. Dry runs, proposals, and generic success emit nothing. |
| `TestRun` | `src/agent/snapshot.rs::record_tool_effect`; `src/eval/runner.rs::record_test_run` | Requires a completed test-command effect or evaluator criterion. `failure_count` stays `None` when no structured count exists. |
| `PhaseTransition` | `src/agent/react/run/react_loop.rs` | Records the ReAct phase transition emitted by the run loop. |
| `SubagentRun` | `src/agent/snapshot.rs::record_tool_effect`, fed by `src/tools/builtin/agent_dispatch.rs` | Records the settled Subagent outcome, including failure and cancellation; a launch acknowledgement is not terminal. |

The generic shell tool does not infer `FileEdit` or `TestRun` from a command
name, path, output text, or exit code. Only a tool-owned, confirmed
`ToolEffect` or the evaluator's explicit completed-command boundary can create
those facts. This keeps trace projections from becoming a second side-effect
authority.

Background dispatch has the same boundary: the launch acknowledgement carries
no `SubagentRun`; an invocation-scoped effect sink records exactly one terminal
outcome later. Durable admission, generation fencing, shutdown cancellation and
evidence settlement for a detached background owner remain the embedding
application's responsibility (the residual framework/consumer boundary tracked
by Issues #38 and #61), as described in [ADR 0059](../adr/0059-observed-tool-effects-and-background-dispatch.md).

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

    // Default implementation: load → apply first terminal → save
    async fn finalize_run(
        &self,
        run_id: &str,
        status: RunStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<bool>;
}
```

Backends that allow concurrent append and finalization override both mutation
methods under one authority. `finalize_run` returns `false` when the run does
not exist; the built-in stores retain late events and preserve the first
terminal result.

### Retention contract for custom backends

The React producer applies the default `ContentRetentionPolicy` before calling
custom `save` and `append_event` methods; the default `finalize_run`
implementation sanitizes terminal output and error fields. A backend that
overrides `save`, `append_event`, or `finalize_run` must apply the same policy
(or a stricter one) before accepting a durable write. `Run::apply_retention`
and `RunEvent::apply_retention` are the reusable boundary helpers, while
`ContentRetentionPolicy::sanitize_text` covers standalone finalization strings.

Retention targets user/model/tool content such as prompts, outputs, errors,
tool arguments, and human-readable reasons. Typed addressing and effect facts
such as `run_id`, session/turn/execution IDs, call IDs, tool names, paths,
status values, counters, and timestamps remain unchanged so a diagnostic can
be queried and replayed. They are not secret-content storage; callers should
never place credentials in an identity they expect to remain addressable.

Custom stores and audit sinks must return an error for partial writes or
unknown durability. The producer keeps the accepted state observable and
reports the error through diagnostic delivery; a backend error must not rewrite
the Agent execution terminal.
See [ADR 0074](../adr/0074-trace-audit-retention-contract.md) for the field
classification and custom-backend ownership decision.

### Built-in Implementations

| Implementation | Storage | Use Case |
|---------------|---------|----------|
| `InMemoryRunStore` | `RwLock<HashMap>` | Testing, short-lived sessions |
| `JsonlRunStore` | Snapshot plus event-line `.jsonl` files | Production, persistent traces |

#### InMemoryRunStore

Backed by `RwLock<HashMap<String, Run>>`. Extra helpers: `len()`, `is_empty()`.

```rust
let store = InMemoryRunStore::new();
```

#### JsonlRunStore

File-based persistence. Each run is stored as `{dir}/{run_id}.jsonl`: the first
line is a compacted `Run` snapshot and later lines are individual `RunEvent`
values. Event appends add a line; save and finalization atomically compact the
file back to one current snapshot. Construction scans existing files into an
in-memory cache.

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

### Run Lifecycle

```
1. start_trace_run(input)
   → Creates Run { status: Running, run_id: "run_<uuid>" }
   → Saves to store; a rejected save publishes no trace run ID

2. record_trace_event(event)   (called multiple times)
   → Appends event to Run via store.append_event()
   → Reports rejected delivery without changing Agent execution

3. finalize_trace_run(status, output, error)
   → Sets status, final_output, finished_at
   → Saves final state to store
   → Does not alter the product/business current_run_id
```

### Diagnostic Delivery Failures

Trace and Audit persistence are observations of an Agent execution, not a
second execution terminal. Their direct Store/Logger methods return `Result`
and callers that require persistence must handle that result. The Agent
integration continues the producer execution when an optional diagnostic write
fails and sends a structured `DiagnosticDeliveryFailure` to its observer.

The Agent producer submits each failure with a bounded, non-blocking `try_send`.
A process-local diagnostic dispatcher emits the tracing target
`echo_agent::diagnostic_delivery` with stable `record_kind`, `operation`,
`record_id_present`, `occurred_at`, and `error` fields. Applications can also install a
structured observer; it receives the same fact from that dispatcher:

```rust
use echo_agent::audit::{DiagnosticDeliveryFailure, DiagnosticDeliveryObserver};
use echo_agent::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct DiagnosticCounter(AtomicUsize);

impl DiagnosticDeliveryObserver for DiagnosticCounter {
    fn on_failure(&self, _failure: DiagnosticDeliveryFailure) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

let diagnostic_failures = Arc::new(DiagnosticCounter::default());
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are helpful")
    .diagnostic_delivery_observer(diagnostic_failures.clone())
    .build()?;
# Ok::<(), echo_agent::error::ReactError>(())
```

The observer has no control return and never runs on the Agent producer.
Queue saturation, disconnect, initialization failure, reentrant reporting, and
observer unwind increment `diagnostic_delivery_dropped_count()`. A blocking
observer can delay later diagnostic notifications, but not Completed, Failed,
or Cancelled producer settlement. Process abort and termination remain outside
in-process recovery. Skill-usage metrics remain separate best-effort telemetry
and are not promoted to diagnostic delivery authority.
See [ADR 0053](../adr/0053-trace-audit-persistence-visibility.md) for the
failure-policy decision and industry references.

### Producer Source Index

The [authoritative producer matrix](#authoritative-producer-matrix) is the
single source for event ownership. The main producer modules are
`src/agent/react/run/react_loop.rs`,
`src/agent/react/run/phases/think.rs`,
`src/agent/react/run/phases/compact.rs`,
`src/agent/react/run/pipeline.rs`,
`src/agent/react/run/stream_channel.rs`,
`src/agent/snapshot.rs`, `src/eval/runner.rs`, and
`src/trace/mod.rs`. The previous generic source index has been retired; use the
variant-level matrix above when adding or reviewing a producer.

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
