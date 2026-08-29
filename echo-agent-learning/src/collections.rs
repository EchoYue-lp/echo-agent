//! `Vec`, `HashMap`, `HashSet`, closures, and iterator ownership modes.

use crate::basics::LearningTask;
use crate::errors::LearningError;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct TaskCatalog {
    ordered: Vec<LearningTask>,
    positions: HashMap<String, usize>,
    tags: HashSet<String>,
}

impl TaskCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, task: LearningTask) -> Result<(), LearningError> {
        if self.positions.contains_key(&task.title) {
            return Err(LearningError::DuplicateTask(task.title));
        }
        let position = self.ordered.len();
        self.positions.insert(task.title.clone(), position);
        self.ordered.push(task);
        Ok(())
    }

    pub fn get(&self, title: &str) -> Option<&LearningTask> {
        self.positions
            .get(title)
            .and_then(|position| self.ordered.get(*position))
    }

    pub fn add_tag(&mut self, tag: impl Into<String>) -> bool {
        self.tags.insert(tag.into())
    }

    pub fn tags(&self) -> Vec<&str> {
        let mut tags = self.tags.iter().map(String::as_str).collect::<Vec<_>>();
        tags.sort_unstable();
        tags
    }

    pub fn matching_titles<F>(&self, predicate: F) -> Vec<&str>
    where
        F: Fn(&LearningTask) -> bool,
    {
        self.ordered
            .iter()
            .filter(|task| predicate(task))
            .map(|task| task.title.as_str())
            .collect()
    }
}

/// The entry API performs one lookup and updates the value in place.
pub fn word_frequencies(input: &str) -> HashMap<String, usize> {
    let mut frequencies = HashMap::new();
    for word in input.split_whitespace() {
        let count = frequencies.entry(word.to_lowercase()).or_insert(0usize);
        *count = count.saturating_add(1);
    }
    frequencies
}

/// `into_iter` consumes the vector, so no clone is needed to return owned text.
pub fn normalize_owned(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_preserves_order_and_supports_lookup() -> Result<(), LearningError> {
        let mut catalog = TaskCatalog::new();
        catalog.insert(LearningTask::new("read")?)?;
        catalog.insert(LearningTask::new("test")?)?;
        assert_eq!(
            catalog.get("test").map(|task| task.title.as_str()),
            Some("test")
        );
        assert!(catalog.insert(LearningTask::new("test")?).is_err());
        Ok(())
    }

    #[test]
    fn closures_can_capture_filter_values() -> Result<(), LearningError> {
        let mut catalog = TaskCatalog::new();
        catalog.insert(LearningTask::new("read Rust")?)?;
        catalog.insert(LearningTask::new("test Agent")?)?;
        let keyword = "Rust";
        assert_eq!(
            catalog.matching_titles(|task| task.title.contains(keyword)),
            vec!["read Rust"]
        );
        Ok(())
    }

    #[test]
    fn frequency_count_is_case_insensitive() {
        let frequencies = word_frequencies("Rust rust Agent");
        assert_eq!(frequencies.get("rust"), Some(&2));
    }
}
