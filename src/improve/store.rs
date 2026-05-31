//! Critique storage — persists analysis results for retrieval and trend detection.
//!
//! Supports both in-memory and file-based storage. The `DualLayerCritiqueStore`
//! writes to project-level and global-level directories simultaneously.

use crate::improve::RunCritique;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// In-memory critique store with pattern aggregation.
pub struct CritiqueStore {
    critiques: Mutex<Vec<RunCritique>>,
    /// Aggregated issue patterns across all critiques.
    pattern_counts: Mutex<HashMap<String, usize>>,
    /// Optional directory for file-based persistence.
    persist_dir: Option<PathBuf>,
}

impl CritiqueStore {
    pub fn new() -> Self {
        Self {
            critiques: Mutex::new(Vec::new()),
            pattern_counts: Mutex::new(HashMap::new()),
            persist_dir: None,
        }
    }

    /// Create a store with file-based persistence.
    pub fn with_persistence(dir: PathBuf) -> Self {
        let store = Self {
            critiques: Mutex::new(Vec::new()),
            pattern_counts: Mutex::new(HashMap::new()),
            persist_dir: Some(dir.clone()),
        };
        // Ensure directory exists
        let _ = std::fs::create_dir_all(&dir);
        // Load existing data
        store.load_from_disk();
        store
    }

    /// Store a critique and update pattern counts.
    pub fn store(&self, critique: RunCritique) {
        let mut patterns = self.pattern_counts.lock().unwrap_or_else(|e| e.into_inner());
        for issue in &critique.issues {
            *patterns.entry(format!("{:?}", issue)).or_default() += 1;
        }
        self.critiques.lock().unwrap_or_else(|e| e.into_inner()).push(critique);

        // Persist to disk if configured
        if let Some(ref dir) = self.persist_dir {
            self.save_patterns_to_disk(dir);
        }
    }

    /// Get top-N most frequent issue patterns.
    pub fn top_patterns(&self, n: usize) -> Vec<(String, usize)> {
        let patterns = self.pattern_counts.lock().unwrap_or_else(|e| e.into_inner());
        let mut sorted: Vec<_> = patterns.iter().map(|(k, v)| (k.clone(), *v)).collect();
        sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        sorted.truncate(n);
        sorted
    }

    /// Get all patterns with counts.
    pub fn all_patterns(&self) -> HashMap<String, usize> {
        self.pattern_counts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Retrieve critiques for a specific run.
    pub fn get_by_run(&self, run_id: &str) -> Vec<RunCritique> {
        self.critiques
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|c| c.run_id == run_id)
            .cloned()
            .collect()
    }

    /// Total stored critiques.
    pub fn len(&self) -> usize {
        self.critiques.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all stored data.
    pub fn clear(&self) {
        self.critiques.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.pattern_counts.lock().unwrap_or_else(|e| e.into_inner()).clear();
        if let Some(ref dir) = self.persist_dir {
            let patterns_file = dir.join("patterns.json");
            let _ = std::fs::remove_file(patterns_file);
        }
    }

    /// Save patterns to disk.
    fn save_patterns_to_disk(&self, dir: &Path) {
        let patterns = self.pattern_counts.lock().unwrap_or_else(|e| e.into_inner());
        let patterns_file = dir.join("patterns.json");
        if let Ok(json) = serde_json::to_string_pretty(&*patterns) {
            let _ = std::fs::write(patterns_file, json);
        }
    }

    /// Load patterns from disk.
    fn load_from_disk(&self) {
        if let Some(ref dir) = self.persist_dir {
            let patterns_file = dir.join("patterns.json");
            if let Ok(content) = std::fs::read_to_string(patterns_file) {
                if let Ok(patterns) = serde_json::from_str::<HashMap<String, usize>>(&content) {
                    let mut stored = self.pattern_counts.lock().unwrap_or_else(|e| e.into_inner());
                    *stored = patterns;
                }
            }
        }
    }
}

impl Default for CritiqueStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Dual-Layer Store ────────────────────────────────────────────────

/// Dual-layer critique store that writes to both project and global directories.
///
/// ## Storage rules
///
/// | Pattern type | Project layer | Global layer |
/// |-------------|:------------:|:------------:|
/// | ExcessiveRetries | ✓ | ✓ |
/// | ContextOverflow | ✓ | ✓ |
/// | WriteWithoutRead | ✓ | ✗ |
/// | MissingTool | ✓ | ✗ |
/// | ToolErrorPattern | ✓ | depends |
///
/// Universal patterns (agent behavior, not project-specific) are written to
/// both layers. Project-specific patterns are only written to the project layer.
pub struct DualLayerCritiqueStore {
    /// Project-level store (`.echo-agent/evolution/critiques/`).
    project_store: CritiqueStore,
    /// Global-level store (`~/.echo-agent/evolution/critiques/`).
    global_store: CritiqueStore,
}

impl DualLayerCritiqueStore {
    /// Create a dual-layer store with the given directories.
    pub fn new(project_dir: PathBuf, global_dir: PathBuf) -> Self {
        Self {
            project_store: CritiqueStore::with_persistence(project_dir),
            global_store: CritiqueStore::with_persistence(global_dir),
        }
    }

    /// Create from a project root path (auto-resolves directories).
    pub fn from_project_root(project_root: &Path) -> Self {
        let project_dir = project_root
            .join(".echo-agent")
            .join("evolution")
            .join("critiques");

        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("~"));
        let global_dir = home.join(".echo-agent").join("evolution").join("critiques");

        Self::new(project_dir, global_dir)
    }

    /// Store a critique. Universal patterns go to both layers,
    /// project-specific patterns only to the project layer.
    pub fn store(&self, critique: RunCritique) {
        // Always store in project layer
        self.project_store.store(critique.clone());

        // Check if any issues are universal (not project-specific)
        let has_universal = critique.issues.iter().any(|issue| {
            let debug = format!("{:?}", issue);
            debug.contains("ExcessiveRetries") || debug.contains("ContextOverflow")
        });

        if has_universal {
            // Filter to only universal issues for global store
            let universal_issues: Vec<_> = critique
                .issues
                .iter()
                .filter(|issue| {
                    let debug = format!("{:?}", issue);
                    debug.contains("ExcessiveRetries") || debug.contains("ContextOverflow")
                })
                .cloned()
                .collect();

            if !universal_issues.is_empty() {
                let global_critique = RunCritique {
                    run_id: critique.run_id.clone(),
                    success: critique.success,
                    score: critique.score,
                    issues: universal_issues,
                    suggestions: critique.suggestions.clone(),
                };
                self.global_store.store(global_critique);
            }
        }
    }

    /// Get merged patterns from both layers.
    pub fn top_patterns(&self, n: usize) -> Vec<(String, usize)> {
        let mut merged = self.project_store.all_patterns();
        for (pattern, count) in self.global_store.all_patterns() {
            *merged.entry(pattern).or_default() += count;
        }
        let mut sorted: Vec<_> = merged.into_iter().collect();
        sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        sorted.truncate(n);
        sorted
    }

    /// Get project-specific patterns only.
    pub fn project_patterns(&self, n: usize) -> Vec<(String, usize)> {
        self.project_store.top_patterns(n)
    }

    /// Get global patterns only.
    pub fn global_patterns(&self, n: usize) -> Vec<(String, usize)> {
        self.global_store.top_patterns(n)
    }

    /// Total critiques across both layers.
    pub fn total_count(&self) -> usize {
        self.project_store.len() + self.global_store.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_critique_store_basic() {
        let store = CritiqueStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_dual_layer_creation() {
        let tmp = std::env::temp_dir().join("echo-test-dual-layer");
        let _ = std::fs::remove_dir_all(&tmp);
        let project_dir = tmp.join("project");
        let global_dir = tmp.join("global");

        let store = DualLayerCritiqueStore::new(project_dir, global_dir);
        assert_eq!(store.total_count(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
