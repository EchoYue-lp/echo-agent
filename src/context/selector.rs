//! Context selector — scores files by relevance to the current task.
//!
//! Uses symbol matching, recency, and git change detection to find
//! the most relevant files for context assembly.

use std::collections::HashMap;
use std::path::PathBuf;

/// Scores and selects files relevant to a given task.
pub struct ContextSelector {
    /// Weight for symbol name matches (high — direct code relevance).
    pub symbol_weight: f64,
    /// Weight for recently modified files (medium — likely related).
    pub recency_weight: f64,
    /// Weight for files in git diff (medium — actively changing).
    pub git_diff_weight: f64,
    /// Maximum number of files to include in context.
    pub max_files: usize,
}

impl Default for ContextSelector {
    fn default() -> Self {
        Self {
            symbol_weight: 1.0,
            recency_weight: 0.6,
            git_diff_weight: 0.8,
            max_files: 10,
        }
    }
}

impl ContextSelector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Score files by relevance to the given task description.
    ///
    /// Returns file paths sorted by relevance (highest first).
    /// `symbols` is a map from file path to extracted symbol names.
    /// `recently_modified` lists recently changed files.
    /// `git_changed` lists files with git modifications.
    pub fn score_files(
        &self,
        task: &str,
        symbols: &HashMap<PathBuf, Vec<String>>,
        recently_modified: &[PathBuf],
        git_changed: &[PathBuf],
    ) -> Vec<(PathBuf, f64)> {
        let task_lower = task.to_lowercase();
        let task_words: Vec<&str> = task_lower.split_whitespace().collect();
        let mut scores: HashMap<PathBuf, f64> = HashMap::new();

        // Score by symbol matches
        for (path, syms) in symbols {
            let mut score = 0.0;
            for sym in syms {
                let sym_lower = sym.to_lowercase();
                // Direct word match
                for word in &task_words {
                    if sym_lower.contains(word) || word.contains(&sym_lower) {
                        score += self.symbol_weight;
                    }
                }
                // Substring match (partial)
                if task_lower.contains(&sym_lower) {
                    score += self.symbol_weight * 0.5;
                }
            }
            if score > 0.0 {
                *scores.entry(path.clone()).or_default() += score;
            }
        }

        // Score by recency
        for path in recently_modified {
            *scores.entry(path.clone()).or_default() += self.recency_weight;
        }

        // Score by git changes
        for path in git_changed {
            *scores.entry(path.clone()).or_default() += self.git_diff_weight;
        }

        // Sort by score descending
        let mut scored: Vec<(PathBuf, f64)> = scores.into_iter().collect();
        scored.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.as_os_str().cmp(b.0.as_os_str()))
        });
        scored
    }

    /// Select the top-N most relevant files for a task.
    pub fn select_relevant(
        &self,
        task: &str,
        symbols: &HashMap<PathBuf, Vec<String>>,
        recently_modified: &[PathBuf],
        git_changed: &[PathBuf],
    ) -> Vec<PathBuf> {
        let scored = self.score_files(task, symbols, recently_modified, git_changed);
        scored
            .into_iter()
            .take(self.max_files)
            .map(|(path, _)| path)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_symbol_match() {
        let selector = ContextSelector::new();
        let mut symbols = HashMap::new();
        symbols.insert(
            PathBuf::from("src/auth.rs"),
            vec!["login".into(), "authenticate".into()],
        );
        symbols.insert(
            PathBuf::from("src/utils.rs"),
            vec!["format".into(), "parse".into()],
        );

        let scored = selector.score_files("fix the login bug", &symbols, &[], &[]);
        assert!(!scored.is_empty());
        // auth.rs should score higher than utils.rs for "login bug"
        assert_eq!(scored[0].0, PathBuf::from("src/auth.rs"));
    }

    #[test]
    fn test_score_recency_and_git() {
        let selector = ContextSelector::new();
        let symbols = HashMap::new();
        let recent = vec![PathBuf::from("src/recent.rs")];
        let git = vec![PathBuf::from("src/changed.rs")];

        let scored = selector.score_files("some task", &symbols, &recent, &git);
        assert_eq!(scored.len(), 2);
    }

    #[test]
    fn equal_scores_are_ordered_by_path() {
        let selector = ContextSelector::new();
        let recent = vec![PathBuf::from("src/z.rs"), PathBuf::from("src/a.rs")];
        let scored = selector.score_files("task", &HashMap::new(), &recent, &[]);
        assert_eq!(
            scored.iter().map(|(path, _)| path).collect::<Vec<_>>(),
            vec![&PathBuf::from("src/a.rs"), &PathBuf::from("src/z.rs")]
        );
    }
}
