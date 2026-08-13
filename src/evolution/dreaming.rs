//! (stage4 F1) Dreaming — recall-frequency-driven self-evolution.
//!
//! Replaces the old "every-N-writes triggers a full review" model (割裂点 6/9)
//! with an OpenClaw-Dreaming-style scheduled pass: scan the unified memory
//! store, promote high-recall memories (incl. Archived, revived first) to the
//! hot layer (MEMORY.md → system prompt stable prefix), and batch-demote stale
//! low-recall memories to Archived. Staleness is a recall-decay weight, not a
//! death sentence — Archived memories stay recallable (with decay) and can be
//! revived by future Dreaming passes.
//!
//! Design basis: `stage4-memory-evolution-compression-decision.md` (D14),
//! OpenClaw Dreaming (cron + recall-statistics-driven promotion), hermes
//! composite-score recall (`recall_count` bumped fire-and-forget by
//! `MemoryRecaller`).

use crate::evolution::layer::MemoryLayerManager;
use echo_core::memory::types::MemoryStatus;
use echo_core::utils::time::now_secs;
use echo_state::memory::typed_store::MemoryFilter;
use std::sync::Arc;

// ── Config ─────────────────────────────────────────────────────────────

/// Configuration for a Dreaming pass.
#[derive(Debug, Clone)]
pub struct DreamingConfig {
    /// Minimum `recall_count` for a memory to be considered for hot promotion.
    /// Memories recalled fewer times than this are not promoted (even if old).
    pub recall_count_threshold: u32,
    /// Cap on how many memories a single pass promotes to hot (avoids flooding
    /// MEMORY.md / the system prompt stable prefix in one run).
    pub max_promoted_per_run: usize,
    /// A high-recall memory must also have been used within this window before
    /// it can be promoted or revived. This prevents lifetime recall counts from
    /// making an obsolete fact permanently hot-eligible.
    pub promotion_recency_days: u32,
    /// A memory older than this many days AND below `low_recall_threshold` is
    /// demoted to Archived (staleness decay, not deletion).
    pub stale_age_days: u32,
    /// Below this `recall_count`, a stale Active memory is demoted to Archived.
    pub low_recall_threshold: u32,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            recall_count_threshold: 5,
            max_promoted_per_run: 20,
            promotion_recency_days: 30,
            stale_age_days: 30,
            low_recall_threshold: 1,
        }
    }
}

// ── Report ─────────────────────────────────────────────────────────────

/// Summary of one Dreaming pass.
#[derive(Debug, Clone, Default)]
pub struct DreamingReport {
    /// Total memories scanned in the unified namespace.
    pub scanned: usize,
    /// Memories promoted to the hot layer (MEMORY.md).
    pub promoted: usize,
    /// Archived memories revived back to Active (G2 — prerequisite for promotion).
    pub revived: usize,
    /// Stale low-recall Active memories demoted to Archived.
    pub demoted: usize,
    /// Explainable deterministic changes applied by this pass.
    pub decisions: Vec<DreamingDecision>,
}

/// One deterministic maintenance action applied by Dreaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamingAction {
    Revived,
    PromotedToHot,
    Archived,
}

#[derive(Debug, Clone)]
pub struct DreamingDecision {
    pub key: String,
    pub action: DreamingAction,
    pub recall_count: u32,
    pub inactive_days: f64,
    pub reason: String,
}

// ── Dreaming ───────────────────────────────────────────────────────────

/// Dreaming self-evolution pass driver.
///
/// Construct with the shared `MemoryLayerManager` (same store the agent recalls
/// from, so revives/demotes land in the unified `["agent","memories"]` namespace
/// the agent reads). Call [`Dreaming::run`] on a cron/daily schedule.
pub struct Dreaming {
    config: DreamingConfig,
    layer_manager: Arc<MemoryLayerManager>,
}

impl Dreaming {
    /// Create a new Dreaming driver.
    pub fn new(layer_manager: Arc<MemoryLayerManager>, config: DreamingConfig) -> Self {
        Self {
            config,
            layer_manager,
        }
    }

    /// Run one Dreaming pass over the unified `["agent","memories"]` namespace.
    ///
    /// Best-effort per-memory: a revive/promote/demote error on one memory is
    /// logged and does not abort the pass. Returns a summary report.
    pub async fn run(&self) -> crate::error::Result<DreamingReport> {
        let now = now_secs();
        let entries = self
            .layer_manager
            .list_warm_memories(&MemoryFilter::new())
            .await?;
        let mut report = DreamingReport {
            scanned: entries.len(),
            ..Default::default()
        };

        for e in &entries {
            // Superseded memories are tombstones — never revive/promote/demote.
            if e.meta.status == MemoryStatus::Superseded {
                continue;
            }
            let inactive_days = age_days(e.meta.last_recalled_at.unwrap_or(e.raw.created_at), now);

            // 1. Promote high-recall memories (incl Archived) to hot. G2: revive
            //    Archived→Active first because `is_hot_eligible()` requires
            //    `status == Active` (otherwise every Archived memory would fail
            //    the gate regardless of confidence/stability).
            if e.meta.recall_count >= self.config.recall_count_threshold
                && inactive_days <= self.config.promotion_recency_days as f64
                && report.promoted < self.config.max_promoted_per_run
            {
                if e.meta.status == MemoryStatus::Archived {
                    match self.layer_manager.revive_archived(&e.key).await {
                        Ok(true) => {
                            report.revived += 1;
                            report.decisions.push(DreamingDecision {
                                key: e.key.clone(),
                                action: DreamingAction::Revived,
                                recall_count: e.meta.recall_count,
                                inactive_days,
                                reason: "archived memory was recalled frequently and recently"
                                    .to_string(),
                            });
                        }
                        Ok(false) => {}
                        Err(error) => {
                            tracing::warn!(key = %e.key, %error, "Dreaming failed to revive memory");
                        }
                    }
                }
                match self.layer_manager.consider_promotion(&e.key).await {
                    Ok(Some(_)) => {
                        report.promoted += 1;
                        report.decisions.push(DreamingDecision {
                            key: e.key.clone(),
                            action: DreamingAction::PromotedToHot,
                            recall_count: e.meta.recall_count,
                            inactive_days,
                            reason: "memory met deterministic hot-layer eligibility".to_string(),
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(key = %e.key, %error, "Dreaming failed to promote memory");
                    }
                }
            }

            // 2. Batch-demote stale low-recall Active memories to Archived
            //    (replaces the old review's staleness demote; Archived stays
            //    recallable with decay and can be revived by a future pass).
            if e.meta.status == MemoryStatus::Active
                && e.meta.recall_count < self.config.low_recall_threshold
                && inactive_days > self.config.stale_age_days as f64
            {
                match self
                    .layer_manager
                    .demote(&e.key, "dreaming: stale + low recall")
                    .await
                {
                    Ok(_) => {
                        report.demoted += 1;
                        report.decisions.push(DreamingDecision {
                            key: e.key.clone(),
                            action: DreamingAction::Archived,
                            recall_count: e.meta.recall_count,
                            inactive_days,
                            reason: "memory exceeded the inactivity window with low recall"
                                .to_string(),
                        });
                    }
                    Err(error) => {
                        tracing::warn!(key = %e.key, %error, "Dreaming failed to archive memory");
                    }
                }
            }
        }

        tracing::info!(
            scanned = report.scanned,
            promoted = report.promoted,
            revived = report.revived,
            demoted = report.demoted,
            "Dreaming pass complete"
        );
        Ok(report)
    }
}

fn age_days(created_at: u64, now: u64) -> f64 {
    now.saturating_sub(created_at) as f64 / 86400.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::audit::NullChangeLog;
    use echo_core::memory::store::StoreItem;
    use echo_core::memory::types::{
        MemoryMeta, MemorySource, MemoryStatus, MemoryType, TypedMemoryValue,
    };
    use echo_state::memory::store::InMemoryStore;

    fn make_manager() -> (Arc<MemoryLayerManager>, Arc<InMemoryStore>) {
        let store = Arc::new(InMemoryStore::new());
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let lm = Arc::new(MemoryLayerManager::new(
            dir,
            store.clone(),
            Box::new(NullChangeLog),
        ));
        (lm, store)
    }

    /// High-recall Archived memory is revived (G2) + promoted to hot.
    #[tokio::test]
    async fn dreaming_revives_and_promotes_high_recall_archived() {
        let (lm, store) = make_manager();
        // High-confidence UserPreference, Archived, recall_count=10. Insert via
        // put_raw to bypass `write_memory`'s auto-promote-to-hot (which would
        // move the entry to hot before we can mark it Archived in warm).
        let mut meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "user",
        )
        .with_confidence(0.9)
        .with_recall_weight(0.9);
        meta.status = MemoryStatus::Archived;
        meta.recall_count = 10;
        let value = TypedMemoryValue::new("user prefers Rust over Python", meta)
            .to_value()
            .expect("to_value");
        store
            .put_raw(StoreItem::new(
                vec!["agent".to_string(), "memories".to_string()],
                "k1".to_string(),
                value,
            ))
            .await;

        let dreaming = Dreaming::new(lm.clone(), DreamingConfig::default());
        let report = dreaming.run().await.expect("dreaming run");

        assert_eq!(report.scanned, 1, "one memory scanned");
        assert_eq!(
            report.revived, 1,
            "Archived high-recall memory revived to Active"
        );
        assert_eq!(
            report.promoted, 1,
            "hot-eligible revived memory promoted to hot"
        );
        assert_eq!(report.decisions.len(), 2);
        // Promoted memory is removed from the warm layer (now in MEMORY.md).
        let warm = lm
            .list_warm_memories(&MemoryFilter::new())
            .await
            .expect("list");
        assert!(
            !warm.iter().any(|e| e.key == "k1"),
            "promoted memory should be gone from warm, got: {:?}",
            warm
        );
    }

    /// Stale low-recall Active memory is demoted to Archived (staleness decay,
    /// not deletion). Uses `put_raw` to backdate `created_at`.
    #[tokio::test]
    async fn dreaming_demotes_stale_low_recall_active() {
        let (lm, store) = make_manager();
        // Low-confidence ProjectFact, recall_count=0, Active. Backdate created_at.
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "project",
        )
        .with_confidence(0.4);
        let value = TypedMemoryValue::new("maybe uses yarn", meta)
            .to_value()
            .expect("to_value");
        let mut item = StoreItem::new(
            vec!["agent".to_string(), "memories".to_string()],
            "k_stale".to_string(),
            value,
        );
        item.created_at = now_secs().saturating_sub(40 * 86400); // 40 days old
        store.put_raw(item).await;

        let dreaming = Dreaming::new(lm.clone(), DreamingConfig::default());
        let report = dreaming.run().await.expect("dreaming run");

        assert_eq!(report.scanned, 1);
        assert_eq!(report.demoted, 1, "stale low-recall Active memory demoted");
        // Demoted memory stays in warm (in place) but is now Archived.
        let warm = lm
            .list_warm_memories(&MemoryFilter::new())
            .await
            .expect("list");
        let e = warm
            .iter()
            .find(|e| e.key == "k_stale")
            .expect("still in warm");
        assert_eq!(
            e.meta.status,
            MemoryStatus::Archived,
            "demoted to Archived in place"
        );
    }

    /// Fresh low-recall Active memory (not stale) is left alone.
    #[tokio::test]
    async fn dreaming_leaves_fresh_low_recall_active_alone() {
        let (lm, _store) = make_manager();
        let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::AutoExtracted, "p")
            .with_confidence(0.4);
        lm.write_memory("k_fresh", "a fresh low-recall fact", meta)
            .await
            .expect("write");

        let dreaming = Dreaming::new(lm.clone(), DreamingConfig::default());
        let report = dreaming.run().await.expect("dreaming run");

        assert_eq!(report.scanned, 1);
        assert_eq!(report.promoted, 0, "low-recall not promoted");
        assert_eq!(report.demoted, 0, "fresh memory not demoted (not stale)");
    }

    #[tokio::test]
    async fn dreaming_does_not_promote_old_lifetime_recall_count() -> crate::error::Result<()> {
        let (lm, store) = make_manager();
        let mut meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "user",
        )
        .with_confidence(0.9)
        .with_recall_weight(0.9);
        meta.status = MemoryStatus::Archived;
        meta.recall_count = 10;
        let value = TypedMemoryValue::new("old preference", meta).to_value()?;
        let mut item = StoreItem::new(
            vec!["agent".to_string(), "memories".to_string()],
            "old_recall".to_string(),
            value,
        );
        item.created_at = now_secs().saturating_sub(90 * 86400);
        store.put_raw(item).await;

        let report = Dreaming::new(lm, DreamingConfig::default()).run().await?;

        assert_eq!(report.revived, 0);
        assert_eq!(report.promoted, 0);
        assert!(report.decisions.is_empty());
        Ok(())
    }
}
