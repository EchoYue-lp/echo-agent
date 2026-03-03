//! 向量增强 Store（EmbeddingStore）
//!
//! 在任意 [`Store`] 实现外包装一层向量索引，透传所有 KV 操作，
//! 覆盖 [`semantic_search`](Store::semantic_search) 以提供余弦相似度检索。
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
//! use echo_agent::memory::{EmbeddingStore, FileStore, HttpEmbedder};
//! use echo_agent::prelude::ReactAgent;
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! # let config = unimplemented!();
//! let inner = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
//! let embedder = Arc::new(HttpEmbedder::from_env());
//! let store = Arc::new(
//!     EmbeddingStore::with_persistence(inner, embedder, "~/.echo-agent/store.vecs.json")?
//! );
//!
//! let mut agent = ReactAgent::new(config);
//! agent.set_memory_store(store);
//! # Ok(())
//! # }
//! ```

use crate::error::{MemoryError, Result};
use crate::memory::embedder::Embedder;
use crate::memory::store::{Store, StoreItem};
use async_trait::async_trait;
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
pub struct EmbeddingStore {
    inner: Arc<dyn Store>,
    embedder: Arc<dyn Embedder>,
    index: RwLock<VecIndex>,
    vec_path: Option<PathBuf>,
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
            std::fs::create_dir_all(parent)
                .map_err(|e| MemoryError::IoError(format!("创建向量索引目录失败: {e}")))?;
        }
        let index = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| MemoryError::IoError(format!("读取向量索引失败: {e}")))?;
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
        })
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
            .map_err(|e| MemoryError::IoError(format!("写入向量索引失败: {e}")))?;
        debug!(path = %path.display(), "💾 向量索引已持久化");
        Ok(())
    }
}

#[async_trait]
impl Store for EmbeddingStore {
    async fn put(&self, namespace: &[&str], key: &str, value: Value) -> Result<()> {
        self.inner.put(namespace, key, value.clone()).await?;

        // 嵌入计算失败只打 warn，不影响写入
        let text = Self::extract_text(&value);
        match self.embedder.embed(&text).await {
            Ok(vec) => {
                let ns_key = namespace.join("/");
                debug!(ns = %ns_key, key = %key, dims = vec.len(), "📌 向量索引已更新");
                self.index.write().await.insert(&ns_key, key, vec);
                if let Err(e) = self.flush_index().await {
                    warn!("向量索引持久化失败（不影响数据写入）: {e}");
                }
            }
            Err(e) => {
                warn!(key = %key, error = %e, "⚠️ 嵌入计算失败，该条目不加入向量索引");
            }
        }
        Ok(())
    }

    async fn get(&self, namespace: &[&str], key: &str) -> Result<Option<StoreItem>> {
        self.inner.get(namespace, key).await
    }

    async fn search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        self.inner.search(namespace, query, limit).await
    }

    async fn delete(&self, namespace: &[&str], key: &str) -> Result<bool> {
        let found = self.inner.delete(namespace, key).await?;
        if found {
            let ns_key = namespace.join("/");
            self.index.write().await.remove(&ns_key, key);
            if let Err(e) = self.flush_index().await {
                warn!("向量索引持久化失败: {e}");
            }
        }
        Ok(found)
    }

    async fn list_namespaces(&self, prefix: Option<&[&str]>) -> Result<Vec<Vec<String>>> {
        self.inner.list_namespaces(prefix).await
    }

    fn supports_semantic_search(&self) -> bool {
        true
    }

    async fn semantic_search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        let ns_key = namespace.join("/");

        // 计算查询向量，失败时回退到关键词检索
        let query_vec = match self.embedder.embed(query).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "⚠️ 查询嵌入计算失败，回退到关键词检索");
                return self.inner.search(namespace, query, limit).await;
            }
        };

        // 余弦相似度打分
        let scored: Vec<(f32, String)> = {
            let index = self.index.read().await;
            let Some(ns_vecs) = index.get_namespace(&ns_key) else {
                debug!(ns = %ns_key, "向量索引为空，回退到关键词检索");
                drop(index);
                return self.inner.search(namespace, query, limit).await;
            };
            let mut scored: Vec<(f32, String)> = ns_vecs
                .iter()
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

        // 按匹配 key 取出完整条目
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

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/")
        && let Some(home) = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
    {
        return PathBuf::from(home).join(&s[2..]);
    }
    path.to_path_buf()
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::InMemoryStore;
    use crate::testing::MockEmbedder;
    use serde_json::json;

    async fn make_store() -> EmbeddingStore {
        let inner = Arc::new(InMemoryStore::new());
        let embedder = Arc::new(MockEmbedder::new(4));
        EmbeddingStore::new(inner, embedder)
    }

    #[tokio::test]
    async fn test_supports_semantic_search() {
        let store = make_store().await;
        assert!(store.supports_semantic_search());
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

        let results = store.semantic_search(ns, "Rust", 5).await.unwrap();
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
