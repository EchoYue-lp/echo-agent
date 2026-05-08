//! Reflection types, store, and prompt builders
//!
//! Shared data structures and persistence abstractions for the Self-Reflection pattern.

use crate::agent::types::Critique;
use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

// ── ReflectionRecord ──────────────────────────────────────────────────────────

/// Record of a single reflection iteration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionRecord {
    /// Iteration number (0-based)
    pub iteration: usize,
    /// Current response
    pub answer: String,
    /// Evaluation result
    pub critique: Critique,
    /// Reflection text (analysis of failure reasons)
    pub reflection_text: String,
    /// Refined response
    pub refined_answer: Option<String>,
}

// ── ReflectionExperience ──────────────────────────────────────────────────────

/// Reflection experience (for episodic memory)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionExperience {
    /// Unique ID
    pub id: String,
    /// Lesson learned (e.g., "Confirm data availability before querying")
    pub lesson: String,
    /// Error pattern (e.g., "Assumes data exists without checking")
    pub error_pattern: String,
    /// Task category
    #[serde(default)]
    pub task_category: Option<String>,
    /// Number of times referenced
    #[serde(default)]
    pub use_count: u32,
}

impl ReflectionExperience {
    /// Create a reflection experience record
    pub fn new(lesson: impl Into<String>, error_pattern: impl Into<String>) -> Self {
        Self {
            id: format!("exp_{}", uuid::Uuid::new_v4().as_simple()),
            lesson: lesson.into(),
            error_pattern: error_pattern.into(),
            task_category: None,
            use_count: 0,
        }
    }

    /// Set task category
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.task_category = Some(category.into());
        self
    }
}

// ── Prompt builders ───────────────────────────────────────────────────────────

/// Build the default refinement prompt.
pub fn default_refinement_prompt(
    task: &str,
    current_answer: &str,
    critique: &Critique,
    reflection: &str,
    iteration: usize,
) -> String {
    let suggestions_text = if critique.suggestions.is_empty() {
        String::new()
    } else {
        format!(
            "\nImprovement suggestions:\n{}",
            critique
                .suggestions
                .iter()
                .map(|s| format!("- {}", s))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!(
        "Original task: {}\n\n\
         Your previous response:\n{}\n\n\
         Evaluation feedback (score: {:.1}/10.0):\n{}{}\n\n\
         Reflection analysis:\n{}\n\n\
         This is improvement iteration #{}. Based on the above evaluation feedback and reflection analysis, provide a more accurate and complete response.",
        task,
        current_answer,
        critique.score,
        critique.feedback,
        suggestions_text,
        reflection,
        iteration + 1,
    )
}

/// Build the default reflection prompt.
pub fn default_reflection_prompt(task: &str, answer: &str, critique: &Critique) -> String {
    let errors_text = if critique.suggestions.is_empty() {
        String::new()
    } else {
        format!(
            "\nSpecific issues:\n{}",
            critique
                .suggestions
                .iter()
                .map(|s| format!("- {}", s))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!(
        "Task: {}\n\n\
         Generated response:\n{}\n\n\
         Evaluation result: score {:.1}/10.0, did not pass.\n\
         Evaluation feedback: {}{}\n\n\
         Please deeply analyze the issues in the above response, considering:\n\
         1. Why did these errors or deficiencies occur?\n\
         2. What is the root cause?\n\
         3. How can similar issues be avoided next time?\n\n\
         Please output concise reflection text.",
        task, answer, critique.score, critique.feedback, errors_text,
    )
}

// ── ReflectionStore trait ─────────────────────────────────────────────────────

/// Reflection store trait
pub trait ReflectionStore: Send + Sync {
    /// Save reflection records
    fn save_reflections<'a>(
        &'a self,
        task_id: &'a str,
        records: &'a [ReflectionRecord],
    ) -> BoxFuture<'a, Result<()>>;

    /// Load reflection records
    fn load_reflections<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ReflectionRecord>>>;

    /// Save experiences
    fn save_experiences<'a>(
        &'a self,
        experiences: &'a [ReflectionExperience],
    ) -> BoxFuture<'a, Result<()>>;

    /// Load all experiences
    fn load_experiences(&self) -> BoxFuture<'_, Result<Vec<ReflectionExperience>>>;
}

// ── InMemoryReflectionStore ───────────────────────────────────────────────────

/// In-memory store (for testing)
pub struct InMemoryReflectionStore {
    reflections: Arc<RwLock<std::collections::HashMap<String, Vec<ReflectionRecord>>>>,
    experiences: Arc<RwLock<Vec<ReflectionExperience>>>,
}

impl InMemoryReflectionStore {
    /// Create an in-memory store instance
    pub fn new() -> Self {
        Self {
            reflections: Arc::new(RwLock::new(std::collections::HashMap::new())),
            experiences: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryReflectionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ReflectionStore for InMemoryReflectionStore {
    fn save_reflections<'a>(
        &'a self,
        task_id: &'a str,
        records: &'a [ReflectionRecord],
    ) -> BoxFuture<'a, Result<()>> {
        let reflections = self.reflections.clone();
        let task_id = task_id.to_string();
        let records = records.to_vec();
        Box::pin(async move {
            reflections.write().await.insert(task_id, records);
            Ok(())
        })
    }

    fn load_reflections<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ReflectionRecord>>> {
        let reflections = self.reflections.clone();
        let task_id = task_id.to_string();
        Box::pin(async move {
            Ok(reflections
                .read()
                .await
                .get(&task_id)
                .cloned()
                .unwrap_or_default())
        })
    }

    fn save_experiences<'a>(
        &'a self,
        experiences: &'a [ReflectionExperience],
    ) -> BoxFuture<'a, Result<()>> {
        let store = self.experiences.clone();
        let experiences = experiences.to_vec();
        Box::pin(async move {
            *store.write().await = experiences;
            Ok(())
        })
    }

    fn load_experiences(&self) -> BoxFuture<'_, Result<Vec<ReflectionExperience>>> {
        let store = self.experiences.clone();
        Box::pin(async move { Ok(store.read().await.clone()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_refinement_prompt() {
        let critique = Critique {
            score: 5.0,
            passed: false,
            feedback: "Not accurate enough".to_string(),
            suggestions: vec!["Add examples".to_string()],
        };
        let prompt = default_refinement_prompt(
            "Explain Rust",
            "Rust is...",
            &critique,
            "Needs more detail",
            0,
        );
        assert!(prompt.contains("Explain Rust"));
        assert!(prompt.contains("Not accurate enough"));
        assert!(prompt.contains("Add examples"));
        assert!(prompt.contains("improvement iteration #1"));
    }

    #[test]
    fn test_default_reflection_prompt() {
        let critique = Critique {
            score: 4.0,
            passed: false,
            feedback: "Concept is incorrect".to_string(),
            suggestions: vec!["Fix the definition".to_string()],
        };
        let prompt = default_reflection_prompt("Explain ownership", "Rust has GC...", &critique);
        assert!(prompt.contains("Explain ownership"));
        assert!(prompt.contains("Concept is incorrect"));
        assert!(prompt.contains("Fix the definition"));
    }

    #[test]
    fn test_reflection_experience() {
        let exp = ReflectionExperience::new("Confirm data before querying", "Assumes data exists")
            .with_category("database");
        assert!(exp.id.starts_with("exp_"));
        assert_eq!(exp.lesson, "Confirm data before querying");
        assert_eq!(exp.task_category, Some("database".to_string()));
    }

    #[tokio::test]
    async fn test_in_memory_store_reflections() {
        let store = InMemoryReflectionStore::new();
        let records = vec![ReflectionRecord {
            iteration: 0,
            answer: "test answer".to_string(),
            critique: Critique {
                score: 8.0,
                passed: true,
                feedback: "good".to_string(),
                suggestions: vec![],
            },
            reflection_text: "looks good".to_string(),
            refined_answer: None,
        }];

        store.save_reflections("task_1", &records).await.unwrap();
        let loaded = store.load_reflections("task_1").await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].answer, "test answer");

        let empty = store.load_reflections("nonexistent").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_in_memory_store_experiences() {
        let store = InMemoryReflectionStore::new();
        let experiences = vec![
            ReflectionExperience::new("Verify first, then execute", "Skipping verification"),
            ReflectionExperience::new("Check boundary conditions", "Ignoring boundaries"),
        ];

        store.save_experiences(&experiences).await.unwrap();
        let loaded = store.load_experiences().await.unwrap();
        assert_eq!(loaded.len(), 2);
    }
}
