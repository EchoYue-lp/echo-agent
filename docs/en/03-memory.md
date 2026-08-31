# Memory System

## What It Is

echo-agent's memory system has three orthogonal layers, each solving a different "remembering" problem:

| Layer | Interface | Analogy | Problem Solved |
|-------|-----------|---------|----------------|
| **Runtime checkpoint** | `RuntimeStateStore` | Black box recorder | Resume an in-flight conversation across process restarts |
| **Transcript** | `ConversationStore` | Chat log | User-visible message history projection (drives GUI/TUI history panes) |
| **Long-term knowledge** | `Store` | Notebook | Persist user preferences, domain facts, task results across sessions |

Runtime checkpoint and transcript address the same conversation from different angles: the checkpoint contains the ReAct loop state (messages + current plan text + active skills + blocked reason) used to restart the loop; the transcript is the *user-visible* projection of just the message stream. Revisioned task relations and lifecycle state live in the canonical task runtime, not in this checkpoint. The Store is the orthogonal long-term knowledge backend.

---

## Runtime Checkpoint: RuntimeStateStore

`MemoryScope` is also a typed framework value. It accepts the documented
aliases through the standard `scope.parse()` API.

Capability and preference profiles are available from the stable
`echo_agent::profiles` facade (`AgentProfile`, `UserProfile`, and
`ProfileStore`).

### Problem It Solves

An LLM's context window vanishes after each request ends, and a process can crash mid-loop. Without a runtime checkpoint, a long task interrupted halfway requires starting over, and a user wanting to continue yesterday's conversation must repeat themselves.

`RuntimeStateStore` saves the full `AgentCheckpoint` (messages + current plan + active skills + blocked reason + timestamp) as the run progresses. The next time an Agent is launched with the same `conversation_id`, it automatically restores the previous runtime state — providing **thread continuity**.

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
    "current_plan":   "Step 3: draft the haiku",
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

`ConversationStore` is the user-visible projection of the message stream — one row per `StoredMessage`, populated automatically at `run_core_loop` finalization. It is what GUI/TUI history panes render.

- Keyed by `conversation_id` (same key as `RuntimeStateStore`).
- Independent of `RuntimeStateStore` — you can enable either, both, or neither.
- Built-in implementations: dependency-free `FileConversationStore`, or
  `SqliteConversationStore` when the `sqlite` feature is enabled.

```rust,no_run
use echo_agent::memory::FileConversationStore;
use echo_agent::prelude::*;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let conv_store = Arc::new(FileConversationStore::new("./agent-data")?);
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")
    .conversation_store(conv_store)
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
concurrency details. SQLite remains an optional, unchanged framework backend.

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

When `enable_memory=true`, the Agent automatically uses `[agent_name, "memories"]` as its namespace.

### How It Works

The Agent operates the Store through three built-in tools (no manual API calls needed):

```
LLM decides to remember something:
    └─► remember("Fibonacci first 10 terms: 1,1,2,3,5,8,13,21,34,55", importance=8)
            └─► store.put(["agent_name", "memories"], uuid, {
                    "content": "Fibonacci first 10 terms...",
                    "importance": 8,
                    "created_at": "2026-02-28T..."
                })

LLM needs to retrieve:
    └─► recall("fibonacci")
            └─► store.search(["agent_name", "memories"], "fibonacci", limit=5)
                    → keyword matching (exact match first, then relevance scoring)
                    → returns top 5 most relevant memories
```

### Usage

```rust,no_run
use echo_agent::prelude::*;

# async fn demo() -> echo_agent::error::Result<()> {
// Option 1: Via AgentConfig — auto-registers remember/recall/forget tools
let config = AgentConfig::new("qwen3-max", "my_agent", "You are an assistant")
    .enable_memory(true)
    .memory_path("./store.json");

let mut agent = ReactAgent::new(config);
// LLM can autonomously call remember / recall / forget

// Option 2: Direct Store API
let store = FileStore::new("./store.json")?;

// Write a memory
store.put(
    &["my_agent", "memories"],
    "fact-001",
    serde_json::json!({ "content": "User prefers dark theme", "importance": 7 })
).await?;

// Keyword search
let results = store.search(&["my_agent", "memories"], "theme", 5).await?;
for item in results {
    let content = item.value["content"].as_str().unwrap_or("");
    println!("[score={:.2}] {}", item.score.unwrap_or(0.0), content);
}

// Exact fetch
let item = store.get(&["my_agent", "memories"], "fact-001").await?;

// Delete
store.delete(&["my_agent", "memories"], "fact-001").await?;

// List all namespaces
let namespaces = store.list_namespaces(None).await?;
# Ok(())
# }
```

---

## Three Layers in Practice

```
Day 1:
  user:  "My name is Alice and I love jazz music"
  agent → remember("Alice loves jazz music")  ← stored in Store (persists forever)
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

Each Agent has an independent Store namespace and `conversation_id`:

```
Main Agent    conversation_id = "main-conv-001"     namespace = ["main_agent", "memories"]
Subagent A    conversation_id = "sub-a-conv-001"    namespace = ["sub_a", "memories"]
Subagent B    conversation_id = "sub-b-conv-001"    namespace = ["sub_b", "memories"]
```

- Subagent A cannot read Subagent B's memories (different namespace).
- Subagent A cannot see the main Agent's runtime state (different `conversation_id`).
- The main Agent holds the `Store` / `RuntimeStateStore` objects and can explicitly read any conversation or namespace (for auditing).

---

## conversation_id vs session_id

- `conversation_id`: durable conversation identity. Keys both `RuntimeStateStore` (full runtime state) and `ConversationStore` (transcript projection). This is the field you set to resume across process restarts.
- `session_id`: in-process logical run-grouping label. Not persisted; not used to drive restore.

---

## Typed and Layered Memory (Self-Evolution)

This page covers the three underlying Stores (long-term `Store`, runtime `RuntimeStateStore`, conversation `ConversationStore`).
For **structured memory with metadata** (type, confidence, source) and **hot/warm/cold tiered management, write triggers, review/GC, skill auto-creation** and other runtime evolution capabilities,
see [25 - Self-Evolution](./25-self-improvement.md) (the `evolution` module).
