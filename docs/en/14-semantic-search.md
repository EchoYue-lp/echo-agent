# Store Semantic Search (Vector Retrieval)

## What It Is

`EmbeddingStore` is a vector-augmented wrapper around any `Store` implementation. It transparently delegates all KV operations while using an `Embedder` to convert text to dense vectors, overriding `Store::semantic_search()` to provide **cosine similarity retrieval**.

---

## Problem It Solves

The default `FileStore` / `InMemoryStore` use keyword matching:

```text
Stored: {"content": "用户喜好：古典音乐"}  (user prefers classical music)
Query: recall("music preference")  ← English query, Chinese content, hits = 0

Stored: {"content": "深色主题偏好"}  (dark theme preference)
Query: recall("dark mode")          ← synonym mismatch, hits = 0
```

`EmbeddingStore` aligns in semantic space — cross-lingual and synonym queries succeed:

```text
Query: "music preference"  →  [score=0.87] 用户喜好：古典音乐  ✅
Query: "dark mode"         →  [score=0.81] 深色主题偏好        ✅
```

---

## Architecture

```
EmbeddingStore
├── inner: Arc<dyn Store>       ← KV persistence (FileStore / InMemoryStore / ...)
├── embedder: Arc<dyn Embedder> ← text → vector conversion (HttpEmbedder / MockEmbedder)
└── index: VecIndex             ← in-memory vector index (optionally persisted to JSON)
```

**Two storage layers:**

| Layer | Owner | Content |
|-------|-------|---------|
| Content layer | `inner` Store | Item key-value (raw JSON) |
| Vector layer | `VecIndex` + optional `.vecs.json` | Embedding for each item |

---

## Quick Start

### 1. Set environment variables

> ⚠️ **You must use a real Embedding model.** Chat models (DeepSeek-Chat, GPT-4, etc.) do not expose an embedding endpoint.

```bash
# OpenAI
export EMBEDDING_API_KEY="sk-..."    # or EMBEDDING_APIKEY
export EMBEDDING_MODEL="text-embedding-3-small"

# Qwen (DashScope) — two URL styles are both supported:
# Style A: base URL (code auto-appends /v1/embeddings)
export EMBEDDING_API_URL="https://dashscope.aliyuncs.com/compatible-mode"
export EMBEDDING_APIKEY="sk-..."
export EMBEDDING_MODEL="text-embedding-v3"

# Style B: full endpoint URL (used as-is)
export EMBEDDING_BASEURL="https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings"
export EMBEDDING_APIKEY="sk-..."
export EMBEDDING_MODEL="text-embedding-v3"
```

**Environment variable priority:**

| Purpose | First | Fallback | Last resort |
|---------|-------|----------|-------------|
| Full endpoint URL | `EMBEDDING_BASEURL` | — | — |
| Base URL | `EMBEDDING_API_URL` | — | `https://api.openai.com` |
| API key | `EMBEDDING_APIKEY` | `EMBEDDING_API_KEY` | `OPENAI_API_KEY` |
| Model name | `EMBEDDING_MODEL` | — | `text-embedding-3-small` |

### 2. Create an EmbeddingStore

```rust
use echo_agent::memory::{EmbeddingStore, FileStore, HttpEmbedder};
use std::sync::Arc;

// Content persisted to store.json; vector index persisted to store.vecs.json
let inner = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
let embedder = Arc::new(HttpEmbedder::from_env());
let store = Arc::new(
    EmbeddingStore::with_persistence(inner, embedder, "~/.echo-agent/store.vecs.json")?
);
```

### 3. Attach to an Agent

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgent::new(config);
agent.set_memory_store(store); // ← also re-registers remember/recall/forget tools
```

Once attached, three behaviors all use semantic search:
- **Auto-injection**: before each `execute()` / `chat()`, semantically recalled memories are injected into context
- **`recall` tool**: when the Agent calls it, vector similarity search is used
- **`remember` tool**: on write, the embedding is computed and stored automatically

---

## API Reference

### New `Store` trait methods

```rust
pub trait Store: Send + Sync {
    // existing methods (put / get / search / delete / list_namespaces)...

    /// Whether semantic (vector) search is supported. EmbeddingStore returns true.
    fn supports_semantic_search(&self) -> bool { false }

    /// Semantic retrieval. EmbeddingStore performs cosine similarity; others fall back to search().
    async fn semantic_search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        self.search(namespace, query, limit).await  // default fallback
    }
}
```

### `EmbeddingStore` constructors

```rust
// In-memory index (lost on restart)
EmbeddingStore::new(inner, embedder)

// Persisted index (recommended for production)
EmbeddingStore::with_persistence(inner, embedder, vec_path)?
```

### `Embedder` trait

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}
```

Built-in implementations:

| Implementation | Description |
|---|---|
| `HttpEmbedder` | OpenAI-compatible HTTP embedding client (production) |
| `MockEmbedder` | Deterministic pseudo-embeddings via byte hashing (testing) |

---

## `set_store` vs `set_memory_store`

| Method | Effect |
|--------|--------|
| `agent.set_store(store)` | Replaces auto-injection channel only (tools not updated) |
| `agent.set_memory_store(store)` | Replaces auto-injection channel **and** re-registers `remember` / `recall` / `forget` tools |

**Always prefer `set_memory_store()` when switching stores.**

---

## Vector Persistence

`with_persistence` writes the vector index to a separate JSON file alongside the content store:

```json
{
  "alice/memories": {
    "uuid-1": [0.12, -0.34, 0.56, ...],
    "uuid-2": [...]
  }
}
```

On restart, stored vectors are loaded directly — no re-embedding needed. If no persistence file exists, the index starts empty; only newly written items will be indexed.

---

## Usage in Tests

```rust
use echo_agent::testing::MockEmbedder;
use echo_agent::memory::{EmbeddingStore, InMemoryStore};
use std::sync::Arc;

let inner = Arc::new(InMemoryStore::new());
let embedder = Arc::new(MockEmbedder::new(8)); // 8-dim pseudo-embeddings, no API needed
let store = Arc::new(EmbeddingStore::new(inner, embedder));

store.put(&["test"], "k1", json!({"content": "hello"})).await?;
let hits = store.semantic_search(&["test"], "greeting", 3).await?;
```

---

## Notes

1. **Vector dimension consistency**: All vectors within a single `EmbeddingStore` must have the same dimension (enforced by the Embedder). Mixing different models causes incorrect similarity scores.
2. **Embedding failures**: `put()` is unaffected (content layer still writes); the item simply won't be in the vector index. `semantic_search()` falls back to keyword search.
3. **Cold start (empty index)**: If no vec file exists, `semantic_search()` silently falls back to keyword search — no errors.
4. **API latency**: Each `put()` and `semantic_search()` requires one embedding API call. Monitor rate limits in high-concurrency scenarios.

See: `examples/demo18_semantic_memory.rs`
