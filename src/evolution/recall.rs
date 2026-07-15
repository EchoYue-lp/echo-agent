//! (stage4 D1) Unified memory recall entry.
//!
//! Both the auto recall path (`ReactAgent::recall_long_term_memories`) and the
//! tool recall path (`LayeredRecallTool` → `MemoryLayerManager::search_layered`)
//! delegate to [`MemoryRecaller`] so they return consistently composite-score-
//! ranked results over the unified `["agent","memories"]` namespace
//! (割裂点 3/9 — previously the two paths read different namespaces and ranked
//! differently).

use echo_core::memory::store::{Store, StoreItem};
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
    /// Superseded memories are filtered out (割裂点 5); `recall_count` is bumped
    /// fire-and-forget for Dreaming self-evolution (stage 2).
    pub async fn recall(&self, query: &str, top_k: usize) -> crate::error::Result<Vec<StoreItem>> {
        let ns = crate::evolution::layer::WARM_NAMESPACE; // ["agent","memories"]

        // 1. Candidates via hybrid search; any hybrid error falls back to keyword
        //    search (no string-matching on error text).
        let candidates = match self
            .store
            .search_with(ns, SearchQuery::hybrid(query, top_k * 3))
            .await
        {
            Ok(items) => items,
            Err(_) => self.store.search(ns, query, top_k * 3).await?,
        };

        // 2. Composite-score re-rank + status filter (Superseded dropped).
        let mut scored: Vec<(f64, TypedMemoryEntry)> = candidates
            .into_iter()
            .filter_map(|item| {
                let entry = TypedMemoryEntry::from_store_item(item);
                if entry.meta.status == MemoryStatus::Superseded {
                    return None;
                }
                let sim = entry.raw.score.unwrap_or(0.0) as f64;
                let age = age_days_from_storeitem(&entry.raw);
                let s = composite_score(sim, age, entry.meta.recall_weight as f64);
                Some((s, entry))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        // 3. recall_count +1 (fire-and-forget; Dreaming consumes it in stage 2).
        let typed_for_count = TypedMemoryStore::new(self.store.clone());
        let keys: Vec<String> = scored.iter().map(|(_, e)| e.raw.key.clone()).collect();
        tokio::spawn(async move {
            for key in keys {
                if let Err(error) =
                    incr_recall_count(&typed_for_count, &["agent", "memories"], &key).await
                {
                    tracing::debug!(%key, %error, "failed to update memory recall telemetry");
                }
            }
        });

        Ok(scored.into_iter().map(|(_, e)| e.raw).collect())
    }
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
    key: &str,
) -> crate::error::Result<()> {
    if let Some(entry) = typed.get_typed(ns, key).await? {
        let mut meta = entry.meta;
        meta.recall_count = meta.recall_count.saturating_add(1);
        meta.last_recalled_at = Some(crate::utils::time::now_secs());
        typed.update_meta(ns, key, meta).await?;
    }
    Ok(())
}
