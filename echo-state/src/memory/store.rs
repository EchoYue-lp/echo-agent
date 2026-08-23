//! Long-term memory Store — concrete implementations
//!
//! Trait definition and data types live in [`echo_core::memory::store`].
//! This module provides [`InMemoryStore`] and [`FileStore`].

use crate::util::expand_tilde;
use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::store::{Store, StoreItem};
use echo_core::utils::fs::{ExclusiveFileLease, try_exclusive_file_lease};
use echo_core::utils::time::now_secs;
use futures::future::BoxFuture;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, RwLock};
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
        let ns_key = namespace_key_owned(&item.namespace);
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
            let ns_key = namespace_key(namespace);
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
            let ns_key = namespace_key(namespace);
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
            let ns_key = namespace_key(namespace);
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
            let ns_key = namespace_key(namespace);
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
            let prefix = prefix.map(|values| {
                values
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<_>>()
            });
            Ok(data
                .keys()
                .filter_map(|key| parse_namespace_key(key).ok())
                .filter(|namespace| {
                    prefix
                        .as_ref()
                        .is_none_or(|prefix| namespace.starts_with(prefix))
                })
                .collect())
        })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace_key(namespace);
            let data = self.data.read().await;
            Ok(data
                .get(&ns_key)
                .map(|bucket| bucket.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn prune_expired<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace_key(namespace);
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
        let ns_key = namespace_key(namespace);
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

/// JSON file-based persistent Store.
///
/// Handles opened on the same canonical path share one in-process authority.
/// Mutations persist a copy-on-write candidate before publishing it to readers,
/// and a lifetime-held sidecar lease rejects a competing process.
pub struct FileStore {
    path: PathBuf,
    authority: Arc<FileStoreAuthority>,
}

struct FileStoreAuthority {
    data: RwLock<FileStoreData>,
    transaction: AsyncMutex<()>,
    _lease: ExclusiveFileLease,
    #[cfg(test)]
    persist_failure: Mutex<Option<String>>,
}

type FileStoreData = HashMap<String, HashMap<String, StoreItem>>;

fn file_store_registry() -> &'static Mutex<HashMap<PathBuf, Weak<FileStoreAuthority>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<FileStoreAuthority>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl FileStore {
    /// Open or create the Store file, auto-create parent directories
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| MemoryError::IoError(e.to_string()))?;
        }
        let path = path
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .and_then(|parent| path.file_name().map(|name| parent.join(name)))
            .unwrap_or(path);
        let mut registry = file_store_registry().lock().map_err(|error| {
            MemoryError::IoError(format!("FileStore registry poisoned: {error}"))
        })?;
        if let Some(authority) = registry.get(&path).and_then(Weak::upgrade) {
            return Ok(Self { path, authority });
        }
        let lease = try_exclusive_file_lease(&path)
            .map_err(|error| MemoryError::IoError(format!("acquire FileStore lease: {error}")))?;
        let data = if path.exists() {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| MemoryError::IoError(e.to_string()))?;
            serde_json::from_str(&raw).map_err(|e| {
                MemoryError::SerializationError(format!("parse {}: {e}", path.display()))
            })?
        } else {
            HashMap::new()
        };
        let ns_count = data.len();
        let item_count: usize = data
            .values()
            .map(|b: &HashMap<String, StoreItem>| b.len())
            .sum();
        info!(path = %path.display(), namespaces = ns_count, items = item_count, "FileStore initialized");
        let authority = Arc::new(FileStoreAuthority {
            data: RwLock::new(data),
            transaction: AsyncMutex::new(()),
            _lease: lease,
            #[cfg(test)]
            persist_failure: Mutex::new(None),
        });
        registry.insert(path.clone(), Arc::downgrade(&authority));
        Ok(Self { path, authority })
    }

    async fn persist_candidate(&self, candidate: FileStoreData) -> Result<FileStoreData> {
        let path = self.path.clone();
        let persisted_path = path.clone();
        #[cfg(test)]
        let injected_failure = self
            .authority
            .persist_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        #[cfg(not(test))]
        let injected_failure: Option<String> = None;
        let candidate = tokio::task::spawn_blocking(move || {
            if let Some(message) = injected_failure {
                return Err(MemoryError::IoError(message));
            }
            let json = serde_json::to_vec_pretty(&candidate)
                .map_err(|error| MemoryError::SerializationError(error.to_string()))?;
            echo_core::utils::fs::atomic_write(&path, &json)
                .map_err(|error| MemoryError::IoError(error.to_string()))?;
            Ok::<FileStoreData, MemoryError>(candidate)
        })
        .await
        .map_err(|error| {
            MemoryError::IoError(format!("FileStore persistence task failed: {error}"))
        })??;
        debug!(path = %persisted_path.display(), "Store persisted");
        Ok(candidate)
    }

    async fn transact<T, F>(&self, mutate: F) -> Result<T>
    where
        T: Send,
        F: FnOnce(&mut FileStoreData) -> (T, bool) + Send,
    {
        let _transaction = self.authority.transaction.lock().await;
        let mut candidate = self.authority.data.read().await.clone();
        let (result, changed) = mutate(&mut candidate);
        if !changed {
            return Ok(result);
        }
        let committed = self.persist_candidate(candidate).await?;
        *self.authority.data.write().await = committed;
        Ok(result)
    }

    /// Batch write: write multiple records to memory then flush to disk once, reducing IO overhead.
    pub async fn put_batch(
        &self,
        entries: impl IntoIterator<Item = (Vec<&str>, &str, Value)>,
    ) -> Result<()> {
        let entries = entries
            .into_iter()
            .map(|(namespace, key, value)| {
                (
                    namespace
                        .into_iter()
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                    key.to_string(),
                    value,
                )
            })
            .collect::<Vec<_>>();
        self.transact(move |data| {
            for (namespace, key, value) in entries {
                let ns_key = namespace_key_owned(&namespace);
                let bucket = data.entry(ns_key).or_default();
                bucket
                    .entry(key.clone())
                    .and_modify(|item| {
                        item.value = value.clone();
                        item.updated_at = now_secs();
                    })
                    .or_insert_with(|| StoreItem::new(namespace, key, value));
            }
            ((), true)
        })
        .await
    }

    /// Flush in-memory data to disk.
    pub async fn flush_public(&self) -> Result<()> {
        let _transaction = self.authority.transaction.lock().await;
        let candidate = self.authority.data.read().await.clone();
        let committed = self.persist_candidate(candidate).await?;
        *self.authority.data.write().await = committed;
        Ok(())
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
            let ns_key = namespace_key(namespace);
            let ns_vec: Vec<String> = namespace.iter().map(|s| s.to_string()).collect();
            let key = key.to_string();
            self.transact(move |data| {
                let bucket = data.entry(ns_key).or_default();
                bucket
                    .entry(key.clone())
                    .and_modify(|item| {
                        item.value = value.clone();
                        item.updated_at = now_secs();
                    })
                    .or_insert_with(|| StoreItem::new(ns_vec, key, value));
                ((), true)
            })
            .await
        })
    }

    fn get<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
    ) -> BoxFuture<'a, Result<Option<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace_key(namespace);
            let data = self.authority.data.read().await;
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
            let ns_key = namespace_key(namespace);
            let data = self.authority.data.read().await;
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
            let ns_key = namespace_key(namespace);
            let key = key.to_string();
            self.transact(move |data| {
                let found = data
                    .get_mut(&ns_key)
                    .map(|b| b.remove(&key).is_some())
                    .unwrap_or(false);
                (found, found)
            })
            .await
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
        Box::pin(async move {
            let data = self.authority.data.read().await;
            let prefix = prefix.map(|values| {
                values
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<_>>()
            });
            Ok(data
                .keys()
                .filter_map(|key| parse_namespace_key(key).ok())
                .filter(|namespace| {
                    prefix
                        .as_ref()
                        .is_none_or(|prefix| namespace.starts_with(prefix))
                })
                .collect())
        })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace_key(namespace);
            let data = self.authority.data.read().await;
            Ok(data
                .get(&ns_key)
                .map(|bucket| bucket.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn prune_expired<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace_key(namespace);
        Box::pin(async move {
            self.transact(move |data| {
                let Some(bucket) = data.get_mut(&ns_key) else {
                    return (0, false);
                };
                let before = bucket.len();
                let now = now_secs();
                bucket.retain(|_k, item| is_item_valid(item, now));
                let removed = (before - bucket.len()) as u64;
                (removed, removed > 0)
            })
            .await
        })
    }

    fn dedup_by_content<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace_key(namespace);
        Box::pin(async move {
            self.transact(move |data| {
                let Some(bucket) = data.get_mut(&ns_key) else {
                    return (0, false);
                };
                let before = bucket.len();
                let mut seen: std::collections::HashMap<u64, String> =
                    std::collections::HashMap::new();
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
                (removed, removed > 0)
            })
            .await
        })
    }
}

// ── Private utility functions ───────────────────────────────────────────────────

/// Compute a simple content hash for deduplication.
fn content_hash(value: &serde_json::Value) -> u64 {
    echo_core::utils::hash::fnv1a_64(value.to_string().as_bytes())
}

pub(crate) fn namespace_key(namespace: &[&str]) -> String {
    serde_json::to_string(namespace).unwrap_or_else(|_| "[]".to_string())
}

pub(crate) fn namespace_key_owned(namespace: &[String]) -> String {
    serde_json::to_string(namespace).unwrap_or_else(|_| "[]".to_string())
}

pub(crate) fn parse_namespace_key(key: &str) -> Result<Vec<String>> {
    serde_json::from_str(key).map_err(|error| {
        MemoryError::SerializationError(format!("parse namespace key: {error}")).into()
    })
}

/// Check if a StoreItem is still valid (not expired).
/// Checks both `item.expires_at` (metadata) and `item.value["expires_at"]` (JSON).
fn is_item_valid(item: &StoreItem, now: u64) -> bool {
    // Check metadata field first
    if let Some(exp) = item.expires_at
        && exp <= now
    {
        return false;
    }
    // Also check JSON value for embedded expiry (used by MemoryPromoter)
    if let Some(exp) = item.value.get("expires_at").and_then(|v| v.as_u64())
        && exp <= now
    {
        return false;
    }
    true
}

fn tokenize(text: &str) -> Vec<String> {
    use std::collections::HashSet;
    text.split(|c: char| c.is_whitespace() || "，。！？、；：,.!?;: ".contains(c))
        .filter(|s| s.chars().count() > 1)
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

    #[test]
    fn tokenizer_filters_by_character_count() {
        assert!(tokenize("中").is_empty());
        assert_eq!(tokenize("中文"), vec!["中文".to_string()]);
    }

    #[test]
    fn file_store_rejects_corrupt_json() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-store-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).map_err(MemoryError::from)?;
        let path = root.join("store.json");
        std::fs::write(&path, b"{broken").map_err(MemoryError::from)?;
        assert!(FileStore::new(&path).is_err());
        assert_eq!(std::fs::read(&path).map_err(MemoryError::from)?, b"{broken");
        std::fs::remove_dir_all(root).map_err(MemoryError::from)?;
        Ok(())
    }

    #[tokio::test]
    async fn file_store_handles_share_one_authority() -> Result<()> {
        let root = std::env::temp_dir().join(format!("echo-store-shared-{}", uuid::Uuid::new_v4()));
        let path = root.join("store.json");
        let first = FileStore::new(&path)?;
        let second = FileStore::new(&path)?;
        first.put(&["one"], "a", json!(1)).await?;
        second.put(&["two"], "b", json!(2)).await?;
        assert!(first.get(&["two"], "b").await?.is_some());
        drop(first);
        drop(second);
        let reopened = FileStore::new(&path)?;
        assert!(reopened.get(&["one"], "a").await?.is_some());
        assert!(reopened.get(&["two"], "b").await?.is_some());
        std::fs::remove_dir_all(root).map_err(MemoryError::from)?;
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_transactions_publish_in_serial_commit_order() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("echo-store-flush-order-{}", uuid::Uuid::new_v4()));
        let path = root.join("store.json");
        let first = Arc::new(FileStore::new(&path)?);
        let second = Arc::new(FileStore::new(&path)?);
        assert!(Arc::ptr_eq(&first.authority, &second.authority));

        // Queue both copy-on-write transactions behind the same authority
        // lock. Tokio's FIFO mutex ordering makes the expected commit order
        // deterministic while neither candidate is visible before commit.
        let transaction_guard = first.authority.transaction.lock().await;
        let (old_started, old_ready) = tokio::sync::oneshot::channel();
        let old_store = Arc::clone(&first);
        let old_write = tokio::spawn(async move {
            let _ = old_started.send(());
            old_store.put(&["order"], "key", json!("old")).await
        });
        old_ready
            .await
            .map_err(|error| MemoryError::IoError(format!("old writer did not start: {error}")))?;
        tokio::task::yield_now().await;

        let (new_started, new_ready) = tokio::sync::oneshot::channel();
        let new_store = Arc::clone(&second);
        let new_write = tokio::spawn(async move {
            let _ = new_started.send(());
            new_store.put(&["order"], "key", json!("new")).await
        });
        new_ready
            .await
            .map_err(|error| MemoryError::IoError(format!("new writer did not start: {error}")))?;
        tokio::task::yield_now().await;
        assert!(first.get(&["order"], "key").await?.is_none());

        drop(transaction_guard);
        old_write
            .await
            .map_err(|error| MemoryError::IoError(format!("old write task failed: {error}")))??;
        new_write
            .await
            .map_err(|error| MemoryError::IoError(format!("new write task failed: {error}")))??;
        drop(first);
        drop(second);

        let reopened = FileStore::new(&path)?;
        assert_eq!(
            reopened
                .get(&["order"], "key")
                .await?
                .map(|item| item.value),
            Some(json!("new"))
        );
        drop(reopened);
        std::fs::remove_dir_all(root).map_err(MemoryError::from)?;
        Ok(())
    }

    #[tokio::test]
    async fn failed_persistence_never_publishes_or_leaks_into_later_commit() -> Result<()> {
        let root = std::env::temp_dir().join(format!("echo-store-atomic-{}", uuid::Uuid::new_v4()));
        let path = root.join("store.json");
        let store = FileStore::new(&path)?;
        *store
            .authority
            .persist_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some("injected persistence failure".to_string());

        assert!(
            store
                .put(&["atomic"], "failed", json!("must stay invisible"))
                .await
                .is_err()
        );
        assert!(store.get(&["atomic"], "failed").await?.is_none());
        store
            .put(&["atomic"], "committed", json!("visible"))
            .await?;
        assert!(store.get(&["atomic"], "failed").await?.is_none());
        drop(store);

        let reopened = FileStore::new(&path)?;
        assert!(reopened.get(&["atomic"], "failed").await?.is_none());
        assert_eq!(
            reopened
                .get(&["atomic"], "committed")
                .await?
                .map(|item| item.value),
            Some(json!("visible"))
        );
        drop(reopened);
        std::fs::remove_dir_all(root).map_err(MemoryError::from)?;
        Ok(())
    }

    #[tokio::test]
    async fn namespace_segments_do_not_alias_slash_content() -> Result<()> {
        let store = InMemoryStore::new();
        store.put(&["a/b"], "key", json!(1)).await?;
        store.put(&["a", "b"], "key", json!(2)).await?;
        assert_eq!(
            store.get(&["a/b"], "key").await?.map(|item| item.value),
            Some(json!(1))
        );
        assert_eq!(
            store.get(&["a", "b"], "key").await?.map(|item| item.value),
            Some(json!(2))
        );
        Ok(())
    }
}
