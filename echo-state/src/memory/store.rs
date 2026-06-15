//! Long-term memory Store — concrete implementations
//!
//! Trait definition and data types live in [`echo_core::memory::store`].
//! This module provides [`InMemoryStore`] and [`FileStore`].

use crate::util::expand_tilde;
use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::store::{Store, StoreItem};
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info};

// ── InMemoryStore ─────────────────────────────────────────────────────────────

/// In-process memory Store, no persistence, suitable for testing and short-lived use
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

    /// Insert a complete `StoreItem` directly, preserving its timestamps.
    ///
    /// Unlike `Store::put`, this method does **not** override `created_at` or
    /// `updated_at` — the caller controls the full item state. Intended for
    /// test code that needs to simulate old entries.
    pub async fn put_raw(&self, item: StoreItem) {
        let ns_key = item.namespace.join("/");
        let mut data = self.data.write().await;
        let bucket = data.entry(ns_key).or_default();
        bucket.insert(item.key.clone(), item);
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

    fn prune_expired<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace.join("/");
        Box::pin(async move {
            let mut data = self.data.write().await;
            let Some(bucket) = data.get_mut(&ns_key) else {
                return Ok(0);
            };
            let before = bucket.len();
            let now = now_secs();
            bucket.retain(|_k, item| is_item_valid(item, now));
            Ok((before - bucket.len()) as u64)
        })
    }

    fn dedup_by_content<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace.join("/");
        Box::pin(async move {
            let mut data = self.data.write().await;
            let Some(bucket) = data.get_mut(&ns_key) else {
                return Ok(0);
            };
            let before = bucket.len();
            let mut seen: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
            let mut to_remove: Vec<String> = Vec::new();
            for (key, item) in bucket.iter() {
                let hash = content_hash(&item.value);
                if let Some(existing_key) = seen.get(&hash) {
                    // Keep the newer one
                    let existing_updated =
                        bucket.get(existing_key).map(|i| i.updated_at).unwrap_or(0);
                    if item.updated_at > existing_updated {
                        to_remove.push(existing_key.clone());
                        seen.insert(hash, key.clone());
                    } else {
                        to_remove.push(key.clone());
                    }
                } else {
                    seen.insert(hash, key.clone());
                }
            }
            for key in &to_remove {
                bucket.remove(key);
            }
            Ok((before - bucket.len()) as u64)
        })
    }
}

// ── FileStore ─────────────────────────────────────────────────────────────────

/// JSON file-based persistent Store
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
        let tmp = format!("{}.tmp", self.path.display());
        // Atomic write: tmp + fsync + rename
        {
            let mut file = tokio::fs::File::create(&tmp)
                .await
                .map_err(|e| MemoryError::IoError(e.to_string()))?;
            use tokio::io::AsyncWriteExt;
            file.write_all(json.as_bytes())
                .await
                .map_err(|e| MemoryError::IoError(e.to_string()))?;
            file.sync_all()
                .await
                .map_err(|e| MemoryError::IoError(e.to_string()))?;
        }
        if let Err(e) = tokio::fs::rename(&tmp, &self.path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(MemoryError::IoError(e.to_string()).into());
        }
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

    /// Flush in-memory data to disk.
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

    fn prune_expired<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace.join("/");
        Box::pin(async move {
            let mut data = self.data.write().await;
            let Some(bucket) = data.get_mut(&ns_key) else {
                return Ok(0);
            };
            let before = bucket.len();
            let now = now_secs();
            bucket.retain(|_k, item| is_item_valid(item, now));
            let removed = (before - bucket.len()) as u64;
            if removed > 0 {
                let _ = self.flush().await;
            }
            Ok(removed)
        })
    }

    fn dedup_by_content<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace.join("/");
        Box::pin(async move {
            let mut data = self.data.write().await;
            let Some(bucket) = data.get_mut(&ns_key) else {
                return Ok(0);
            };
            let before = bucket.len();
            let mut seen: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
            let mut to_remove: Vec<String> = Vec::new();
            for (key, item) in bucket.iter() {
                let hash = content_hash(&item.value);
                if let Some(existing_key) = seen.get(&hash) {
                    let existing_updated =
                        bucket.get(existing_key).map(|i| i.updated_at).unwrap_or(0);
                    if item.updated_at > existing_updated {
                        to_remove.push(existing_key.clone());
                        seen.insert(hash, key.clone());
                    } else {
                        to_remove.push(key.clone());
                    }
                } else {
                    seen.insert(hash, key.clone());
                }
            }
            for key in &to_remove {
                bucket.remove(key);
            }
            let removed = (before - bucket.len()) as u64;
            if removed > 0 {
                let _ = self.flush().await;
            }
            Ok(removed)
        })
    }
}

// ── Private utility functions ───────────────────────────────────────────────────

/// Compute a simple content hash for deduplication.
fn content_hash(value: &serde_json::Value) -> u64 {
    echo_core::utils::hash::fnv1a_64(value.to_string().as_bytes())
}

/// Check if a StoreItem is still valid (not expired).
/// Checks both `item.expires_at` (metadata) and `item.value["expires_at"]` (JSON).
fn is_item_valid(item: &StoreItem, now: u64) -> bool {
    // Check metadata field first
    if let Some(exp) = item.expires_at {
        if exp <= now {
            return false;
        }
    }
    // Also check JSON value for embedded expiry (used by MemoryPromoter)
    if let Some(exp) = item.value.get("expires_at").and_then(|v| v.as_u64()) {
        if exp <= now {
            return false;
        }
    }
    true
}

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
    use echo_core::memory::SearchQuery;
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
