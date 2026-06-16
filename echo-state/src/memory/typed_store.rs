//! Typed memory store — wraps a `Store` with structured metadata support.
//!
//! Provides typed read/write operations that automatically serialize/deserialize
//! `MemoryMeta` alongside content in `StoreItem.value`. Backward-compatible:
//! untyped entries (raw JSON without the `meta` field) are read with default metadata.

use echo_core::error::Result;
use echo_core::memory::store::{Store, StoreItem};
use echo_core::memory::types::{
    MemoryMeta, MemoryRisk, MemorySource, MemoryStatus, MemoryType, TypedMemoryValue,
};
use futures::future::BoxFuture;
use std::sync::Arc;

// ── MemoryFilter ────────────────────────────────────────────────────────

/// Filter criteria for searching typed memories.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Filter by memory type.
    pub memory_type: Option<MemoryType>,
    /// Filter by status.
    pub status: Option<MemoryStatus>,
    /// Minimum confidence threshold.
    pub min_confidence: Option<f32>,
    /// Filter by topic (exact match).
    pub topic: Option<String>,
    /// Filter by source.
    pub source: Option<MemorySource>,
    /// Maximum risk level (inclusive).
    pub max_risk: Option<MemoryRisk>,
}

impl MemoryFilter {
    /// Create an empty filter (matches everything).
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by memory type.
    pub fn with_type(mut self, memory_type: MemoryType) -> Self {
        self.memory_type = Some(memory_type);
        self
    }

    /// Filter by status.
    pub fn with_status(mut self, status: MemoryStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Filter by minimum confidence.
    pub fn with_min_confidence(mut self, confidence: f32) -> Self {
        self.min_confidence = Some(confidence);
        self
    }

    /// Filter by topic.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Filter by source.
    pub fn with_source(mut self, source: MemorySource) -> Self {
        self.source = Some(source);
        self
    }

    /// Filter by maximum risk level.
    pub fn with_max_risk(mut self, risk: MemoryRisk) -> Self {
        self.max_risk = Some(risk);
        self
    }

    /// Check if a memory entry matches this filter.
    pub fn matches(&self, entry: &TypedMemoryEntry) -> bool {
        if let Some(ref mt) = self.memory_type {
            if entry.meta.memory_type != *mt {
                return false;
            }
        }
        if let Some(ref st) = self.status {
            if entry.meta.status != *st {
                return false;
            }
        }
        if let Some(min_c) = self.min_confidence {
            if entry.meta.confidence < min_c {
                return false;
            }
        }
        if let Some(ref topic) = self.topic {
            if entry.meta.topic != *topic {
                return false;
            }
        }
        if let Some(ref src) = self.source {
            if entry.meta.source != *src {
                return false;
            }
        }
        if let Some(max_r) = self.max_risk {
            if risk_order(&entry.meta.risk) > risk_order(&max_r) {
                return false;
            }
        }
        true
    }
}

// ── TypedMemoryEntry ────────────────────────────────────────────────────

/// A typed memory entry with its key, content, metadata, and raw StoreItem.
#[derive(Debug, Clone)]
pub struct TypedMemoryEntry {
    /// The key within the namespace.
    pub key: String,
    /// The memory content text.
    pub content: String,
    /// Structured metadata.
    pub meta: MemoryMeta,
    /// The raw StoreItem (for access to timestamps, importance, etc.).
    pub raw: StoreItem,
}

impl TypedMemoryEntry {
    /// Create a TypedMemoryEntry from a StoreItem, parsing the typed value.
    ///
    /// If the StoreItem value doesn't contain the typed format (no `meta` field),
    /// creates an entry with default metadata, treating the entire value as content.
    pub fn from_store_item(item: StoreItem) -> Self {
        let key = item.key.clone();

        // Try to parse as TypedMemoryValue
        if let Ok(typed) = TypedMemoryValue::from_value(&item.value) {
            Self {
                key,
                content: typed.content,
                meta: typed.meta,
                raw: item,
            }
        } else {
            // Backward compatibility: treat as untyped entry
            // Try to extract a string content from the value
            let content = extract_content_fallback(&item.value);
            Self {
                key,
                content,
                meta: MemoryMeta::default(),
                raw: item,
            }
        }
    }
}

/// Extract a human-readable content string from a non-typed StoreItem value.
fn extract_content_fallback(value: &serde_json::Value) -> String {
    // If it's a simple string
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    // If it has a "content" field
    if let Some(content) = value.get("content").and_then(|v| v.as_str()) {
        return content.to_string();
    }
    // If it has a "review" field (from BackgroundReviewer)
    if let Some(review) = value.get("review").and_then(|v| v.as_str()) {
        return review.to_string();
    }
    // Fall back to JSON string
    value.to_string()
}

/// Numeric ordering for risk comparison (Low=0 < Medium=1 < High=2).
fn risk_order(risk: &MemoryRisk) -> u8 {
    match risk {
        MemoryRisk::Low => 0,
        MemoryRisk::Medium => 1,
        MemoryRisk::High => 2,
    }
}

// ── TypedMemoryStore ────────────────────────────────────────────────────

/// A typed wrapper around any `Store` that adds structured metadata support.
///
/// Typed memories are stored as JSON in `StoreItem.value` with the format:
/// ```json
/// { "content": "...", "meta": { "memory_type": "...", ... } }
/// ```
///
/// The underlying `Store` is unchanged — `TypedMemoryStore` is a thin
/// serialization/deserialization layer on top.
#[derive(Clone)]
pub struct TypedMemoryStore {
    inner: Arc<dyn Store>,
}

impl TypedMemoryStore {
    /// Create a new typed store wrapping the given Store.
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { inner: store }
    }

    /// Write a typed memory entry.
    pub fn put_typed<'a>(
        &'a self,
        namespace: &'a [&'a str],
        key: &'a str,
        content: &'a str,
        meta: MemoryMeta,
    ) -> BoxFuture<'a, Result<()>> {
        let value = TypedMemoryValue::new(content, meta);
        let json_value = match value.to_value() {
            Ok(v) => v,
            Err(e) => {
                return Box::pin(async move {
                    Err(echo_core::error::MemoryError::SerializationError(e.to_string()).into())
                });
            }
        };
        self.inner.put(namespace, key, json_value)
    }

    /// Read a typed memory entry by key.
    pub async fn get_typed(
        &self,
        namespace: &[&str],
        key: &str,
    ) -> Result<Option<TypedMemoryEntry>> {
        match self.inner.get(namespace, key).await? {
            Some(item) => Ok(Some(TypedMemoryEntry::from_store_item(item))),
            None => Ok(None),
        }
    }

    /// Search typed memories with keyword query and optional filter.
    pub async fn search_typed(
        &self,
        namespace: &[&str],
        query: &str,
        limit: usize,
        filter: &MemoryFilter,
    ) -> Result<Vec<TypedMemoryEntry>> {
        let items = self.inner.search(namespace, query, limit * 3).await?;
        let mut entries: Vec<TypedMemoryEntry> = items
            .into_iter()
            .map(TypedMemoryEntry::from_store_item)
            .filter(|e| filter.matches(e))
            .collect();

        // Sort by confidence descending, then by recency
        entries.sort_by(|a, b| {
            b.meta
                .confidence
                .partial_cmp(&a.meta.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        entries.truncate(limit);
        Ok(entries)
    }

    /// List all typed memories in a namespace, with optional filter.
    pub async fn list_typed(
        &self,
        namespace: &[&str],
        filter: &MemoryFilter,
    ) -> Result<Vec<TypedMemoryEntry>> {
        let items = self.inner.list(namespace).await?;
        Ok(items
            .into_iter()
            .map(TypedMemoryEntry::from_store_item)
            .filter(|e| filter.matches(e))
            .collect())
    }

    /// Delete a typed memory entry.
    pub async fn delete_typed(&self, namespace: &[&str], key: &str) -> Result<bool> {
        self.inner.delete(namespace, key).await
    }

    /// Update the metadata of an existing typed memory entry.
    ///
    /// Returns `Ok(false)` if the entry doesn't exist.
    pub async fn update_meta(
        &self,
        namespace: &[&str],
        key: &str,
        meta: MemoryMeta,
    ) -> Result<bool> {
        match self.inner.get(namespace, key).await? {
            Some(item) => {
                // Parse existing content
                let content = if let Ok(typed) = TypedMemoryValue::from_value(&item.value) {
                    typed.content
                } else {
                    extract_content_fallback(&item.value)
                };

                let new_value = TypedMemoryValue::new(content, meta);
                let json_value = new_value.to_value().map_err(|e| {
                    echo_core::error::MemoryError::SerializationError(e.to_string())
                })?;
                self.inner.put(namespace, key, json_value).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Get a reference to the underlying Store.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::memory::types::{MemoryRisk, MemorySource, MemoryStatus, MemoryType};

    fn make_state_store() -> Arc<dyn Store> {
        Arc::new(crate::memory::store::InMemoryStore::new())
    }

    #[tokio::test]
    async fn test_put_and_get_typed() {
        let store = make_state_store();
        let typed = TypedMemoryStore::new(store);

        let meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        );
        typed
            .put_typed(&["test", "memories"], "m1", "Maven needs Java 8", meta)
            .await
            .unwrap();

        let entry = typed
            .get_typed(&["test", "memories"], "m1")
            .await
            .unwrap()
            .expect("entry should exist");

        assert_eq!(entry.content, "Maven needs Java 8");
        assert_eq!(entry.meta.memory_type, MemoryType::DebuggingLesson);
        assert_eq!(entry.meta.source, MemorySource::ErrorResolution);
        assert_eq!(entry.meta.topic, "build");
    }

    #[tokio::test]
    async fn test_search_with_filter() {
        let store = make_state_store();
        let typed = TypedMemoryStore::new(store);

        // Insert two memories with different types
        let meta1 = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        );
        typed
            .put_typed(&["test", "memories"], "m1", "Maven compile error", meta1)
            .await
            .unwrap();

        let meta2 = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        );
        typed
            .put_typed(
                &["test", "memories"],
                "m2",
                "User prefers concise answers",
                meta2,
            )
            .await
            .unwrap();

        // Filter by type
        let filter = MemoryFilter::new().with_type(MemoryType::UserPreference);
        let results = typed
            .search_typed(&["test", "memories"], "answers", 10, &filter)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].meta.memory_type, MemoryType::UserPreference);
    }

    #[tokio::test]
    async fn test_list_with_filter() {
        let store = make_state_store();
        let typed = TypedMemoryStore::new(store);

        let meta_active = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        )
        .with_confidence(0.9);
        typed
            .put_typed(&["test", "memories"], "m1", "Uses npm", meta_active)
            .await
            .unwrap();

        let meta_draft = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        )
        .with_confidence(0.3)
        .with_status(MemoryStatus::Draft);
        typed
            .put_typed(&["test", "memories"], "m2", "Maybe uses yarn", meta_draft)
            .await
            .unwrap();

        // Filter by min_confidence
        let filter = MemoryFilter::new().with_min_confidence(0.8);
        let results = typed
            .list_typed(&["test", "memories"], &filter)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Uses npm");
    }

    #[tokio::test]
    async fn test_backward_compat_read() {
        let store = make_state_store();

        // Write a raw (untyped) entry
        let raw_value = serde_json::json!({
            "review": "Some review content",
            "run_id": "test-123"
        });
        store.put(&["test", "raw"], "r1", raw_value).await.unwrap();

        // Read through TypedMemoryStore
        let typed = TypedMemoryStore::new(store);
        let entry = typed
            .get_typed(&["test", "raw"], "r1")
            .await
            .unwrap()
            .expect("entry should exist");

        // Should parse with default metadata
        assert!(entry.content.contains("Some review content"));
        assert_eq!(entry.meta.memory_type, MemoryType::ProjectFact); // default
    }

    #[tokio::test]
    async fn test_update_meta() {
        let store = make_state_store();
        let typed = TypedMemoryStore::new(store);

        let meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        );
        typed
            .put_typed(&["test", "memories"], "m1", "Maven needs Java 8", meta)
            .await
            .unwrap();

        // Update status to Superseded
        let new_meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        )
        .with_status(MemoryStatus::Superseded)
        .with_confidence(0.5);
        let updated = typed
            .update_meta(&["test", "memories"], "m1", new_meta)
            .await
            .unwrap();
        assert!(updated);

        let entry = typed
            .get_typed(&["test", "memories"], "m1")
            .await
            .unwrap()
            .expect("entry should exist");
        assert_eq!(entry.meta.status, MemoryStatus::Superseded);
        assert_eq!(entry.content, "Maven needs Java 8"); // content preserved
    }

    #[tokio::test]
    async fn test_delete_typed() {
        let store = make_state_store();
        let typed = TypedMemoryStore::new(store);

        let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::AutoExtracted, "test");
        typed
            .put_typed(&["test", "memories"], "m1", "Some fact", meta)
            .await
            .unwrap();

        let deleted = typed
            .delete_typed(&["test", "memories"], "m1")
            .await
            .unwrap();
        assert!(deleted);

        let entry = typed.get_typed(&["test", "memories"], "m1").await.unwrap();
        assert!(entry.is_none());
    }

    #[test]
    fn test_memory_filter_matches() {
        let meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        )
        .with_confidence(0.85)
        .with_status(MemoryStatus::Active);

        let entry = TypedMemoryEntry {
            key: "m1".to_string(),
            content: "test".to_string(),
            meta,
            raw: echo_core::memory::store::StoreItem::new(
                vec!["test".to_string()],
                "m1".to_string(),
                serde_json::json!({}),
            ),
        };

        // Should match type filter
        assert!(
            MemoryFilter::new()
                .with_type(MemoryType::DebuggingLesson)
                .matches(&entry)
        );
        // Should not match different type
        assert!(
            !MemoryFilter::new()
                .with_type(MemoryType::UserPreference)
                .matches(&entry)
        );
        // Should match min_confidence
        assert!(MemoryFilter::new().with_min_confidence(0.8).matches(&entry));
        assert!(!MemoryFilter::new().with_min_confidence(0.9).matches(&entry));
        // Should match topic
        assert!(MemoryFilter::new().with_topic("build").matches(&entry));
        assert!(!MemoryFilter::new().with_topic("style").matches(&entry));
    }

    #[test]
    fn test_risk_ordering() {
        assert!(risk_order(&MemoryRisk::Low) < risk_order(&MemoryRisk::Medium));
        assert!(risk_order(&MemoryRisk::Medium) < risk_order(&MemoryRisk::High));
        assert!(risk_order(&MemoryRisk::Low) < risk_order(&MemoryRisk::High));
    }
}
