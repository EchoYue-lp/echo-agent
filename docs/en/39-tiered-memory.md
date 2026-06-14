# Tiered Memory

> **Status: Implemented.**
> The `tiered` module provides a four-layer memory architecture inspired by Letta's memory hierarchy. Memories flow down the hierarchy automatically via summarization and periodic reflection, with importance-weighted eviction at every layer.

---

## What It Is

`TieredMemory` is a multi-layer memory manager that automatically organizes memories by lifespan, access frequency, and importance. Unlike a flat `Vec<String>` of summaries, each entry carries structured metadata (importance, timestamps, tags) — enabling relevance-based recall and importance-weighted context injection.

The four layers, from fastest to most persistent:

| Layer | Name | Storage | Lifespan | Purpose |
|-------|------|---------|----------|---------|
| **Core** | Core memory | System prompt | Permanent | Identity, preferences, goals — always visible to the agent |
| **ShortTerm** | Short-term memory | In-memory `Vec` | Minutes | Recent structured entries with metadata |
| **Overflow** | Overflow queue | In-memory `Vec` | Minutes–hours | Evicted short-term entries awaiting async flush |
| **LongTerm** | Long-term store | `Store` (DB/file) | Days–months | Searchable, persistent episodic memories |

```
User conversation
       │
       ▼
┌──────────────┐     evict      ┌──────────────┐    flush     ┌──────────────┐
│  ShortTerm   │ ─────────────► │  Overflow    │ ───────────► │  LongTerm    │
│  (max N)     │  by importance │  (max M)     │  async       │  (Store)     │
└──────────────┘                └──────────────┘              └──────────────┘
       │                                                          │
       ▼                                                          ▼
┌──────────────┐                                          ┌──────────────┐
│  Context     │                                          │  Decay &     │
│  Injection   │                                          │  Pruning     │
│  (prompt)    │                                          │  (periodic)  │
└──────────────┘                                          └──────────────┘
       ▲
       │
┌──────────────┐
│  Core        │  ← always injected (system prompt fragment)
│  Memory      │
└──────────────┘
```

---

## MemoryEntry

Unlike bare `String` summaries, every short-term entry is a `MemoryEntry` with rich metadata:

```rust
pub struct MemoryEntry {
    pub content: String,       // The memory content
    pub importance: f64,       // 1.0–10.0; higher = kept longer, injected first
    pub timestamp: DateTime<Utc>, // When this entry was created
    pub tags: Vec<String>,     // Semantic tags for keyword-based recall
    pub source: String,        // Origin: "conversation", "reflection", "tool_result", "overflow"
}
```

| Field | Type | Range | Purpose |
|-------|------|-------|---------|
| `content` | `String` | — | The summarized text (conversation turn, reflection, tool result, etc.) |
| `importance` | `f64` | 1.0–10.0 | Controls eviction priority and context injection order. Clamped on creation. |
| `timestamp` | `DateTime<Utc>` | — | Creation time. Used for age-based summarization decisions. |
| `tags` | `Vec<String>` | — | Semantic tags for keyword-based recall (e.g. `["rust", "error", "debug"]`). |
| `source` | `String` | — | Origin of the entry. Used for filtering and provenance tracking. |

### Creating Entries

```rust
use echo_core::memory::tiered::MemoryEntry;

// Structured entry with full metadata
let entry = MemoryEntry::new(
    "Found a null pointer bug in parser.rs".to_string(),
    8.0,                                          // high importance
    vec!["rust".to_string(), "bug".to_string()],  // tags
    "tool_result".to_string(),                    // source
);

// Simple entry with defaults (importance 5.0, no tags, source "conversation")
let simple = MemoryEntry::simple("User prefers dark theme".to_string());
```

### Keyword Matching

`MemoryEntry::matches_keyword()` searches both `content` and `tags` (case-insensitive):

```rust
let entry = MemoryEntry::new(
    "Rust compilation error in module A".to_string(),
    7.0,
    vec!["rust".to_string(), "error".to_string()],
    "conversation".to_string(),
);

entry.matches_keyword("rust");       // true  (tag match)
entry.matches_keyword("compilation"); // true  (content match)
entry.matches_keyword("python");     // false
```

---

## TieredMemory Struct

```rust
pub struct TieredMemory {
    pub core: CoreMemory,              // Always injected into the system prompt
    pub short_term: Vec<MemoryEntry>,  // Recent structured entries (max N)
    pub max_short_term: usize,         // Cap on short-term entries
    pub long_term: Option<Arc<dyn Store>>, // Optional persistent store
    pub overflow_queue: Vec<MemoryEntry>,  // Evicted short-term entries
    pub max_overflow: usize,           // Cap on overflow queue (default 100)
}
```

### Construction

```rust
use echo_core::memory::tiered::TieredMemory;
use std::sync::Arc;

// Basic construction: 5 short-term entries, 2000-char core memory budget
let memory = TieredMemory::new(5, 2000);

// With explicit overflow bound
let memory = TieredMemory::new(5, 2000)
    .with_overflow_bound(50);

// With a long-term store attached
let store: Arc<dyn Store> = Arc::new(FileStore::new("./store.json")?);
let memory = TieredMemory::new(5, 2000)
    .with_overflow_bound(50)
    .with_store(store);

// Default: max_short_term=5, max_core_chars=2000, max_overflow=100
let memory = TieredMemory::default();
```

| Method | Purpose |
|--------|---------|
| `new(max_short_term, max_core_chars)` | Create with short-term cap and core memory character budget |
| `with_overflow_bound(n)` | Set the maximum overflow queue size (default 100) |
| `with_store(store)` | Attach a long-term `Store` for persistence |

---

## Configuration Parameters

| Parameter | Default | Purpose |
|-----------|---------|---------|
| `max_short_term` | 5 | Maximum entries in the short-term layer. When exceeded, the lowest-importance entry is evicted to overflow. |
| `max_core_chars` | 2000 | Total character budget for core memory blocks. When exceeded, the lowest-importance block is evicted. |
| `max_overflow` | 100 | Maximum entries in the overflow queue. When exceeded with no store attached, the lowest-importance entry is permanently evicted with a warning. |
| `auto_summarize_threshold` | `max_short_term × 2` | When `short_term.len() + overflow_queue.len()` reaches this, an LLM summarization pass should compress older entries. |

### Sizing Guidelines

| Scenario | `max_short_term` | `max_overflow` | `max_core_chars` |
|----------|------------------|----------------|------------------|
| Short chat session | 3–5 | 50 | 1000 |
| Long coding session | 10–20 | 100 | 2000 |
| Multi-agent orchestration | 5–10 | 50 | 1500 |
| Research / paper writing | 15–30 | 200 | 3000 |

---

## Eviction Policies

Eviction happens at three points in the memory hierarchy. All three use **importance-based** eviction (not pure LRU), ensuring that high-value memories survive even when newer entries arrive.

### 1. Short-term Eviction

When `add_short_term()` pushes `short_term.len()` beyond `max_short_term`, the entry with the **lowest importance** is removed and pushed to the overflow queue:

```rust
let mut memory = TieredMemory::new(2, 2000);

memory.add_short_term(MemoryEntry::simple("low importance".to_string()));      // imp 5.0
memory.add_short_term(MemoryEntry::new("high".into(), 9.0, vec![], "conv".into())); // imp 9.0
// short_term: [5.0, 9.0]

memory.add_short_term(MemoryEntry::new("medium".into(), 7.0, vec![], "conv".into())); // imp 7.0
// short_term.len() would be 3 > max 2
// Evicts lowest: "low importance" (5.0) → overflow_queue
// short_term: [9.0, 7.0], overflow: [5.0]
```

### 2. Overflow Eviction

When the overflow queue reaches `max_overflow` **and no long-term store is attached**, the lowest-importance entry is permanently evicted with a `tracing::warn!`:

```
Overflow queue full (max 100), evicted entry (importance=2.0): User mentioned they like...
```

When a store **is** attached, the overflow queue is allowed to briefly exceed the bound — the entries will be flushed to the store on the next `flush_overflow()` call.

### 3. Long-term Decay and Pruning

Long-term memories use exponential decay to gradually reduce the effective importance of old, unaccessed entries:

```text
effective_score = importance × e^(-λ × days_since_access)
where λ = 0.05 (half-life ≈ 14 days)
```

Items with `effective_score < 1.0` become pruning candidates:

```rust
// Identify candidates for removal
let candidates = memory.prune_candidates(&items);

// Sort and truncate by decayed importance
let mut items = store.search(&["memories", "short_term"], "", 100).await?;
TieredMemory::rank_by_importance(&mut items, 20);
```

| Days since access | Decay factor (λ=0.05) | Original imp 8.0 → effective |
|-------------------|-----------------------|-------------------------------|
| 0 | 1.000 | 8.00 |
| 7 | 0.705 | 5.64 |
| 14 | 0.497 | 3.97 |
| 30 | 0.223 | 1.78 |
| 60 | 0.050 | 0.40 ← prune candidate |
| 90 | 0.011 | 0.09 ← prune candidate |

---

## Overflow Handling and Flush

The overflow queue acts as a buffer between short-term memory and the long-term store. Call `flush_overflow()` to persist entries:

```rust
// Flush overflow entries to the long-term store
let flushed = memory.flush_overflow().await;
println!("Persisted {} entries to long-term store", flushed);
```

### Behavior Matrix

| Store attached? | Queue full? | Behavior |
|-----------------|-------------|----------|
| No | No | Entry stays in bounded overflow queue |
| No | Yes | Lowest-importance entry evicted with warning |
| Yes | No | Entry stays until next flush |
| Yes | Yes | Queue briefly exceeds bound; flush drains all entries |

Each flushed entry is written to the store under the namespace `["memories", "short_term"]` with key `short_term_{uuid}`:

```json
{
  "content": "User prefers Rust over Python for backend services",
  "importance": 7.0,
  "timestamp": "2026-06-02T10:30:00Z",
  "tags": ["rust", "preferences"],
  "source": "conversation"
}
```

---

## Context Injection

`build_context_injection()` assembles a string from Core + ShortTerm memory that gets injected into the agent's system prompt. Short-term entries are sorted by **importance descending**, not FIFO:

```rust
if let Some(injection) = memory.build_context_injection() {
    // Inject into system prompt
    system_prompt.push_str(&injection);
}
```

Example output:

```text
## Core Memory
- user_name: Alice
- project_goal: Build a Rust agent framework

## Recent Context
1. Critical: found a null pointer bug in parser.rs
2. User is building an Agent framework with tokio
3. Previous conversation about Rust error handling
```

Entry #1 appears first because it has importance 9.0, even though it was added last.

---

## Recall

### Short-term Recall

Search short-term entries by keyword (matches both content and tags):

```rust
let results = memory.recall("rust", 5);
for entry in results {
    println!("[imp={:.1}] {}", entry.importance, entry.content);
}
// Results sorted by importance descending, limited to 5
```

### Long-term Recall

Search the long-term store via its keyword search interface:

```rust
if let Some(items) = memory.recall_from_long_term("parser bug", 10).await {
    for item in items {
        println!("[score={:.2}] {:?}", item.score, item.value["content"]);
    }
}
// Returns None if no long-term store is attached
```

---

## Auto-summarization

`TieredMemory` tracks when the total pending entries (short-term + overflow) reach a threshold that indicates the need for an LLM summarization pass:

```rust
// Check if summarization is needed
if memory.needs_summarization() {
    let entries = &memory.short_term;
    // Send older entries to LLM for compression
    let summary = llm_summarize(entries).await;
    // Replace old entries with the compressed summary
    memory.short_term.clear();
    memory.overflow_queue.clear();
    memory.add_short_term(MemoryEntry::new(
        summary,
        7.0,
        vec!["summary".to_string()],
        "reflection".to_string(),
    ));
}
```

| Method | Returns | Purpose |
|--------|---------|---------|
| `auto_summarize_threshold()` | `max_short_term × 2` | The entry count that triggers summarization |
| `needs_summarization()` | `bool` | Whether `short_term + overflow >= threshold` |
| `total_pending_entries()` | `usize` | Current `short_term.len() + overflow_queue.len()` |

---

## Integration with Agent Memory Tools

`TieredMemory` integrates with the agent's built-in `remember` / `recall` / `forget` tools through the `Store` trait. When a long-term store is attached, overflow entries are automatically persisted and become searchable via `recall`.

### Flow

```
Agent calls remember("User prefers dark theme", importance=7)
    │
    ├─► CoreMemory.upsert() if high importance (≥ 8.0)
    │       → Always visible in system prompt
    │
    └─► TieredMemory.add_short_term()
            │
            ├─► Fits in short-term → stays in working context
            │
            └─► Short-term full → evicted to overflow
                    │
                    └─► flush_overflow() → Store.put(["memories", "short_term"], ...)
                            │
                            └─► Searchable via recall("dark theme")
```

### Agent Configuration

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "assistant", "You are a helpful assistant")
    .enable_memory(true)
    .memory_path("./store.json");

let mut agent = ReactAgent::new(config);
// The agent now has access to remember/recall/forget tools
// and TieredMemory manages the memory hierarchy automatically
```

---

## Complete Example

```rust
use echo_core::memory::tiered::{TieredMemory, MemoryEntry};
use echo_core::memory::core_memory::CoreMemoryBlock;
use std::sync::Arc;

// 1. Create tiered memory with all layers
let store: Arc<dyn echo_core::memory::Store> =
    Arc::new(echo_state::FileStore::new("./store.json").unwrap());

let mut memory = TieredMemory::new(5, 2000)
    .with_overflow_bound(50)
    .with_store(store);

// 2. Set up core memory (always visible)
memory.core.upsert(
    CoreMemoryBlock::new("user", "user_name", "Alice")
        .with_importance(9.0)
);
memory.core.upsert(
    CoreMemoryBlock::new("proj", "project", "Rust agent framework")
        .with_importance(7.0)
);

// 3. Add structured short-term memories
memory.add_short_term(MemoryEntry::new(
    "User reported a null pointer bug in parser.rs".to_string(),
    9.0,
    vec!["rust".to_string(), "bug".to_string()],
    "conversation".to_string(),
));
memory.add_short_term(MemoryEntry::new(
    "Discussed error handling patterns in async Rust".to_string(),
    7.0,
    vec!["rust".to_string(), "async".to_string()],
    "conversation".to_string(),
));
memory.add_short_term(MemoryEntry::simple(
    "User asked about memory tier configuration".to_string(),
));

// 4. Build context injection (importance-sorted)
if let Some(ctx) = memory.build_context_injection() {
    println!("=== Injected into system prompt ===");
    println!("{}", ctx);
}

// 5. Recall by keyword
let rust_entries = memory.recall("rust", 10);
println!("\n=== Recall 'rust' ===");
for e in rust_entries {
    println!("[imp={:.1}] {}", e.importance, e.content);
}

// 6. Check summarization need
if memory.needs_summarization() {
    println!("\nSummarization needed! {} pending entries", memory.total_pending_entries());
}

// 7. Flush overflow to long-term store
tokio::runtime::Runtime::new().unwrap().block_on(async {
    let flushed = memory.flush_overflow().await;
    println!("\nFlushed {} entries to long-term store", flushed);

    // 8. Recall from long-term
    if let Some(items) = memory.recall_from_long_term("bug", 5).await {
        for item in items {
            println!("[long-term] {:?}", item.value["content"]);
        }
    }
});
```

---

## Migration from Flat Memory

If you have an existing agent that uses a flat `Vec<String>` or a bare message list for memory, here is how to migrate to `TieredMemory`.

### Before: Flat Memory

```rust
// Old approach: a list of string summaries
let mut memories: Vec<String> = vec![];
memories.push("User likes Rust".to_string());
memories.push("Project uses tokio".to_string());
// No importance, no eviction, no persistence
```

### After: Tiered Memory

```rust
use echo_core::memory::tiered::{TieredMemory, MemoryEntry};

let mut memory = TieredMemory::new(10, 2000).with_overflow_bound(50);

// Migrate existing strings as simple entries
for s in &old_memories {
    memory.add_short_term(MemoryEntry::simple(s.clone()));
}

// Or create structured entries with importance
memory.add_short_term(MemoryEntry::new(
    "User likes Rust".to_string(),
    7.0,
    vec!["preferences".to_string(), "rust".to_string()],
    "migration".to_string(),
));
```

### Migration Checklist

| Step | Action |
|------|--------|
| 1 | Replace `Vec<String>` with `TieredMemory::new(max_short_term, max_core_chars)` |
| 2 | Wrap each existing string in `MemoryEntry::simple()` or `MemoryEntry::new()` with appropriate importance |
| 3 | Set `max_short_term` to roughly the size of your old list |
| 4 | Set `with_overflow_bound()` based on how many evicted entries you want to buffer |
| 5 | Attach a `Store` via `with_store()` if you need persistence across sessions |
| 6 | Replace manual context building with `build_context_injection()` |
| 7 | Replace manual keyword search with `recall()` and `recall_from_long_term()` |
| 8 | Add a periodic `flush_overflow()` call in your agent loop |

### Backward Compatibility

`add_short_term_simple()` provides a drop-in replacement for `push()` on a `Vec<String>`:

```rust
// Before
memories.push("summary text".to_string());

// After
memory.add_short_term_simple("summary text".to_string());
// Creates a MemoryEntry with importance=5.0, no tags, source="conversation"
```

---

## Architecture Summary

```
┌─────────────────────────────────────────────────────────────────┐
│                         TieredMemory                             │
│                                                                  │
│  ┌────────────────────────────────────────────────────────┐     │
│  │  Core Memory (CoreMemory)                               │     │
│  │  • Fixed blocks, always in system prompt                │     │
│  │  • Character-budget bounded (max_core_chars)            │     │
│  │  • Eviction: lowest importance block removed            │     │
│  └────────────────────────────────────────────────────────┘     │
│                                                                  │
│  ┌──────────────────────┐    ┌──────────────────────┐           │
│  │  ShortTerm            │    │  Overflow Queue       │           │
│  │  Vec<MemoryEntry>     │───►│  Vec<MemoryEntry>     │           │
│  │  • max_short_term cap │    │  • max_overflow cap   │           │
│  │  • Imp-based eviction │    │  • Imp-based eviction │           │
│  │  • Importance-sorted  │    │    (when no store)    │           │
│  │    context injection  │    │  • Async flush to     │           │
│  └──────────────────────┘    │    long-term store    │           │
│                               └──────────┬───────────┘           │
│                                          │                       │
│                                          ▼                       │
│  ┌────────────────────────────────────────────────────────┐     │
│  │  LongTerm Store (Option<Arc<dyn Store>>)                │     │
│  │  • Persistent KV storage                                │     │
│  │  • Namespace: ["memories", "short_term"]                │     │
│  │  • Keyword search via recall_from_long_term()           │     │
│  │  • Decay-based pruning (λ=0.05, half-life ≈ 14 days)   │     │
│  └────────────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────────┘
```

---

## See Also

- [03-memory.md](03-memory.md) — Store, RuntimeStateStore, and ConversationStore basics
- [04-compression.md](04-compression.md) — Context window compression
- [19-self-reflection.md](19-self-reflection.md) — Reflection-driven memory updates
- [28-config-reference.md](28-config-reference.md) — Full configuration reference
