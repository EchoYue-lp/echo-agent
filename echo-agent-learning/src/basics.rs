//! Bindings, domain types, pattern matching, collections, and iterators.

use crate::errors::LearningError;
use serde::{Deserialize, Serialize};

/// A small domain enum similar to the state enums used throughout echo-agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Completed { summary: String },
}

/// A deliberately small task type used across the lessons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningTask {
    pub title: String,
    pub state: TaskState,
}

impl LearningTask {
    pub fn new(title: impl Into<String>) -> Result<Self, LearningError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(LearningError::EmptyTitle);
        }
        Ok(Self {
            title,
            state: TaskState::Pending,
        })
    }

    pub fn complete(&mut self, summary: impl Into<String>) {
        self.state = TaskState::Completed {
            summary: summary.into(),
        };
    }

    pub fn start(&mut self) -> bool {
        if self.state == TaskState::Pending {
            self.state = TaskState::Running;
            true
        } else {
            false
        }
    }

    pub fn summary(&self) -> Option<&str> {
        match &self.state {
            TaskState::Completed { summary } => Some(summary.as_str()),
            TaskState::Pending | TaskState::Running => None,
        }
    }
}

/// Truncate by Unicode scalar values, never by UTF-8 byte offsets.
pub fn unicode_preview(value: &str, max_chars: usize) -> String {
    let mut preview = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

/// Select completed task titles with iterator adapters and pattern matching.
pub fn completed_titles(tasks: &[LearningTask]) -> Vec<&str> {
    tasks
        .iter()
        .filter_map(|task| match task.state {
            TaskState::Completed { .. } => Some(task.title.as_str()),
            TaskState::Pending | TaskState::Running => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_preview_handles_chinese_and_emoji() {
        assert_eq!(unicode_preview("你好Rust", 3), "你好R...");
        assert_eq!(unicode_preview("A🦀B", 2), "A🦀...");
    }

    #[test]
    fn filters_completed_tasks() -> Result<(), LearningError> {
        let pending = LearningTask::new("read")?;
        let mut completed = LearningTask::new("test")?;
        completed.complete("green");
        assert_eq!(completed_titles(&[pending, completed]), vec!["test"]);
        Ok(())
    }
}
