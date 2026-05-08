//! Long-term memory Store trait and data types
//!
//! Data is organized as `namespace / key / value` triples, where namespace is a `&[&str]` slice
//! (e.g. `&["alice", "memories"]`), naturally supporting multi-user/multi-agent isolation.
//!
//! Concrete implementations ([`InMemoryStore`], [`FileStore`]) live in `echo_state`.

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
    pub score: Option<f32>,
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
    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>>;
}
