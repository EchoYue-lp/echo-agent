# Memory System

## What It Is

echo-agent's memory system has two independent layers, each solving a different granularity of "remembering":

| Layer | Interface | Analogy | Problem Solved |
|-------|-----------|---------|----------------|
| **Short-term** | `Checkpointer` | Voice recorder | Resume interrupted conversations across sessions |
| **Long-term** | `Store` | Notebook | Retain domain knowledge and user preferences across sessions |

This design directly mirrors LangGraph's two-tier architecture: `Checkpointer` (short-term) and `Store` (long-term).

---

## Short-term Memory: Checkpointer

### Problem It Solves

An LLM's context window vanishes after each request ends. Without a Checkpointer, a long task interrupted halfway requires starting over, and a user wanting to continue yesterday's conversation must repeat themselves.

The Checkpointer automatically saves the full message history to disk at the end of each conversation turn. The next time an Agent is launched with the same `session_id`, it automatically restores the previous context — providing **conversation continuity**.

### How It Works

```
session_id: "user-123-chat-5"
                │
                ▼
checkpoints.json:
{
  "user-123-chat-5": {
    "session_id": "user-123-chat-5",
    "messages": [
      { "role": "system",    "content": "You are an assistant" },
      { "role": "user",      "content": "Write me a poem" },
      { "role": "assistant", "content": "..." },
      { "role": "user",      "content": "Make it a haiku" }
    ]
  }
}
```

### Usage

```rust
use echo_agent::prelude::*;

// Option 1: Auto-managed via AgentConfig (recommended)
let config = AgentConfig::new("gpt-4o", "assistant", "You are an assistant")
    .session_id("user-alice-session-1")       // specify session ID
    .checkpointer_path("./checkpoints.json"); // persistence file path

let mut agent = ReactAgent::new(config);
// First run: saves session history to file
// Subsequent runs (same session_id): automatically restores previous conversation
let _ = agent.execute("Hello").await?;

// Option 2: Direct Checkpointer API (for auditing, cross-agent reads, etc.)
let cp = FileCheckpointer::new("./checkpoints.json")?;

if let Some(checkpoint) = cp.get("user-alice-session-1").await? {
    println!("Message count: {}", checkpoint.messages.len());
}

let sessions = cp.list_sessions().await?;
println!("All sessions: {:?}", sessions);

cp.delete_session("user-alice-session-1").await?;
```

---

## Long-term Memory: Store

### Problem It Solves

The Checkpointer saves the "conversation process" (message stream), but many pieces of information shouldn't be stored as a conversation — they need to persist in a structured way:
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

```rust
use echo_agent::prelude::*;

// Option 1: Via AgentConfig — auto-registers remember/recall/forget tools
let config = AgentConfig::new("gpt-4o", "my_agent", "You are an assistant")
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
```

---

## Two-layer Memory in Practice

```
Day 1:
  user:  "My name is Alice and I love jazz music"
  agent → remember("Alice loves jazz music")  ← stored in Store (persists forever)
  session ends → Checkpointer saves conversation history

Day 2, same session resumed:
  Checkpointer restores: agent knows what was said on Day 1
  user:  "Recommend a song"
  agent → recall("music preferences") → "Alice loves jazz music"
  → Recommends Miles Davis

Day 3, brand new session:
  Checkpointer: no matching session_id → empty message history
  user:  "Recommend a song"
  agent → recall("music preferences") → "Alice loves jazz music" (Store still exists!)
  → Still recommends jazz
```

---

## In-memory Implementations (for testing)

```rust
use echo_agent::prelude::*;

let cp    = InMemoryCheckpointer::new(); // data lost on process exit
let store = InMemoryStore::new();
```

---

## Context Isolation

Each Agent has an independent Store namespace and Checkpointer session_id:

```
Main Agent    session_id = "main-001"     namespace = ["main_agent", "memories"]
SubAgent A    session_id = "sub-a-001"    namespace = ["sub_a", "memories"]
SubAgent B    session_id = "sub-b-001"    namespace = ["sub_b", "memories"]
```

- SubAgent A cannot read SubAgent B's memories (different namespace)
- SubAgent A cannot see the main Agent's conversation history (different session_id)
- The main Agent holds the `Store` and `Checkpointer` objects and can explicitly read any session or namespace (for auditing)

See: `examples/demo14_memory_isolation.rs`
