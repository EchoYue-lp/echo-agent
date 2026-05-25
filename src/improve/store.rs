//! Critique storage — persists analysis results for retrieval and trend detection.

use crate::improve::RunCritique;
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory critique store with pattern aggregation.
pub struct CritiqueStore {
    critiques: Mutex<Vec<RunCritique>>,
    /// Aggregated issue patterns across all critiques.
    pattern_counts: Mutex<HashMap<String, usize>>,
}

impl CritiqueStore {
    pub fn new() -> Self {
        Self { critiques: Mutex::new(Vec::new()), pattern_counts: Mutex::new(HashMap::new()) }
    }

    /// Store a critique and update pattern counts.
    pub fn store(&self, critique: RunCritique) {
        let mut patterns = self.pattern_counts.lock().unwrap();
        for issue in &critique.issues {
            *patterns.entry(format!("{:?}", issue)).or_default() += 1;
        }
        self.critiques.lock().unwrap().push(critique);
    }

    /// Get top-N most frequent issue patterns.
    pub fn top_patterns(&self, n: usize) -> Vec<(String, usize)> {
        let patterns = self.pattern_counts.lock().unwrap();
        let mut sorted: Vec<_> = patterns.iter().map(|(k, v)| (k.clone(), *v)).collect();
        sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        sorted.truncate(n);
        sorted
    }

    /// Retrieve critiques for a specific run.
    pub fn get_by_run(&self, run_id: &str) -> Vec<RunCritique> {
        self.critiques.lock().unwrap().iter()
            .filter(|c| c.run_id == run_id).cloned().collect()
    }

    /// Total stored critiques.
    pub fn len(&self) -> usize { self.critiques.lock().unwrap().len() }

    /// Whether empty.
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Clear all stored data.
    pub fn clear(&self) {
        self.critiques.lock().unwrap().clear();
        self.pattern_counts.lock().unwrap().clear();
    }
}

impl Default for CritiqueStore {
    fn default() -> Self { Self::new() }
}
