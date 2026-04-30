//! Long-term memory Store
//!
//! Data is organized as `namespace / key / value` triples, where namespace is a `&[&str]` slice
//! (e.g. `&["alice", "memories"]`), naturally supporting multi-user/multi-agent isolation.
//!
//! ## Built-in implementations
//!
//! - [`InMemoryStore`]: In-process memory, suitable for testing
//! - [`FileStore`]: JSON file persistence, zero extra dependencies
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::store::{FileStore, Store};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! let store = Arc::new(FileStore::new("~/.echo-agent/store.json")?);
//!
//! store.put(&["alice", "memories"], "pref-001", serde_json::json!({
//!     "content": "User prefers dark theme",
//!     "importance": 8
//! })).await?;
//!
//! let items = store.search(&["alice", "memories"], "theme", 5).await?;
//! println!("{} relevant memories", items.len());
//! # Ok(())
//! # }
//! ```

use crate::util::expand_tilde;
use echo_core::error::{MemoryError, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info};

// ── StoreItem ────────────────────────────────────────────────────────────────

/// A single record in the Store
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreItem {
    /// Namespace (e.g. `["user_123", "memories"]`)
    pub namespace: Vec<String>,
    /// Unique key for the item
    pub key: String,
    /// Arbitrary JSON value
    pub value: Value,
    /// Creation time (Unix seconds)
    pub created_at: u64,
    /// Last update time (Unix seconds)
    pub updated_at: u64,
    /// Relevance score from search (non-None only when returned by `search`)
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

/// Search mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    /// Keyword search only
    Keyword,
    /// Semantic search only
    Semantic,
    /// Hybrid keyword + semantic search
    Hybrid,
}

/// Unified search request
#[derive(Debug, Clone, Copy)]
pub struct SearchQuery<'a> {
    pub text: &'a str,
    pub limit: usize,
    pub mode: SearchMode,
}

impl<'a> SearchQuery<'a> {
    pub fn keyword(text: &'a str, limit: usize) -> Self {
        Self {
            text,
            limit,
            mode: SearchMode::Keyword,
        }
    }

    pub fn semantic(text: &'a str, limit: usize) -> Self {
        Self {
            text,
            limit,
            mode: SearchMode::Semantic,
        }
    }

    pub fn hybrid(text: &'a str, limit: usize) -> Self {
        Self {
            text,
            limit,
            mode: SearchMode::Hybrid,
        }
    }
}

// ── Store trait ───────────────────────────────────────────────────────────────

/// Unified storage interface for long-term memory
pub trait Store: Send + Sync {
    /// Write or update a record (upsert)
    fn put<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        value: Value,
    ) -> BoxFuture<'a, Result<()>>;

    /// Exact fetch by key
    fn get<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<StoreItem>>>;

    /// Keyword search, returns at most `limit` items (sorted by relevance)
    fn search<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>>;

    /// Unified search entry point.
    ///
    /// By default only supports keyword search; semantic/hybrid search must be explicitly overridden by concrete implementations.
    fn search_with<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: SearchQuery<'a>,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            match query.mode {
                SearchMode::Keyword => self.search(namespace, query.text, query.limit).await,
                SearchMode::Semantic => Err(MemoryError::Unsupported(
                    "semantic search is not supported by this Store".to_string(),
                )
                .into()),
                SearchMode::Hybrid => Err(MemoryError::Unsupported(
                    "hybrid search is not supported by this Store".to_string(),
                )
                .into()),
            }
        })
    }

    /// Delete the specified key, returns whether it existed and was deleted
    fn delete<'a>(&'a self, namespace: &'a [&'a str], key: &'a str) -> BoxFuture<'a, Result<bool>>;

    /// List all namespaces matching the given `prefix`
    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>>;

    /// List all entries in the namespace (no keyword filter, no pagination limit).
    ///
    /// Used for scenarios that require full enumeration (e.g. `load_all()` in a task store),
    /// avoiding infinite loops caused by empty queries matching all entries when using `search` for pagination.
    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>>;
}

// ── InMemoryStore ─────────────────────────────────────────────────────────────

/// In-process memory Store, no persistence, suitable for testing and short-lived use
///
/// # 示例
///
/// ```rust,no_run
/// use echo_core::error::Result;
/// use echo_state::memory::store::{InMemoryStore, Store};
/// use std::sync::Arc;
///
/// # async fn example() -> Result<()> {
/// let store = Arc::new(InMemoryStore::new());
/// store.put(&["ns"], "k1", serde_json::json!({"text": "hello"})).await?;
/// let item = store.get(&["ns"], "k1").await?;
/// # Ok(())
/// # }
/// ```
pub struct InMemoryStore {
    /// namespace_key -> items
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

impl Store for InMemoryStore {
    fn put<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        value: Value,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
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
        })
    }

    fn get<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let data = self.data.read().await;
            Ok(data.get(&ns_key).and_then(|b| b.get(key)).cloned())
        })
    }

    fn search<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
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
        })
    }

    fn delete<'a>(&'a self, namespace: &'a [&'a str], key: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let mut data = self.data.write().await;
            Ok(data
                .get_mut(&ns_key)
                .map(|b| b.remove(key).is_some())
                .unwrap_or(false))
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
        Box::pin(async move {
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
        })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let data = self.data.read().await;
            Ok(data
                .get(&ns_key)
                .map(|bucket| bucket.values().cloned().collect())
                .unwrap_or_default())
        })
    }
}

// ── FileStore ─────────────────────────────────────────────────────────────────

/// JSON file-based persistent Store
///
/// Storage format:
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
    /// Open or create the Store file, auto-create parent directories
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError::IoError(e.to_string()))?;
        }
        let data = if path.exists() {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| MemoryError::IoError(e.to_string()))?;
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("Store file parse failed, starting from empty state: {e}");
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
        info!(path = %path.display(), namespaces = ns_count, items = item_count, "FileStore initialized");
        Ok(Self {
            path,
            data: RwLock::new(data),
        })
    }

    async fn flush(&self) -> Result<()> {
        let data = self.data.read().await;
        let json = serde_json::to_string_pretty(&*data)
            .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
        // Atomic write: write to a temp file first then rename, avoiding data corruption from mid-write crashes
        let tmp = format!("{}.tmp", self.path.display());
        tokio::fs::write(&tmp, &json)
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .map_err(|e| MemoryError::IoError(e.to_string()))?;
        debug!(path = %self.path.display(), "Store persisted");
        Ok(())
    }

    /// Batch write: write multiple records to memory then flush to disk once, reducing IO overhead.
    pub async fn put_batch(
        &self,
        entries: impl IntoIterator<Item = (Vec<&str>, &str, Value)>,
    ) -> Result<()> {
        {
            let mut data = self.data.write().await;
            for (namespace, key, value) in entries {
                let ns_key = namespace.join("/");
                let ns_vec: Vec<String> = namespace.iter().map(|s| s.to_string()).collect();
                let bucket = data.entry(ns_key).or_default();
                bucket
                    .entry(key.to_string())
                    .and_modify(|item| {
                        item.value = value.clone();
                        item.updated_at = now_secs();
                    })
                    .or_insert_with(|| StoreItem::new(ns_vec, key.to_string(), value));
            }
        }
        self.flush().await
    }

    /// Flush in-memory data to disk. Can be used in periodic flush scenarios together with `put()`
    /// to avoid triggering disk IO on every write.
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use echo_core::error::Result;
    /// use echo_state::memory::store::{FileStore, Store};
    ///
    /// # async fn example() -> Result<()> {
    /// let store = FileStore::new("~/.echo-agent/store.json")?;
    /// store.put(&["ns"], "k1", serde_json::json!({"text": "hello"})).await?;
    /// store.put(&["ns"], "k2", serde_json::json!({"text": "world"})).await?;
    /// store.flush_public().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn flush_public(&self) -> Result<()> {
        self.flush().await
    }
}

impl Store for FileStore {
    fn put<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        value: Value,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
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
        })
    }

    fn get<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let data = self.data.read().await;
            Ok(data.get(&ns_key).and_then(|b| b.get(key)).cloned())
        })
    }

    fn search<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
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
            debug!(namespace = %ns_key, query = %query, hits = scored.len(), "Store search");
            Ok(scored
                .into_iter()
                .take(limit)
                .map(|(s, mut item)| {
                    item.score = Some(s);
                    item
                })
                .collect())
        })
    }

    fn delete<'a>(&'a self, namespace: &'a [&'a str], key: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
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
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
        Box::pin(async move {
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
        })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let data = self.data.read().await;
            Ok(data
                .get(&ns_key)
                .map(|bucket| bucket.values().cloned().collect())
                .unwrap_or_default())
        })
    }
}

// ── Private utility functions ───────────────────────────────────────────────────

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

/// Calculate the match score between a JSON Value and keywords (matched keyword count / total keyword count)
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

// ── Unit tests ────────────────────────────────────────────────────────────────────

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
            .put(ns, "k1", json!({"content": "Rust programming language"}))
            .await
            .unwrap();
        store
            .put(ns, "k2", json!({"content": "Python machine learning"}))
            .await
            .unwrap();
        store
            .put(
                ns,
                "k3",
                json!({"content": "JavaScript frontend development"}),
            )
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
        store.put(ns, "key1", json!({"count": 2})).await.unwrap(); // update

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
    fn test_store_semantic_search_default_is_unsupported() {
        let store = InMemoryStore::new();
        let err = futures::executor::block_on(
            store.search_with(&["user", "memories"], SearchQuery::semantic("Rust", 5)),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("semantic search"));
    }
}
