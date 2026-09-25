# Memory System

## What It Is

echo-agent's memory system has three orthogonal layers, each solving a different "remembering" problem:

| Layer | Interface | Analogy | Problem Solved |
|-------|-----------|---------|----------------|
| **Runtime checkpoint** | `RuntimeStateStore` | Black box recorder | Resume an in-flight conversation across process restarts |
| **Transcript** | `ConversationStore` | Chat log | User-visible message history projection (drives GUI/TUI history panes) |
| **Long-term knowledge** | `Store` | Notebook | Persist user preferences, domain facts, task results across sessions |

Runtime checkpoint and transcript address the same conversation from different angles: the checkpoint contains the ReAct loop state (messages + active skills + blocked reason) used to restart the loop; the transcript is the *user-visible* projection of just the message stream. Revisioned task relations, plan artifacts, and lifecycle state live in the canonical task runtime, not in this checkpoint. The Store is the orthogonal long-term knowledge backend.

---

## Runtime Checkpoint: RuntimeStateStore

`MemoryScope` is also a typed framework value. It accepts the documented
aliases through the standard `scope.parse()` API.

Capability and preference profiles are available from the stable
`echo_agent::profiles` facade (`AgentProfile`, `UserProfile`, and
`ProfileStore`).

### Problem It Solves

An LLM's context window vanishes after each request ends, and a process can crash mid-loop. Without a runtime checkpoint, a long task interrupted halfway requires starting over, and a user wanting to continue yesterday's conversation must repeat themselves.

`RuntimeStateStore` saves the `AgentCheckpoint` runtime fields (messages + active skills + blocked reason + timestamp) as the run progresses. The next time an Agent is launched with the same `conversation_id`, it restores the previous runtime state — providing **thread continuity**. The public `current_plan` field remains readable in older checkpoints, but ReactAgent does not restore it as a task plan or write it into new checkpoints.

### How It Works

```
conversation_id: "user-123-chat-5"
                │
                ▼
FileRuntimeStateStore (./agent-data/runtime_state/_runtime_owners/):
<encoded-runtime-id>.json
{
  "runtime_state_id": "user-123-chat-5",
  "scope_id": "user-123-chat-5",
  "phase": "active",
  "checkpoint": {
    "messages_json":  "...full message history...",
    "current_plan":   null,
    "active_skills":  ["doc-writing"],
    "blocked_reason": null,
    "timestamp":      "2026-06-14T..."
  }
}
```

### Usage

```rust,no_run
use echo_agent::prelude::*;
use echo_agent::state::FileRuntimeStateStore;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let state_store = Arc::new(FileRuntimeStateStore::new("./agent-data")?);

let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")  // restore key
    .state_store(state_store)
    .build()?;
// First run: persists AgentCheckpoint after each turn finalization.
// Subsequent runs (same conversation_id): runtime restores the previous state.
let _ = agent.execute("Hello").await?;
# Ok(())
# }
```

See `echo-agent/src/state/mod.rs` for the trait and its file-backed and optional
SQLite implementations.

---

## Transcript: ConversationStore

`ConversationStore` is the user-visible projection of the message stream, one row per `StoredMessage`. The framework settles it at pre-compact, tool, guard, hook, and terminal safe points; GUI/TUI history panes render the committed result.

- Keyed by the stable product `conversation_id`. `RuntimeStateStore` keys each
  runtime generation separately and durably binds it back to that stable scope.
- Durable transcript projection is paired with `RuntimeStateStore`. You may
  enable checkpoint-only persistence or neither Store, but configuring
  `ConversationStore` without a revision-capable `RuntimeStateStore` is rejected
  before invocation side effects.
- Built-in implementations: dependency-free `FileConversationStore`, or
  `SqliteConversationStore` when the `sqlite` feature is enabled.
- `AgentConfig::persistence_settlement_timeout` and
  `ReactAgentBuilder::persistence_settlement_timeout` set the total budget for
  each managed safe point (default 10 seconds; zero is invalid). The same
  absolute deadline propagates to both Stores; a later recovery gets a new
  budget without changing its durable operation identity.

```rust,no_run
use echo_agent::memory::FileConversationStore;
use echo_agent::prelude::*;
use echo_agent::state::FileRuntimeStateStore;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let conv_store = Arc::new(FileConversationStore::new("./agent-data")?);
let state_store = Arc::new(FileRuntimeStateStore::new("./agent-data")?);
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")
    .conversation_store(conv_store)
    .state_store(state_store)
    .build()?;
# Ok(())
# }
```

### Async behavior of file backends

`FileRuntimeStateStore` and `FileConversationStore` keep their public APIs and
durability rules while running filesystem work outside Tokio runtime threads.
Operations for one `conversation_id` remain ordered. Independent conversations
may run concurrently within a process-wide bound. Once a file operation is
accepted, dropping its caller does not abort a partially completed durable
write; the owner finishes and later same-conversation operations observe it in
order. Exact UTF-8 IDs are encoded into collision-free ASCII filenames, so case
folding and Unicode normalization do not merge conversations. Corrupt UTF-8/JSON
and filesystem failures still return typed errors.

Both `new(...)` constructors remain synchronous bootstrap APIs: they create and
canonicalize directories, and `FileConversationStore::new` also acquires its
lease and reconciles existing manifests. Construct them before latency-sensitive
async work or from a blocking setup task. Only their async trait methods use the
process file-operation owner.

See [ADR 0004](../adr/0004-async-file-store-ownership.md) for ownership and
concurrency details. SQLite remains optional and implements the same atomic projection, deadline, and retirement contracts as the file backend.

Projection uses prepare/apply/ack rather than read-modify-replace. The runtime
checkpoint first records one complete pending batch; `ConversationStore` then
atomically returns `Applied` or `AlreadyApplied`; checkpoint CAS finally
advances the cursor and clears pending. Timeout after dispatch leaves durable
`Deferred` debt, which warm admission and cold recovery settle before model
execution. `TranscriptProjectionSettlement` is observed before the invocation
terminal. See [ADR 0056](../adr/0056-durable-transcript-projection-settlement.md).

---

## Long-term Memory: Store

### Problem It Solves

The runtime checkpoint preserves the message stream, but many pieces of information shouldn't be stored as raw conversation state — they need to persist in a structured way:
- User preferences ("prefers classical music")
- Domain knowledge ("project codename is OMEGA")
- Task results ("analysis: Fibonacci first 10 terms are...")

The Store provides `namespace + key → JSON value` KV storage with keyword search for accumulating and retrieving **cross-session knowledge**.

### Namespace Isolation

The Store uses a namespace (string array) for logical isolation of data:

```
store.json:
├── ["math_agent", "memories"]   ← math_agent's private memories
├── ["writer_agent", "memories"] ← writer_agent's private memories
└── ["shared", "facts"]          ← shared knowledge base
```

Same physical file, different namespaces — data is completely inaccessible across boundaries (unless the holder of the `Store` object explicitly queries a different namespace).

Agent memory uses the unified `["agent", "memories"]` namespace.

### How It Works

Without a layer manager, the Agent exposes only `recall` and `search_memory`.
Both filter the Store through the approved-memory rule; raw KV values and
Drafts remain available to direct Store callers but never enter an Agent tool
result. With a layer manager, `remember` creates a journaled Draft and
`forget` uses the manager's mutation authority:

```
LLM decides to remember something:
    └─► remember("Fibonacci first 10 terms: 1,1,2,3,5,8,13,21,34,55", importance=8)
            └─► manager.write_memory(["agent", "memories"], uuid, Draft)
                    → caller reviews and activates the exact proposal

LLM needs to retrieve:
    └─► recall("fibonacci")
            └─► MemoryRecaller searches ["agent", "memories"]
                    → returns only approved Active or Archived memories
```

`install_memory_layer_manager`, `set_memory_store`, and `install_memory_store`
return `Result`. A busy synchronous install fails without publishing half the
configuration; replacing a manager's Store requires replacing that manager.

### Usage

```rust,no_run
use echo_agent::prelude::*;

# async fn demo() -> echo_agent::error::Result<()> {
// Option 1: AgentConfig registers approved-memory recall/search tools
let config = AgentConfig::new("qwen3-max", "my_agent", "You are an assistant")
    .enable_memory(true)
    .memory_path("./store.json");

let mut agent = ReactAgent::new(config);
// Install MemoryLayerManager to enable journaled remember / forget.

// Option 2: Direct Store API
let store = FileStore::new("./store.json")?;

// Write a memory
store.put(
    &["my_agent", "raw_notes"],
    "fact-001",
    serde_json::json!({ "content": "User prefers dark theme", "importance": 7 })
).await?;

// Keyword search
let results = store.search(&["my_agent", "raw_notes"], "theme", 5).await?;
for item in results {
    let content = item.value["content"].as_str().unwrap_or("");
    println!("[score={:.2}] {}", item.score.unwrap_or(0.0), content);
}

// Exact fetch
let item = store.get(&["my_agent", "raw_notes"], "fact-001").await?;

// Delete
store.delete(&["my_agent", "raw_notes"], "fact-001").await?;

// List all namespaces
let namespaces = store.list_namespaces(None).await?;
# Ok(())
# }
```

### Reviewed Typed Memory

`MemoryLayerManager` owns the framework's evidence-bearing long-term memory.
Pre-compaction LLM extraction, evicted-message promotion, memory triggers,
layered `remember`, and optional Background Review persistence create
`Draft` candidates in the unified `["agent", "memories"]` namespace.
`MemoryMeta.provenance` records verbatim source excerpts with their user,
assistant, or tool roles. Source mechanism (`L3Promotion`, `AutoExtracted`,
and so on) does not prove speaker trust or approval. A claimed user preference
without exact user evidence cannot be activated or recalled; some automatic
extractors reject it before writing a Draft. Secrets and instruction-like
evidence are rejected before persistence.

The embedding host reviews a Draft through
`MemoryLayerManager::preview_activation(key)`, then supplies a
`MemoryApproval` to `activate_draft(proposal, approval)`. The proposal binds
content, metadata, and the operation-journal generation. A changed value,
provenance, or generation fails as stale, even after an A-to-B-to-A edit.
Cancelled or uncertain activation is reconciled by the same manager after
restart. Only approved Active or Archived typed memories enter automatic
context recall and Store-backed or layered `recall`/`search_memory`. Approved
Hot entries remain in turn context after promotion. Draft, Superseded, and
older records without provenance remain inspectable but are not injected.
See the executable [layered-memory example](../../echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs)
and [ADR 0070](../adr/0070-memory-provenance-and-recall-authority.md).

---

## Memory Across Conversations

```
Day 1:
  user:  "My name is Alice and I love jazz music"
  agent with layer manager → remember("Alice loves jazz music")  ← Draft in Store
  caller → preview_activation + activate_draft  ← reviewed and approved
  turn finalization → RuntimeStateStore saves AgentCheckpoint
                    → ConversationStore saves message rows

Day 2, same conversation_id:
  RuntimeStateStore restores: agent resumes the runtime loop with prior state
  user:  "Recommend a song"
  agent → recall("music preferences") → "Alice loves jazz music"
  → Recommends Miles Davis

Day 3, brand new conversation_id:
  RuntimeStateStore: no matching key → fresh runtime state
  user:  "Recommend a song"
  agent → recall("music preferences") → "Alice loves jazz music" (Store still exists!)
  → Still recommends jazz
```

---

## In-memory Implementations (for testing)

```rust,no_run
use echo_agent::prelude::*;

let store = InMemoryStore::new(); // data lost on process exit
// FileConversationStore works with a temporary directory and needs no feature.
// SQLite implementations remain available behind the `sqlite` feature.
```

---

## Context Isolation

The base Store API lets a consumer assign distinct namespaces to Agents;
`conversation_id` independently scopes runtime and transcript state:

```
Main Agent    conversation_id = "main-conv-001"     namespace = ["main_agent", "memories"]
Subagent A    conversation_id = "sub-a-conv-001"    namespace = ["sub_a", "memories"]
Subagent B    conversation_id = "sub-b-conv-001"    namespace = ["sub_b", "memories"]
```

- Subagent A cannot read Subagent B's memories in this consumer-defined layout (different namespace).
- Subagent A cannot see the main Agent's runtime state (different `conversation_id`).
- The main Agent holds the `Store` / `RuntimeStateStore` objects and can explicitly read any conversation or namespace (for auditing).

This example is not the `MemoryLayerManager` layout. Its warm tier always uses
`WARM_NAMESPACE = ["agent", "memories"]` within the supplied Store; distinct
Agents need separate Store instances or a consumer-owned isolation boundary
when their layered memories must not be shared.

---

## conversation_id vs session_id

- `conversation_id`: durable conversation identity. Keys both `RuntimeStateStore` (full runtime state) and `ConversationStore` (transcript projection). This is the field you set to resume across process restarts.
- `session_id`: in-process logical run-grouping label. Not persisted; not used to drive restore.

---

## Typed and Layered Memory (Self-Evolution)

This page covers the three underlying Stores (long-term `Store`, runtime `RuntimeStateStore`, conversation `ConversationStore`).
For **structured memory with metadata** (type, confidence, source) and **hot/warm tiered management with Archived entries in warm, write triggers, review/GC, skill auto-creation** and other runtime evolution capabilities,
see [25 - Self-Evolution](./25-self-improvement.md) (the `evolution` module).
