//! Agent memory tools: remember / recall / forget
//!
//! Uses LangGraph-aligned [`Store`] API for persistent long-term memory.
//!
//! | Tool       | Store Operation                              |
//! |------------|----------------------------------------------|
//! | `remember` | `MemoryLayerManager::write_memory()` when layered memory is installed |
//! | `recall`   | `store.search(namespace, query, limit)`      |
//! | `forget`   | `store.delete(namespace, key)`              |

use futures::future::BoxFuture;

use crate::error::ToolError;
use crate::evolution::{MemoryLayer, MemoryLayerManager};
use crate::memory::{SearchQuery, Store, StoreItem};
use crate::tools::{Tool, ToolParameters, ToolResult};
use echo_core::memory::types::{MemoryMeta, MemorySource, MemoryType};
use echo_core::tools::pagination::PageRequest;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

// ── LegacyStoreRememberTool ─────────────────────────────────────────────────

/// Legacy store-backed remember tool.
///
/// New runtime paths should install [`LayeredRememberTool`] so explicit saves
/// go through `MemoryLayerManager::write_memory`. This legacy variant remains
/// only for agents that enable the old Store memory tools without a layer
/// manager.
pub struct LegacyStoreRememberTool {
    pub store: Arc<dyn Store>,
    /// Storage namespace, e.g. `["alice", "memories"]`
    pub namespace: Vec<String>,
}

impl LegacyStoreRememberTool {
    pub fn new(store: Arc<dyn Store>, namespace: Vec<String>) -> Self {
        Self { store, namespace }
    }

    fn ns_refs(&self) -> Vec<&str> {
        self.namespace.iter().map(String::as_str).collect()
    }
}

impl Tool for LegacyStoreRememberTool {
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

// ── LayeredRememberTool ────────────────────────────────────────────────────

/// Store memory through the evolution layer manager.
///
/// This preserves the public `remember` tool contract while routing writes
/// through typed memory, security checks, audit logging, promotion, and the
/// shared review counter.
pub struct LayeredRememberTool {
    pub layer_manager: Arc<MemoryLayerManager>,
}

impl LayeredRememberTool {
    pub fn new(layer_manager: Arc<MemoryLayerManager>) -> Self {
        Self { layer_manager }
    }
}

impl Tool for LayeredRememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "Store information worth long-term retention into persistent typed memory. \
         Suitable for user preferences, project facts, decisions, debugging lessons, and durable workflow patterns."
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

            let memory_type = classify_memory_type(content, &tags);
            let topic = tags
                .first()
                .cloned()
                .unwrap_or_else(|| memory_type_topic(memory_type).to_string());
            let confidence = (0.55 + importance as f32 * 0.045).clamp(0.0, 1.0);
            let meta = MemoryMeta::new(memory_type, MemorySource::ExplicitSave, topic)
                .with_confidence(confidence);
            let key = uuid::Uuid::new_v4().to_string();

            self.layer_manager.write_memory(&key, content, meta).await?;

            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!(" (tags: {})", tags.join(", "))
            };
            Ok(ToolResult::success(format!(
                "✅ Remembered in typed memory (ID: {}, importance: {}): \"{}\"{}",
                key.get(..8).unwrap_or(&key),
                importance,
                content,
                tag_str
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
        "Search persistent memory by keyword or natural-language fragment."
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
        "Hybrid search persistent memory, with keyword fallback."
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
                    "description": "Results per page (default 5)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Cursor from the previous page"
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

            let page_request = match PageRequest::from_parameters(&parameters, 5, 20) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };

            debug!(query = %query, limit = page_request.limit, "🔎 search_memory hybrid search on Store");

            let ns: Vec<&str> = self.ns_refs();
            let items = match self
                .store
                .search_with(&ns, SearchQuery::hybrid(query, 20))
                .await
            {
                Ok(items) => items,
                Err(err) if format!("{err}").contains("hybrid search") => {
                    self.store.search(&ns, query, 20).await?
                }
                Err(err) => return Err(err),
            };

            let records = items
                .iter()
                .map(|item| {
                    format!(
                        "[ID:{}] {}",
                        memory_key_preview(&item.key),
                        format_store_item(item)
                    )
                })
                .collect();
            let query_identity = serde_json::json!({
                "query": query,
                "namespace": self.namespace,
                "mode": "hybrid",
            });
            paginated_memory_result(&page_request, records, &query_identity, query)
        })
    }
}

// ── LayeredRecallTool / LayeredSearchMemoryTool ────────────────────────────

pub struct LayeredRecallTool {
    pub layer_manager: Arc<MemoryLayerManager>,
}

impl LayeredRecallTool {
    pub fn new(layer_manager: Arc<MemoryLayerManager>) -> Self {
        Self { layer_manager }
    }
}

impl Tool for LayeredRecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn description(&self) -> &str {
        "Search typed long-term memory across hot and warm layers."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords or description, e.g. \"user preferences\" or \"project convention\""
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

            let items = self.layer_manager.search_layered(query, limit).await?;
            if items.is_empty() {
                return Ok(ToolResult::success(format!(
                    "No memories found matching \"{}\".",
                    query
                )));
            }

            let mut lines = vec![format!("Found {} matching typed memories:", items.len())];
            for (i, (layer, item)) in items.iter().enumerate() {
                lines.push(format!(
                    "{}. [{}:{}] {}",
                    i + 1,
                    format_memory_layer(*layer),
                    item.key.get(..8).unwrap_or(&item.key),
                    format_typed_memory(item),
                ));
            }
            Ok(ToolResult::success(lines.join("\n")))
        })
    }
}

pub struct LayeredSearchMemoryTool {
    pub layer_manager: Arc<MemoryLayerManager>,
}

impl LayeredSearchMemoryTool {
    pub fn new(layer_manager: Arc<MemoryLayerManager>) -> Self {
        Self { layer_manager }
    }
}

impl Tool for LayeredSearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }

    fn description(&self) -> &str {
        "Search typed long-term memory across hot and warm layers."
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
                    "description": "Results per page (default 5)"
                },
                "cursor": {
                    "type": "string",
                    "description": "Cursor from the previous page"
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
            let page_request = match PageRequest::from_parameters(&parameters, 5, 20) {
                Ok(request) => request,
                Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
            };
            let items = self.layer_manager.search_layered(query, 20).await?;
            let records = items
                .iter()
                .map(|(layer, item)| {
                    format!(
                        "[{}:{}] {}",
                        format_memory_layer(*layer),
                        memory_key_preview(&item.key),
                        format_typed_memory(item)
                    )
                })
                .collect();
            let query_identity = serde_json::json!({
                "query": query,
                "mode": "layered_hybrid",
            });
            paginated_memory_result(&page_request, records, &query_identity, query)
        })
    }
}

fn paginated_memory_result(
    request: &PageRequest,
    records: Vec<String>,
    query_identity: &Value,
    query: &str,
) -> crate::error::Result<ToolResult> {
    let (page, page_info) = match request.paginate(records, query_identity) {
        Ok(page) => page,
        Err(error) => return Ok(ToolResult::invalid_arguments(error.to_string())),
    };
    let output = if page.is_empty() {
        format!("No memories found matching \"{query}\".")
    } else {
        format!(
            "Found {} matching memories on this page ({} total):\n{}",
            page_info.returned,
            page_info.total.unwrap_or(0),
            page.join("\n")
        )
    };
    let mut result = ToolResult::success(output);
    page_info.apply_to(&mut result);
    Ok(result)
}

fn memory_key_preview(key: &str) -> String {
    key.chars().take(8).collect()
}

// ── LayeredForgetTool ──────────────────────────────────────────────────────

/// Delete a typed long-term memory via the evolution layer manager (C4 fix).
///
/// Routes through [`MemoryLayerManager::delete_memory`] so the unified
/// namespace `["agent", "memories"]` is used — matching where
/// [`LayeredRememberTool`] writes. This replaces the legacy `ForgetTool` when a
/// layer manager is installed; without it, `forget` would delete from the
/// per-agent namespace and never remove what `remember` stored.
///
/// Accepts either the full key (UUID) or an 8-char prefix as shown in recall
/// results. If the prefix is ambiguous (multiple matches), no deletion happens
/// and the matches are listed for disambiguation.
pub struct LayeredForgetTool {
    pub layer_manager: Arc<MemoryLayerManager>,
}

impl LayeredForgetTool {
    pub fn new(layer_manager: Arc<MemoryLayerManager>) -> Self {
        Self { layer_manager }
    }
}

/// Resolve a user-supplied id (full key or short prefix) to at most one
/// concrete memory key.
///
/// Returns `Ok(None)` when nothing matches; `Err(InvalidParameter)` when the
/// prefix is ambiguous (caller asks the user to disambiguate).
async fn resolve_forget_key(
    layer_manager: &Arc<MemoryLayerManager>,
    id: &str,
) -> crate::error::Result<Option<String>> {
    // Fast path: exact match.
    if layer_manager.locate(id).await.is_some() {
        return Ok(Some(id.to_string()));
    }
    // Slow path: treat `id` as a prefix and scan both layers.
    let mut matches = Vec::new();
    for entry in layer_manager.list_hot() {
        if entry.key.starts_with(id) {
            matches.push(entry.key);
        }
    }
    if let Ok(warm) = layer_manager
        .list_warm_memories(&echo_state::memory::typed_store::MemoryFilter::new())
        .await
    {
        for entry in warm {
            if entry.key.starts_with(id) {
                matches.push(entry.key);
            }
        }
    }
    matches.sort();
    matches.dedup();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(ToolError::InvalidParameter {
            name: "id".to_string(),
            message: format!(
                "ID prefix '{id}' matches {} memories; provide more characters. Matches: {}",
                matches.len(),
                matches
                    .iter()
                    .map(|k| k.chars().take(8).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
        .into()),
    }
}

impl Tool for LayeredForgetTool {
    fn name(&self) -> &str {
        "forget"
    }

    fn description(&self) -> &str {
        "Delete a typed long-term memory by its ID (full key or the 8-char prefix shown in recall results). \
         If the prefix matches multiple memories, no deletion happens and the matches are listed for disambiguation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Memory ID to delete (full key or first 8 chars from recall results)"
                }
            },
            "required": ["id"]
        })
    }

    fn execute(
        &self,
        parameters: ToolParameters,
    ) -> BoxFuture<'_, crate::error::Result<ToolResult>> {
        let layer_manager = self.layer_manager.clone();
        Box::pin(async move {
            let id = parameters
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("id".to_string()))?;

            // Resolve full key (exact or unambiguous prefix).
            let key = match resolve_forget_key(&layer_manager, id).await? {
                Some(k) => k,
                None => {
                    return Ok(ToolResult::success(format!(
                        "No memory entry found with ID \"{id}\", nothing to delete.\n\
                         Tip: use the recall tool to find the correct ID."
                    )));
                }
            };

            let deleted = layer_manager.delete_memory(&key).await?;
            if deleted {
                Ok(ToolResult::success(format!(
                    "🗑️ Deleted memory ID: {}",
                    key.chars().take(8).collect::<String>()
                )))
            } else {
                Ok(ToolResult::success(format!(
                    "No memory entry found with ID \"{id}\", nothing to delete."
                )))
            }
        })
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

fn classify_memory_type(content: &str, tags: &[String]) -> MemoryType {
    let lower = format!(
        "{} {}",
        content.to_lowercase(),
        tags.join(" ").to_lowercase()
    );
    if lower.contains("prefer")
        || lower.contains("always")
        || lower.contains("never")
        || lower.contains("style")
        || lower.contains("user")
    {
        MemoryType::UserPreference
    } else if lower.contains("decision")
        || lower.contains("decided")
        || lower.contains("architecture")
        || lower.contains("choose")
    {
        MemoryType::ArchitectureDecision
    } else if lower.contains("error")
        || lower.contains("bug")
        || lower.contains("fix")
        || lower.contains("failed")
    {
        MemoryType::DebuggingLesson
    } else if lower.contains("command") || lower.contains("cargo") || lower.contains("npm") {
        MemoryType::CommandPattern
    } else if lower.contains("workflow") || lower.contains("process") {
        MemoryType::WorkflowPattern
    } else {
        MemoryType::ProjectFact
    }
}

fn memory_type_topic(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::UserPreference => "user",
        MemoryType::ProjectFact => "project",
        MemoryType::ArchitectureDecision => "architecture",
        MemoryType::DebuggingLesson | MemoryType::ErrorResolution => "debugging",
        MemoryType::CommandPattern => "commands",
        MemoryType::ToolUsage => "tools",
        MemoryType::WorkflowPattern | MemoryType::SkillCandidate => "workflow",
        MemoryType::DeprecatedNote => "deprecated",
    }
}

fn format_memory_layer(layer: MemoryLayer) -> &'static str {
    match layer {
        MemoryLayer::Hot => "hot",
        MemoryLayer::Warm => "warm",
        MemoryLayer::Cold => "cold",
    }
}

fn format_typed_memory(item: &echo_state::memory::typed_store::TypedMemoryEntry) -> String {
    format!(
        "{} (type: {:?}, confidence: {:.0}%, topic: {})",
        item.content,
        item.meta.memory_type,
        item.meta.confidence * 100.0,
        item.meta.topic
    )
}

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
