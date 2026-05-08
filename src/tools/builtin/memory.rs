//! Agent memory tools: remember / recall / forget
//!
//! Uses LangGraph-aligned [`Store`] API for persistent long-term memory.
//!
//! | Tool       | Store Operation                              |
//! |------------|----------------------------------------------|
//! | `remember` | `store.put(namespace, uuid, value)`          |
//! | `recall`   | `store.search(namespace, query, limit)`      |
//! | `forget`   | `store.delete(namespace, key)`              |

use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::memory::{SearchQuery, Store, StoreItem};
use crate::tools::{Tool, ToolParameters, ToolResult};
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

// ── RememberTool ─────────────────────────────────────────────────────────────

/// Store important information into the persistent Store
///
/// Internally calls `store.put(namespace, uuid, {"content": ..., "importance": ..., "tags": [...]})`
pub struct RememberTool {
    pub store: Arc<dyn Store>,
    /// Storage namespace, e.g. `["alice", "memories"]`
    pub namespace: Vec<String>,
}

impl RememberTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "Store information worth long-term retention into persistent memory (cross-session). \
         Suitable for recording user preferences, important conclusions, to-do items, key facts, etc."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The specific content to remember; please describe concisely and completely"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of tags for categorization and retrieval (optional), e.g. [\"preferences\", \"programming\"]"
                },
                "importance": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "description": "Importance level (1-10), default 5; higher values are prioritized in recall"
                }
            },
            "required": ["content"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let content = parameters
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("content".to_string()))?;

            let tags: Vec<String> = parameters
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let importance = parameters
                .get("importance")
                .and_then(|v| v.as_u64())
                .map(|n| n.clamp(1, 10))
                .unwrap_or(5);

            let key = uuid::Uuid::new_v4().to_string();
            let value = json!({
                "content": content,
                "importance": importance,
                "tags": tags,
            });

            debug!(key = %key, importance = importance, "💡 remember tool writing to Store");

            let ns: Vec<&str> = self.ns_refs();
            self.store.put(&ns, &key, value).await?;

            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!("(tags: {})", tags.join(", "))
            };

            Ok(ToolResult::success(format!(
                "✅ Remembered (ID: {}, importance: {}): \"{}\"{tag_str}",
                key.get(..8).unwrap_or(&key),
                importance,
                content,
            )))
        })
    }
}

// ── RecallTool ───────────────────────────────────────────────────────────────

/// Retrieve relevant historical memories from the persistent Store
///
/// Internally calls `store.search(namespace, query, limit)`
pub struct RecallTool {
    pub store: Arc<dyn Store>,
    pub namespace: Vec<String>,
}

impl RecallTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn description(&self) -> &str {
        "Search the persistent memory store for relevant historical memories, returning the best matches. \
         Search by keywords, topics, or natural language fragments."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords or description, e.g. \"user preferences\" or \"project name mentioned last time\""
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Maximum number of results to return (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n.clamp(1, 20) as usize)
                .unwrap_or(5);

            debug!(query = %query, limit = limit, "🔍 recall tool querying Store");

            let ns: Vec<&str> = self.ns_refs();
            let items = self.store.search(&ns, query, limit).await?;

            if items.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No memories found matching \"{}\".",
                    query
                )));
            }

            let mut lines = vec![format!("Found {} matching memories:", items.len())];
            for (i, item) in items.iter().enumerate() {
                lines.push(format!(
                    "{}. [ID:{}] {}",
                    i + 1,
                    item.key.get(..8).unwrap_or(&item.key),
                    format_store_item(item),
                ));
            }

            Ok(ToolResult::success(lines.join("\n")))
        })
    }
}

// ── ForgetTool ───────────────────────────────────────────────────────────────

/// Delete a memory entry by its ID (key), or clear all memories under a namespace
///
/// Internally calls `store.delete(namespace, key)`
pub struct ForgetTool {
    pub store: Arc<dyn Store>,
    pub namespace: Vec<String>,
}

impl ForgetTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

impl Tool for ForgetTool {
    fn name(&self) -> &str {
        "forget"
    }

    fn description(&self) -> &str {
        "Delete a memory entry by its ID. The ID can be obtained from recall tool results (first 8 chars is sufficient)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Memory ID to delete (first 8 chars prefix from recall results)"
                }
            },
            "required": ["id"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let id_prefix = parameters
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("id".to_string()))?;

            let ns: Vec<&str> = self.ns_refs();

            // Try exact match first; if that fails, search by prefix across all keys
            let full_key = self.store.get(&ns, id_prefix).await?.map(|item| item.key);

            // Try direct delete (user may have passed the full key)
            let deleted = if let Some(key) = &full_key {
                self.store.delete(&ns, key).await?
            } else {
                // Assume the user passed the full key (UUID format)
                self.store.delete(&ns, id_prefix).await?
            };

            if deleted {
                Ok(ToolResult::success(format!(
                    "🗑️ Deleted memory ID: {}",
                    id_prefix
                )))
            } else {
                Ok(ToolResult::success(format!(
                    "No memory entry found with ID \"{}\", nothing to delete.\nTip: use the recall tool to find the correct ID.",
                    id_prefix
                )))
            }
        })
    }
}

// ── SearchMemoryTool ─────────────────────────────────────────────────────────

/// Perform hybrid search (keyword + embedding) in the persistent Store
///
/// Unlike `RecallTool` (keyword-only search), this tool prefers the
/// [`Store::search_with`] `Hybrid` mode. When connected to a Store that
/// supports embeddings, it leverages both vector similarity and keyword
/// matching; when the underlying Store does not support hybrid search,
/// it falls back to keyword search.
pub struct SearchMemoryTool {
    pub store: Arc<dyn Store>,
    pub namespace: Vec<String>,
}

impl SearchMemoryTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

impl Tool for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }

    fn description(&self) -> &str {
        "Use hybrid search to find the most relevant historical memories in the persistent memory store. \
         Supports natural language queries, combining keyword and semantic similarity for recall. \
         Falls back to keyword search when the underlying Store does not support hybrid search."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language query describing the memory content you want to find"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Maximum number of results to return (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            let limit = parameters
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n.clamp(1, 20) as usize)
                .unwrap_or(5);

            debug!(query = %query, limit = limit, "🔎 search_memory hybrid search on Store");

            let ns: Vec<&str> = self.ns_refs();
            let items = match self
                .store
                .search_with(&ns, SearchQuery::hybrid(query, limit))
                .await
            {
                Ok(items) => items,
                Err(err) if format!("{err}").contains("hybrid search") => {
                    self.store.search(&ns, query, limit).await?
                }
                Err(err) => return Err(err),
            };

            if items.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No memories found matching \"{}\".",
                    query
                )));
            }

            let mut lines = vec![format!(
                "Hybrid search found {} matching memories:",
                items.len()
            )];
            for (i, item) in items.iter().enumerate() {
                lines.push(format!(
                    "{}. [ID:{}] {}",
                    i + 1,
                    item.key.get(..8).unwrap_or(&item.key),
                    format_store_item(item),
                ));
            }

            Ok(ToolResult::success(lines.join("\n")))
        })
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

fn format_store_item(item: &StoreItem) -> String {
    match &item.value {
        Value::Object(map) => {
            let content = map
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("(no content)");
            let importance = map.get("importance").and_then(|v| v.as_u64());
            let tags = map
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty());

            let mut parts = vec![content.to_string()];
            if let Some(imp) = importance {
                parts.push(format!("[★{}]", imp));
            }
            if let Some(t) = tags {
                parts.push(format!("[{}]", t));
            }
            parts.join(" ")
        }
        other => other.to_string(),
    }
}
