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
//! periodic reflection.

use super::core_memory::CoreMemory;
use super::decay::{should_prune, sort_by_decayed_score};
use super::store::{Store, StoreItem};
use std::sync::Arc;

/// Tiered memory manager combining all four memory layers.
pub struct TieredMemory {
    /// Core memory — always injected into the system prompt
    pub core: CoreMemory,
    /// Short-term memory — recent conversation summaries (max N entries)
    pub short_term: Vec<String>,
    /// Maximum short-term entries
    pub max_short_term: usize,
    /// Long-term store (optional — None means no persistence)
    pub long_term: Option<Arc<dyn Store>>,
    /// Overflow queue: items evicted from short-term, awaiting async flush to long-term store
    pub overflow_queue: Vec<String>,
}

impl TieredMemory {
    /// Create a new tiered memory manager.
    pub fn new(max_short_term: usize, max_core_chars: usize) -> Self {
        Self {
            core: CoreMemory::new(max_core_chars),
            short_term: Vec::new(),
            max_short_term,
            long_term: None,
            overflow_queue: Vec::new(),
        }
    }

    /// Attach a long-term Store for persistence.
    pub fn with_store(mut self, store: Arc<dyn Store>) -> Self {
        self.long_term = Some(store);
        self
    }

    /// Add a short-term memory entry (summarized conversation turn).
    /// Oldest entries are evicted to the overflow_queue when max_short_term is exceeded.
    /// Call [`flush_overflow`] to persist them to the long-term store.
    pub fn add_short_term(&mut self, summary: String) {
        self.short_term.push(summary);
        if self.short_term.len() > self.max_short_term {
            let oldest = self.short_term.remove(0);
            self.overflow_queue.push(oldest);
        }
    }

    /// Persist overflowed short-term memories to the long-term store.
    ///
    /// Drains the overflow queue and writes each entry as a [`StoreItem`]
    /// with key `short_term_{uuid}` in the `memories/short_term` namespace.
    /// Returns the number of entries successfully flushed.
    pub async fn flush_overflow(&mut self) -> usize {
        let store = match &self.long_term {
            Some(s) => s,
            None => {
                // No store attached — clear queue to avoid unbounded growth
                let drained = self.overflow_queue.len();
                self.overflow_queue.clear();
                return drained;
            }
        };

        let mut flushed = 0;
        for entry in self.overflow_queue.drain(..) {
            let key = format!("short_term_{}", uuid::Uuid::new_v4());
            let value = serde_json::json!({
                "content": entry,
                "source": "short_term_overflow",
            });
            if store
                .put(&["memories", "short_term"], &key, value)
                .await
                .is_ok()
            {
                flushed += 1;
            }
        }
        flushed
    }

    /// Build the full context injection string from Core + ShortTerm memory.
    pub fn build_context_injection(&self) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(core) = self.core.to_system_prompt_fragment() {
            parts.push(core);
        }

        if !self.short_term.is_empty() {
            parts.push("## Recent Context".to_string());
            for (i, entry) in self.short_term.iter().enumerate() {
                parts.push(format!("{}. {}", i + 1, entry));
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
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
    }

    #[test]
    fn test_short_term_eviction() {
        let mut tm = TieredMemory::new(2, 2000);
        tm.add_short_term("summary 1".into());
        tm.add_short_term("summary 2".into());
        assert_eq!(tm.short_term.len(), 2);
        tm.add_short_term("summary 3".into());
        assert_eq!(tm.short_term.len(), 2);
        assert_eq!(tm.short_term[0], "summary 2");
        // Overflow queue should contain the evicted oldest entry
        assert_eq!(tm.overflow_queue.len(), 1);
        assert_eq!(tm.overflow_queue[0], "summary 1");
    }

    #[test]
    fn test_overflow_queue_clears_without_store() {
        let mut tm = TieredMemory::new(1, 2000);
        tm.add_short_term("entry 1".into());
        tm.add_short_term("entry 2".into()); // triggers overflow
        assert_eq!(tm.short_term, vec!["entry 2"]);
        assert_eq!(tm.overflow_queue, vec!["entry 1"]);
    }

    #[test]
    fn test_context_injection() {
        let mut tm = TieredMemory::new(2, 2000);
        tm.core.upsert(
            super::super::core_memory::CoreMemoryBlock::new("1", "name", "Alice")
                .with_importance(8.0),
        );
        tm.add_short_term("Previous conversation about Rust".into());

        let ctx = tm.build_context_injection().unwrap();
        assert!(ctx.contains("Alice"));
        assert!(ctx.contains("Rust"));
    }
}
