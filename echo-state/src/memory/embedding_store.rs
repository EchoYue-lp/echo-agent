//! 向量增强 Store（EmbeddingStore）
//!
//! 在任意 [`Store`] 实现外包装一层向量索引，透传所有 KV 操作，
//! 并通过 [`Store::search_with`] 提供语义/混合检索能力。
//!
//! ## 存储分层
//!
//! | 层 | 负责方 | 内容 |
//! |----|--------|------|
//! | 内容层 | `inner: Arc<dyn Store>` | 条目键值（KV 语义）|
//! | 向量层 | 内存 `RwLock<VecIndex>` + 可选 JSON 文件 | 嵌入向量 |
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::{EmbeddingStore, FileStore, HttpEmbedder};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! let inner = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
//! let embedder = Arc::new(HttpEmbedder::from_env());
//! let store = Arc::new(
//!     EmbeddingStore::with_persistence(inner, embedder, "~/.echo-agent/store.vecs.json")?
//! );
//!
//! // 将 `store` 注入你自己的 runtime 层，或通过 `echo_agent` façade 使用。
//! let _ = store;
//! # Ok(())
//! # }
//! ```

use crate::util::expand_tilde;
use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::embedder::Embedder;
pub use echo_core::memory::store::{SearchMode, SearchQuery, Store, StoreItem};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// ── VecIndex ─────────────────────────────────────────────────────────────────

/// 内存向量索引：namespace_key → key → 嵌入向量
#[derive(Default)]
struct VecIndex {
    /// namespace_key（如 "alice/memories"）→ key（UUID）→ 向量
    data: HashMap<String, HashMap<String, Vec<f32>>>,
}

impl VecIndex {
    fn insert(&mut self, ns_key: &str, key: &str, vec: Vec<f32>) {
        self.data
            .entry(ns_key.to_string())
            .or_default()
            .insert(key.to_string(), vec);
    }

    fn remove(&mut self, ns_key: &str, key: &str) {
        if let Some(ns) = self.data.get_mut(ns_key) {
            ns.remove(key);
        }
    }

    fn get_namespace(&self, ns_key: &str) -> Option<&HashMap<String, Vec<f32>>> {
        self.data.get(ns_key)
    }
}

// ── EmbeddingStore ────────────────────────────────────────────────────────────

/// 向量增强 Store：在任意 `Store` 外包装余弦相似度语义检索
///
/// > **注意**：此实现适用于**小规模**场景（数千条目以内）。向量索引
/// > 全部加载到内存中，对于大规模检索场景，推荐使用专门的向量数据库
/// > （如 Milvus、Qdrant、pgvector 等）。
pub struct EmbeddingStore {
    inner: Arc<dyn Store>,
    embedder: Arc<dyn Embedder>,
    index: RwLock<VecIndex>,
    vec_path: Option<PathBuf>,
    /// 语义检索时最多加载到内存进行比对的向量数量。
    /// 超出此数量的向量会被跳过，避免 OOM。
    /// 默认 10000。
    max_candidates: usize,
}

impl EmbeddingStore {
    /// 创建 EmbeddingStore，向量索引仅保存在内存中（进程重启后需重新写入条目以重建索引）
    pub fn new(inner: Arc<dyn Store>, embedder: Arc<dyn Embedder>) -> Self {
        info!("🧠 EmbeddingStore 初始化（内存索引）");
        Self {
            inner,
            embedder,
            index: RwLock::new(VecIndex::default()),
            vec_path: None,
            max_candidates: 10_000,
        }
    }

    /// 创建 EmbeddingStore，向量索引持久化到指定 JSON 文件
    ///
    /// 若文件已存在则加载已有向量索引；不存在则从空索引开始（新条目写入时实时更新）。
    pub fn with_persistence(
        inner: Arc<dyn Store>,
        embedder: Arc<dyn Embedder>,
        vec_path: impl AsRef<Path>,
    ) -> Result<Self> {
        let path = expand_tilde(vec_path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError::IoError(e.to_string()))?;
        }
        let index = if path.exists() {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| MemoryError::IoError(e.to_string()))?;
            let data: HashMap<String, HashMap<String, Vec<f32>>> = serde_json::from_str(&raw)
                .unwrap_or_else(|e| {
                    warn!("向量索引文件解析失败，从空索引开始: {e}");
                    HashMap::new()
                });
            let entry_count: usize = data.values().map(|m| m.len()).sum();
            info!(
                path = %path.display(),
                entries = entry_count,
                "🧠 EmbeddingStore 初始化（持久化索引，已加载 {} 条向量）",
                entry_count,
            );
            VecIndex { data }
        } else {
            info!(path = %path.display(), "🧠 EmbeddingStore 初始化（空索引）");
            VecIndex::default()
        };
        Ok(Self {
            inner,
            embedder,
            index: RwLock::new(index),
            vec_path: Some(path),
            max_candidates: 10_000,
        })
    }

    /// 设置语义检索时最多加载到内存进行比对的向量数量（默认 10000）。
    ///
    /// 超出此数量的向量会被跳过，避免大规模场景下的 OOM 问题。
    pub fn with_max_candidates(mut self, max: usize) -> Self {
        self.max_candidates = max;
        self
    }

    /// 从 JSON Value 提取可嵌入文本
    ///
    /// 优先使用 `content` 字段（`remember` 工具写入格式），追加 `tags` 以丰富语义；
    /// 否则将整个 Value 序列化为文本。
    fn extract_text(value: &Value) -> String {
        if let Value::Object(map) = value
            && let Some(content) = map.get("content").and_then(|v| v.as_str())
        {
            let tags: String = map
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            return if tags.is_empty() {
                content.to_string()
            } else {
                format!("{content} {tags}")
            };
        }

        value_to_text(value)
    }

    async fn flush_index(&self) -> Result<()> {
        let Some(ref path) = self.vec_path else {
            return Ok(());
        };
        let index = self.index.read().await;
        let json = serde_json::to_string(&index.data)
            .map_err(|e| MemoryError::SerializationError(format!("向量索引序列化失败: {e}")))?;
        tokio::fs::write(path, json)
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        debug!(path = %path.display(), "💾 向量索引已持久化");
        Ok(())
    }

    /// 将向量索引刷盘（仅对有持久化路径的实例有效）。
    ///
    /// 建议在批量写入后调用，以将内存中的向量变更持久化。
    pub async fn flush_vector_index(&self) -> Result<()> {
        self.flush_index().await
    }

    async fn semantic_search_impl(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        let ns_key = namespace.join("/");

        let query_vec = match self.embedder.embed(query).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "⚠️ 查询嵌入计算失败，回退到关键词检索");
                return self.inner.search(namespace, query, limit).await;
            }
        };

        let scored: Vec<(f32, String)> = {
            let index = self.index.read().await;
            let Some(ns_vecs) = index.get_namespace(&ns_key) else {
                debug!(ns = %ns_key, "向量索引为空，回退到关键词检索");
                drop(index);
                return self.inner.search(namespace, query, limit).await;
            };
            let mut scored: Vec<(f32, String)> = ns_vecs
                .iter()
                .take(self.max_candidates)
                .map(|(key, vec)| (cosine_similarity(&query_vec, vec), key.clone()))
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(limit);
            scored
        };

        if scored.is_empty() {
            return Ok(vec![]);
        }

        debug!(ns = %ns_key, query = %query, hits = scored.len(), "🔍 语义检索完成");

        let mut results = Vec::with_capacity(scored.len());
        for (score, key) in scored {
            if let Ok(Some(mut item)) = self.inner.get(namespace, &key).await {
                item.score = Some(score);
                results.push(item);
            }
        }
        Ok(results)
    }
}

impl Store for EmbeddingStore {
    fn put<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        value: Value,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.inner.put(namespace, key, value.clone()).await?;

            // 嵌入计算失败只打 warn，不影响写入
            let text = Self::extract_text(&value);
            match self.embedder.embed(&text).await {
                Ok(vec) => {
                    let ns_key = namespace.join("/");
                    debug!(ns = %ns_key, key = %key, dims = vec.len(), "📌 向量索引已更新");
                    self.index.write().await.insert(&ns_key, key, vec);
                    // 批量刷盘：不在每次 put 时立即写盘，减少 IO 开销。
                    // 调用者应定期调用 `flush_vector_index()` 或使用 `with_persistence`
                    // 配合 `put_batch()` 来持久化向量索引。
                }
                Err(e) => {
                    warn!(key = %key, error = %e, "⚠️ 嵌入计算失败，该条目不加入向量索引");
                }
            }
            Ok(())
        })
    }

    fn get<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<StoreItem>>> {
        Box::pin(async move { self.inner.get(namespace, key).await })
    }

    fn search<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move { self.inner.search(namespace, query, limit).await })
    }

    fn delete<'a>(&'a self, namespace: &'a [&'a str], key: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let found = self.inner.delete(namespace, key).await?;
            if found {
                let ns_key = namespace.join("/");
                self.index.write().await.remove(&ns_key, key);
                // 批量刷盘：调用者应定期调用 `flush_vector_index()` 持久化。
            }
            Ok(found)
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
        Box::pin(async move { self.inner.list_namespaces(prefix).await })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move { self.inner.list(namespace).await })
    }

    fn search_with<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: SearchQuery<'a>,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            match query.mode {
                SearchMode::Keyword => self.inner.search(namespace, query.text, query.limit).await,
                SearchMode::Semantic => {
                    self.semantic_search_impl(namespace, query.text, query.limit)
                        .await
                }
                SearchMode::Hybrid { vector_weight } => {
                    // Get both result lists independently
                    let semantic_items = self
                        .semantic_search_impl(namespace, query.text, query.limit)
                        .await?;
                    let keyword_items = self
                        .inner
                        .search(namespace, query.text, query.limit)
                        .await?;

                    // Build rank maps: key -> 0-based position
                    let semantic_ranks: HashMap<&str, usize> = semantic_items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| (item.key.as_str(), i))
                        .collect();
                    let keyword_ranks: HashMap<&str, usize> = keyword_items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| (item.key.as_str(), i))
                        .collect();

                    // Collect all unique keys
                    let all_keys: Vec<&str> = semantic_ranks
                        .keys()
                        .chain(keyword_ranks.keys())
                        .copied()
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .collect();

                    // Score each key via RRF, pick best item from either list
                    let mut items: Vec<StoreItem> = all_keys
                        .iter()
                        .filter_map(|&key| {
                            let sr = semantic_ranks.get(key).copied().unwrap_or(usize::MAX);
                            let kr = keyword_ranks.get(key).copied().unwrap_or(usize::MAX);
                            let rrf = echo_core::memory::store::rrf_score(sr, kr, vector_weight);

                            // Prefer the semantic item if available, otherwise keyword
                            let mut item = semantic_items
                                .iter()
                                .find(|i| i.key == key)
                                .or_else(|| keyword_items.iter().find(|i| i.key == key))?
                                .clone();
                            item.score = Some(rrf);
                            Some(item)
                        })
                        .collect();

                    items.sort_by(|a, b| {
                        b.score
                            .unwrap_or_default()
                            .partial_cmp(&a.score.unwrap_or_default())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    items.truncate(query.limit);
                    Ok(items)
                }
            }
        })
    }
}

// ── 工具函数 ──────────────────────────────────────────────────────────────────

/// 余弦相似度：两向量维度不匹配或为零向量时返回 0.0
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr.iter().map(value_to_text).collect::<Vec<_>>().join(" "),
        Value::Object(map) => map
            .values()
            .map(value_to_text)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MockEmbedder;
    use crate::memory::store::InMemoryStore;
    use serde_json::json;

    async fn make_store() -> EmbeddingStore {
        let inner = Arc::new(InMemoryStore::new());
        let embedder = Arc::new(MockEmbedder::new(4));
        EmbeddingStore::new(inner, embedder)
    }

    #[tokio::test]
    async fn test_put_and_semantic_search() {
        let store = make_store().await;
        let ns = &["test", "ns"];

        store
            .put(ns, "k1", json!({"content": "Rust programming"}))
            .await
            .unwrap();
        store
            .put(ns, "k2", json!({"content": "Python machine learning"}))
            .await
            .unwrap();

        let results = store
            .search_with(ns, SearchQuery::semantic("Rust", 5))
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(results[0].score.is_some());
    }

    #[tokio::test]
    async fn test_delete_removes_from_index() {
        let store = make_store().await;
        let ns = &["test", "del"];

        store
            .put(ns, "k1", json!({"content": "hello world"}))
            .await
            .unwrap();
        store.delete(ns, "k1").await.unwrap();

        // 向量索引中已无该条目
        let index = store.index.read().await;
        let ns_vecs = index.get_namespace("test/del");
        assert!(ns_vecs.map(|m| m.is_empty()).unwrap_or(true));
    }

    #[tokio::test]
    async fn test_cosine_similarity() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-5);

        let c = vec![0.0f32, 1.0, 0.0];
        let sim2 = cosine_similarity(&a, &c);
        assert!((sim2 - 0.0).abs() < 1e-5);
    }
}
