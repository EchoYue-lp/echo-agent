//! Long-term memory Store trait and data types
//!
//! Data is organized as `namespace / key / value` triples, where namespace is a `&[&str]` slice
//! (e.g. `&["alice", "memories"]`), naturally supporting multi-user/multi-agent isolation.
//!
//! Concrete implementations (`InMemoryStore`, `FileStore`) live in `echo_state`.

use crate::error::{MemoryError, Result};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(default)]
    pub score: Option<f32>,
    /// Importance score (1.0–10.0, default 5.0). Higher = less likely to be pruned.
    #[serde(default = "default_importance")]
    pub importance: f32,
    /// Last access timestamp (Unix seconds). Used for decay/pruning decisions.
    #[serde(default)]
    pub last_accessed: Option<u64>,
    /// Optional expiration time (Unix seconds). Items past this time are
    /// candidates for pruning. `None` means the item never expires.
    #[serde(default)]
    pub expires_at: Option<u64>,
}

fn default_importance() -> f32 {
    5.0
}

impl StoreItem {
    /// Create a new StoreItem with current timestamp
    pub fn new(namespace: Vec<String>, key: String, value: Value) -> Self {
        let now = crate::utils::time::now_secs();
        Self {
            namespace,
            key,
            value,
            created_at: now,
            updated_at: now,
            score: None,
            importance: 5.0,
            last_accessed: None,
            expires_at: None,
        }
    }

    /// Create a StoreItem with explicit importance.
    pub fn with_importance(
        namespace: Vec<String>,
        key: String,
        value: Value,
        importance: f32,
    ) -> Self {
        let mut item = Self::new(namespace, key, value);
        item.importance = importance.clamp(1.0, 10.0);
        item
    }

    /// Create a StoreItem with a TTL (time-to-live) in seconds from now.
    pub fn with_ttl(namespace: Vec<String>, key: String, value: Value, ttl_secs: u64) -> Self {
        let mut item = Self::new(namespace, key, value);
        item.expires_at = Some(item.created_at.saturating_add(ttl_secs));
        item
    }

    /// Mark this item as accessed (updates `last_accessed`).
    pub fn touch(&mut self) {
        self.last_accessed = Some(crate::utils::time::now_secs());
    }
}

/// Result of a prune/dedup operation.
#[derive(Debug, Clone, Default)]
pub struct PruneResult {
    /// Number of items removed.
    pub removed: u64,
    /// Number of items checked.
    pub checked: u64,
}

/// Search mode
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SearchMode {
    /// Keyword search only
    Keyword,
    /// Semantic search only
    Semantic,
    /// Hybrid keyword + semantic search with configurable vector weight.
    /// `weight` is the vector-search contribution (0.0 = pure keyword, 1.0 = pure semantic, default 0.5).
    Hybrid {
        /// Weight of vector/semantic search in the final score (0.0–1.0).
        /// Keyword weight is `1.0 - vector_weight`.
        vector_weight: f32,
    },
}

/// Default RRF constant k (standard value is 60)
pub const RRF_K: f64 = 60.0;

/// Compute Reciprocal Rank Fusion score from two ranked result sets.
///
/// `vector_rank` and `keyword_rank` are 0-based positions in their respective
/// result lists (use `usize::MAX` for items not present in a list).
/// `vector_weight` controls the relative contribution.
pub fn rrf_score(vector_rank: usize, keyword_rank: usize, vector_weight: f32) -> f32 {
    let vw = vector_weight as f64;
    let kw = 1.0 - vw;
    let vr = if vector_rank == usize::MAX {
        0.0
    } else {
        vw / (RRF_K + vector_rank as f64)
    };
    let kr = if keyword_rank == usize::MAX {
        0.0
    } else {
        kw / (RRF_K + keyword_rank as f64)
    };
    (vr + kr) as f32
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
            mode: SearchMode::Hybrid { vector_weight: 0.5 },
        }
    }

    /// Hybrid search with custom vector weight (0.0 = pure keyword, 1.0 = pure semantic).
    pub fn hybrid_weighted(text: &'a str, limit: usize, vector_weight: f32) -> Self {
        Self {
            text,
            limit,
            mode: SearchMode::Hybrid { vector_weight },
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
                SearchMode::Hybrid { .. } => Err(MemoryError::Unsupported(
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
    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>>;

    /// Remove expired items from a namespace.
    ///
    /// Returns the number of items removed. Default implementation is a no-op.
    fn prune_expired<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let _ = namespace;
        Box::pin(async move { Ok(0) })
    }

    /// Deduplicate items in a namespace by content hash.
    ///
    /// Groups items by content fingerprint and keeps only the most recent one.
    /// Returns the number of items removed. Default implementation is a no-op.
    fn dedup_by_content<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let _ = namespace;
        Box::pin(async move { Ok(0) })
    }
}
