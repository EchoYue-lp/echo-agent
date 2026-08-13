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

use super::store::{namespace_key, parse_namespace_key};
use crate::util::{expand_tilde, memory_io_error};
use echo_core::error::{MemoryError, Result};
pub use echo_core::memory::embedder::Embedder;
pub use echo_core::memory::store::{SearchMode, SearchQuery, Store, StoreItem};
use echo_core::utils::time::now_secs;
use futures::future::BoxFuture;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

// ── SqliteStore ─────────────────────────────────────────────────────────────

/// SQLite persistent Store with FTS5 full-text search and optional vector search.
///
/// Uses a single `Mutex<Connection>` to avoid opening a new connection on every
/// put/get/delete/search (P1-2.5).  SQLite serialises all writes anyway, so a
/// single connection eliminates `SQLITE_BUSY` storms under concurrent access.
pub struct SqliteStore {
    embedder: Option<Arc<dyn Embedder>>,
    conn: Arc<Mutex<Connection>>,
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
            .map_err(|e| memory_io_error("failed to count stored items", e))?;

        info!(
            path = %path.display(),
            items = item_count,
            vector = embedder.is_some(),
            "SqliteStore initialized"
        );

        Ok(Self {
            embedder,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn open_connection_at(path: &Path) -> Result<Connection> {
        let conn = Connection::open(path)
            .map_err(|e| memory_io_error("failed to open SQLite database", e))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA cache_size=10000;
             PRAGMA temp_store=MEMORY;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(|e| memory_io_error("SQLite PRAGMA settings failed", e))?;
        Ok(conn)
    }

    async fn run_db<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock().map_err(|error| {
                MemoryError::IoError(format!("SqliteStore lock poisoned: {error}"))
            })?;
            operation(&mut guard)
        })
        .await
        .map_err(|error| {
            MemoryError::IoError(format!("SQLite store operation task failed: {error}"))
        })?
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

        // ── Schema migration: add importance/last_accessed/expires_at columns ──
        // SQLite doesn't support IF NOT EXISTS for ALTER TABLE, so ignore "duplicate column" errors only.
        for col_sql in [
            "ALTER TABLE store_items ADD COLUMN importance REAL NOT NULL DEFAULT 5.0",
            "ALTER TABLE store_items ADD COLUMN last_accessed INTEGER",
            "ALTER TABLE store_items ADD COLUMN expires_at INTEGER",
        ] {
            if let Err(error) = conn.execute_batch(col_sql) {
                let msg = error.to_string();
                // Only ignore "duplicate column" errors (column already exists)
                if !msg.contains("duplicate column") {
                    return Err(memory_io_error("schema migration failed", error).into());
                }
            }
        }

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

        Self::verify_schema(conn)?;
        Ok(())
    }

    fn verify_schema(conn: &Connection) -> Result<()> {
        let mut statement = conn
            .prepare("PRAGMA table_info(store_items)")
            .map_err(|error| memory_io_error("failed to inspect store schema", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .map_err(|error| memory_io_error("failed to query store schema", error))?;
        let mut columns = HashMap::new();
        for row in rows {
            let (name, data_type) =
                row.map_err(|error| memory_io_error("failed to read store schema", error))?;
            columns.insert(name, data_type.to_ascii_uppercase());
        }
        for (name, expected_type) in [
            ("namespace", "TEXT"),
            ("key", "TEXT"),
            ("value", "TEXT"),
            ("created_at", "INTEGER"),
            ("updated_at", "INTEGER"),
            ("importance", "REAL"),
            ("last_accessed", "INTEGER"),
            ("expires_at", "INTEGER"),
        ] {
            match columns.get(name) {
                Some(actual_type) if actual_type == expected_type => {}
                Some(actual_type) => {
                    return Err(MemoryError::SerializationError(format!(
                        "store_items.{name} has type {actual_type}, expected {expected_type}"
                    ))
                    .into());
                }
                None => {
                    return Err(MemoryError::SerializationError(format!(
                        "store_items is missing required column {name}"
                    ))
                    .into());
                }
            }
        }
        for table in ["store_fts", "store_vectors"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
                    params![table],
                    |row| row.get(0),
                )
                .map_err(|error| memory_io_error("failed to verify auxiliary table", error))?;
            if !exists {
                return Err(MemoryError::SerializationError(format!(
                    "required auxiliary table {table} is missing"
                ))
                .into());
            }
        }
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
    fn bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
            return Err(MemoryError::SerializationError(
                "stored embedding has an invalid byte length".to_string(),
            )
            .into());
        }
        let mut values = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            let array: [u8; 4] = chunk.try_into().map_err(|_| {
                MemoryError::SerializationError(
                    "stored embedding contains a partial float".to_string(),
                )
            })?;
            let value = f32::from_le_bytes(array);
            if !value.is_finite() {
                return Err(MemoryError::SerializationError(
                    "stored embedding contains a non-finite value".to_string(),
                )
                .into());
            }
            values.push(value);
        }
        Ok(values)
    }

    /// Cosine similarity
    fn cosine_similarity(a: &[f32], b: &[f32]) -> Result<f32> {
        if a.len() != b.len() || a.is_empty() {
            return Err(MemoryError::SerializationError(format!(
                "embedding dimension mismatch: query {}, stored {}",
                a.len(),
                b.len()
            ))
            .into());
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            Err(
                MemoryError::SerializationError("embedding vector has zero magnitude".to_string())
                    .into(),
            )
        } else {
            let similarity = dot / (norm_a * norm_b);
            if similarity.is_finite() {
                Ok(similarity)
            } else {
                Err(MemoryError::SerializationError(
                    "embedding similarity is non-finite".to_string(),
                )
                .into())
            }
        }
    }
}

impl SqliteStore {
    /// Fetch complete items from the main table by key list (unified score)
    /// Fetch complete items from the main table by key list (unified score).
    /// Uses a single `WHERE key IN (...)` instead of N individual SELECTs (P1-2.6).
    fn fetch_items(
        conn: &Connection,
        namespace: &[&str],
        ns_key: &str,
        keys: &[String],
        default_score: Option<f32>,
    ) -> Result<Vec<StoreItem>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (2..=keys.len() + 1).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT key, value, created_at, updated_at FROM store_items              WHERE namespace = ?1 AND key IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| {
            echo_core::error::MemoryError::IoError(format!("prepare batch fetch: {}", e))
        })?;
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(keys.len() + 1);
        params.push(Box::new(ns_key.to_string()));
        for k in keys {
            params.push(Box::new(k.clone()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let key: String = row.get(0)?;
                let value_str: String = row.get(1)?;
                let created_at: i64 = row.get(2)?;
                let updated_at: i64 = row.get(3)?;
                Ok((key, value_str, created_at, updated_at))
            })
            .map_err(|e| echo_core::error::MemoryError::IoError(format!("batch fetch: {}", e)))?;
        let mut raw = std::collections::HashMap::with_capacity(keys.len());
        for row in rows {
            let (key, value, created_at, updated_at) =
                row.map_err(|e| memory_io_error("failed to read batch item", e))?;
            raw.insert(key, (value, created_at, updated_at));
        }
        let score_map: std::collections::HashMap<&str, f32> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), default_score.unwrap_or(1.0 / (i as f32 + 1.0))))
            .collect();
        let mut results = Vec::with_capacity(raw.len());
        for key in keys {
            if let Some((value_str, created_at, updated_at)) = raw.remove(key) {
                let value = serde_json::from_str::<Value>(&value_str)
                    .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
                let mut item = StoreItem {
                    namespace: namespace.iter().map(|s| s.to_string()).collect(),
                    key: key.clone(),
                    value,
                    created_at: u64::try_from(created_at).map_err(|_| {
                        MemoryError::SerializationError("negative created_at".to_string())
                    })?,
                    updated_at: u64::try_from(updated_at).map_err(|_| {
                        MemoryError::SerializationError("negative updated_at".to_string())
                    })?,
                    score: score_map.get(key.as_str()).copied(),
                    importance: 5.0,
                    last_accessed: None,
                    expires_at: None,
                };
                apply_json_metadata(&mut item);
                results.push(item);
            }
        }
        Ok(results)
    }

    /// Fetch complete items from the main table by (key, score) list.
    /// Uses a single `WHERE key IN (...)` instead of N individual SELECTs (P1-2.6).
    fn fetch_items_with_scores(
        conn: &Connection,
        namespace: &[&str],
        ns_key: &str,
        keys_with_scores: &[(String, Option<f32>)],
    ) -> Result<Vec<StoreItem>> {
        if keys_with_scores.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<&String> = keys_with_scores.iter().map(|(k, _)| k).collect();
        let placeholders: Vec<String> = (2..=keys.len() + 1).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT key, value, created_at, updated_at FROM store_items              WHERE namespace = ?1 AND key IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| {
            echo_core::error::MemoryError::IoError(format!("prepare batch fetch: {}", e))
        })?;
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(keys.len() + 1);
        params.push(Box::new(ns_key.to_string()));
        for k in &keys {
            params.push(Box::new((*k).clone()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let key: String = row.get(0)?;
                let value_str: String = row.get(1)?;
                let created_at: i64 = row.get(2)?;
                let updated_at: i64 = row.get(3)?;
                Ok((key, value_str, created_at, updated_at))
            })
            .map_err(|e| echo_core::error::MemoryError::IoError(format!("batch fetch: {}", e)))?;
        let mut raw = std::collections::HashMap::with_capacity(keys.len());
        for row in rows {
            let (key, value, created_at, updated_at) =
                row.map_err(|e| memory_io_error("failed to read scored batch item", e))?;
            raw.insert(key, (value, created_at, updated_at));
        }
        let mut results = Vec::with_capacity(raw.len());
        for (rank, (key, supplied_score)) in keys_with_scores.iter().enumerate() {
            if let Some((value_str, created_at, updated_at)) = raw.remove(key) {
                let value = serde_json::from_str::<Value>(&value_str)
                    .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
                let mut item = StoreItem {
                    namespace: namespace.iter().map(|s| s.to_string()).collect(),
                    key: key.clone(),
                    value,
                    created_at: u64::try_from(created_at).map_err(|_| {
                        MemoryError::SerializationError("negative created_at".to_string())
                    })?,
                    updated_at: u64::try_from(updated_at).map_err(|_| {
                        MemoryError::SerializationError("negative updated_at".to_string())
                    })?,
                    score: supplied_score.or(Some(1.0 / (rank as f32 + 1.0))),
                    importance: 5.0,
                    last_accessed: None,
                    expires_at: None,
                };
                apply_json_metadata(&mut item);
                results.push(item);
            }
        }
        Ok(results)
    }

    async fn semantic_search_impl(
        &self,
        namespace: &[&str],
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<StoreItem>> {
        let ns_key = namespace_key(namespace);
        let embedder = match &self.embedder {
            Some(e) => e,
            None => {
                return Err(echo_core::error::MemoryError::Unsupported(
                    "semantic search requires an embedder-backed SqliteStore".to_string(),
                )
                .into());
            }
        };
        let query_vec = embedder
            .embed(query_text)
            .await
            .map_err(|e| echo_core::error::MemoryError::IoError(format!("embed failed: {}", e)))?;
        let query_namespace = ns_key.clone();
        let rows: Vec<(String, Vec<u8>)> = self
            .run_db(move |conn| {
                let mut stmt = conn
                    .prepare("SELECT key, vector FROM store_vectors WHERE namespace = ?1")
                    .map_err(|e| {
                        echo_core::error::MemoryError::IoError(format!("prepare vectors: {}", e))
                    })?;
                let mut rows = Vec::new();
                let mut q = stmt.query(params![&query_namespace]).map_err(|e| {
                    echo_core::error::MemoryError::IoError(format!("query vectors: {}", e))
                })?;
                while let Some(row) = q.next().map_err(|e| {
                    echo_core::error::MemoryError::IoError(format!("next vector: {}", e))
                })? {
                    let key: String = row.get(0).map_err(|e| {
                        echo_core::error::MemoryError::IoError(format!("get key: {}", e))
                    })?;
                    let blob: Vec<u8> = row.get(1).map_err(|e| {
                        echo_core::error::MemoryError::IoError(format!("get blob: {}", e))
                    })?;
                    rows.push((key, blob));
                }
                Ok(rows)
            })
            .await?;
        if query_vec.is_empty() || query_vec.iter().any(|value| !value.is_finite()) {
            return Err(MemoryError::SerializationError(
                "query embedder returned an empty or non-finite vector".to_string(),
            )
            .into());
        }
        let mut scored = Vec::new();
        for (key, blob) in rows.iter().take(Self::max_candidates()) {
            let vector = Self::bytes_to_vec(blob)?;
            let similarity = Self::cosine_similarity(&query_vec, &vector)?;
            scored.push((similarity, key.clone()));
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);
        let keys: Vec<String> = scored.iter().map(|(_s, k)| k.clone()).collect();
        let namespace: Vec<String> = namespace.iter().map(|part| (*part).to_string()).collect();
        self.run_db(move |conn| {
            let namespace_refs: Vec<&str> = namespace.iter().map(String::as_str).collect();
            Self::fetch_items(conn, &namespace_refs, &ns_key, &keys, None)
        })
        .await
    }

    fn max_candidates() -> usize {
        10_000
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
            let ns_key = namespace_key(namespace);
            let value_json = serde_json::to_string(&value)
                .map_err(|e| MemoryError::SerializationError(e.to_string()))?;
            let search_text = Self::extract_searchable_text(&value);
            let now = i64::try_from(now_secs()).map_err(|_| {
                MemoryError::SerializationError(
                    "current timestamp exceeds SQLite range".to_string(),
                )
            })?;

            // Compute the embedding BEFORE opening the transaction. `Connection`
            // holds a `RefCell` cache, so a `Transaction<'_>` is `!Send` — keeping
            // an `.await` (here, the embedder call) alive across it would make the
            // whole future `!Send` and break tokio multi-thread executors.
            let vector_bytes: Option<Vec<u8>> = if let Some(ref embedder) = self.embedder {
                match embedder.embed(&search_text).await {
                    Ok(vec) => {
                        if vec.is_empty() || vec.iter().any(|value| !value.is_finite()) {
                            return Err(MemoryError::SerializationError(
                                "embedder returned an empty or non-finite vector".to_string(),
                            )
                            .into());
                        }
                        let dims = vec.len();
                        let bytes = Self::vec_to_bytes(&vec);
                        debug!(ns = %ns_key, key = %key, dims, "Vector embedding ready");
                        Some(bytes)
                    }
                    Err(e) => {
                        warn!(key = %key, error = %e, "Embedding calculation failed, item will not be added to vector index");
                        None
                    }
                }
            } else {
                None
            };

            let key = key.to_string();
            self.run_db(move |conn| {
                // Wrap main table write, FTS update, and vector table update in a transaction.
                // If any step fails, the whole transaction rolls back.
                let tx = conn
                    .transaction()
                    .map_err(|e| memory_io_error("failed to begin transaction", e))?;

                // Upsert into main table (extract metadata from JSON value for columns)
                let importance: f64 = value
                    .get("importance")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(5.0);
                let expires_at: Option<i64> = value
                    .get("expires_at")
                    .and_then(|v| v.as_u64())
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| {
                        MemoryError::SerializationError(
                            "expires_at exceeds SQLite range".to_string(),
                        )
                    })?;

                tx.execute(
                    "INSERT INTO store_items (namespace, key, value, created_at, updated_at, importance, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6)
                     ON CONFLICT(namespace, key) DO UPDATE SET
                        value = excluded.value,
                        updated_at = excluded.updated_at,
                        importance = excluded.importance,
                        expires_at = excluded.expires_at",
                    params![ns_key, key, value_json, now, importance, expires_at],
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

                // Vector index update (if embedding succeeded above) — included in the
                // same transaction so a failure rolls back the FTS/main writes too.
                if let Some(bytes) = vector_bytes {
                    tx.execute(
                        "INSERT INTO store_vectors (namespace, key, vector)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT(namespace, key) DO UPDATE SET vector = excluded.vector",
                        params![ns_key, key, bytes],
                    )
                    .map_err(|e| memory_io_error("failed to write to vector table", e))?;
                } else {
                    tx.execute(
                        "DELETE FROM store_vectors WHERE namespace = ?1 AND key = ?2",
                        params![ns_key, key],
                    )
                    .map_err(|e| memory_io_error("failed to clear stale vector", e))?;
                }

                tx.commit()
                    .map_err(|e| memory_io_error("failed to commit transaction", e))?;
                Ok(())
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
            let namespace: Vec<String> = namespace.iter().map(|part| (*part).to_string()).collect();
            let key = key.to_string();
            self.run_db(move |conn| {
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
                        let mut item = StoreItem {
                            namespace,
                            key,
                            value,
                            created_at: u64::try_from(created_at).map_err(|_| {
                                MemoryError::SerializationError("negative created_at".to_string())
                            })?,
                            updated_at: u64::try_from(updated_at).map_err(|_| {
                                MemoryError::SerializationError("negative updated_at".to_string())
                            })?,
                            score: None,
                            importance: 5.0,
                            last_accessed: None,
                            expires_at: None,
                        };
                        apply_json_metadata(&mut item);
                        Ok(Some(item))
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(memory_io_error("query failed", e).into()),
                }
            })
            .await
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
            let namespace: Vec<String> = namespace.iter().map(|part| (*part).to_string()).collect();
            let query = query.to_string();
            self.run_db(move |conn| {

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

                let sql_limit = i64::try_from(limit).map_err(|_| {
                    MemoryError::SerializationError("search limit exceeds SQLite range".to_string())
                })?;
                let rows = stmt
                    .query_map(params![ns_key, fts_query, sql_limit], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
                    })
                    .map_err(|e| memory_io_error("FTS query failed", e))?;
                let mut matched = Vec::new();
                for row in rows {
                    matched.push(row.map_err(|e| memory_io_error("failed to read FTS row", e))?);
                }
                matched
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

                    let current_limit = i64::try_from(
                        (limit.saturating_sub(fallback_keys.len())).max(1),
                    )
                    .map_err(|_| {
                        MemoryError::SerializationError(
                            "search limit exceeds SQLite range".to_string(),
                        )
                    })?;
                    let keyword_rows = stmt
                        .query_map(params![ns_key, like_pattern, current_limit], |row| {
                            row.get::<_, String>(0)
                        })
                        .map_err(|e| memory_io_error("LIKE query failed", e))?;

                    for row in keyword_rows {
                        let key = row.map_err(|e| memory_io_error("failed to read LIKE row", e))?;
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

                let namespace_refs: Vec<&str> = namespace.iter().map(String::as_str).collect();
                return Self::fetch_items(conn, &namespace_refs, &ns_key, &fallback_keys, None);
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

            let namespace_refs: Vec<&str> = namespace.iter().map(String::as_str).collect();
            Self::fetch_items_with_scores(conn, &namespace_refs, &ns_key, &keys_with_scores)
            }).await
        })
    }

    fn delete<'a>(&'a self, namespace: &'a [&'a str], key: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let ns_key = namespace_key(namespace);
            let key = key.to_string();
            self.run_db(move |conn| {
                // Wrap all three deletes in a transaction to prevent partial deletion
                let tx = conn
                    .transaction()
                    .map_err(|e| memory_io_error("failed to begin delete transaction", e))?;

                let affected = tx
                    .execute(
                        "DELETE FROM store_items WHERE namespace = ?1 AND key = ?2",
                        params![ns_key, key],
                    )
                    .map_err(|e| memory_io_error("failed to delete from main table", e))?;

                // Clean up FTS index
                tx.execute(
                    "DELETE FROM store_fts WHERE namespace = ?1 AND key = ?2",
                    params![ns_key, key],
                )
                .map_err(|e| memory_io_error("failed to delete FTS index", e))?;

                tx.execute(
                    "DELETE FROM store_vectors WHERE namespace = ?1 AND key = ?2",
                    params![ns_key, key],
                )
                .map_err(|e| memory_io_error("failed to delete vector index", e))?;

                tx.commit()
                    .map_err(|e| memory_io_error("failed to commit delete transaction", e))?;

                Ok(affected > 0)
            })
            .await
        })
    }

    fn list_namespaces<'a>(
        &'a self,
        prefix: Option<&'a [&'a str]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<String>>>> {
        Box::pin(async move {
            let prefix = prefix.map(|parts| {
                parts
                    .iter()
                    .map(|part| (*part).to_string())
                    .collect::<Vec<_>>()
            });
            self.run_db(move |conn| {
                let mut stmt = conn
                    .prepare("SELECT DISTINCT namespace FROM store_items")
                    .map_err(|e| memory_io_error("failed to query namespaces", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| memory_io_error("failed to query namespaces", e))?;
                let mut namespaces = Vec::new();
                for row in rows {
                    let encoded =
                        row.map_err(|e| memory_io_error("failed to read namespace", e))?;
                    let namespace = parse_namespace_key(&encoded)?;
                    let matches_prefix = prefix.as_ref().is_none_or(|parts| {
                        namespace.len() >= parts.len()
                            && namespace
                                .iter()
                                .map(String::as_str)
                                .zip(parts.iter().map(String::as_str))
                                .all(|(actual, expected)| actual == expected)
                    });
                    if matches_prefix {
                        namespaces.push(namespace);
                    }
                }

                Ok(namespaces)
            })
            .await
        })
    }

    fn list<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<Vec<StoreItem>>> {
        Box::pin(async move {
            let ns_key = namespace_key(namespace);
            self.run_db(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT namespace, key, value, created_at, updated_at \
                     FROM store_items WHERE namespace = ?1",
                    )
                    .map_err(|e| memory_io_error("list items failed", e))?;
                let items = stmt
                    .query_map(params![ns_key], |row| {
                        let ns_str: String = row.get(0)?;
                        let namespace = parse_namespace_key(&ns_str).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    error.to_string(),
                                )),
                            )
                        })?;
                        let mut item = StoreItem {
                            namespace,
                            key: row.get(1)?,
                            value: Value::String(row.get::<_, String>(2)?),
                            created_at: u64::try_from(row.get::<_, i64>(3)?).map_err(|_| {
                                rusqlite::Error::IntegralValueOutOfRange(3, i64::MIN)
                            })?,
                            updated_at: u64::try_from(row.get::<_, i64>(4)?).map_err(|_| {
                                rusqlite::Error::IntegralValueOutOfRange(4, i64::MIN)
                            })?,
                            score: None,
                            importance: 5.0,
                            last_accessed: None,
                            expires_at: None,
                        };
                        let encoded = item.value.as_str().ok_or_else(|| {
                            rusqlite::Error::InvalidColumnType(
                                2,
                                "value".to_string(),
                                rusqlite::types::Type::Text,
                            )
                        })?;
                        item.value = serde_json::from_str(encoded).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                        apply_json_metadata(&mut item);
                        Ok(item)
                    })
                    .map_err(|e| memory_io_error("list items failed", e))?;
                let mut collected = Vec::new();
                for item in items {
                    collected
                        .push(item.map_err(|e| memory_io_error("failed to read list item", e))?);
                }
                Ok(collected)
            })
            .await
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
                SearchMode::Hybrid { vector_weight } => {
                    let keyword_items = self.search(namespace, query.text, query.limit).await?;

                    let semantic_items = match self
                        .semantic_search_impl(namespace, query.text, query.limit)
                        .await
                    {
                        Ok(items) => items,
                        Err(echo_core::error::ReactError::Memory(error))
                            if matches!(*error, MemoryError::Unsupported(_)) =>
                        {
                            // Fallback to keyword-only: rank keyword items by RRF
                            // (effectively vector_weight forced to 0.0)
                            let mut items = keyword_items;
                            items.sort_by(|a, b| {
                                b.score
                                    .unwrap_or_default()
                                    .partial_cmp(&a.score.unwrap_or_default())
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            });
                            items.truncate(query.limit);
                            return Ok(items);
                        }
                        Err(err) => return Err(err),
                    };

                    // Build rank maps
                    let keyword_ranks: HashMap<&str, usize> = keyword_items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| (item.key.as_str(), i))
                        .collect();
                    let semantic_ranks: HashMap<&str, usize> = semantic_items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| (item.key.as_str(), i))
                        .collect();

                    let all_keys: HashSet<&str> = keyword_ranks
                        .keys()
                        .chain(semantic_ranks.keys())
                        .copied()
                        .collect();

                    let mut items: Vec<StoreItem> = all_keys
                        .iter()
                        .filter_map(|&key| {
                            let kr = keyword_ranks.get(key).copied().unwrap_or(usize::MAX);
                            let sr = semantic_ranks.get(key).copied().unwrap_or(usize::MAX);
                            let rrf = echo_core::memory::store::rrf_score(sr, kr, vector_weight);

                            let mut item = keyword_items
                                .iter()
                                .find(|i| i.key == key)
                                .or_else(|| semantic_items.iter().find(|i| i.key == key))?
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

    fn prune_expired<'a>(&'a self, namespace: &'a [&'a str]) -> BoxFuture<'a, Result<u64>> {
        let ns_key = namespace_key(namespace);
        Box::pin(async move {
            self.run_db(move |conn| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                // Delete items where value contains an expired "expires_at" JSON field
                let now = i64::try_from(now).map_err(|_| {
                    MemoryError::SerializationError(
                        "current timestamp exceeds SQLite range".to_string(),
                    )
                })?;
                let tx = conn
                    .transaction()
                    .map_err(|e| memory_io_error("failed to begin prune transaction", e))?;
                let mut stmt = tx
                    .prepare(
                        "SELECT key FROM store_items WHERE namespace = ?1
                     AND json_extract(value, '$.expires_at') IS NOT NULL
                     AND CAST(json_extract(value, '$.expires_at') AS INTEGER) < ?2",
                    )
                    .map_err(|e| memory_io_error("failed to prepare expired item query", e))?;
                let rows = stmt
                    .query_map(params![ns_key, now], |row| row.get::<_, String>(0))
                    .map_err(|e| memory_io_error("failed to query expired items", e))?;
                let mut keys = Vec::new();
                for row in rows {
                    keys.push(row.map_err(|e| memory_io_error("failed to read expired item", e))?);
                }
                drop(stmt);
                for key in &keys {
                    tx.execute(
                        "DELETE FROM store_items WHERE namespace = ?1 AND key = ?2",
                        params![ns_key, key],
                    )
                    .map_err(|e| memory_io_error("failed to prune main item", e))?;
                    tx.execute(
                        "DELETE FROM store_fts WHERE namespace = ?1 AND key = ?2",
                        params![ns_key, key],
                    )
                    .map_err(|e| memory_io_error("failed to prune FTS item", e))?;
                    tx.execute(
                        "DELETE FROM store_vectors WHERE namespace = ?1 AND key = ?2",
                        params![ns_key, key],
                    )
                    .map_err(|e| memory_io_error("failed to prune vector item", e))?;
                }
                tx.commit()
                    .map_err(|e| memory_io_error("failed to commit prune transaction", e))?;
                u64::try_from(keys.len()).map_err(|_| {
                    MemoryError::SerializationError("expired item count exceeds u64".to_string())
                        .into()
                })
            })
            .await
        })
    }
}

// ── Utility functions ─────────────────────────────────────────────────────────

/// Apply metadata from JSON value to a StoreItem after reading from SQL.
/// Extracts `importance`, `expires_at`, and `last_accessed` if present in the JSON.
fn apply_json_metadata(item: &mut StoreItem) {
    if let Some(imp) = item.value.get("importance").and_then(|v| v.as_f64()) {
        item.importance = imp as f32;
    }
    if let Some(exp) = item.value.get("expires_at").and_then(|v| v.as_u64()) {
        item.expires_at = Some(exp);
    }
    if let Some(ts) = item.value.get("last_accessed").and_then(|v| v.as_u64()) {
        item.last_accessed = Some(ts);
    }
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
    async fn slash_in_namespace_component_does_not_alias_nested_namespace() -> Result<()> {
        let store = temp_db();
        store.put(&["a/b"], "key", json!({"value": "flat"})).await?;
        store
            .put(&["a", "b"], "key", json!({"value": "nested"}))
            .await?;

        let flat = store.get(&["a/b"], "key").await?;
        let nested = store.get(&["a", "b"], "key").await?;
        assert!(flat.is_some_and(|item| item.value.get("value") == Some(&json!("flat"))));
        assert!(nested.is_some_and(|item| item.value.get("value") == Some(&json!("nested"))));
        let prefix = store.list_namespaces(Some(&["a"])).await?;
        assert_eq!(prefix, vec![vec!["a".to_string(), "b".to_string()]]);
        Ok(())
    }

    #[test]
    fn schema_verification_rejects_wrong_column_type() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)
            .map_err(|error| memory_io_error("failed to create test directory", error))?;
        let path = dir.join("bad-schema.db");
        let conn = Connection::open(&path)
            .map_err(|error| memory_io_error("failed to create test database", error))?;
        conn.execute_batch(
            "CREATE TABLE store_items (
                namespace TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                importance TEXT NOT NULL DEFAULT '5', last_accessed INTEGER,
                expires_at INTEGER, PRIMARY KEY (namespace, key)
             );",
        )
        .map_err(|error| memory_io_error("failed to create invalid schema", error))?;
        drop(conn);

        let error = SqliteStore::new(path).err().ok_or_else(|| {
            MemoryError::SerializationError("invalid schema was accepted".to_string())
        })?;
        assert!(error.to_string().contains("importance"));
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_vector_is_reported_instead_of_ranked() -> Result<()> {
        use crate::memory::MockEmbedder;

        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)
            .map_err(|error| memory_io_error("failed to create test directory", error))?;
        let store =
            SqliteStore::with_embedder(dir.join("test.db"), Arc::new(MockEmbedder::new(4)?))?;
        let ns = &["vector", "corrupt"];
        store.put(ns, "key", json!({"content": "value"})).await?;
        let ns_key = namespace_key(ns);
        store
            .run_db(move |conn| {
                conn.execute(
                    "UPDATE store_vectors SET vector = ?1 WHERE namespace = ?2 AND key = ?3",
                    params![vec![0_u8, 1, 2], ns_key, "key"],
                )
                .map_err(|error| memory_io_error("failed to corrupt test vector", error))?;
                Ok(())
            })
            .await?;

        let error = store
            .search_with(ns, SearchQuery::semantic("value", 5))
            .await
            .err()
            .ok_or_else(|| {
                MemoryError::SerializationError("corrupt vector was accepted".to_string())
            })?;
        assert!(error.to_string().contains("invalid byte length"));
        Ok(())
    }

    #[tokio::test]
    async fn ranked_fetch_preserves_requested_order() -> Result<()> {
        let store = temp_db();
        let ns = &["ranked"];
        store.put(ns, "first", json!({"content": "one"})).await?;
        store.put(ns, "second", json!({"content": "two"})).await?;
        let items = store
            .run_db(move |conn| {
                SqliteStore::fetch_items(
                    conn,
                    ns,
                    &namespace_key(ns),
                    &["second".to_string(), "first".to_string()],
                    None,
                )
            })
            .await?;
        assert_eq!(
            items
                .iter()
                .map(|item| item.key.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn prune_expired_removes_all_indexes() -> Result<()> {
        use crate::memory::MockEmbedder;

        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)
            .map_err(|error| memory_io_error("failed to create test directory", error))?;
        let store =
            SqliteStore::with_embedder(dir.join("test.db"), Arc::new(MockEmbedder::new(4)?))?;
        let ns = &["expired"];
        store
            .put(
                ns,
                "key",
                json!({"content": "expired value", "expires_at": 1}),
            )
            .await?;
        assert_eq!(store.prune_expired(ns).await?, 1);

        let ns_key = namespace_key(ns);
        store
            .run_db(move |conn| {
                for table in ["store_items", "store_fts", "store_vectors"] {
                    let sql =
                        format!("SELECT COUNT(*) FROM {table} WHERE namespace = ?1 AND key = ?2");
                    let count: i64 = conn
                        .query_row(&sql, params![ns_key, "key"], |row| row.get(0))
                        .map_err(|error| {
                            memory_io_error("failed to inspect pruned index", error)
                        })?;
                    assert_eq!(count, 0, "stale row remains in {table}");
                }
                Ok(())
            })
            .await?;
        Ok(())
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
    async fn test_with_embedder() -> Result<()> {
        use crate::memory::MockEmbedder;

        let dir = std::env::temp_dir().join(format!("echo-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(4)?);
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
        Ok(())
    }

    #[test]
    fn test_vec_serialization() {
        let vec = vec![1.0f32, 2.5, -3.7, 0.0];
        let bytes = SqliteStore::vec_to_bytes(&vec);
        assert_eq!(SqliteStore::bytes_to_vec(&bytes).ok(), Some(vec));
        assert!(SqliteStore::bytes_to_vec(&[1, 2, 3]).is_err());
        assert!(SqliteStore::bytes_to_vec(&f32::NAN.to_le_bytes()).is_err());
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        assert!(
            SqliteStore::cosine_similarity(&a, &b).is_ok_and(|value| (value - 1.0).abs() < 1e-5)
        );

        let c = vec![0.0f32, 1.0, 0.0];
        assert!(SqliteStore::cosine_similarity(&a, &c).is_ok_and(|value| value.abs() < 1e-5));
        assert!(SqliteStore::cosine_similarity(&a, &[1.0]).is_err());
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
