//! 情景记忆存储 — 持久化反思历史和经验

use crate::agent::self_reflection::types::{ReflectionExperience, ReflectionRecord};
use crate::error::Result;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 反思存储 trait
pub trait ReflectionStore: Send + Sync {
    /// 保存反思记录
    fn save_reflections<'a>(
        &'a self,
        task_id: &'a str,
        records: &'a [ReflectionRecord],
    ) -> BoxFuture<'a, Result<()>>;

    /// 加载反思记录
    fn load_reflections<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ReflectionRecord>>>;

    /// 保存经验
    fn save_experiences<'a>(
        &'a self,
        experiences: &'a [ReflectionExperience],
    ) -> BoxFuture<'a, Result<()>>;

    /// 加载所有经验
    fn load_experiences(&self) -> BoxFuture<'_, Result<Vec<ReflectionExperience>>>;
}

/// 内存存储（用于测试）
pub struct InMemoryReflectionStore {
    reflections: Arc<RwLock<std::collections::HashMap<String, Vec<ReflectionRecord>>>>,
    experiences: Arc<RwLock<Vec<ReflectionExperience>>>,
}

impl InMemoryReflectionStore {
    /// 创建内存存储实例
    ///
    /// # 说明
    /// 内存存储用于测试和开发环境，数据仅保存在内存中，
    /// 进程退出后数据丢失。
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

        // 不存在的任务返回空
        let empty = store.load_reflections("nonexistent").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_in_memory_store_experiences() {
        let store = InMemoryReflectionStore::new();
        let experiences = vec![
            ReflectionExperience::new("先验证再执行", "跳过验证"),
            ReflectionExperience::new("检查边界条件", "忽略边界"),
        ];

        store.save_experiences(&experiences).await.unwrap();
        let loaded = store.load_experiences().await.unwrap();
        assert_eq!(loaded.len(), 2);
    }
}
