//! 长期记忆 Store
//!
//! 以 `namespace / key / value` 三元组组织数据，namespace 是 `&[&str]` 切片
//! （如 `&["alice", "memories"]`），天然支持多用户/多 Agent 隔离。
//!
//! ## 内置实现
//!
//! - [`InMemoryStore`]：进程内存，适合测试
//! - [`FileStore`]：JSON 文件持久化，零额外依赖
//!
//! ## 快速上手
//!
//! ```rust,no_run
//! use echo_agent::memory::store::{FileStore, Store};
//! use std::sync::Arc;
//!
//! # async fn example() -> echo_agent::error::Result<()> {
//! let store = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
//!
//! store.put(&["alice", "memories"], "pref-001", serde_json::json!({
//!     "content": "用户偏好深色主题",
//!     "importance": 8
//! })).await?;
//!
//! let items = store.search(&["alice", "memories"], "主题", 5).await?;
//! println!("{} 条相关记忆", items.len());
//! # Ok(())
//! # }
//! ```

use crate::error::{MemoryError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info};

// ── StoreItem ────────────────────────────────────────────────────────────────

/// Store 中的单条记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreItem {
    /// 命名空间（如 `["user_123", "memories"]`）
    pub namespace: Vec<String>,
    /// 条目唯一键
    pub key: String,
    /// 任意 JSON 值
    pub value: Value,
    /// 创建时间（Unix 秒）
    pub created_at: u64,
    /// 最后更新时间（Unix 秒）
    pub updated_at: u64,
    /// 检索相关度分数（仅 `search` 返回时非 None）
    pub score: Option<f32>,
}

impl StoreItem {
    fn new(namespace: Vec<String>, key: String, value: Value) -> Self {
        let now = now_secs();
        Self {
            namespace,
            key,
            value,
            created_at: now,
            updated_at: now,
            score: None,
        }
    }
}

// ── Store trait ───────────────────────────────────────────────────────────────

/// 长期记忆的统一存储接口
#[async_trait]
pub trait Store: Send + Sync {
    /// 写入或更新一条记录（upsert）
    async fn put(&self, namespace: &[&str], key: &str, value: Value) -> Result<()>;

    /// 按 key 精确获取
    async fn get(&self, namespace: &[&str], key: &str) -> Result<Option<StoreItem>>;

    /// 关键词检索，返回最多 `limit` 条（按相关度排序）
    async fn search(&self, namespace: &[&str], query: &str, limit: usize)
    -> Result<Vec<StoreItem>>;

    /// 删除指定 key，返回是否存在并删除
    async fn delete(&self, namespace: &[&str], key: &str) -> Result<bool>;

    /// 列举满足 `prefix` 前缀的所有命名空间
    async fn list_namespaces(&self, prefix: Option<&[&str]>) -> Result<Vec<Vec<String>>>;

    /// 是否支持语义（向量）搜索。[`EmbeddingStore`](super::EmbeddingStore) 返回 `true`，其余返回 `false`。
    fn supports_semantic_search(&self) -> bool {
        false
    }

    /// 语义检索：若实现类支持向量搜索则执行余弦相似度检索，否则回退到关键词 [`search`](Store::search)。
    async fn semantic_search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        self.search(namespace, query, limit).await
    }
}

// ── InMemoryStore ─────────────────────────────────────────────────────────────

/// 进程内存 Store，不持久化，适合测试和短生命周期使用
///
/// # 示例
///
/// ```rust,no_run
/// use echo_agent::memory::store::{InMemoryStore, Store};
/// use std::sync::Arc;
///
/// # async fn example() -> echo_agent::error::Result<()> {
/// let store = Arc::new(InMemoryStore::new());
/// store.put(&["ns"], "k1", serde_json::json!({"text": "hello"})).await?;
/// let item = store.get(&["ns"], "k1").await?;
/// # Ok(())
/// # }
/// ```
pub struct InMemoryStore {
    /// namespace_key → items
    data: RwLock<HashMap<String, HashMap<String, StoreItem>>>,
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn put(&self, namespace: &[&str], key: &str, value: Value) -> Result<()> {
        let ns_key = namespace.join("/");
        let mut data = self.data.write().await;
        let bucket = data.entry(ns_key).or_default();
        bucket
            .entry(key.to_string())
            .and_modify(|item| {
                item.value = value.clone();
                item.updated_at = now_secs();
            })
            .or_insert_with(|| {
                StoreItem::new(
                    namespace.iter().map(|s| s.to_string()).collect(),
                    key.to_string(),
                    value,
                )
            });
        Ok(())
    }

    async fn get(&self, namespace: &[&str], key: &str) -> Result<Option<StoreItem>> {
        let ns_key = namespace.join("/");
        let data = self.data.read().await;
        Ok(data.get(&ns_key).and_then(|b| b.get(key)).cloned())
    }

    async fn search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        let ns_key = namespace.join("/");
        let data = self.data.read().await;
        let Some(bucket) = data.get(&ns_key) else {
            return Ok(vec![]);
        };
        let keywords = tokenize(query);
        let mut scored: Vec<(f32, StoreItem)> = bucket
            .values()
            .filter_map(|item| {
                let score = value_relevance_score(&item.value, &keywords);
                if score > 0.0 {
                    Some((score, item.clone()))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(s, mut item)| {
                item.score = Some(s);
                item
            })
            .collect())
    }

    async fn delete(&self, namespace: &[&str], key: &str) -> Result<bool> {
        let ns_key = namespace.join("/");
        let mut data = self.data.write().await;
        Ok(data
            .get_mut(&ns_key)
            .map(|b| b.remove(key).is_some())
            .unwrap_or(false))
    }

    async fn list_namespaces(&self, prefix: Option<&[&str]>) -> Result<Vec<Vec<String>>> {
        let data = self.data.read().await;
        let prefix_str = prefix.map(|p| p.join("/"));
        Ok(data
            .keys()
            .filter(|k| {
                prefix_str
                    .as_deref()
                    .map(|p| k.starts_with(p))
                    .unwrap_or(true)
            })
            .map(|k| k.split('/').map(String::from).collect())
            .collect())
    }
}

// ── FileStore ─────────────────────────────────────────────────────────────────

/// 基于 JSON 文件的持久化 Store
///
/// 存储格式：
/// ```json
/// {
///   "user_123/memories": {
///     "key1": { "namespace": [...], "key": "key1", "value": {...}, "created_at": 123, "updated_at": 456, "score": null }
///   }
/// }
/// ```
pub struct FileStore {
    path: PathBuf,
    data: RwLock<HashMap<String, HashMap<String, StoreItem>>>,
}

impl FileStore {
    /// 打开或创建 Store 文件，自动建父目录
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MemoryError::IoError(format!("创建目录失败: {e}")))?;
        }
        let data = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| MemoryError::IoError(format!("读取 store 文件失败: {e}")))?;
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("Store 文件解析失败，从空状态开始: {e}");
                HashMap::new()
            })
        } else {
            HashMap::new()
        };
        let ns_count = data.len();
        let item_count: usize = data
            .values()
            .map(|b: &HashMap<String, StoreItem>| b.len())
            .sum();
        info!(path = %path.display(), namespaces = ns_count, items = item_count, "🗄️ FileStore 初始化");
        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    async fn flush(&self) -> Result<()> {
        let data = self.data.read().await;
        let json = serde_json::to_string_pretty(&*data)
            .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
        tokio::fs::write(&self.path, json)
            .await
            .map_err(|e| MemoryError::IoError(format!("写入 store 文件失败: {e}")))?;
        debug!(path = %self.path.display(), "💾 Store 已持久化");
        Ok(())
    }
}

#[async_trait]
impl Store for FileStore {
    async fn put(&self, namespace: &[&str], key: &str, value: Value) -> Result<()> {
        let ns_key = namespace.join("/");
        let ns_vec: Vec<String> = namespace.iter().map(|s| s.to_string()).collect();
        {
            let mut data = self.data.write().await;
            let bucket = data.entry(ns_key).or_default();
            bucket
                .entry(key.to_string())
                .and_modify(|item| {
                    item.value = value.clone();
                    item.updated_at = now_secs();
                })
                .or_insert_with(|| StoreItem::new(ns_vec, key.to_string(), value));
        }
        self.flush().await
    }

    async fn get(&self, namespace: &[&str], key: &str) -> Result<Option<StoreItem>> {
        let ns_key = namespace.join("/");
        let data = self.data.read().await;
        Ok(data.get(&ns_key).and_then(|b| b.get(key)).cloned())
    }

    async fn search(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        let ns_key = namespace.join("/");
        let data = self.data.read().await;
        let Some(bucket) = data.get(&ns_key) else {
            return Ok(vec![]);
        };
        let keywords = tokenize(query);
        let mut scored: Vec<(f32, StoreItem)> = bucket
            .values()
            .filter_map(|item| {
                let score = value_relevance_score(&item.value, &keywords);
                if score > 0.0 {
                    Some((score, item.clone()))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        debug!(namespace = %ns_key, query = %query, hits = scored.len(), "🔍 Store 检索");
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(s, mut item)| {
                item.score = Some(s);
                item
            })
            .collect())
    }

    async fn delete(&self, namespace: &[&str], key: &str) -> Result<bool> {
        let ns_key = namespace.join("/");
        let found = {
            let mut data = self.data.write().await;
            data.get_mut(&ns_key)
                .map(|b| b.remove(key).is_some())
                .unwrap_or(false)
        };
        if found {
            self.flush().await?;
        }
        Ok(found)
    }

    async fn list_namespaces(&self, prefix: Option<&[&str]>) -> Result<Vec<Vec<String>>> {
        let data = self.data.read().await;
        let prefix_str = prefix.map(|p| p.join("/"));
        Ok(data
            .keys()
            .filter(|k| {
                prefix_str
                    .as_deref()
                    .map(|p| k.starts_with(p))
                    .unwrap_or(true)
            })
            .map(|k| k.split('/').map(String::from).collect())
            .collect())
    }
}

// ── 私有工具函数 ──────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn tokenize(text: &str) -> Vec<String> {
    use std::collections::HashSet;
    text.split(|c: char| c.is_whitespace() || "，。！？、；：,.!?;: ".contains(c))
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(|s| s.to_lowercase())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

/// 计算 JSON Value 与关键词的匹配度（匹配关键词数 / 总关键词数）
fn value_relevance_score(value: &Value, keywords: &[String]) -> f32 {
    if keywords.is_empty() {
        return 1.0;
    }
    let text = value_to_searchable_text(value).to_lowercase();
    let matched = keywords
        .iter()
        .filter(|kw| text.contains(kw.as_str()))
        .count();
    if matched == 0 {
        0.0
    } else {
        matched as f32 / keywords.len() as f32
    }
}

fn value_to_searchable_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .map(value_to_searchable_text)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(map) => map
            .values()
            .map(value_to_searchable_text)
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

// ── 单元测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_in_memory_store_put_and_get() {
        let store = InMemoryStore::new();
        let ns = &["user", "memories"];

        store
            .put(ns, "key1", json!({"data": "value1"}))
            .await
            .unwrap();
        store
            .put(ns, "key2", json!({"data": "value2"}))
            .await
            .unwrap();

        let item1 = store.get(ns, "key1").await.unwrap();
        assert!(item1.is_some());
        assert_eq!(item1.unwrap().value["data"], "value1");

        let item2 = store.get(ns, "key2").await.unwrap();
        assert!(item2.is_some());
    }

    #[tokio::test]
    async fn test_in_memory_store_get_nonexistent() {
        let store = InMemoryStore::new();
        let ns = &["user", "memories"];

        let item = store.get(ns, "nonexistent").await.unwrap();
        assert!(item.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_store_delete() {
        let store = InMemoryStore::new();
        let ns = &["user", "memories"];

        store
            .put(ns, "key1", json!({"data": "value1"}))
            .await
            .unwrap();

        let deleted = store.delete(ns, "key1").await.unwrap();
        assert!(deleted);

        let item = store.get(ns, "key1").await.unwrap();
        assert!(item.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_store_delete_nonexistent() {
        let store = InMemoryStore::new();
        let ns = &["user", "memories"];

        let deleted = store.delete(ns, "nonexistent").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_in_memory_store_search() {
        let store = InMemoryStore::new();
        let ns = &["user", "memories"];

        store
            .put(ns, "k1", json!({"content": "Rust 编程语言"}))
            .await
            .unwrap();
        store
            .put(ns, "k2", json!({"content": "Python 机器学习"}))
            .await
            .unwrap();
        store
            .put(ns, "k3", json!({"content": "JavaScript 前端开发"}))
            .await
            .unwrap();

        let results = store.search(ns, "Rust", 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].score.is_some());
    }

    #[tokio::test]
    async fn test_in_memory_store_list_namespaces() {
        let store = InMemoryStore::new();

        store
            .put(&["user1", "memories"], "k1", json!({}))
            .await
            .unwrap();
        store
            .put(&["user2", "memories"], "k2", json!({}))
            .await
            .unwrap();
        store
            .put(&["user1", "settings"], "k3", json!({}))
            .await
            .unwrap();

        let namespaces = store.list_namespaces(None).await.unwrap();
        assert_eq!(namespaces.len(), 3);

        let user1_ns = store.list_namespaces(Some(&["user1"])).await.unwrap();
        assert_eq!(user1_ns.len(), 2);
    }

    #[tokio::test]
    async fn test_in_memory_store_upsert() {
        let store = InMemoryStore::new();
        let ns = &["user", "memories"];

        store.put(ns, "key1", json!({"count": 1})).await.unwrap();
        store.put(ns, "key1", json!({"count": 2})).await.unwrap(); // 更新

        let item = store.get(ns, "key1").await.unwrap().unwrap();
        assert_eq!(item.value["count"], 2);
    }

    #[tokio::test]
    async fn test_in_memory_store_namespace_isolation() {
        let store = InMemoryStore::new();

        store
            .put(&["ns1"], "key", json!({"value": "ns1"}))
            .await
            .unwrap();
        store
            .put(&["ns2"], "key", json!({"value": "ns2"}))
            .await
            .unwrap();

        let item1 = store.get(&["ns1"], "key").await.unwrap().unwrap();
        let item2 = store.get(&["ns2"], "key").await.unwrap().unwrap();

        assert_eq!(item1.value["value"], "ns1");
        assert_eq!(item2.value["value"], "ns2");
    }

    #[test]
    fn test_store_item_new() {
        let item = StoreItem::new(
            vec!["user".to_string(), "memories".to_string()],
            "key1".to_string(),
            json!({"data": "value"}),
        );

        assert_eq!(item.namespace, vec!["user", "memories"]);
        assert_eq!(item.key, "key1");
        assert_eq!(item.value["data"], "value");
        assert!(item.score.is_none());
        assert!(item.created_at > 0);
        assert_eq!(item.created_at, item.updated_at);
    }

    #[test]
    fn test_store_supports_semantic_search_default() {
        let store = InMemoryStore::new();
        assert!(!store.supports_semantic_search());
    }
}
