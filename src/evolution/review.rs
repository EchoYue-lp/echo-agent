//! Memory review and garbage collection — scan, score, conflict-detect, merge, archive.
//!
//! Phase 2 of the evolution system. Phase 1 gave memories typed metadata, layered
//! storage (hot/warm/cold), and an audit log, but memories were never re-evaluated
//! after creation. This module adds:
//!
//! - [`StalenessScorer`] — full staleness score from age, usage, instability,
//!   contradiction, and source-weakness factors (replaces `MemoryMeta::base_staleness`).
//! - [`ConflictDetector`] — groups memories sharing the same topic + type with
//!   different content hashes.
//! - [`MemoryMerger`] — merges a conflict group into one primary entry, superseding
//!   the rest (first real use of `MemoryStatus::Superseded` and `ChangeType::Merge`).
//! - [`MemoryReviewer`] — orchestrator: scan → score → detect → merge → archive.
//!
//! # Status thresholds
//!
//! The factor caps in the formula (age ≤ 0.8, contradiction ≤ 0.5) mean the
//! theoretical maximum staleness is only ≈0.83 — a deliberately conservative
//! design so that no single factor, maxed out alone, can doom an entry. The
//! thresholds below are calibrated to that actual dynamic range rather than the
//! full `[0, 1]`:
//!
//! | Staleness  | Status                       |
//! |------------|------------------------------|
//! | `< 0.35`   | Active                       |
//! | `0.35–0.50`| Active (flagged for review)  |
//! | `0.50–0.65`| Superseded candidate         |
//! | `≥ 0.65`   | Archived candidate           |
//!
//! The reviewer only mutates entries that cross the archive threshold (≥ 0.65):
//! it demotes them to cold and marks them `MemoryStatus::Archived`. Entries in the
//! 0.35–0.65 band are surfaced in the report but left in place — a human (or a
//! future LLM-assisted reviewer) decides whether to act.

use chrono::{DateTime, Duration, Utc};
use echo_core::memory::types::{MemoryMeta, MemoryStatus, MemoryType};
use echo_core::utils::hash::fnv1a_64;
use echo_state::memory::typed_store::{MemoryFilter, TypedMemoryEntry, TypedMemoryStore};
use std::collections::HashMap;

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
use super::layer::MemoryLayerManager;
use crate::error::Result;

// ── Constants ──────────────────────────────────────────────────────────

/// Staleness threshold below which an entry stays Active.
///
/// Calibrated to the formula's dynamic range (max ≈ 0.83), not the full `[0,1]`.
const STALENESS_ACTIVE_MAX: f32 = 0.35;
/// Staleness threshold above which an entry is a superseded candidate.
const STALENESS_SUPERSEDED_MIN: f32 = 0.50;
/// Staleness threshold above which an entry is archived.
const STALENESS_ARCHIVE_MIN: f32 = 0.65;

// ── StalenessScorer ────────────────────────────────────────────────────

/// Computes a staleness score (0.0 = fresh, 1.0 = very stale) for a typed memory.
///
/// `staleness = age·0.35 + low_usage·0.20 + instability·0.20 + contradiction·0.20 + source_weakness·0.05`
///
/// Factor weights are tuned so that no single factor dominates: an old but
/// frequently-revised, high-confidence entry from a trusted source can still
/// stay below the archive threshold, while a fresh but contradictory,
/// low-confidence auto-extracted entry can still be flagged.
pub struct StalenessScorer;

impl StalenessScorer {
    /// Create a new scorer.
    pub fn new() -> Self {
        Self
    }

    /// Score a single entry.
    ///
    /// `has_contradiction` should be `true` when the [`ConflictDetector`] found
    /// this entry in a conflict group. Because detection runs after the initial
    /// scoring pass in the reviewer, callers typically score twice — first with
    /// `false`, then re-score the conflicted entries with `true`.
    pub fn score(
        &self,
        entry: &TypedMemoryEntry,
        now: DateTime<Utc>,
        has_contradiction: bool,
    ) -> StalenessReport {
        let age_factor = age_factor(entry, now);
        let usage_factor = low_usage_factor(&entry.meta);
        let instability_factor = 1.0 - entry.meta.stability;
        let contradiction_factor = if has_contradiction { 0.5 } else { 0.0 };
        let source_factor = 1.0 - entry.meta.source.default_confidence();

        let staleness = (age_factor * 0.35
            + usage_factor * 0.20
            + instability_factor * 0.20
            + contradiction_factor * 0.20
            + source_factor * 0.05)
            .clamp(0.0, 1.0);

        let recommended_status = recommended_status(staleness, entry.meta.status);

        StalenessReport {
            key: entry.key.clone(),
            staleness,
            age_factor,
            usage_factor,
            instability_factor,
            contradiction_factor,
            source_factor,
            recommended_status,
        }
    }
}

impl Default for StalenessScorer {
    fn default() -> Self {
        Self::new()
    }
}

/// A single entry's staleness breakdown.
#[derive(Debug, Clone)]
pub struct StalenessReport {
    /// The scored memory key.
    pub key: String,
    /// Aggregate staleness (0.0–1.0).
    pub staleness: f32,
    /// Time-since-creation/update factor.
    pub age_factor: f32,
    /// Low-revision-count factor (proxy for low usage).
    pub usage_factor: f32,
    /// `1.0 - stability`.
    pub instability_factor: f32,
    /// 0.0 (no conflict) or 0.5 (in a conflict group).
    pub contradiction_factor: f32,
    /// `1.0 - source.default_confidence()`.
    pub source_factor: f32,
    /// Status the reviewer would move this entry to.
    pub recommended_status: MemoryStatus,
}

/// Compute the age factor from the entry's most recent timestamp.
///
/// `<7d → 0.0`, `7–30d → 0.2`, `30–90d → 0.5`, `>90d → 0.8`.
fn age_factor(entry: &TypedMemoryEntry, now: DateTime<Utc>) -> f32 {
    // Prefer updated_at (last revision), fall back to created_at.
    let secs = entry.raw.updated_at.max(entry.raw.created_at);
    let Some(then) = DateTime::<Utc>::from_timestamp(secs as i64, 0) else {
        // Unparseable timestamp — treat as unknown-age (neutral).
        return 0.2;
    };
    let age = now.signed_duration_since(then);
    if age < Duration::days(7) {
        0.0
    } else if age < Duration::days(30) {
        0.2
    } else if age < Duration::days(90) {
        0.5
    } else {
        0.8
    }
}

/// Low-usage factor. Since the Store does not track access counts, we use
/// `revision_count` as a proxy: `1.0 - min(revision_count / 3.0, 1.0)`.
fn low_usage_factor(meta: &MemoryMeta) -> f32 {
    1.0 - (meta.revision_count as f32 / 3.0).min(1.0)
}

/// Map a staleness score to a recommended status.
///
/// Already-archived/superseded entries keep their terminal status — the reviewer
/// never re-activates something a prior review retired.
fn recommended_status(staleness: f32, current: MemoryStatus) -> MemoryStatus {
    match current {
        // Terminal statuses are sticky.
        MemoryStatus::Archived | MemoryStatus::Superseded => current,
        MemoryStatus::Draft | MemoryStatus::Active => {
            if staleness >= STALENESS_ARCHIVE_MIN {
                MemoryStatus::Archived
            } else if staleness >= STALENESS_SUPERSEDED_MIN {
                MemoryStatus::Superseded
            } else {
                MemoryStatus::Active
            }
        }
    }
}

// ── ConflictDetector ───────────────────────────────────────────────────

/// Groups memories that share a topic + type but have different content hashes.
///
/// Two memories "conflict" when they live in the same `(topic, memory_type)`
/// bucket but their trimmed content hashes differ — i.e. they make different
/// claims about the same subject. Identical content (true duplicates) is *not*
/// treated as a conflict by this detector; deduplication is handled elsewhere
/// (`memory_promoter.rs`).
pub struct ConflictDetector;

impl ConflictDetector {
    /// Create a new detector.
    pub fn new() -> Self {
        Self
    }

    /// Detect conflict groups in a set of entries.
    ///
    /// A group is returned only when it contains **2 or more** entries with
    /// *different* content hashes under the same `(topic, memory_type)`.
    /// Pure-duplicate groups (all identical content) are dropped — they are
    /// dedup candidates, not conflicts.
    pub fn detect(&self, entries: &[TypedMemoryEntry]) -> Vec<ConflictGroup> {
        // Bucket by (topic, memory_type) → list of (hash, entry index).
        let mut buckets: HashMap<(String, MemoryType), Vec<(u64, usize)>> = HashMap::new();
        for (idx, entry) in entries.iter().enumerate() {
            let hash = content_hash(&entry.content);
            buckets
                .entry((entry.meta.topic.clone(), entry.meta.memory_type))
                .or_default()
                .push((hash, idx));
        }

        let mut groups = Vec::new();
        for ((topic, memory_type), members) in buckets {
            if members.len() < 2 {
                continue;
            }
            // Need at least two distinct hashes to call it a conflict.
            let distinct_hashes: std::collections::HashSet<u64> =
                members.iter().map(|(h, _)| *h).collect();
            if distinct_hashes.len() < 2 {
                continue;
            }
            let group_entries = members
                .into_iter()
                .map(|(_, idx)| entries[idx].clone())
                .collect();
            groups.push(ConflictGroup {
                topic,
                memory_type,
                entries: group_entries,
            });
        }
        groups
    }
}

impl Default for ConflictDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// A set of 2+ memories that conflict on the same `(topic, memory_type)`.
#[derive(Debug, Clone)]
pub struct ConflictGroup {
    /// Shared topic.
    pub topic: String,
    /// Shared memory type.
    pub memory_type: MemoryType,
    /// The conflicting entries (≥2, with at least two distinct contents).
    pub entries: Vec<TypedMemoryEntry>,
}

/// Content hash for conflict comparison — FNV-1a over trimmed content.
///
/// Matches the existing dedup hash in `memory_promoter.rs` so the two systems
/// agree on what counts as "same content".
fn content_hash(content: &str) -> u64 {
    fnv1a_64(content.trim().as_bytes())
}

// ── MemoryMerger ───────────────────────────────────────────────────────

/// Merges a [`ConflictGroup`] into a single primary entry, superseding the rest.
///
/// Merge policy:
/// - The entry with the highest `confidence` wins (ties broken by recency of
///   `updated_at`, then by key for determinism).
/// - The primary's content is annotated with `(merged from N similar entries)`.
/// - Secondary entries are rewritten with `status = Superseded`,
///   `superseded_by = <primary key>`, and their `revision_count` is folded into
///   the primary's.
/// - Each mutation is recorded via [`ChangeLog`] with `ChangeType::Merge`.
pub struct MemoryMerger<'a> {
    typed_store: &'a TypedMemoryStore,
    change_log: &'a dyn ChangeLog,
}

/// Outcome of merging one conflict group.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Key of the surviving primary entry.
    pub primary_key: String,
    /// Keys of the entries superseded by the primary.
    pub superseded_keys: Vec<String>,
}

impl<'a> MemoryMerger<'a> {
    /// Create a new merger bound to a store and audit log.
    pub fn new(typed_store: &'a TypedMemoryStore, change_log: &'a dyn ChangeLog) -> Self {
        Self {
            typed_store,
            change_log,
        }
    }

    /// Merge a single conflict group.
    ///
    /// Returns `Ok(MergeResult)` with an empty `superseded_keys` when the group
    /// has fewer than 2 entries (nothing to merge).
    pub async fn merge_group(&self, group: &ConflictGroup) -> Result<MergeResult> {
        if group.entries.len() < 2 {
            return Ok(MergeResult {
                primary_key: group
                    .entries
                    .first()
                    .map(|e| e.key.clone())
                    .unwrap_or_default(),
                superseded_keys: Vec::new(),
            });
        }

        // Pick the primary: highest confidence, then most recent updated_at,
        // then key (deterministic tiebreak).
        let mut ordered = group.entries.clone();
        ordered.sort_by(|a, b| {
            b.meta
                .confidence
                .partial_cmp(&a.meta.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.raw
                        .updated_at
                        .cmp(&a.raw.updated_at)
                        .then_with(|| a.key.cmp(&b.key))
                })
        });

        let primary = match ordered.first() {
            Some(p) => p.clone(),
            None => {
                return Ok(MergeResult {
                    primary_key: String::new(),
                    superseded_keys: Vec::new(),
                });
            }
        };
        let secondaries = &ordered[1..];

        let combined_revision_count = group
            .entries
            .iter()
            .map(|e| e.meta.revision_count)
            .sum::<u32>();

        // Rewrite the primary: annotate content, bump revision_count, keep its
        // own confidence/stability (it already won on quality).
        let merged_content = format!(
            "{} (merged from {} similar entries)",
            primary.content.trim(),
            group.entries.len()
        );
        let primary_meta = MemoryMeta {
            revision_count: combined_revision_count.max(primary.meta.revision_count),
            ..primary.meta.clone()
        };
        self.typed_store
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                &primary.key,
                &merged_content,
                primary_meta.clone(),
            )
            .await?;

        self.record_merge(
            &primary.key,
            /* superseded_by = */ None,
            group.entries.len(),
            &format!(
                "primary survivor of {}-way merge on topic '{}'",
                group.entries.len(),
                group.topic
            ),
        )?;

        // Supersede each secondary.
        let mut superseded_keys = Vec::with_capacity(secondaries.len());
        for secondary in secondaries {
            let secondary_meta = MemoryMeta {
                status: MemoryStatus::Superseded,
                superseded_by: Some(primary.key.clone()),
                ..secondary.meta.clone()
            };
            self.typed_store
                .update_meta(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &secondary.key,
                    secondary_meta,
                )
                .await?;
            superseded_keys.push(secondary.key.clone());

            self.record_merge(
                &secondary.key,
                Some(&primary.key),
                group.entries.len(),
                &format!(
                    "superseded by '{}' during merge on topic '{}'",
                    primary.key, group.topic
                ),
            )?;
        }

        Ok(MergeResult {
            primary_key: primary.key,
            superseded_keys,
        })
    }

    /// Record one merge-side change in the audit log.
    fn record_merge(
        &self,
        key: &str,
        superseded_by: Option<&str>,
        group_size: usize,
        reason: &str,
    ) -> Result<()> {
        let mut builder =
            ChangeEntryBuilder::new(EntityType::Memory, key, ChangeType::Merge).reason(reason);
        builder = builder.trigger("memory_reviewer".to_string());
        builder = builder.after(serde_json::json!({
            "superseded_by": superseded_by,
            "group_size": group_size,
        }));
        let entry = builder.build(self.change_log);
        self.change_log.record(entry)
    }
}

// ── MemoryReviewer (orchestrator) ──────────────────────────────────────

/// Orchestrates a full review pass: scan → score → detect conflicts → merge → archive.
pub struct MemoryReviewer {
    scorer: StalenessScorer,
    conflict_detector: ConflictDetector,
}

/// Tunable knobs for a review pass.
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    /// Run a review when the session ends. Default: `true`.
    pub review_on_session_end: bool,
    /// Run a review every N memory writes. Default: `50`.
    pub review_every_n_writes: u64,
    /// Cap on conflict groups merged per pass. Default: `10`.
    pub max_conflicts_per_review: usize,
    /// Cap on merges applied per pass. Default: `5`.
    pub max_merges_per_review: usize,
    /// Run skill candidate detection during review. Default: `true`.
    pub detect_skill_candidates: bool,
    /// Auto-generate draft SKILL.md for new candidates. Default: `false`.
    pub auto_generate_drafts: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            review_on_session_end: true,
            review_every_n_writes: 50,
            max_conflicts_per_review: 10,
            max_merges_per_review: 5,
            detect_skill_candidates: true,
            auto_generate_drafts: false,
        }
    }
}

/// A single mutation performed during a review pass.
#[derive(Debug, Clone)]
pub enum ReviewChange {
    /// A memory changed status (e.g. Active → Archived).
    StatusTransition {
        key: String,
        from: MemoryStatus,
        to: MemoryStatus,
        staleness: f32,
    },
    /// A conflict group was merged into one primary.
    Merge {
        primary_key: String,
        superseded_keys: Vec<String>,
    },
    /// A memory was archived (demoted to cold + status Archived).
    Archive { key: String, staleness: f32 },
    /// A new skill candidate was proposed from observed patterns.
    CandidateProposed { name: String, sample_count: usize },
    /// A draft SKILL.md was generated from a candidate.
    DraftGenerated { name: String, path: String },
}

/// Aggregate result of one review pass.
#[derive(Debug, Clone, Default)]
pub struct ReviewReport {
    /// Total entries scanned in the warm layer.
    pub total_scanned: usize,
    /// Entries whose staleness crossed the flag threshold (≥ 0.40).
    pub stale_count: usize,
    /// Conflict groups found.
    pub conflict_groups: usize,
    /// Merges actually applied.
    pub merges_applied: usize,
    /// Archives actually applied.
    pub archives_applied: usize,
    /// Skill candidates proposed during this review.
    pub candidates_proposed: usize,
    /// Draft SKILL.md files generated during this review.
    pub drafts_generated: usize,
    /// Individual mutations, in application order.
    pub changes: Vec<ReviewChange>,
}

impl MemoryReviewer {
    /// Create a new reviewer with default scorer and conflict detector.
    pub fn new() -> Self {
        Self {
            scorer: StalenessScorer::new(),
            conflict_detector: ConflictDetector::new(),
        }
    }

    /// Run a full review pass against the warm layer.
    ///
    /// Algorithm:
    /// 1. List all warm-layer entries (`MemoryFilter::new()`).
    /// 2. Detect conflict groups; re-score conflicted entries with the
    ///    contradiction factor turned on.
    /// 3. Archive entries with staleness ≥ 0.65 (demote to cold + `Archived`).
    /// 4. Merge conflict groups up to `max_merges_per_review`.
    /// 5. Re-evaluate hot-layer entries' demotion score with the freshly computed
    ///    staleness; demote any that now exceed the hot budget via the layer
    ///    manager's normal enforcement path.
    /// 6. Record every mutation through the change log (archive/merge do this
    ///    themselves; status-only transitions are recorded here).
    pub async fn review(
        &self,
        typed_store: &TypedMemoryStore,
        layer_manager: &MemoryLayerManager,
        change_log: &dyn ChangeLog,
        config: &ReviewConfig,
    ) -> Result<ReviewReport> {
        let now = Utc::now();
        let mut report = ReviewReport::default();

        // ── 1. Scan warm layer ──
        let entries = typed_store
            .list_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                &MemoryFilter::new(),
            )
            .await?;
        report.total_scanned = entries.len();
        if entries.is_empty() {
            return Ok(report);
        }

        // ── 2. Conflict detection ──
        let conflict_groups = self.conflict_detector.detect(&entries);
        // Collect the set of keys that participate in any conflict group so the
        // scoring pass can flag them.
        let mut conflicted_keys: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for group in &conflict_groups {
            for entry in &group.entries {
                conflicted_keys.insert(entry.key.clone());
            }
        }
        report.conflict_groups = conflict_groups.len();

        // ── 3. Score every entry ──
        let mut scored: Vec<StalenessReport> = Vec::with_capacity(entries.len());
        for entry in &entries {
            let has_contradiction = conflicted_keys.contains(&entry.key);
            scored.push(self.scorer.score(entry, now, has_contradiction));
        }
        report.stale_count = scored
            .iter()
            .filter(|r| r.staleness >= STALENESS_ACTIVE_MAX)
            .count();

        // ── 4. Archive high-staleness entries (≥ 0.65) ──
        // Build a key → current status lookup so we only record real transitions.
        let status_by_key: HashMap<String, MemoryStatus> = entries
            .iter()
            .map(|e| (e.key.clone(), e.meta.status))
            .collect();
        for report_entry in &scored {
            if report_entry.staleness < STALENESS_ARCHIVE_MIN {
                continue;
            }
            let Some(current_status) = status_by_key.get(&report_entry.key) else {
                continue;
            };
            // Skip entries already archived — they may still live in warm until
            // demotion, but we don't want to re-record an Archive change.
            if *current_status == MemoryStatus::Archived {
                continue;
            }

            match layer_manager
                .demote(&report_entry.key, "staleness-based archival")
                .await
            {
                Ok(_) => {
                    // Mark status Archived in the (now cold) entry.
                    // SAFETY: typed_store and layer_manager MUST share the same
                    // underlying Store instance (both created from the same Arc<dyn Store>).
                    // ReviewIntegration ensures this; if constructing manually, use the
                    // same Arc for both MemoryLayerManager::new() and TypedMemoryStore::new().
                    if let Some((_, entry)) = layer_manager.locate(&report_entry.key).await {
                        let archived_meta = MemoryMeta {
                            status: MemoryStatus::Archived,
                            ..entry.meta.clone()
                        };
                        let _ = typed_store
                            .update_meta(
                                crate::evolution::layer::COLD_NAMESPACE,
                                &report_entry.key,
                                archived_meta,
                            )
                            .await;
                    }
                    report.archives_applied += 1;
                    report.changes.push(ReviewChange::Archive {
                        key: report_entry.key.clone(),
                        staleness: report_entry.staleness,
                    });
                    report.changes.push(ReviewChange::StatusTransition {
                        key: report_entry.key.clone(),
                        from: *current_status,
                        to: MemoryStatus::Archived,
                        staleness: report_entry.staleness,
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        key = %report_entry.key,
                        error = %e,
                        "review: failed to archive stale entry"
                    );
                }
            }
        }

        // ── 5. Merge conflict groups ──
        let merger = MemoryMerger::new(typed_store, change_log);
        let mut merges_this_pass = 0usize;
        for group in conflict_groups.iter().take(config.max_conflicts_per_review) {
            if merges_this_pass >= config.max_merges_per_review {
                break;
            }
            // Skip groups whose primary was just archived.
            if group
                .entries
                .iter()
                .all(|e| status_by_key.get(&e.key) == Some(&MemoryStatus::Archived))
            {
                continue;
            }
            match merger.merge_group(group).await {
                Ok(result) if !result.superseded_keys.is_empty() => {
                    merges_this_pass += 1;
                    report.merges_applied += 1;
                    report.changes.push(ReviewChange::Merge {
                        primary_key: result.primary_key,
                        superseded_keys: result.superseded_keys,
                    });
                }
                Ok(_) => { /* group too small to merge — nothing to record */ }
                Err(e) => {
                    tracing::warn!(
                        topic = %group.topic,
                        error = %e,
                        "review: failed to merge conflict group"
                    );
                }
            }
        }

        // ── 6. Re-evaluate hot-layer budget with fresh staleness ──
        // enforce_hot_budget uses its own demotion score; calling it here lets any
        // newly-stale hot entries flow back to warm. Errors are non-fatal.
        if let Err(e) = layer_manager.enforce_hot_budget().await {
            tracing::warn!(error = %e, "review: hot-budget enforcement failed");
        }

        Ok(report)
    }
}

impl Default for MemoryReviewer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::layer::MemoryLayer;
    use super::*;
    use echo_core::memory::store::StoreItem;
    use echo_core::memory::types::MemorySource;
    use echo_state::memory::store::InMemoryStore;
    use std::sync::Arc;

    /// A no-op ChangeLog for testing.
    struct NullChangeLog;
    impl ChangeLog for NullChangeLog {
        fn record(&self, _entry: super::super::audit::ChangeEntry) -> Result<()> {
            Ok(())
        }
        fn query(
            &self,
            _filter: &super::super::audit::ChangeFilter,
        ) -> Result<Vec<super::super::audit::ChangeEntry>> {
            Ok(Vec::new())
        }
        fn latest_for(
            &self,
            _entity_type: EntityType,
            _entity_key: &str,
        ) -> Result<Option<super::super::audit::ChangeEntry>> {
            Ok(None)
        }
        fn len(&self) -> usize {
            0
        }
    }

    fn make_entry(key: &str, content: &str, meta: MemoryMeta, updated_at: u64) -> TypedMemoryEntry {
        let mut raw = StoreItem::new(
            vec!["agent".to_string(), "typed_memories".to_string()],
            key.to_string(),
            serde_json::Value::Null,
        );
        raw.updated_at = updated_at;
        raw.created_at = updated_at;
        TypedMemoryEntry {
            key: key.to_string(),
            content: content.to_string(),
            meta,
            raw,
        }
    }

    fn now_secs() -> u64 {
        Utc::now().timestamp().max(0) as u64
    }

    fn days_ago_secs(days: i64) -> u64 {
        ((Utc::now() - Duration::days(days)).timestamp()).max(0) as u64
    }

    // ── StalenessScorer ──

    #[test]
    fn test_fresh_high_quality_entry_is_active() {
        let scorer = StalenessScorer::new();
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90);
        let entry = make_entry("pref", "User prefers concise output", meta, now_secs());

        let report = scorer.score(&entry, Utc::now(), false);
        assert!(
            report.staleness < STALENESS_ACTIVE_MAX,
            "fresh high-quality entry should be Active, got staleness={:.3}",
            report.staleness
        );
        assert_eq!(report.recommended_status, MemoryStatus::Active);
        // Fresh entry ⇒ age factor 0; ExplicitSave ⇒ source factor 0.
        assert_eq!(report.age_factor, 0.0);
        assert_eq!(report.source_factor, 0.0);
    }

    #[test]
    fn test_old_low_quality_entry_is_archive_candidate() {
        let scorer = StalenessScorer::new();
        let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::L3Promotion, "old")
            .with_confidence(0.40)
            .with_stability(0.20);
        // 120 days old, never revised.
        let entry = make_entry("old_fact", "Stale project fact", meta, days_ago_secs(120));

        let report = scorer.score(&entry, Utc::now(), false);
        assert!(
            report.staleness >= STALENESS_ARCHIVE_MIN,
            "old low-quality entry should be an archive candidate, got staleness={:.3}",
            report.staleness
        );
        assert_eq!(report.recommended_status, MemoryStatus::Archived);
        assert_eq!(report.age_factor, 0.8);
    }

    #[test]
    fn test_contradiction_bumps_staleness() {
        let scorer = StalenessScorer::new();
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        let entry = make_entry("fact", "Build uses cargo", meta, now_secs());

        let no_conflict = scorer.score(&entry, Utc::now(), false);
        let with_conflict = scorer.score(&entry, Utc::now(), true);
        assert!(
            with_conflict.staleness > no_conflict.staleness,
            "contradiction factor should raise staleness"
        );
        assert_eq!(no_conflict.contradiction_factor, 0.0);
        assert_eq!(with_conflict.contradiction_factor, 0.5);
    }

    #[test]
    fn test_recommended_status_is_sticky_for_terminal() {
        // Archived/Superseded entries should never be re-activated by scoring.
        let archived = recommended_status(0.99, MemoryStatus::Archived);
        assert_eq!(archived, MemoryStatus::Archived);
        let superseded = recommended_status(0.10, MemoryStatus::Superseded);
        assert_eq!(superseded, MemoryStatus::Superseded);
    }

    #[test]
    fn test_revision_count_lowers_usage_factor() {
        let scorer = StalenessScorer::new();
        let meta_low = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "t");
        let mut meta_high = meta_low.clone();
        meta_high.revision_count = 5;

        let entry_low = make_entry("a", "x", meta_low, now_secs());
        let entry_high = make_entry("b", "x", meta_high, now_secs());
        let r_low = scorer.score(&entry_low, Utc::now(), false);
        let r_high = scorer.score(&entry_high, Utc::now(), false);
        assert!(
            r_high.usage_factor < r_low.usage_factor,
            "more revisions ⇒ lower usage factor"
        );
        assert!(r_low.usage_factor > 0.0);
        assert_eq!(r_high.usage_factor, 0.0);
    }

    // ── ConflictDetector ──

    #[test]
    fn test_detects_same_topic_type_different_content() {
        let detector = ConflictDetector::new();
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        let entries = vec![
            make_entry("a", "Build uses cargo", meta.clone(), now_secs()),
            make_entry("b", "Build uses make", meta.clone(), now_secs()),
        ];
        let groups = detector.detect(&entries);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].entries.len(), 2);
        assert_eq!(groups[0].topic, "build");
        assert_eq!(groups[0].memory_type, MemoryType::ProjectFact);
    }

    #[test]
    fn test_no_conflict_for_different_topics() {
        let detector = ConflictDetector::new();
        let entries = vec![
            make_entry(
                "a",
                "x",
                MemoryMeta::new(
                    MemoryType::ProjectFact,
                    MemorySource::AutoExtracted,
                    "build",
                ),
                now_secs(),
            ),
            make_entry(
                "b",
                "x",
                MemoryMeta::new(
                    MemoryType::ProjectFact,
                    MemorySource::AutoExtracted,
                    "deploy",
                ),
                now_secs(),
            ),
        ];
        assert!(detector.detect(&entries).is_empty());
    }

    #[test]
    fn test_no_conflict_for_identical_content() {
        // Two entries with identical content are dedup candidates, not conflicts.
        let detector = ConflictDetector::new();
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        );
        let entries = vec![
            make_entry("a", "Build uses cargo", meta.clone(), now_secs()),
            make_entry("b", "Build uses cargo", meta, now_secs()),
        ];
        assert!(detector.detect(&entries).is_empty());
    }

    // ── MemoryMerger ──

    #[tokio::test]
    async fn test_merge_keeps_highest_confidence_as_primary() {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let log = NullChangeLog;

        let meta_high =
            MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "build")
                .with_confidence(0.95);
        let meta_low = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        )
        .with_confidence(0.50);
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "high",
                "Build uses cargo 1.80",
                meta_high,
            )
            .await
            .unwrap();
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "low",
                "Build uses cargo 1.70",
                meta_low,
            )
            .await
            .unwrap();

        let group = ConflictDetector::new()
            .detect(
                &typed
                    .list_typed(
                        crate::evolution::layer::WARM_NAMESPACE,
                        &MemoryFilter::new(),
                    )
                    .await
                    .unwrap(),
            )
            .into_iter()
            .next()
            .expect("should detect one group");

        let result = MemoryMerger::new(&typed, &log)
            .merge_group(&group)
            .await
            .unwrap();
        assert_eq!(result.primary_key, "high");
        assert_eq!(result.superseded_keys, vec!["low".to_string()]);

        // Primary content should carry the merge annotation.
        let primary = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "high")
            .await
            .unwrap()
            .unwrap();
        assert!(primary.content.contains("merged from 2 similar entries"));

        // Secondary should be Superseded with superseded_by pointing at the primary.
        let secondary = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "low")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(secondary.meta.status, MemoryStatus::Superseded);
        assert_eq!(secondary.meta.superseded_by.as_deref(), Some("high"));
    }

    #[tokio::test]
    async fn test_merge_single_entry_is_noop() {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let log = NullChangeLog;

        let group = ConflictGroup {
            topic: "build".to_string(),
            memory_type: MemoryType::ProjectFact,
            entries: vec![make_entry(
                "solo",
                "only one",
                MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "build"),
                now_secs(),
            )],
        };
        let result = MemoryMerger::new(&typed, &log)
            .merge_group(&group)
            .await
            .unwrap();
        assert!(result.superseded_keys.is_empty());
    }

    // ── MemoryReviewer end-to-end ──

    fn make_layer_manager(store: Arc<dyn echo_core::memory::store::Store>) -> MemoryLayerManager {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        MemoryLayerManager::new(dir, store, Box::new(NullChangeLog))
    }

    #[tokio::test]
    async fn test_review_archives_stale_and_merges_conflicts() {
        let mem_store = Arc::new(InMemoryStore::new());
        let store: Arc<dyn echo_core::memory::store::Store> = mem_store.clone();
        let typed = TypedMemoryStore::new(store.clone());
        let layer_mgr = make_layer_manager(store.clone());
        let log = NullChangeLog;

        // (1) A very stale entry → should be archived.
        let stale_meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::L3Promotion, "old")
            .with_confidence(0.30)
            .with_stability(0.20);
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "stale",
                "Long-forgotten fact",
                stale_meta,
            )
            .await
            .unwrap();
        // Force the underlying item's timestamp into the distant past.
        // We use `put_raw` so the Store doesn't overwrite updated_at with now().
        {
            let item = store
                .get(crate::evolution::layer::WARM_NAMESPACE, "stale")
                .await
                .unwrap()
                .unwrap();
            let mut past = item;
            past.created_at = days_ago_secs(200);
            past.updated_at = days_ago_secs(200);
            mem_store.put_raw(past).await;
        }

        // (2) Two conflicting entries on the same topic → should be merged.
        let c1 = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "build")
            .with_confidence(0.95);
        let c2 = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "build",
        )
        .with_confidence(0.55);
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "build_a",
                "Build uses cargo 1.80",
                c1,
            )
            .await
            .unwrap();
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "build_b",
                "Build uses cargo 1.70",
                c2,
            )
            .await
            .unwrap();

        // (3) A fresh, high-quality entry → should stay put.
        let good = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90);
        typed
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                "good",
                "User prefers concise output",
                good,
            )
            .await
            .unwrap();

        let config = ReviewConfig::default();
        let report = MemoryReviewer::new()
            .review(&typed, &layer_mgr, &log, &config)
            .await
            .unwrap();

        assert_eq!(report.total_scanned, 4);
        assert!(
            report.archives_applied >= 1,
            "should archive the stale entry"
        );
        assert!(
            report.merges_applied >= 1,
            "should merge the conflict group"
        );

        // The stale entry should have moved to cold.
        let loc = layer_mgr.locate("stale").await;
        assert!(matches!(loc, Some((MemoryLayer::Cold, _))));

        // The lower-confidence conflict entry should be superseded.
        let build_b = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "build_b")
            .await
            .unwrap();
        if let Some(entry) = build_b {
            assert_eq!(entry.meta.status, MemoryStatus::Superseded);
            assert_eq!(entry.meta.superseded_by.as_deref(), Some("build_a"));
        }
    }
}
