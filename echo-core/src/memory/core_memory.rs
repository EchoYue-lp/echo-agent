//! Core memory — fixed-size blocks always injected into the system prompt.
//!
//! Inspired by Letta's "core memory" concept: a small set of high-importance
//! facts that are always visible to the agent, formatted similarly to Claude
//! Code's `MEMORY.md`.

use serde::{Deserialize, Serialize};

/// A block of core memory — a single high-importance fact or preference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreMemoryBlock {
    /// Unique identifier for this block.
    pub id: String,
    /// Human-readable label (e.g., "user_name", "project_goal").
    pub label: String,
    /// The memory content (plain text or markdown).
    pub value: String,
    /// Importance score (1.0–10.0). Higher importance blocks are less likely
    /// to be evicted when the total character budget is exceeded.
    pub importance: f32,
    /// Maximum character count for this block. When exceeded during update,
    /// the content is truncated or summarized.
    pub limit: usize,
}

impl CoreMemoryBlock {
    /// Create a new core memory block.
    pub fn new(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            importance: 5.0,
            limit: 500,
        }
    }

    /// Set the importance.
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(1.0, 10.0);
        self
    }

    /// Set the character limit.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// Collection of core memory blocks, managed as a fixed-size character budget.
///
/// Blocks are sorted by importance (descending). When the total character count
/// exceeds `max_chars`, the lowest-importance blocks are evicted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreMemory {
    /// Memory blocks, sorted by importance descending.
    blocks: Vec<CoreMemoryBlock>,
    /// Total character budget across all blocks (default 2000).
    max_chars: usize,
}

impl Default for CoreMemory {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            max_chars: 2000,
        }
    }
}

impl CoreMemory {
    /// Create an empty core memory with the given character budget.
    pub fn new(max_chars: usize) -> Self {
        Self {
            blocks: Vec::new(),
            max_chars,
        }
    }

    /// Insert or update a memory block.
    ///
    /// If a block with the same `id` exists, it is replaced. Otherwise added.
    /// Blocks are re-sorted by importance after insertion.
    /// Content exceeding the block's `limit` is truncated.
    pub fn upsert(&mut self, block: CoreMemoryBlock) {
        let mut block = block;
        // Truncate content to the block's limit
        if block.value.chars().count() > block.limit {
            block.value = block.value.chars().take(block.limit).collect();
            block.value.push_str("…");
        }

        // Remove existing block with same id
        self.blocks.retain(|b| b.id != block.id);

        self.blocks.push(block);
        self.blocks
            .sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap());
        self.evict_if_needed();
    }

    /// Remove a block by id.
    pub fn remove(&mut self, id: &str) {
        self.blocks.retain(|b| b.id != id);
    }

    /// Get all blocks, sorted by importance.
    pub fn blocks(&self) -> &[CoreMemoryBlock] {
        &self.blocks
    }

    /// Total character count of all blocks.
    pub fn total_chars(&self) -> usize {
        self.blocks.iter().map(|b| b.value.chars().count()).sum()
    }

    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether there are no blocks.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Generate a formatted system prompt fragment suitable for injection.
    ///
    /// Output format resembles Claude Code's MEMORY.md:
    ///
    /// ```text
    /// ## Core Memory
    /// - user_name: Alice
    /// - project_goal: Build a Rust agent framework
    /// ```
    pub fn to_system_prompt_fragment(&self) -> Option<String> {
        if self.blocks.is_empty() {
            return None;
        }

        let mut lines = vec!["## Core Memory".to_string()];
        for block in &self.blocks {
            lines.push(format!("- {}: {}", block.label, block.value));
        }
        Some(lines.join("\n"))
    }

    /// Set the character budget.
    pub fn set_max_chars(&mut self, max: usize) {
        self.max_chars = max;
        self.evict_if_needed();
    }

    // ── Internal ──────────────────────────────────────────────────────────

    fn evict_if_needed(&mut self) {
        while self.total_chars() > self.max_chars && !self.blocks.is_empty() {
            // Remove the lowest-importance block (last in sorted order)
            self.blocks.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_memory_basic() {
        let mut cm = CoreMemory::new(500);
        cm.upsert(CoreMemoryBlock::new("1", "name", "Alice").with_importance(8.0));
        cm.upsert(CoreMemoryBlock::new("2", "goal", "Build an agent").with_importance(5.0));

        assert_eq!(cm.len(), 2);

        let prompt = cm.to_system_prompt_fragment().unwrap();
        assert!(prompt.contains("Alice"));
        assert!(prompt.contains("Build an agent"));
    }

    #[test]
    fn test_eviction_by_importance() {
        let mut cm = CoreMemory::new(30);
        cm.upsert(CoreMemoryBlock::new("1", "high_imp", "key_a").with_importance(9.0));
        cm.upsert(CoreMemoryBlock::new("2", "low_imp", "key_b").with_importance(1.0));

        // Both fit (5+5 = 10 < 30)
        assert_eq!(cm.len(), 2);

        // Add a third that pushes over budget
        cm.upsert(CoreMemoryBlock::new("3", "highest", "this_is_sixteen_chars!").with_importance(10.0));
        // Total: 5 + 5 + 22 = 32 > 30. Lowest importance (key_b, imp 1.0) evicted.
        assert_eq!(cm.len(), 2);
        let labels: Vec<&str> = cm.blocks().iter().map(|b| b.label.as_str()).collect();
        assert!(labels.contains(&"highest")); // imp 10.0
        assert!(labels.contains(&"high_imp")); // imp 9.0
        assert!(!labels.contains(&"low_imp")); // evicted
    }

    #[test]
    fn test_upsert_replaces() {
        let mut cm = CoreMemory::new(200);
        cm.upsert(CoreMemoryBlock::new("1", "key", "original"));
        cm.upsert(CoreMemoryBlock::new("1", "key", "updated"));

        assert_eq!(cm.len(), 1);
        assert_eq!(cm.blocks()[0].value, "updated");
    }

    #[test]
    fn test_truncation() {
        let mut cm = CoreMemory::new(200);
        cm.upsert(
            CoreMemoryBlock::new("1", "key", "a very long string")
                .with_limit(3),
        );
        assert_eq!(cm.blocks()[0].value, "a v…"); // truncated to 3 chars + ellipsis
    }
}
