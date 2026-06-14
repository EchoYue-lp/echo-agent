# Memory System

## What It Is

echo-agent's memory system has three orthogonal layers, each solving a different "remembering" problem:

| Layer | Interface | Analogy | Problem Solved |
|-------|-----------|---------|----------------|
| **Runtime checkpoint** | `RuntimeStateStore` | Black box recorder | Resume an in-flight conversation across process restarts |
| **Transcript** | `ConversationStore` | Chat log | User-visible message history projection (drives GUI/TUI history panes) |
| **Long-term knowledge** | `Store` | Notebook | Persist user preferences, domain facts, task results across sessions |

Runtime checkpoint and transcript address the same conversation from different angles: the checkpoint is the *complete* runtime state (messages + plan + active skills + blocked reason + TaskNode DAG) used to restart the loop; the transcript is the *user-visible* projection of just the message stream. The Store is the orthogonal long-term knowledge backend.

---

## Runtime Checkpoint: RuntimeStateStore

### Problem It Solves

An LLM's context window vanishes after each request ends, and a process can crash mid-loop. Without a runtime checkpoint, a long task interrupted halfway requires starting over, and a user wanting to continue yesterday's conversation must repeat themselves.

`RuntimeStateStore` saves the full `AgentCheckpoint` (messages + current plan + active skills + blocked reason + timestamp) as the run progresses. The next time an Agent is launched with the same `conversation_id`, it automatically restores the previous runtime state — providing **thread continuity**.

### How It Works

```
conversation_id: "user-123-chat-5"
                │
                ▼
SqliteRuntimeStateStore (~/.echo-agent/state.db):
{
  "user-123-chat-5": {
    "messages_json":  "...full message history...",
    "current_plan":   "Step 3: draft the haiku",
    "active_skills":  ["doc-writing"],
    "blocked_reason": null,
    "timestamp":      "2026-06-14T...",
  }
}
```

### Usage

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let state_store = Arc::new(SqliteRuntimeStateStore::open("./state.db").await?);

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

See `echo-agent/src/state/mod.rs` for the trait and `SqliteRuntimeStateStore` implementation.

---

## Transcript: ConversationStore

`ConversationStore` is the user-visible projection of the message stream — one row per `StoredMessage`, populated automatically at `run_core_loop` finalization. It is what GUI/TUI history panes render.

- Keyed by `conversation_id` (same key as `RuntimeStateStore`).
- Independent of `RuntimeStateStore` — you can enable either, both, or neither.
- Concrete implementation: `SqliteConversationStore` (`echo-agent/echo-state/src/memory/sqlite_conversation.rs`).

```rust,no_run
use echo_agent::prelude::*;
use std::sync::Arc;

# async fn demo() -> echo_agent::error::Result<()> {
let conv_store = Arc::new(SqliteConversationStore::open("./conversations.db").await?);
let agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .conversation_id("user-alice-conv-001")
    .conversation_store(conv_store)
    .build()?;
# Ok(())
# }
```

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
// For RuntimeStateStore / ConversationStore in tests, use the SQLite implementations
// against a temp file (see `tempfile::NamedTempFile`) or a `:memory:` SQLite URI.
```

---

## Context Isolation

Each Agent has an independent Store namespace and `conversation_id`:

```
Main Agent    conversation_id = "main-conv-001"     namespace = ["main_agent", "memories"]
SubAgent A    conversation_id = "sub-a-conv-001"    namespace = ["sub_a", "memories"]
SubAgent B    conversation_id = "sub-b-conv-001"    namespace = ["sub_b", "memories"]
```

- SubAgent A cannot read SubAgent B's memories (different namespace).
- SubAgent A cannot see the main Agent's runtime state (different `conversation_id`).
- The main Agent holds the `Store` / `RuntimeStateStore` objects and can explicitly read any conversation or namespace (for auditing).

---

## conversation_id vs session_id

- `conversation_id`: durable conversation identity. Keys both `RuntimeStateStore` (full runtime state) and `ConversationStore` (transcript projection). This is the field you set to resume across process restarts.
- `session_id`: in-process logical run-grouping label. Not persisted; not used to drive restore.

See: `examples/demo14_memory_isolation.rs`

---

## Tiered Memory System (TieredMemory)

> **New in v0.2.1.** Automatically manages memory storage, retrieval, and eviction.

`TieredMemory` implements a four-layer memory architecture, automatically managing memory entries across different tiers:

### Four-Layer Architecture

| Layer | Name | Characteristics | Storage |
|-------|------|-----------------|---------|
| **Working** | Working layer | Active messages in current turn | Context window |
| **ShortTerm** | Short-term layer | Recent structured entries | In-memory (`Vec<MemoryEntry>`) |
| **LongTerm** | Long-term layer | Archived memories, searchable | Persistent (`Store`) |
| **Core** | Core layer | Permanent memories, injected into system prompt | In-memory (`CoreMemory`) |

### Configuration

```rust
use echo_core::memory::tiered::TieredMemory;

let memory = TieredMemory::new(
    5,     // max_short_term: maximum entries in short-term layer
    2000,  // max_core_chars: character limit for core layer
)
.with_overflow_bound(50)   // overflow queue limit
.with_store(store);        // optional: attach persistent store (LongTerm layer)
```

### Automatic Eviction

When `short_term` exceeds `max_short_term`, the entry with the **lowest importance** is moved to `overflow_queue` (eviction by importance, not by time). When the overflow queue is full:
- With `Store`: wait for `flush_overflow()` to write to long-term
- Without `Store`: evict the lowest-importance entries

### Usage Example

```rust
use echo_core::memory::tiered::TieredMemory;
use echo_core::memory::MemoryEntry;

let mut memory = TieredMemory::new(3, 2000).with_overflow_bound(10);

// Add memory (simple way)
memory.add_short_term_simple("User prefers clean code style".into());
memory.add_short_term_simple("Project uses Rust + tokio".into());

// Add structured memory entry
memory.add_short_term(MemoryEntry::new(
    "User is building an Agent framework".into(),
    7.5,                          // importance (1.0-10.0)
    vec!["project".into()],       // tags
    "conversation".into(),        // source
));

// Recall relevant memories (keyword matching)
let results = memory.recall("Rust project", 3);
println!("Related memories: {:?}", results);

// Build context injection (Core + ShortTerm, sorted by importance)
if let Some(ctx) = memory.build_context_injection() {
    println!("Context injection: {}", ctx);
}
```

### Agent Integration

`TieredMemory` is currently decoupled from `AgentConfig` — construct it independently and integrate with the Agent system:

```rust
use echo_core::memory::tiered::TieredMemory;

let mut memory = TieredMemory::new(5, 2000)
    .with_store(store);
// Integrate with Agent via memory subsystem or custom logic
```

See [demo63_tiered_memory.rs](../examples/demo63_tiered_memory.rs).
