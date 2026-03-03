# Store 语义搜索（向量检索）

## 是什么

`EmbeddingStore` 是对任意 `Store` 实现的向量增强包装层。它在透传所有 KV 操作的同时，通过 `Embedder` 接口将文本转换为稠密向量，并覆盖 `Store::semantic_search()` 以提供**余弦相似度检索**。

---

## 解决什么问题

默认的 `FileStore` / `InMemoryStore` 使用关键词匹配：

```text
存储：{"content": "用户喜好：古典音乐"}
查询：recall("music preference")  ← 英文查询，中文内容，命中率 = 0

存储：{"content": "深色主题偏好"}
查询：recall("dark mode")         ← 同义词不同，命中率 = 0
```

`EmbeddingStore` 在语义空间对齐，跨语言和同义词均能正确召回：

```text
查询："music preference"  →  [score=0.87] 用户喜好：古典音乐  ✅
查询："dark mode"         →  [score=0.81] 深色主题偏好        ✅
```

---

## 架构

```
EmbeddingStore
├── inner: Arc<dyn Store>     ← 负责 KV 持久化（FileStore / InMemoryStore / ...）
├── embedder: Arc<dyn Embedder> ← 负责文本 → 向量转换（HttpEmbedder / MockEmbedder）
└── index: VecIndex           ← 内存向量索引（可选持久化到 JSON 文件）
```

**数据分层存储：**

| 层 | 负责方 | 内容 |
|----|--------|------|
| 内容层 | `inner` Store | 条目键值（原始 JSON） |
| 向量层 | `VecIndex` + 可选 `.vecs.json` | 每条记忆的嵌入向量 |

---

## 快速上手

### 1. 配置环境变量

> ⚠️ **必须使用真正的 Embedding 模型**，对话模型（如 DeepSeek-Chat、GPT-4 等）不提供嵌入接口。

```bash
# OpenAI
export EMBEDDING_API_KEY="sk-..."       # 或 EMBEDDING_APIKEY
export EMBEDDING_MODEL="text-embedding-3-small"

# Qwen（DashScope）—— 两种 URL 写法均支持
# 写法 A：base URL（代码自动追加 /v1/embeddings）
export EMBEDDING_API_URL="https://dashscope.aliyuncs.com/compatible-mode"
export EMBEDDING_APIKEY="sk-..."
export EMBEDDING_MODEL="text-embedding-v3"

# 写法 B：完整端点 URL（直接使用）
export EMBEDDING_BASEURL="https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings"
export EMBEDDING_APIKEY="sk-..."
export EMBEDDING_MODEL="text-embedding-v3"
```

**环境变量优先级：**

| 用途 | 优先 | 备选 | 最终备选 |
|------|------|------|--------|
| 完整端点 URL | `EMBEDDING_BASEURL` | — | — |
| Base URL | `EMBEDDING_API_URL` | — | `https://api.openai.com` |
| API 密钥 | `EMBEDDING_APIKEY` | `EMBEDDING_API_KEY` | `OPENAI_API_KEY` |
| 模型 | `EMBEDDING_MODEL` | — | `text-embedding-3-small` |

### 2. 创建 EmbeddingStore

```rust
use echo_agent::memory::{EmbeddingStore, FileStore, HttpEmbedder};
use std::sync::Arc;

// 内容持久化到 store.json，向量索引持久化到 store.vecs.json
let inner = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
let embedder = Arc::new(HttpEmbedder::from_env());
let store = Arc::new(
    EmbeddingStore::with_persistence(inner, embedder, "~/.echo-agent/store.vecs.json")?
);
```

### 3. 挂载到 Agent

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgent::new(config);
agent.set_memory_store(store); // ← 同时更新 remember/recall/forget 工具
```

挂载后，Agent 的三个行为均使用语义搜索：
- **自动注入**：每次 `execute()` / `chat()` 前，自动语义召回相关记忆注入上下文
- **`recall` 工具**：Agent 主动调用时执行语义检索
- **`remember` 工具**：写入时自动计算并存储嵌入向量

---

## API 参考

### `Store` trait 新增接口

```rust
pub trait Store: Send + Sync {
    // 已有方法（put / get / search / delete / list_namespaces）...

    /// 是否支持语义搜索（EmbeddingStore 返回 true，其余返回 false）
    fn supports_semantic_search(&self) -> bool { false }

    /// 语义检索。EmbeddingStore 执行余弦相似度检索；其余实现回退到 search()。
    async fn semantic_search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        self.search(namespace, query, limit).await  // 默认回退
    }
}
```

### `EmbeddingStore` 构造

```rust
// 内存索引（进程重启后需重建）
EmbeddingStore::new(inner, embedder)

// 持久化索引（推荐生产使用）
EmbeddingStore::with_persistence(inner, embedder, vec_path)?
```

### `Embedder` trait

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}
```

内置实现：

| 实现 | 说明 |
|------|------|
| `HttpEmbedder` | OpenAI 兼容 HTTP 嵌入客户端（生产使用）|
| `MockEmbedder` | 基于字节哈希的确定性伪嵌入（测试使用）|

---

## 与 `set_store` 的区别

| 方法 | 作用 |
|------|------|
| `agent.set_store(store)` | 仅替换自动注入通道（不更新工具）|
| `agent.set_memory_store(store)` | 替换自动注入通道 + 重注册 `remember` / `recall` / `forget` 工具 |

**推荐始终使用 `set_memory_store()`。**

---

## 向量持久化

`with_persistence` 创建的 `EmbeddingStore` 会把向量索引写入独立的 JSON 文件：

```json
{
  "alice/memories": {
    "uuid-1": [0.12, -0.34, 0.56, ...],
    "uuid-2": [...]
  }
}
```

重启后直接加载已有向量，无需重新计算嵌入。若无持久化文件，索引从空开始，仅新写入的条目会加入向量索引。

---

## 在测试中使用

```rust
use echo_agent::testing::MockEmbedder;
use echo_agent::memory::{EmbeddingStore, InMemoryStore};
use std::sync::Arc;

let inner = Arc::new(InMemoryStore::new());
let embedder = Arc::new(MockEmbedder::new(8)); // 8 维伪嵌入，无需 API
let store = Arc::new(EmbeddingStore::new(inner, embedder));

// 正常使用
store.put(&["test"], "k1", json!({"content": "hello"})).await?;
let hits = store.semantic_search(&["test"], "greeting", 3).await?;
```

---

## 注意事项

1. **向量维度一致性**：同一个 `EmbeddingStore` 实例内所有向量维度必须相同（由 Embedder 保证），混用不同模型会导致相似度计算错误
2. **嵌入计算失败时**：`put()` 不受影响（内容层仍写入），仅向量索引中缺失该条目；`semantic_search()` 回退到关键词检索
3. **冷启动（索引为空）**：若 vec 文件不存在或为空，`semantic_search()` 自动回退到关键词检索，不报错
4. **API 延迟**：每次 `put()` 和 `semantic_search()` 都需要一次嵌入 API 调用，高并发场景需关注限流

对应示例：`examples/demo18_semantic_memory.rs`
