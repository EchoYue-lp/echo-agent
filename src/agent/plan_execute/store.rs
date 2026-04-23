//! Plan persistence layer — stores plans to SQLite via SqliteStore

use crate::error::Result;
use crate::memory::store::Store;
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;

/// Lightweight plan summary for listing/search
#[derive(Debug, Clone)]
pub struct PlanSummary {
    /// 计划唯一标识符
    pub id: String,
    /// 可读的短标识符（用于 URL 等场景）
    pub slug: Option<String>,
    /// 计划目标描述
    pub goal: Option<String>,
    /// 计划版本号（用于乐观锁）
    pub version: u32,
    /// 计划总步骤数
    pub total_steps: usize,
    /// 已完成的步骤数
    pub completed_steps: usize,
}

/// Trait for plan persistence operations
pub trait PlanStore: Send + Sync {
    /// 保存计划到存储
    ///
    /// # 参数
    /// * `plan` - 要保存的计划
    fn save_plan<'a>(
        &'a self,
        plan: &'a crate::agent::plan_execute::types::Plan,
    ) -> BoxFuture<'a, Result<()>>;
    /// 根据计划ID加载计划
    ///
    /// # 参数
    /// * `plan_id` - 计划唯一标识符
    /// # 返回
    /// * `Ok(Some(plan))` - 找到计划
    /// * `Ok(None)` - 计划不存在
    fn load_plan<'a>(
        &'a self,
        plan_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::agent::plan_execute::types::Plan>>>;
    /// 根据slug加载计划
    ///
    /// # 参数
    /// * `slug` - 计划的可读短标识符
    /// # 返回
    /// * `Ok(Some(plan))` - 找到计划
    /// * `Ok(None)` - 计划不存在
    fn load_plan_by_slug<'a>(
        &'a self,
        slug: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::agent::plan_execute::types::Plan>>>;
    /// 列出计划摘要列表
    ///
    /// # 参数
    /// * `limit` - 返回结果的最大数量
    fn list_plans<'a>(&'a self, limit: usize) -> BoxFuture<'a, Result<Vec<PlanSummary>>>;
    /// 删除计划
    ///
    /// # 参数
    /// * `plan_id` - 计划唯一标识符
    /// # 返回
    /// * `Ok(true)` - 删除成功
    /// * `Ok(false)` - 计划不存在
    fn delete_plan<'a>(&'a self, plan_id: &'a str) -> BoxFuture<'a, Result<bool>>;
    /// 搜索计划
    ///
    /// # 参数
    /// * `query` - 搜索关键词
    /// * `limit` - 返回结果的最大数量
    fn search_plans<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<PlanSummary>>>;
}

/// SQLite-backed plan store using the existing SqliteStore
pub struct SqlitePlanStore {
    store: Arc<dyn Store>,
}

const PLAN_NAMESPACE: &[&str] = &["plans"];

impl SqlitePlanStore {
    /// 创建基于 SQLite 存储的计划存储
    ///
    /// # 参数
    /// * `store` - 底层存储实现
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    fn plan_to_value(plan: &crate::agent::plan_execute::types::Plan) -> serde_json::Value {
        json!({
            "id": plan.id,
            "version": plan.version,
            "slug": plan.slug,
            "steps": plan.steps,
            "goal": plan.goal,
            "parent_plan_id": plan.parent_plan_id,
            "metadata": plan.metadata,
            "created_at": plan.created_at,
            "updated_at": plan.updated_at,
            "content": plan.goal.clone().unwrap_or_default(),
        })
    }
}

impl PlanStore for SqlitePlanStore {
    fn save_plan<'a>(
        &'a self,
        plan: &'a crate::agent::plan_execute::types::Plan,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let key = plan.id.as_deref().unwrap_or("unknown");
            let value = Self::plan_to_value(plan);
            self.store
                .put(PLAN_NAMESPACE, key, value)
                .await
                .map_err(|e| crate::error::ReactError::Other(format!("save_plan: {}", e)))
        })
    }

    fn load_plan<'a>(
        &'a self,
        plan_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::agent::plan_execute::types::Plan>>> {
        Box::pin(async move {
            let item = self
                .store
                .get(PLAN_NAMESPACE, plan_id)
                .await
                .map_err(|e| crate::error::ReactError::Other(format!("load_plan: {}", e)))?;

            match item {
                Some(item) => {
                    let plan = serde_json::from_value(item.value).map_err(|e| {
                        crate::error::ReactError::Other(format!("load_plan parse: {}", e))
                    })?;
                    Ok(Some(plan))
                }
                None => Ok(None),
            }
        })
    }

    fn load_plan_by_slug<'a>(
        &'a self,
        slug: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::agent::plan_execute::types::Plan>>> {
        Box::pin(async move {
            let results = self
                .store
                .search(PLAN_NAMESPACE, slug, 20)
                .await
                .map_err(|e| {
                    crate::error::ReactError::Other(format!("load_plan_by_slug: {}", e))
                })?;

            for item in results {
                if let Ok(plan) =
                    serde_json::from_value::<crate::agent::plan_execute::types::Plan>(item.value)
                    && plan.slug.as_deref() == Some(slug)
                {
                    return Ok(Some(plan));
                }
            }

            Ok(None)
        })
    }

    fn list_plans<'a>(&'a self, limit: usize) -> BoxFuture<'a, Result<Vec<PlanSummary>>> {
        Box::pin(async move {
            let results = self
                .store
                .search(PLAN_NAMESPACE, "", limit)
                .await
                .map_err(|e| crate::error::ReactError::Other(format!("list_plans: {}", e)))?;

            let mut summaries = Vec::new();
            for item in results {
                if let Ok(plan) =
                    serde_json::from_value::<crate::agent::plan_execute::types::Plan>(item.value)
                {
                    summaries.push(PlanSummary {
                        id: plan.id.clone().unwrap_or_default(),
                        slug: plan.slug.clone(),
                        goal: plan.goal.clone(),
                        version: plan.version,
                        total_steps: plan.steps.len(),
                        completed_steps: plan.completed_count(),
                    });
                }
            }
            Ok(summaries)
        })
    }

    fn delete_plan<'a>(&'a self, plan_id: &'a str) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            self.store
                .delete(PLAN_NAMESPACE, plan_id)
                .await
                .map_err(|e| crate::error::ReactError::Other(format!("delete_plan: {}", e)))
        })
    }

    fn search_plans<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<PlanSummary>>> {
        Box::pin(async move {
            let results = self
                .store
                .search(PLAN_NAMESPACE, query, limit)
                .await
                .map_err(|e| crate::error::ReactError::Other(format!("search_plans: {}", e)))?;

            let mut summaries = Vec::new();
            for item in results {
                if let Ok(plan) =
                    serde_json::from_value::<crate::agent::plan_execute::types::Plan>(item.value)
                {
                    summaries.push(PlanSummary {
                        id: plan.id.clone().unwrap_or_default(),
                        slug: plan.slug.clone(),
                        goal: plan.goal.clone(),
                        version: plan.version,
                        total_steps: plan.steps.len(),
                        completed_steps: plan.completed_count(),
                    });
                }
            }
            Ok(summaries)
        })
    }
}

/// Generate a readable slug from two random words (e.g. "quick-fox")
pub fn generate_plan_slug() -> String {
    const ADJECTIVES: &[&str] = &[
        "swift", "bold", "calm", "dark", "fast", "keen", "lucky", "neat", "pure", "quiet", "rare",
        "safe", "tidy", "vast", "warm", "wise", "zippy", "agile", "brave", "clean",
    ];
    const NOUNS: &[&str] = &[
        "fox", "wolf", "bear", "hawk", "lynx", "tiger", "eagle", "shark", "crane", "otter",
        "raven", "whale", "panda", "cobra", "falcon", "badger", "heron", "moose", "robin", "stoat",
    ];

    let adj = ADJECTIVES[fastrand::usize(0..ADJECTIVES.len())];
    let noun = NOUNS[fastrand::usize(0..NOUNS.len())];
    format!("{}-{}", adj, noun)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::plan_execute::types::{Plan, PlanStep};
    use crate::memory::store::InMemoryStore;

    fn sample_plan() -> Plan {
        Plan::new(vec![
            PlanStep::new("分析代码结构"),
            PlanStep::new("优化性能").with_dependencies(vec!["step_0".to_string()]),
        ])
        .with_goal("性能优化")
        .with_slug("test-plan")
    }

    #[tokio::test]
    async fn test_save_and_load_plan() {
        let store = Arc::new(InMemoryStore::new());
        let plan_store = SqlitePlanStore::new(store);
        let plan = sample_plan();

        plan_store.save_plan(&plan).await.unwrap();

        let plan_id = plan.id.as_deref().unwrap();
        let loaded = plan_store.load_plan(plan_id).await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(loaded.goal.as_deref(), Some("性能优化"));
        assert_eq!(loaded.steps[1].dependencies, vec!["step_0"]);
    }

    #[tokio::test]
    async fn test_load_nonexistent_plan() {
        let store = Arc::new(InMemoryStore::new());
        let plan_store = SqlitePlanStore::new(store);
        let loaded = plan_store.load_plan("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_load_plan_by_slug() {
        let store = Arc::new(InMemoryStore::new());
        let plan_store = SqlitePlanStore::new(store);
        let plan = sample_plan();

        plan_store.save_plan(&plan).await.unwrap();

        let loaded = plan_store.load_plan_by_slug("test-plan").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().slug.as_deref(), Some("test-plan"));
    }

    #[tokio::test]
    async fn test_list_plans() {
        let store = Arc::new(InMemoryStore::new());
        let plan_store = SqlitePlanStore::new(store);

        let p1 = Plan::new(vec![PlanStep::new("step A")]).with_slug("plan-a");
        let p2 = Plan::new(vec![PlanStep::new("step B")]).with_slug("plan-b");
        plan_store.save_plan(&p1).await.unwrap();
        plan_store.save_plan(&p2).await.unwrap();

        let summaries = plan_store.list_plans(10).await.unwrap();
        assert_eq!(summaries.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_plan() {
        let store = Arc::new(InMemoryStore::new());
        let plan_store = SqlitePlanStore::new(store);
        let plan = sample_plan();

        plan_store.save_plan(&plan).await.unwrap();
        let plan_id = plan.id.as_deref().unwrap();

        let deleted = plan_store.delete_plan(plan_id).await.unwrap();
        assert!(deleted);

        let loaded = plan_store.load_plan(plan_id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_generate_slug() {
        let slug = generate_plan_slug();
        assert!(slug.contains('-'));
        let parts: Vec<&str> = slug.split('-').collect();
        assert_eq!(parts.len(), 2);
    }
}
