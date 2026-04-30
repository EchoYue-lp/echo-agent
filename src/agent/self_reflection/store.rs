//! Episodic memory store — persists reflection history and experiences

use crate::agent::self_reflection::types::{ReflectionExperience, ReflectionRecord};
use crate::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// In-memory store (for testing)
pub struct InMemoryReflectionStore {
    reflections: Arc<RwLock<std::collections::HashMap<String, Vec<ReflectionRecord>>>>,
    experiences: Arc<RwLock<Vec<ReflectionExperience>>>,
}

impl InMemoryReflectionStore {
    /// Create an in-memory store instance
    ///
    /// # Description
    /// In-memory store is intended for testing and development environments. Data is only stored in memory,
    /// and is lost when the process exits.
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
    use crate::agent::self_reflection::types::Critique;

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

        // Non-existent task returns empty
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
