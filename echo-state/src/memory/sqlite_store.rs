//! SQLite persistent Store (FTS5 full-text search + optional vector search)
//!
//! Production-grade persistent storage, based on SQLite + FTS5 full-text search engine,
//! with optional vector similarity retrieval integration.
//!
//! ## Storage structure
//!
//! | Table | Purpose |
//! |----|------|
//! | `store_items` | KV main table (namespace, key, value, timestamps) |
//! | `store_fts` | FTS5 full-text index (auto-synced) |
//! | `store_vectors` | Vector table (optional, stores embedding vectors) |
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use echo_core::error::Result;
//! use echo_state::memory::store::Store;
//! use echo_state::memory::SqliteStore;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<()> {
//! // Basic usage: FTS5 full-text search
//! let store = Arc::new(SqliteStore::new("~/.echo-agent/memory.db")?);
//!
//! store.put(&["alice", "memories"], "pref-001", serde_json::json!({
//!     "content": "User prefers dark theme",
//!     "importance": 8
//! })).await?;
//!
//! // FTS5 full-text search
//! let items = store.search(&["alice", "memories"], "dark theme", 5).await?;
//!
//! // With hybrid search (requires Embedder)
//! use echo_state::memory::{Embedder, HttpEmbedder};
//! let embedder: Arc<dyn Embedder> = Arc::new(HttpEmbedder::from_env());
//! let store = Arc::new(SqliteStore::with_embedder("~/.echo-agent/memory.db", embedder)?);
//! let items = store
//!     .search_with(&["alice", "memories"], echo_state::memory::SearchQuery::hybrid("theme preference", 5))
//!     .await?;
//! # Ok(())
//! # }
//! ```

use crate::util::{expand_tilde, memory_io_error};
use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::embedder::Embedder;
pub use echo_core::memory::store::{SearchMode, SearchQuery, Store, StoreItem};
use futures::future::BoxFuture;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

// ── SqliteStore ─────────────────────────────────────────────────────────────

/// SQLite persistent Store with FTS5 full-text search and optional vector search
pub struct SqliteStore {
    embedder: Option<Arc<dyn Embedder>>,
    path: PathBuf,
}

impl SqliteStore {
    /// Open or create a SQLite database, auto-create tables and FTS5 index
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(path, None)
    }

    /// Open or create a SQLite database with vector search enabled
    pub fn with_embedder(path: impl AsRef<Path>, embedder: Arc<dyn Embedder>) -> Result<Self> {
        Self::open(path, Some(embedder))
    }

    fn open(path: impl AsRef<Path>, embedder: Option<Arc<dyn Embedder>>) -> Result<Self> {
        let path = expand_tilde(path.as_ref());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| memory_io_error("failed to create directory", e))?;
        }

        let conn = Self::open_connection_at(&path)?;

        Self::init_tables(&conn)?;

        let item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM store_items", [], |row| row.get(0))
            .unwrap_or(0);

        info!(
            path = %path.display(),
            items = item_count,
            vector = embedder.is_some(),
            "SqliteStore initialized"
        );

        Ok(Self { embedder, path })
    }

    fn open_connection_at(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)
            .map_err(|e| memory_io_error("failed to open SQLite database", e))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size=10000;
             PRAGMA temp_store=MEMORY;",
        )
        .map_err(|e| memory_io_error("SQLite PRAGMA settings failed", e))?;
        Ok(conn)
    }

    fn open_connection(&self) -> Result<Connection> {
        Self::open_connection_at(&self.path)
    }

    fn init_tables(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS store_items (
                namespace TEXT NOT NULL,
                key       TEXT NOT NULL,
                value     TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (namespace, key)
            );

            CREATE INDEX IF NOT EXISTS idx_store_ns ON store_items(namespace);",
        )
        .map_err(|e| memory_io_error("failed to create main table", e))?;

        // FTS5 full-text index (content= external content table mode is not suitable for this scenario; use independent table)
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS store_fts USING fts5(
                namespace,
                key,
                content,
                tokenize='unicode61'
            );",
        )
        .map_err(|e| memory_io_error("failed to create FTS5 index", e))?;

        // Vector table (optional, for cosine similarity retrieval)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS store_vectors (
                namespace TEXT NOT NULL,
                key       TEXT NOT NULL,
                vector    BLOB NOT NULL,
                PRIMARY KEY (namespace, key)
            );",
        )
        .map_err(|e| memory_io_error("failed to create vector table", e))?;

        Ok(())
    }

    /// Extract searchable text from a JSON Value
    fn extract_searchable_text(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Object(map) => {
                // Prefer the content field
                if let Some(content) = map.get("content").and_then(|v| v.as_str()) {
                    let mut text = content.to_string();
                    // Append tags
                    if let Some(tags) = map.get("tags").and_then(|v| v.as_array()) {
                        for tag in tags {
                            if let Some(t) = tag.as_str() {
                                text.push(' ');
                                text.push_str(t);
                            }
                        }
                    }
                    return text;
                }
                map.values()
                    .map(Self::extract_searchable_text)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            Value::Array(arr) => arr
                .iter()
                .map(Self::extract_searchable_text)
                .collect::<Vec<_>>()
                .join(" "),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
        }
    }

    /// Serialize an f32 vector to bytes (little-endian)
    fn vec_to_bytes(vec: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(vec.len() * 4);
        for &v in vec {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    /// Deserialize bytes back to an f32 vector
    fn bytes_to_vec(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    /// Cosine similarity
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
}

impl SqliteStore {
    /// Fetch complete items from the main table by key list (unified score)
    fn fetch_items(
        &self,
        conn: &Connection,
        namespace: &[&str],
        ns_key: &str,
        keys: &[String],
        default_score: Option<f32>,
    ) -> Result<Vec<StoreItem>> {
        let mut results = Vec::with_capacity(keys.len());
        for (i, key) in keys.iter().enumerate() {
            let row = conn.query_row(
                "SELECT value, created_at, updated_at FROM store_items
                 WHERE namespace = ?1 AND key = ?2",
                params![ns_key, key],
                |row| {
                    let value_str: String = row.get(0)?;
                    let created_at: i64 = row.get(1)?;
                    let updated_at: i64 = row.get(2)?;
                    Ok((value_str, created_at, updated_at))
                },
            );
            if let Ok((value_str, created_at, updated_at)) = row
                && let Ok(value) = serde_json::from_str::<Value>(&value_str)
            {
                results.push(StoreItem {
                    namespace: namespace.iter().map(|s| s.to_string()).collect(),
                    key: key.clone(),
                    value,
                    created_at: created_at as u64,
                    updated_at: updated_at as u64,
                    score: default_score.or(Some(1.0 / (i as f32 + 1.0))),
                });
            }
        }
        Ok(results)
    }

    /// Fetch complete items from the main table by (key, score) list
    fn fetch_items_with_scores(
        &self,
        conn: &Connection,
        namespace: &[&str],
        ns_key: &str,
        keys_with_scores: &[(String, Option<f32>)],
    ) -> Result<Vec<StoreItem>> {
        let mut results = Vec::with_capacity(keys_with_scores.len());
        for (key, score) in keys_with_scores {
            let row = conn.query_row(
                "SELECT value, created_at, updated_at FROM store_items
                 WHERE namespace = ?1 AND key = ?2",
                params![ns_key, key],
                |row| {
                    let value_str: String = row.get(0)?;
                    let created_at: i64 = row.get(1)?;
                    let updated_at: i64 = row.get(2)?;
                    Ok((value_str, created_at, updated_at))
                },
            );
            if let Ok((value_str, created_at, updated_at)) = row
                && let Ok(value) = serde_json::from_str::<Value>(&value_str)
            {
                results.push(StoreItem {
                    namespace: namespace.iter().map(|s| s.to_string()).collect(),
                    key: key.clone(),
                    value,
                    created_at: created_at as u64,
                    updated_at: updated_at as u64,
                    score: *score,
                });
            }
        }
        Ok(results)
    }

    async fn semantic_search_impl(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        let Some(ref embedder) = self.embedder else {
            return Err(MemoryError::Unsupported(
                "semantic search requires an embedder-backed SqliteStore".to_string(),
            )
            .into());
        };

        let ns_key = namespace.join("/");

        let query_vec = match embedder.embed(query).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Query embedding calculation failed, falling back to FTS5 search");
                return self.search(namespace, query, limit).await;
            }
        };

        let scored = {
            let conn = self.open_connection()?;
            let mut stmt = conn
                .prepare("SELECT key, vector FROM store_vectors WHERE namespace = ?1")
                .map_err(|e| memory_io_error("failed to query vector table", e))?;

            let mut scored: Vec<(f32, String)> = stmt
                .query_map(params![ns_key], |row| {
                    let key: String = row.get(0)?;
                    let bytes: Vec<u8> = row.get(1)?;
                    Ok((key, bytes))
                })
                .map_err(|e| memory_io_error("failed to query vectors", e))?
                .filter_map(|r| r.ok())
                .map(|(key, bytes)| {
                    let vec = Self::bytes_to_vec(&bytes);
                    let score = Self::cosine_similarity(&query_vec, &vec);
                    (score, key)
                })
                .collect();

            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(limit);
            scored
        };

        if scored.is_empty() {
            debug!(ns = %ns_key, "Vector index is empty, falling back to FTS5 search");
            return self.search(namespace, query, limit).await;
        }

        debug!(ns = %ns_key, query = %query, hits = scored.len(), "Semantic search completed");

        let conn = self.open_connection()?;
        let mut results = Vec::with_capacity(scored.len());
        for (score, key) in scored {
            let row = conn.query_row(
                "SELECT value, created_at, updated_at FROM store_items
                 WHERE namespace = ?1 AND key = ?2",
                params![ns_key, key],
                |row| {
                    let value_str: String = row.get(0)?;
                    let created_at: i64 = row.get(1)?;
                    let updated_at: i64 = row.get(2)?;
                    Ok((value_str, created_at, updated_at))
                },
            );

            if let Ok((value_str, created_at, updated_at)) = row
                && let Ok(value) = serde_json::from_str::<Value>(&value_str)
            {
                results.push(StoreItem {
                    namespace: namespace.iter().map(|s| s.to_string()).collect(),
                    key,
                    value,
                    created_at: created_at as u64,
                    updated_at: updated_at as u64,
                    score: Some(score),
                });
            }
        }

        Ok(results)
    }
}

impl Store for SqliteStore {
    fn put<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        value: Value,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let value_json = serde_json::to_string(&value)
                .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
            let search_text = Self::extract_searchable_text(&value);
            let now = now_secs() as i64;

            {
                let mut conn = self.open_connection()?;

                // Wrap main table write, FTS update, and vector table update in a transaction.
                // If any step fails, the whole transaction rolls back.
                let tx = conn
                    .transaction()
                    .map_err(|e| memory_io_error("failed to begin transaction", e))?;

                // Upsert into main table
                tx.execute(
                    "INSERT INTO store_items (namespace, key, value, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)
                     ON CONFLICT(namespace, key) DO UPDATE SET
                        value = excluded.value,
                        updated_at = excluded.updated_at",
                    params![ns_key, key, value_json, now],
                )
                .map_err(|e| memory_io_error("failed to write to main table", e))?;

                // Update FTS5 index (delete then insert)
                tx.execute(
                    "DELETE FROM store_fts WHERE namespace = ?1 AND key = ?2",
                    params![ns_key, key],
                )
                .map_err(|e| memory_io_error("failed to delete FTS index", e))?;

                tx.execute(
                    "INSERT INTO store_fts (namespace, key, content) VALUES (?1, ?2, ?3)",
                    params![ns_key, key, search_text],
                )
                .map_err(|e| memory_io_error("failed to write FTS index", e))?;

                tx.commit()
                    .map_err(|e| memory_io_error("failed to commit transaction", e))?;
            }

            // Vector index update (if embedder is available)
            if let Some(ref embedder) = self.embedder {
                match embedder.embed(&search_text).await {
                    Ok(vec) => {
                        let bytes = Self::vec_to_bytes(&vec);
                        let conn = self.open_connection()?;
                        conn.execute(
                            "INSERT INTO store_vectors (namespace, key, vector)
                             VALUES (?1, ?2, ?3)
                             ON CONFLICT(namespace, key) DO UPDATE SET vector = excluded.vector",
                            params![ns_key, key, bytes],
                        )
                        .map_err(|e| memory_io_error("failed to write to vector table", e))?;
                        debug!(ns = %ns_key, key = %key, dims = vec.len(), "Vector index updated");
                    }
                    Err(e) => {
                        warn!(key = %key, error = %e, "Embedding calculation failed, item will not be added to vector index");
                    }
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
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let conn = self.open_connection()?;

            let result = conn.query_row(
                "SELECT value, created_at, updated_at FROM store_items
                 WHERE namespace = ?1 AND key = ?2",
                params![ns_key, key],
                |row| {
                    let value_str: String = row.get(0)?;
                    let created_at: i64 = row.get(1)?;
                    let updated_at: i64 = row.get(2)?;
                    Ok((value_str, created_at, updated_at))
                },
            );

            match result {
                Ok((value_str, created_at, updated_at)) => {
                    let value: Value = serde_json::from_str(&value_str)
                        .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
                    Ok(Some(StoreItem {
                        namespace: namespace.iter().map(|s| s.to_string()).collect(),
                        key: key.to_string(),
                        value,
                        created_at: created_at as u64,
                        updated_at: updated_at as u64,
                        score: None,
                    }))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(memory_io_error("query failed", e).into()),
            }
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
            let conn = self.open_connection()?;

            // FTS5 query syntax: join space-separated words with OR
            let keywords: Vec<&str> = query
                .split(|c: char| c.is_whitespace() || "，。！？、；：,.!?;:".contains(c))
                .filter(|s| !s.is_empty())
                .collect();

            if keywords.is_empty() {
                return Ok(vec![]);
            }

            let fts_query = keywords
                .iter()
                .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR ");

            // First try FTS5 MATCH search
            let matched_keys: Vec<(String, f64)> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT f.key, bm25(store_fts) as score
                         FROM store_fts f
                         WHERE f.namespace = ?1 AND store_fts MATCH ?2
                         ORDER BY score
                         LIMIT ?3",
                    )
                    .map_err(|e| memory_io_error("FTS query preparation failed", e))?;

                stmt.query_map(params![ns_key, fts_query, limit as i64], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(|e| memory_io_error("FTS query failed", e))?
                .filter_map(|r| r.ok())
                .collect()
            };

            // When FTS5 returns no results, fall back to LIKE fuzzy matching (suitable for CJK and other non-Latin scripts).
            // For multi-keyword queries, match individual keywords separately rather than requiring the original text to contain the entire contiguous substring.
            if matched_keys.is_empty() {
                debug!(namespace = %ns_key, query = %query, "FTS5 returned no results, falling back to LIKE fuzzy matching");
                let mut fallback_keys = Vec::new();
                for keyword in &keywords {
                    let like_pattern = format!("%{}%", keyword.replace('%', "\\%"));
                    let mut stmt = conn
                        .prepare(
                            "SELECT f.key FROM store_fts f
                             WHERE f.namespace = ?1 AND f.content LIKE ?2
                             LIMIT ?3",
                        )
                        .map_err(|e| memory_io_error("LIKE query preparation failed", e))?;

                    let current_limit = (limit.saturating_sub(fallback_keys.len())).max(1) as i64;
                    let keyword_keys = stmt
                        .query_map(params![ns_key, like_pattern, current_limit], |row| {
                            row.get::<_, String>(0)
                        })
                        .map_err(|e| memory_io_error("LIKE query failed", e))?
                        .filter_map(|r| r.ok());

                    for key in keyword_keys {
                        if !fallback_keys.iter().any(|existing| existing == &key) {
                            fallback_keys.push(key);
                            if fallback_keys.len() >= limit {
                                break;
                            }
                        }
                    }

                    if fallback_keys.len() >= limit {
                        break;
                    }
                }

                return self.fetch_items(&conn, namespace, &ns_key, &fallback_keys, None);
            }

            debug!(
                namespace = %ns_key,
                query = %query,
                hits = matched_keys.len(),
                "FTS5 search"
            );

            let keys_with_scores: Vec<(String, Option<f32>)> = matched_keys
                .into_iter()
                .map(|(k, s)| (k, Some(-s as f32)))
                .collect();

            self.fetch_items_with_scores(&conn, namespace, &ns_key, &keys_with_scores)
        })
    }

    fn delete<'a>(&'a self, namespace: &'a [&'a str], key: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let conn = self.open_connection()?;

            let affected = conn
                .execute(
                    "DELETE FROM store_items WHERE namespace = ?1 AND key = ?2",
                    params![ns_key, key],
                )
                .map_err(|e| memory_io_error("failed to delete from main table", e))?;

            // Clean up FTS index
            conn.execute(
                "DELETE FROM store_fts WHERE namespace = ?1 AND key = ?2",
                params![ns_key, key],
            )
            .map_err(|e| memory_io_error("failed to delete FTS index", e))?;

            // Clean up vector index
            let _ = conn.execute(
                "DELETE FROM store_vectors WHERE namespace = ?1 AND key = ?2",
                params![ns_key, key],
            );

            Ok(affected > 0)
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
        Box::pin(async move {
            let conn = self.open_connection()?;

            let namespaces: Vec<Vec<String>> = match prefix {
                Some(p) => {
                    let prefix_str = p.join("/");
                    let pattern = format!("{prefix_str}%");
                    let mut stmt = conn
                        .prepare(
                            "SELECT DISTINCT namespace FROM store_items WHERE namespace LIKE ?1",
                        )
                        .map_err(|e| memory_io_error("failed to query namespaces", e))?;
                    stmt.query_map(params![pattern], |row| row.get::<_, String>(0))
                        .map_err(|e| memory_io_error("failed to query namespaces", e))?
                        .filter_map(|r| r.ok())
                        .map(|ns| ns.split('/').map(String::from).collect())
                        .collect()
                }
                None => {
                    let mut stmt = conn
                        .prepare("SELECT DISTINCT namespace FROM store_items")
                        .map_err(|e| memory_io_error("failed to query namespaces", e))?;
                    stmt.query_map([], |row| row.get::<_, String>(0))
                        .map_err(|e| memory_io_error("failed to query namespaces", e))?
                        .filter_map(|r| r.ok())
                        .map(|ns| ns.split('/').map(String::from).collect())
                        .collect()
                }
            };

            Ok(namespaces)
        })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace.join("/");
            let conn = self.open_connection()?;
            let mut stmt = conn
                .prepare(
                    "SELECT namespace, key, value, created_at, updated_at \
                     FROM store_items WHERE namespace = ?1",
                )
                .map_err(|e| memory_io_error("list items failed", e))?;
            let items = stmt
                .query_map(params![ns_key], |row| {
                    let ns_str: String = row.get(0)?;
                    let namespace: Vec<String> = ns_str.split('/').map(String::from).collect();
                    Ok(StoreItem {
                        namespace,
                        key: row.get(1)?,
                        value: serde_json::from_str(&row.get::<_, String>(2)?)
                            .unwrap_or(Value::Null),
                        created_at: row.get::<_, i64>(3)? as u64,
                        updated_at: row.get::<_, i64>(4)? as u64,
                        score: None,
                    })
                })
                .map_err(|e| memory_io_error("list items failed", e))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(items)
        })
    }

    fn search_with<'a>(
        &'a self,
        namespace: &'a [&'a str],
        query: SearchQuery<'a>,
    ) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            match query.mode {
                SearchMode::Keyword => self.search(namespace, query.text, query.limit).await,
                SearchMode::Semantic => {
                    self.semantic_search_impl(namespace, query.text, query.limit)
                        .await
                }
                SearchMode::Hybrid => {
                    let mut merged: std::collections::HashMap<String, StoreItem> =
                        std::collections::HashMap::new();
                    for item in self.search(namespace, query.text, query.limit).await? {
                        merged.insert(item.key.clone(), item);
                    }
                    match self
                        .semantic_search_impl(namespace, query.text, query.limit)
                        .await
                    {
                        Ok(items) => {
                            for item in items {
                                merged
                                    .entry(item.key.clone())
                                    .and_modify(|existing| {
                                        let incoming = item.score.unwrap_or_default();
                                        if incoming > existing.score.unwrap_or_default() {
                                            *existing = item.clone();
                                        }
                                    })
                                    .or_insert(item);
                            }
                        }
                        Err(err)
                            if format!("{err}").contains(
                                "semantic search requires an embedder-backed SqliteStore",
                            ) => {}
                        Err(err) => return Err(err),
                    }
                    let mut items: Vec<StoreItem> = merged.into_values().collect();
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

// ── Utility functions ─────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn temp_db() -> SqliteStore {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteStore::new(dir.join("test.db")).unwrap()
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let store = temp_db();
        let ns = &["user", "memories"];

        store
            .put(ns, "key1", json!({"content": "hello world"}))
            .await
            .unwrap();

        let item = store.get(ns, "key1").await.unwrap();
        assert!(item.is_some());
        let item = item.unwrap();
        assert_eq!(item.key, "key1");
        assert_eq!(item.value["content"], "hello world");
        assert_eq!(item.namespace, vec!["user", "memories"]);
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let store = temp_db();
        let item = store.get(&["ns"], "nonexistent").await.unwrap();
        assert!(item.is_none());
    }

    #[tokio::test]
    async fn test_upsert() {
        let store = temp_db();
        let ns = &["user", "mem"];

        store.put(ns, "k1", json!({"count": 1})).await.unwrap();
        store.put(ns, "k1", json!({"count": 2})).await.unwrap();

        let item = store.get(ns, "k1").await.unwrap().unwrap();
        assert_eq!(item.value["count"], 2);
    }

    #[tokio::test]
    async fn test_delete() {
        let store = temp_db();
        let ns = &["user", "mem"];

        store.put(ns, "k1", json!({"data": "value"})).await.unwrap();

        let deleted = store.delete(ns, "k1").await.unwrap();
        assert!(deleted);

        let item = store.get(ns, "k1").await.unwrap();
        assert!(item.is_none());

        let deleted_again = store.delete(ns, "k1").await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_fts5_search() {
        let store = temp_db();
        let ns = &["user", "memories"];

        store
            .put(
                ns,
                "k1",
                json!({"content": "Rust programming language systems-level"}),
            )
            .await
            .unwrap();
        store
            .put(
                ns,
                "k2",
                json!({"content": "Python machine learning deep learning"}),
            )
            .await
            .unwrap();
        store
            .put(ns, "k3", json!({"content": "JavaScript frontend React"}))
            .await
            .unwrap();

        let results = store.search(ns, "Rust", 5).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].key, "k1");
        assert!(results[0].score.is_some());
    }

    #[tokio::test]
    async fn test_fts5_search_multiple_keywords() {
        let store = temp_db();
        let ns = &["search", "test"];

        store
            .put(
                ns,
                "k1",
                json!({"content": "dark theme user preference settings"}),
            )
            .await
            .unwrap();
        store
            .put(
                ns,
                "k2",
                json!({"content": "light theme default configuration"}),
            )
            .await
            .unwrap();

        let results = store.search(ns, "dark theme", 5).await.unwrap();
        assert!(!results.is_empty());
        // "dark" only appears in k1
        assert_eq!(results[0].key, "k1");
    }

    #[tokio::test]
    async fn test_fts5_like_fallback_supports_cjk_spaced_keywords() {
        let store = temp_db();
        let ns = &["search", "cjk"];

        store
            .put(
                ns,
                "k1",
                json!({"content": "this memory persists after database closes"}),
            )
            .await
            .unwrap();

        let results = store.search(ns, "memory persist", 5).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].key, "k1");
    }

    #[tokio::test]
    async fn test_fts5_persists_across_store_instances() {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.db");
        let ns = &["persist", "fts"];

        {
            let store = SqliteStore::new(&db_path).unwrap();
            store
                .put(
                    ns,
                    "k1",
                    json!({"content": "this memory persists after database closes"}),
                )
                .await
                .unwrap();
        }

        {
            let store = SqliteStore::new(&db_path).unwrap();
            let results = store.search(ns, "memory persist", 5).await.unwrap();
            assert!(!results.is_empty());
            assert_eq!(results[0].key, "k1");
        }
    }

    #[tokio::test]
    async fn test_list_namespaces() {
        let store = temp_db();

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

        let all = store.list_namespaces(None).await.unwrap();
        assert_eq!(all.len(), 3);

        let user1 = store.list_namespaces(Some(&["user1"])).await.unwrap();
        assert_eq!(user1.len(), 2);
    }

    #[tokio::test]
    async fn test_namespace_isolation() {
        let store = temp_db();

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

    #[tokio::test]
    async fn test_semantic_search_without_embedder_is_unsupported() {
        let store = temp_db();
        let ns = &["test", "fallback"];

        store
            .put(ns, "k1", json!({"content": "Rust programming language"}))
            .await
            .unwrap();

        let err = store
            .search_with(ns, SearchQuery::semantic("Rust", 5))
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("semantic search"));
    }

    #[tokio::test]
    async fn test_with_embedder() {
        use crate::memory::MockEmbedder;

        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(4));
        let store = SqliteStore::with_embedder(dir.join("test.db"), embedder).unwrap();

        let ns = &["test", "vec"];
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

    #[test]
    fn test_vec_serialization() {
        let vec = vec![1.0f32, 2.5, -3.7, 0.0];
        let bytes = SqliteStore::vec_to_bytes(&vec);
        let restored = SqliteStore::bytes_to_vec(&bytes);
        assert_eq!(vec, restored);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        assert!((SqliteStore::cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);

        let c = vec![0.0f32, 1.0, 0.0];
        assert!((SqliteStore::cosine_similarity(&a, &c) - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_extract_searchable_text() {
        let value = json!({"content": "hello", "tags": ["tag1", "tag2"]});
        let text = SqliteStore::extract_searchable_text(&value);
        assert!(text.contains("hello"));
        assert!(text.contains("tag1"));
        assert!(text.contains("tag2"));
    }
}
