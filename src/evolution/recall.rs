//! (stage4 D1) Unified memory recall entry.
//!
//! Both the auto recall path (`ReactAgent::recall_long_term_memories`) and the
//! tool recall path (`LayeredRecallTool` → `MemoryLayerManager::search_layered`)
//! delegate to [`MemoryRecaller`] so they return consistently composite-score-
//! ranked results over the unified `["agent","memories"]` namespace
//! (割裂点 3/9 — previously the two paths read different namespaces and ranked
//! differently).

use echo_core::memory::store::{Store, StoreItem};
#[cfg(test)]
use echo_core::memory::types::MemoryStatus;
use echo_state::memory::typed_store::{TypedMemoryEntry, TypedMemoryStore};
use std::sync::Arc;

use crate::memory::SearchQuery;

/// Unified composite-score recall over the unified memory namespace.
pub struct MemoryRecaller {
    store: Arc<dyn Store>,
}

impl MemoryRecaller {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Recall `top_k` memories for `query` via composite score.
    ///
    /// `S = 0.5·sim + 0.3·decay(age, 30d) + 0.2·recall_weight`.
    /// Only explicitly approved Active or Archived memories are eligible.
    /// Recall telemetry is updated with compare-and-put so a stale search result
    /// cannot overwrite a concurrent lifecycle or provenance change.
    pub async fn recall(&self, query: &str, top_k: usize) -> crate::error::Result<Vec<StoreItem>> {
        self.recall_in(crate::evolution::layer::WARM_NAMESPACE, query, top_k, true)
            .await
    }

    /// Apply the same admission and live-value fence to a Store-backed tool namespace.
    pub async fn recall_in(
        &self,
        ns: &[&str],
        query: &str,
        top_k: usize,
        hybrid: bool,
    ) -> crate::error::Result<Vec<StoreItem>> {
        if top_k == 0 {
            return Ok(Vec::new());
        }

        // A fixed pre-filter limit lets enough Drafts hide every approved
        // record. Expand until enough eligible candidates are visible or the
        // Store reports that its matching results are exhausted.
        let mut limit = top_k.saturating_mul(3).max(16);
        let candidates = loop {
            let items = if hybrid {
                match self
                    .store
                    .search_with(ns, SearchQuery::hybrid(query, limit))
                    .await
                {
                    Ok(items) => items,
                    Err(_) => self.store.search(ns, query, limit).await?,
                }
            } else {
                self.store.search(ns, query, limit).await?
            };
            let eligible = items
                .iter()
                .filter(|item| {
                    TypedMemoryEntry::from_store_item((*item).clone())
                        .meta
                        .is_recallable()
                })
                .count();
            if eligible >= top_k || items.len() < limit {
                break items;
            }
            let next = limit.saturating_mul(2);
            if next == limit {
                break items;
            }
            limit = next;
        };

        // 2. Composite-score re-rank + canonical recall eligibility.
        let mut scored: Vec<(f64, TypedMemoryEntry)> = candidates
            .into_iter()
            .filter_map(|item| {
                let entry = TypedMemoryEntry::from_store_item(item);
                if !entry.meta.is_recallable() {
                    return None;
                }
                let sim = entry.raw.score.unwrap_or(0.0) as f64;
                let age = age_days_from_storeitem(&entry.raw);
                let s = composite_score(sim, age, entry.meta.recall_weight as f64);
                Some((s, entry))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.key.cmp(&b.1.key))
        });

        // A search hit may be older than a concurrent Draft edit or delete.
        // Manager-owned callers hold its operation lock; standalone Store
        // callers still re-read the value before publishing it to a prompt.
        let mut current = Vec::with_capacity(top_k);
        for (_, candidate) in scored {
            if current.len() >= top_k {
                break;
            }
            let Some(raw) = self.store.get(ns, &candidate.key).await? else {
                continue;
            };
            let mut latest = TypedMemoryEntry::from_store_item(raw);
            if !same_recall_fact(&candidate, &latest) || !latest.meta.is_recallable() {
                continue;
            }
            latest.raw.score = candidate.raw.score;
            current.push(latest);
        }

        // 3. recall_count +1 (fire-and-forget; Dreaming consumes it in stage 2).
        let typed_for_count = TypedMemoryStore::new(self.store.clone());
        let recalled = current.clone();
        let namespace = ns.iter().map(|part| (*part).to_owned()).collect::<Vec<_>>();
        tokio::spawn(async move {
            let namespace_refs = namespace.iter().map(String::as_str).collect::<Vec<_>>();
            for entry in recalled {
                if let Err(error) =
                    incr_recall_count(&typed_for_count, &namespace_refs, &entry).await
                {
                    tracing::debug!(key = %entry.key, %error, "failed to update memory recall telemetry");
                }
            }
        });

        Ok(current.into_iter().map(|entry| entry.raw).collect())
    }
}

fn same_recall_fact(before: &TypedMemoryEntry, after: &TypedMemoryEntry) -> bool {
    let mut meta = after.meta.clone();
    meta.recall_count = before.meta.recall_count;
    meta.last_recalled_at = before.meta.last_recalled_at;
    before.content == after.content && before.meta == meta
}

/// Composite recall score: `S = 0.5·sim + 0.3·decay(age, 30d) + 0.2·recall_weight`.
fn composite_score(sim: f64, age_days: f64, recall_weight: f64) -> f64 {
    0.5 * sim + 0.3 * 0.5_f64.powf(age_days / 30.0) + 0.2 * recall_weight
}

/// Age in days from `StoreItem::created_at` (Unix seconds).
fn age_days_from_storeitem(item: &StoreItem) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(item.created_at) as f64 / 86400.0
}

/// Increment `recall_count` (get-modify-put; `update_meta` takes a full
/// `MemoryMeta`, not a closure). Fire-and-forget from recall; lost increments
/// are acceptable (diagnostic counter).
async fn incr_recall_count(
    typed: &TypedMemoryStore,
    ns: &[&str],
    entry: &TypedMemoryEntry,
) -> crate::error::Result<()> {
    let mut meta = entry.meta.clone();
    meta.recall_count = meta.recall_count.saturating_add(1);
    meta.last_recalled_at = Some(crate::utils::time::now_secs());
    let _ = typed
        .compare_and_put_typed(
            ns,
            &entry.key,
            Some(entry.raw.value.clone()),
            &entry.content,
            meta,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::memory::store::StoreCompareAndPutOutcome;
    use echo_core::memory::types::{
        MemoryApproval, MemoryEvidence, MemoryEvidenceRole, MemoryMeta, MemoryProvenance,
        MemorySource, MemoryTrust, MemoryType, TypedMemoryValue,
    };
    use echo_state::memory::store::InMemoryStore;
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn approved_meta(status: MemoryStatus) -> MemoryMeta {
        let mut provenance = MemoryProvenance::draft(
            MemoryTrust::User,
            vec![MemoryEvidence::new(
                MemoryEvidenceRole::User,
                "Use Rust for durable services",
            )],
        );
        provenance.approval = Some(MemoryApproval::new(
            "approval-recall",
            "test-reviewer",
            1_750_000_000,
        ));
        MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        )
        .with_status(status)
        .with_provenance(provenance)
    }

    struct StaleSearchStore {
        inner: InMemoryStore,
        replacement: serde_json::Value,
        replace_on_search: AtomicBool,
    }

    impl Store for StaleSearchStore {
        fn put<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
            value: serde_json::Value,
        ) -> BoxFuture<'a, echo_core::error::Result<()>> {
            self.inner.put(namespace, key, value)
        }

        fn compare_and_put<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
            expected: Option<serde_json::Value>,
            value: serde_json::Value,
        ) -> BoxFuture<'a, echo_core::error::Result<StoreCompareAndPutOutcome>> {
            self.inner.compare_and_put(namespace, key, expected, value)
        }

        fn get<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<Option<StoreItem>>> {
            self.inner.get(namespace, key)
        }

        fn search<'a>(
            &'a self,
            namespace: &'a [&'a str],
            query: &'a str,
            limit: usize,
        ) -> BoxFuture<'a, echo_core::error::Result<Vec<StoreItem>>> {
            Box::pin(async move {
                let stale = self.inner.search(namespace, query, limit).await?;
                if self.replace_on_search.swap(false, Ordering::SeqCst) {
                    self.inner
                        .put(namespace, "stale", self.replacement.clone())
                        .await?;
                }
                Ok(stale)
            })
        }

        fn delete<'a>(
            &'a self,
            namespace: &'a [&'a str],
            key: &'a str,
        ) -> BoxFuture<'a, echo_core::error::Result<bool>> {
            self.inner.delete(namespace, key)
        }

        fn list_namespaces<'a>(
            &'a self,
            prefix: Option<&'a [&'a str]>,
        ) -> BoxFuture<'a, echo_core::error::Result<Vec<Vec<String>>>> {
            self.inner.list_namespaces(prefix)
        }

        fn list<'a>(
            &'a self,
            namespace: &'a [&'a str],
        ) -> BoxFuture<'a, echo_core::error::Result<Vec<StoreItem>>> {
            self.inner.list(namespace)
        }
    }

    #[tokio::test]
    async fn draft_and_unverified_legacy_active_are_not_recalled() -> crate::error::Result<()> {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store.clone());
        for (key, status) in [
            ("draft", MemoryStatus::Draft),
            ("legacy-active", MemoryStatus::Active),
        ] {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    key,
                    "Unverified project preference",
                    MemoryMeta::new(
                        MemoryType::UserPreference,
                        MemorySource::L3Promotion,
                        "project",
                    )
                    .with_status(status),
                )
                .await?;
        }

        let recalled = MemoryRecaller::new(store)
            .recall("Unverified project preference", 5)
            .await?;
        assert!(
            recalled.is_empty(),
            "draft and legacy values reached recall"
        );
        Ok(())
    }

    #[tokio::test]
    async fn approved_active_and_archived_memories_share_recall_eligibility()
    -> crate::error::Result<()> {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store.clone());
        for (key, status) in [
            ("active", MemoryStatus::Active),
            ("archived", MemoryStatus::Archived),
        ] {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    key,
                    "Use Rust for durable services",
                    approved_meta(status),
                )
                .await?;
        }

        let recalled = MemoryRecaller::new(store)
            .recall("Rust durable services", 5)
            .await?;
        assert_eq!(recalled.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn stale_recall_telemetry_cannot_overwrite_concurrent_draft() -> crate::error::Result<()>
    {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store.clone());
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "stale",
                "Use Rust for durable services",
                approved_meta(MemoryStatus::Active),
            )
            .await?;
        let stale = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "stale")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("memory missing".to_string()))?;

        let mut draft = approved_meta(MemoryStatus::Draft);
        draft.provenance.approval = None;
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "stale",
                "Use Rust for durable services",
                draft,
            )
            .await?;
        incr_recall_count(&typed, crate::evolution::layer::WARM_NAMESPACE, &stale).await?;

        let current = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "stale")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("memory missing".to_string()))?;
        assert_eq!(current.meta.status, MemoryStatus::Draft);
        assert_eq!(current.meta.recall_count, 0);
        assert!(current.meta.provenance.approval.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn draft_heavy_search_still_finds_approved_memory() -> crate::error::Result<()> {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store.clone());
        for index in 0..40 {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &format!("draft-{index}"),
                    "Use Rust for durable services",
                    approved_meta(MemoryStatus::Draft),
                )
                .await?;
        }
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "approved",
                "Use Rust for durable services",
                approved_meta(MemoryStatus::Active),
            )
            .await?;

        let recalled = MemoryRecaller::new(store)
            .recall("Rust durable services", 1)
            .await?;
        assert_eq!(
            recalled.first().map(|item| item.key.as_str()),
            Some("approved")
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_search_hit_cannot_enter_prompt_after_draft_replacement()
    -> crate::error::Result<()> {
        let mut draft = approved_meta(MemoryStatus::Draft);
        draft.provenance.approval = None;
        let replacement = TypedMemoryValue::new("Use Rust for durable services", draft)
            .to_value()
            .map_err(|error| crate::error::ReactError::Other(error.to_string()))?;
        let store = Arc::new(StaleSearchStore {
            inner: InMemoryStore::new(),
            replacement,
            replace_on_search: AtomicBool::new(false),
        });
        let typed = TypedMemoryStore::new(store.clone());
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "stale",
                "Use Rust for durable services",
                approved_meta(MemoryStatus::Active),
            )
            .await?;
        store.replace_on_search.store(true, Ordering::SeqCst);

        let recalled = MemoryRecaller::new(store.clone())
            .recall("Rust durable services", 1)
            .await?;
        assert!(recalled.is_empty());
        let current = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "stale")
            .await?
            .ok_or_else(|| crate::error::ReactError::Other("memory missing".to_string()))?;
        assert_eq!(current.meta.status, MemoryStatus::Draft);
        Ok(())
    }
}
