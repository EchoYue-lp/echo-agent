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
}

impl TieredMemory {
    /// Create a new tiered memory manager.
    pub fn new(max_short_term: usize, max_core_chars: usize) -> Self {
        Self {
            core: CoreMemory::new(max_core_chars),
            short_term: Vec::new(),
            max_short_term,
            long_term: None,
        }
    }

    /// Attach a long-term Store for persistence.
    pub fn with_store(mut self, store: Arc<dyn Store>) -> Self {
        self.long_term = Some(store);
        self
    }

    /// Add a short-term memory entry (summarized conversation turn).
    /// Oldest entries are evicted when max_short_term is exceeded.
    pub fn add_short_term(&mut self, summary: String) {
        self.short_term.push(summary);
        if self.short_term.len() > self.max_short_term {
            // Move oldest short-term to long-term if store is available
            let oldest = self.short_term.remove(0);
            // Note: actual persistence to store happens via async, this is sync-only
            let _ = oldest; // In production, trigger async persistence
        }
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
        items.iter().filter(|i| should_prune(i)).map(|i| i.key.clone()).collect()
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
        assert_eq!(tm.short_term[0], "summary 2"); // oldest evicted
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
