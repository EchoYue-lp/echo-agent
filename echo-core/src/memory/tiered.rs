//! Tiered Memory — four-layer memory architecture.
//!
//! Inspired by Letta's memory hierarchy:
//!
//! | Layer     | Storage         | Lifespan     | Purpose                             |
//! |-----------|-----------------|--------------|-------------------------------------|
//! | Working   | Context window  | Current turn | Active conversation messages        |
//! | ShortTerm | Recent summary  | Minutes      | Last N turns summarized             |
//! | LongTerm  | Store (DB)      | Days–months  | Searchable episodic memories        |
//! | Core      | System prompt   | Permanent    | Identity, preferences, goals        |
//!
//! Memories flow down the hierarchy automatically via summarization and
//! periodic reflection. Short-term entries carry metadata (importance,
//! timestamps, tags) for relevance-based recall. The overflow queue is
//! bounded to prevent silent data loss.

use super::core_memory::CoreMemory;
use super::decay::{should_prune, sort_by_decayed_score};
use super::store::{Store, StoreItem};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::warn;

/// Default maximum overflow queue size before LRU eviction.
const DEFAULT_MAX_OVERFLOW: usize = 100;

/// A structured short-term memory entry with metadata.
///
/// Unlike bare `String` summaries, `MemoryEntry` carries importance,
/// timestamps, and semantic tags — enabling relevance-based recall
/// and importance-weighted context injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// The memory content (summarized conversation turn, reflection, etc.)
    pub content: String,
    /// Importance score (1.0–10.0). Higher = more likely to be kept
    /// in context injection and less likely to be evicted.
    pub importance: f64,
    /// When this entry was created.
    pub timestamp: DateTime<Utc>,
    /// Semantic tags for keyword-based recall (e.g. ["rust", "error", "debug"]).
    pub tags: Vec<String>,
    /// Origin of this entry: "conversation", "reflection", "tool_result", "overflow".
    pub source: String,
}

impl MemoryEntry {
    /// Create a new memory entry with the given content and importance.
    pub fn new(content: String, importance: f64, tags: Vec<String>, source: String) -> Self {
        Self {
            content,
            importance: importance.clamp(1.0, 10.0),
            timestamp: Utc::now(),
            tags,
            source,
        }
    }

    /// Create a simple entry with default importance (5.0) and no tags.
    ///
    /// Backward-compatible convenience for callers that don't need metadata.
    pub fn simple(content: String) -> Self {
        Self {
            content,
            importance: 5.0,
            timestamp: Utc::now(),
            tags: vec![],
            source: "conversation".to_string(),
        }
    }

    /// Whether this entry's content or tags contain the given keyword.
    pub fn matches_keyword(&self, query: &str) -> bool {
        let lower = query.to_lowercase();
        self.content.to_lowercase().contains(&lower)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&lower))
    }
}

/// Tiered memory manager combining all four memory layers.
///
/// Each layer has a distinct storage mechanism and lifespan:
/// - **Core**: Fixed blocks always injected into the system prompt
/// - **ShortTerm**: Recent `MemoryEntry` items with metadata, capped at `max_short_term`
/// - **Overflow**: Bounded queue of evicted short-term entries awaiting async flush
/// - **LongTerm**: Optional `Store` for persistent, searchable memories
pub struct TieredMemory {
    /// Core memory — always injected into the system prompt
    pub core: CoreMemory,
    /// Short-term memory — recent structured entries (max N entries)
    pub short_term: Vec<MemoryEntry>,
    /// Maximum short-term entries before eviction to overflow
    pub max_short_term: usize,
    /// Long-term store (optional — None means no persistence)
    pub long_term: Option<Arc<dyn Store>>,
    /// Overflow queue: entries evicted from short-term, bounded at `max_overflow`
    pub overflow_queue: Vec<MemoryEntry>,
    /// Maximum overflow queue size. When exceeded with no store attached,
    /// lowest-importance entries are evicted (LRU by importance).
    pub max_overflow: usize,
}

impl TieredMemory {
    /// Create a new tiered memory manager with default overflow bound (100).
    pub fn new(max_short_term: usize, max_core_chars: usize) -> Self {
        Self {
            core: CoreMemory::new(max_core_chars),
            short_term: Vec::new(),
            max_short_term,
            long_term: None,
            overflow_queue: Vec::new(),
            max_overflow: DEFAULT_MAX_OVERFLOW,
        }
    }

    /// Create with explicit overflow bound.
    pub fn with_overflow_bound(mut self, max_overflow: usize) -> Self {
        self.max_overflow = max_overflow;
        self
    }

    /// Attach a long-term Store for persistence.
    pub fn with_store(mut self, store: Arc<dyn Store>) -> Self {
        self.long_term = Some(store);
        self
    }

    /// Add a structured short-term memory entry.
    ///
    /// Oldest entries are evicted to the overflow_queue when `max_short_term`
    /// is exceeded. Call [`flush_overflow`] to persist them to the long-term store.
    pub fn add_short_term(&mut self, entry: MemoryEntry) {
        self.short_term.push(entry);
        if self.short_term.len() > self.max_short_term {
            // Evict lowest-importance entry (not just the oldest)
            if let Some(idx) = self
                .short_term
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.importance.partial_cmp(&b.importance).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
            {
                let evicted = self.short_term.remove(idx);
                self.push_overflow(evicted);
            }
        }
    }

    /// Add a simple short-term memory entry (backward-compatible convenience).
    ///
    /// Uses default importance (5.0), no tags, source "conversation".
    pub fn add_short_term_simple(&mut self, summary: String) {
        self.add_short_term(MemoryEntry::simple(summary));
    }

    /// Push an entry into the overflow queue, respecting the bound.
    ///
    /// When the queue is full and no long-term store is attached,
    /// the lowest-importance entry is evicted with a warning.
    fn push_overflow(&mut self, entry: MemoryEntry) {
        if self.overflow_queue.len() >= self.max_overflow {
            if self.long_term.is_none() {
                // No store attached — evict lowest-importance to avoid silent data loss
                if let Some(idx) = self
                    .overflow_queue
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.importance.partial_cmp(&b.importance).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
                {
                    let evicted = self.overflow_queue.remove(idx);
                    warn!(
                        "Overflow queue full (max {}), evicted entry (importance={:.1}): {:.50}",
                        self.max_overflow, evicted.importance, evicted.content
                    );
                }
            }
            // If store is attached, we'll flush soon; allow queue to exceed briefly
        }
        self.overflow_queue.push(entry);
    }

    /// Persist overflowed short-term memories to the long-term store.
    ///
    /// Drains the overflow queue and writes each entry as a [`StoreItem`]
    /// with key `short_term_{uuid}` in the `memories/short_term` namespace.
    /// Returns the number of entries successfully flushed.
    ///
    /// If no store is attached, logs a warning and evicts lowest-importance
    /// entries from the bounded queue instead of silently dropping data.
    pub async fn flush_overflow(&mut self) -> usize {
        let store = match &self.long_term {
            Some(s) => s,
            None => {
                let count = self.overflow_queue.len();
                if count > 0 {
                    warn!(
                        "No long-term store attached; {} overflow entries will be retained in bounded queue (max {})",
                        count, self.max_overflow
                    );
                }
                // Trim to bound by evicting lowest-importance entries
                while self.overflow_queue.len() > self.max_overflow {
                    if let Some(idx) = self
                        .overflow_queue
                        .iter()
                        .enumerate()
                        .min_by(|(_, a), (_, b)| a.importance.partial_cmp(&b.importance).unwrap_or(std::cmp::Ordering::Equal))
                        .map(|(i, _)| i)
                    {
                        self.overflow_queue.remove(idx);
                    }
                }
                return count;
            }
        };

        let mut flushed = 0;
        for entry in self.overflow_queue.drain(..) {
            let key = format!("short_term_{}", uuid::Uuid::new_v4());
            let value = serde_json::json!({
                "content": entry.content,
                "importance": entry.importance,
                "timestamp": entry.timestamp.to_rfc3339(),
                "tags": entry.tags,
                "source": entry.source,
            });
            let item = StoreItem::with_importance(
                vec!["memories".into(), "short_term".into()],
                key,
                value,
                entry.importance as f32,
            );
            if store.put(&["memories", "short_term"], &item.key, item.value).await.is_ok() {
                flushed += 1;
            }
        }
        flushed
    }

    /// Build the full context injection string from Core + ShortTerm memory.
    ///
    /// Short-term entries are injected in importance-weighted order,
    /// not just FIFO. This ensures the most relevant context is visible
    /// to the agent even when the budget is constrained.
    pub fn build_context_injection(&self) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(core) = self.core.to_system_prompt_fragment() {
            parts.push(core);
        }

        if !self.short_term.is_empty() {
            parts.push("## Recent Context".to_string());
            // Sort by importance descending for relevance-weighted injection
            let sorted: Vec<&MemoryEntry> = self.short_term_by_importance();
            for (i, entry) in sorted.iter().enumerate() {
                parts.push(format!("{}. {}", i + 1, entry.content));
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Recall short-term entries matching a keyword query.
    ///
    /// Searches both content and tags for the query string.
    /// Returns entries sorted by importance (descending), limited to `limit`.
    pub fn recall(&self, query: &str, limit: usize) -> Vec<&MemoryEntry> {
        let mut matches: Vec<&MemoryEntry> = self
            .short_term
            .iter()
            .filter(|e| e.matches_keyword(query))
            .collect();
        matches.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(limit);
        matches
    }

    /// Recall from long-term store using the Store's keyword search interface.
    ///
    /// Returns `None` if no long-term store is attached.
    pub async fn recall_from_long_term(
        &self,
        query: &str,
        limit: usize,
    ) -> Option<Vec<StoreItem>> {
        let store = self.long_term.as_ref()?;
        Some(store.search(&["memories", "short_term"], query, limit).await.unwrap_or_default())
    }

    /// Get short-term entries sorted by importance (descending).
    pub fn short_term_by_importance(&self) -> Vec<&MemoryEntry> {
        let mut entries: Vec<&MemoryEntry> = self.short_term.iter().collect();
        entries.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries
    }

    /// Threshold at which automatic summarization should be triggered.
    ///
    /// Returns `max_short_term * 2` as the threshold. When the total number
    /// of short-term + overflow entries exceeds this, an LLM summarization
    /// pass should compress the older entries.
    pub fn auto_summarize_threshold(&self) -> usize {
        self.max_short_term * 2
    }

    /// Whether auto-summarization should be triggered now.
    pub fn needs_summarization(&self) -> bool {
        self.short_term.len() + self.overflow_queue.len() >= self.auto_summarize_threshold()
    }

    /// Total entries in short-term + overflow.
    pub fn total_pending_entries(&self) -> usize {
        self.short_term.len() + self.overflow_queue.len()
    }

    /// Prune long-term memories that fall below the decay threshold.
    ///
    /// This is a best-effort sync operation; actual deletion requires async.
    pub fn prune_candidates(&self, items: &[StoreItem]) -> Vec<String> {
        items
            .iter()
            .filter(|i| should_prune(i))
            .map(|i| i.key.clone())
            .collect()
    }

    /// Sort and truncate a list of items using decayed importance scoring.
    pub fn rank_by_importance(items: &mut Vec<StoreItem>, limit: usize) {
        sort_by_decayed_score(items, limit);
    }
}

impl Default for TieredMemory {
    fn default() -> Self {
        Self::new(5, 2000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_tiered_memory() {
        let tm = TieredMemory::new(5, 2000);
        assert!(tm.long_term.is_none());
        assert_eq!(tm.short_term.len(), 0);
        assert_eq!(tm.max_overflow, DEFAULT_MAX_OVERFLOW);
    }

    #[test]
    fn test_memory_entry_simple() {
        let entry = MemoryEntry::simple("summary text".to_string());
        assert_eq!(entry.content, "summary text");
        assert_eq!(entry.importance, 5.0);
        assert!(entry.tags.is_empty());
        assert_eq!(entry.source, "conversation");
    }

    #[test]
    fn test_memory_entry_structured() {
        let entry = MemoryEntry::new(
            "found a bug in parser".to_string(),
            8.0,
            vec!["rust".to_string(), "bug".to_string()],
            "tool_result".to_string(),
        );
        assert_eq!(entry.importance, 8.0);
        assert_eq!(entry.tags.len(), 2);
    }

    #[test]
    fn test_memory_entry_keyword_match() {
        let entry = MemoryEntry::new(
            "Rust compilation error".to_string(),
            7.0,
            vec!["rust".to_string(), "error".to_string()],
            "conversation".to_string(),
        );
        assert!(entry.matches_keyword("rust"));
        assert!(entry.matches_keyword("error"));
        assert!(entry.matches_keyword("compilation"));
        assert!(!entry.matches_keyword("python"));
    }

    #[test]
    fn test_short_term_eviction_by_importance() {
        let mut tm = TieredMemory::new(2, 2000);
        // Add low importance first, then high
        tm.add_short_term(MemoryEntry::simple("low importance".to_string())); // importance 5.0
        tm.add_short_term(MemoryEntry::new(
            "high importance".to_string(),
            9.0,
            vec![],
            "conversation".to_string(),
        ));
        assert_eq!(tm.short_term.len(), 2);

        // Adding a third entry should evict the lowest-importance one
        tm.add_short_term(MemoryEntry::new(
            "medium importance".to_string(),
            7.0,
            vec![],
            "conversation".to_string(),
        ));
        assert_eq!(tm.short_term.len(), 2);
        assert_eq!(tm.overflow_queue.len(), 1);
        // The evicted entry should be the low-importance one
        assert_eq!(tm.overflow_queue[0].content, "low importance");
    }

    #[test]
    fn test_add_short_term_simple_backward_compat() {
        let mut tm = TieredMemory::new(3, 2000);
        tm.add_short_term_simple("summary 1".to_string());
        tm.add_short_term_simple("summary 2".to_string());
        assert_eq!(tm.short_term.len(), 2);
        assert_eq!(tm.short_term[0].content, "summary 1");
        assert_eq!(tm.short_term[0].importance, 5.0);
    }

    #[test]
    fn test_overflow_bounded_no_store() {
        let mut tm = TieredMemory::new(1, 2000).with_overflow_bound(3);
        // Fill overflow beyond bound
        for i in 0..5 {
            tm.add_short_term(MemoryEntry::new(
                format!("entry {}", i),
                (i + 1) as f64, // importance 1-5
                vec![],
                "conversation".to_string(),
            ));
        }
        // Without store, overflow is bounded — lowest-importance evicted
        assert!(tm.overflow_queue.len() <= tm.max_overflow);
    }

    #[test]
    fn test_context_injection_importance_order() {
        let mut tm = TieredMemory::new(3, 2000);
        tm.add_short_term(MemoryEntry::simple("low imp".to_string())); // 5.0
        tm.add_short_term(MemoryEntry::new(
            "critical finding".to_string(),
            9.0,
            vec![],
            "conversation".to_string(),
        ));
        tm.add_short_term(MemoryEntry::new(
            "medium note".to_string(),
            7.0,
            vec![],
            "conversation".to_string(),
        ));

        let ctx = tm.build_context_injection().unwrap();
        // High-importance entry should appear first in context
        let critical_pos = ctx.find("critical finding").unwrap();
        let low_pos = ctx.find("low imp").unwrap();
        assert!(critical_pos < low_pos);
    }

    #[test]
    fn test_recall_keyword() {
        let mut tm = TieredMemory::new(5, 2000);
        tm.add_short_term(MemoryEntry::new(
            "Rust parser bug".to_string(),
            8.0,
            vec!["rust".to_string()],
            "conversation".to_string(),
        ));
        tm.add_short_term(MemoryEntry::new(
            "Python data analysis".to_string(),
            5.0,
            vec!["python".to_string()],
            "conversation".to_string(),
        ));

        let rust_entries = tm.recall("rust", 10);
        assert_eq!(rust_entries.len(), 1);
        assert_eq!(rust_entries[0].content, "Rust parser bug");
    }

    #[test]
    fn test_auto_summarize_threshold() {
        let tm = TieredMemory::new(5, 2000);
        assert_eq!(tm.auto_summarize_threshold(), 10); // max_short_term * 2
    }

    #[test]
    fn test_needs_summarization() {
        let mut tm = TieredMemory::new(3, 2000);
        assert!(!tm.needs_summarization());
        for i in 0..6 {
            tm.add_short_term_simple(format!("entry {}", i));
        }
        assert!(tm.needs_summarization()); // 3 short_term + 3 overflow = 6 >= 6
    }

    #[test]
    fn test_overflow_queue_clears_without_store_warns() {
        let mut tm = TieredMemory::new(1, 2000).with_overflow_bound(2);
        tm.add_short_term_simple("entry 1".to_string());
        tm.add_short_term_simple("entry 2".to_string()); // triggers overflow
        assert_eq!(tm.overflow_queue.len(), 1);
    }

    #[test]
    fn test_context_injection() {
        let mut tm = TieredMemory::new(2, 2000);
        tm.core.upsert(
            super::super::core_memory::CoreMemoryBlock::new("1", "name", "Alice")
                .with_importance(8.0),
        );
        tm.add_short_term(MemoryEntry::simple("Previous conversation about Rust".to_string()));

        let ctx = tm.build_context_injection().unwrap();
        assert!(ctx.contains("Alice"));
        assert!(ctx.contains("Rust"));
    }
}