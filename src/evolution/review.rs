//! Memory review analysis and explicit conflict resolution.
//!
//! Phase 2 of the evolution system. Phase 1 gave memories typed metadata, layered
//! storage (hot/warm/cold), and an audit log, but memories were never re-evaluated
//! after creation. This module adds:
//!
//! - [`StalenessScorer`] — full staleness score from age, usage, instability,
//!   contradiction, and source-weakness factors (replaces `MemoryMeta::base_staleness`).
//! - [`ConflictDetector`] — groups memories sharing the same topic + type with
//!   different content hashes.
//! - [`MemoryMerger`] — explicit mutation primitive that merges a user-approved
//!   conflict group into one primary entry and supersedes the rest.
//! - [`MemoryReviewer`] — analysis-only orchestrator: scan → score → propose.
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
//! The reviewer never mutates memories. Deterministic scheduled maintenance is
//! owned by [`super::dreaming::Dreaming`]; semantic conflict resolution requires
//! an explicit caller to invoke [`MemoryMerger`].

use chrono::{DateTime, Duration, Utc};
use echo_core::memory::types::{MemoryMeta, MemoryStatus, MemoryType};
use echo_core::utils::hash::fnv1a_64;
use echo_state::memory::typed_store::{MemoryFilter, TypedMemoryEntry, TypedMemoryStore};
use std::collections::HashMap;

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
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
    /// this entry in a conflict group. The reviewer detects conflicts before its
    /// single scoring pass.
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
    /// Time since the latest recall, falling back to creation time.
    pub age_factor: f32,
    /// Low-recall-count factor.
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

/// Compute the age factor from the entry's most recent recall activity.
///
/// `<7d → 0.0`, `7–30d → 0.2`, `30–90d → 0.5`, `>90d → 0.8`.
fn age_factor(entry: &TypedMemoryEntry, now: DateTime<Utc>) -> f32 {
    // Metadata-only recall updates also change StoreItem::updated_at, so using
    // that field would make telemetry writes look like semantic edits. Keep a
    // dedicated recall timestamp and otherwise fall back to creation time.
    let secs = entry.meta.last_recalled_at.unwrap_or(entry.raw.created_at);
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

/// Low-usage factor based on actual recall telemetry.
///
/// `recall_count` is incremented by the shared recall path and is the same
/// signal consumed by Dreaming. Revision count measures edits, not usefulness.
fn low_usage_factor(meta: &MemoryMeta) -> f32 {
    1.0 - (meta.recall_count as f32 / 3.0).min(1.0)
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
                .filter_map(|(_, idx)| entries.get(idx).cloned())
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

/// One member of a semantic memory-conflict proposal.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryConflictMember {
    pub key: String,
    pub content: String,
    pub confidence: f32,
    pub status: MemoryStatus,
    pub recall_count: u32,
    pub updated_at: u64,
}

/// Analysis-only proposal describing a conflict that needs an explicit choice.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryConflictProposal {
    pub topic: String,
    pub memory_type: MemoryType,
    pub recommended_primary_key: String,
    pub members: Vec<MemoryConflictMember>,
}

/// Exact pre-merge state used to undo an explicitly approved merge.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryMergeSnapshot {
    pub key: String,
    pub content: String,
    pub meta: MemoryMeta,
}

/// Result of applying one conflict proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppliedMemoryMerge {
    pub primary_key: String,
    pub superseded_keys: Vec<String>,
    pub before: Vec<MemoryMergeSnapshot>,
}

impl MemoryConflictProposal {
    pub(crate) fn from_group(group: &ConflictGroup) -> Option<Self> {
        let ordered = ordered_conflict_entries(group);
        let recommended_primary_key = ordered.first()?.key.clone();
        let members = ordered
            .into_iter()
            .map(|entry| MemoryConflictMember {
                key: entry.key,
                content: entry.content,
                confidence: entry.meta.confidence,
                status: entry.meta.status,
                recall_count: entry.meta.recall_count,
                updated_at: entry.raw.updated_at,
            })
            .collect();
        Some(Self {
            topic: group.topic.clone(),
            memory_type: group.memory_type,
            recommended_primary_key,
            members,
        })
    }
}

fn ordered_conflict_entries(group: &ConflictGroup) -> Vec<TypedMemoryEntry> {
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
    ordered
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
/// - The primary's content stays verbatim; provenance is stored in audit data.
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
        let ordered = ordered_conflict_entries(group);

        let primary = match ordered.first() {
            Some(p) => p.clone(),
            None => {
                return Ok(MergeResult {
                    primary_key: String::new(),
                    superseded_keys: Vec::new(),
                });
            }
        };
        let secondaries = ordered.get(1..).unwrap_or_default();

        let combined_revision_count = group
            .entries
            .iter()
            .map(|e| e.meta.revision_count)
            .sum::<u32>();

        // Preserve the selected fact verbatim. Merge provenance belongs in
        // metadata/audit records, not in model-visible memory content.
        let primary_meta = MemoryMeta {
            revision_count: combined_revision_count.max(primary.meta.revision_count),
            ..primary.meta.clone()
        };
        self.typed_store
            .put_typed(
                crate::evolution::layer::WARM_NAMESPACE,
                &primary.key,
                &primary.content,
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
        builder = builder.trigger("explicit_memory_merge".to_string());
        builder = builder.after(serde_json::json!({
            "superseded_by": superseded_by,
            "group_size": group_size,
        }));
        let entry = builder.build(self.change_log);
        self.change_log.record(entry)
    }
}

// ── MemoryReviewer (orchestrator) ──────────────────────────────────────

/// Orchestrates an analysis-only review pass: scan → score → propose.
pub struct MemoryReviewer {
    scorer: StalenessScorer,
    conflict_detector: ConflictDetector,
}

/// Tunable knobs for a review pass.
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    /// Run a review when the session ends. Default: `false`.
    pub review_on_session_end: bool,
    /// Cap on conflict proposals returned per pass. Default: `10`.
    pub max_conflicts_per_review: usize,
    /// Maximum members allowed in one proposal. Larger groups are reported but
    /// omitted from actionable output to bound JSONL and prompt growth.
    pub max_conflict_members: usize,
    /// Run skill candidate detection during review. Default: `true`.
    pub detect_skill_candidates: bool,
    /// Auto-generate draft SKILL.md for new candidates. Default: `false`.
    pub auto_generate_drafts: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            review_on_session_end: false,
            max_conflicts_per_review: 10,
            max_conflict_members: 16,
            detect_skill_candidates: true,
            auto_generate_drafts: false,
        }
    }
}

/// A single proposal produced during a review pass.
#[derive(Debug, Clone)]
pub enum ReviewChange {
    /// A memory crossed the staleness review threshold.
    StalenessSuggested {
        key: String,
        recommended_status: MemoryStatus,
        staleness: f32,
    },
    /// A semantic conflict needs an explicit primary selection.
    ConflictProposed {
        topic: String,
        recommended_primary_key: String,
        member_keys: Vec<String>,
    },
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
    /// Entries whose staleness crossed the flag threshold (≥ 0.35).
    pub stale_count: usize,
    /// Conflict groups found.
    pub conflict_groups: usize,
    /// Explainable staleness suggestions; no status is changed by the reviewer.
    pub staleness_suggestions: Vec<StalenessReport>,
    /// Semantic conflict proposals capped by `max_conflicts_per_review`.
    pub conflict_proposals: Vec<MemoryConflictProposal>,
    /// Skill candidates proposed during this review.
    pub candidates_proposed: usize,
    /// Draft SKILL.md files generated during this review.
    pub drafts_generated: usize,
    /// Individual proposals, in deterministic order.
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

    /// Analyze the warm layer without mutating it.
    ///
    /// Deterministic lifecycle maintenance belongs to `Dreaming`; semantic
    /// conflict resolution belongs to an explicit `MemoryMerger` caller.
    pub async fn review(
        &self,
        typed_store: &TypedMemoryStore,
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
        let reviewable_entries: Vec<_> = entries
            .iter()
            .filter(|entry| entry.meta.status != MemoryStatus::Superseded)
            .cloned()
            .collect();
        let conflict_groups = self.conflict_detector.detect(&reviewable_entries);
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
        report.staleness_suggestions = scored
            .into_iter()
            .filter(|entry| entry.staleness >= STALENESS_ACTIVE_MAX)
            .collect();
        report.stale_count = report.staleness_suggestions.len();
        for suggestion in &report.staleness_suggestions {
            report.changes.push(ReviewChange::StalenessSuggested {
                key: suggestion.key.clone(),
                recommended_status: suggestion.recommended_status,
                staleness: suggestion.staleness,
            });
        }

        report.conflict_proposals = conflict_groups
            .iter()
            .filter(|group| group.entries.len() <= config.max_conflict_members)
            .take(config.max_conflicts_per_review)
            .filter_map(MemoryConflictProposal::from_group)
            .collect();
        for proposal in &report.conflict_proposals {
            report.changes.push(ReviewChange::ConflictProposed {
                topic: proposal.topic.clone(),
                recommended_primary_key: proposal.recommended_primary_key.clone(),
                member_keys: proposal
                    .members
                    .iter()
                    .map(|member| member.key.clone())
                    .collect(),
            });
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
    fn test_recall_count_lowers_usage_factor() {
        let scorer = StalenessScorer::new();
        let meta_low = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "t");
        let mut meta_high = meta_low.clone();
        meta_high.recall_count = 5;

        let entry_low = make_entry("a", "x", meta_low, now_secs());
        let entry_high = make_entry("b", "x", meta_high, now_secs());
        let r_low = scorer.score(&entry_low, Utc::now(), false);
        let r_high = scorer.score(&entry_high, Utc::now(), false);
        assert!(
            r_high.usage_factor < r_low.usage_factor,
            "more recalls ⇒ lower usage factor"
        );
        assert!(r_low.usage_factor > 0.0);
        assert_eq!(r_high.usage_factor, 0.0);
    }

    #[test]
    fn review_defaults_are_manual_and_proposal_only() {
        let config = ReviewConfig::default();
        assert!(!config.review_on_session_end);
        assert_eq!(config.max_conflicts_per_review, 10);
        assert_eq!(config.max_conflict_members, 16);
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

        // Primary content stays verbatim; provenance belongs in audit metadata.
        let primary = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "high")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(primary.content, "Build uses cargo 1.80");

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

    #[tokio::test]
    async fn test_review_reports_stale_and_conflicts_without_mutation() {
        let mem_store = Arc::new(InMemoryStore::new());
        let store: Arc<dyn echo_core::memory::store::Store> = mem_store.clone();
        let typed = TypedMemoryStore::new(store.clone());

        // (1) A very stale entry → should be reported as an archive candidate.
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

        // (2) Two conflicting entries on the same topic → should produce a proposal.
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
        let report = MemoryReviewer::new().review(&typed, &config).await.unwrap();

        assert_eq!(report.total_scanned, 4);
        assert!(
            report
                .staleness_suggestions
                .iter()
                .any(|s| s.key == "stale")
        );
        assert_eq!(report.conflict_proposals.len(), 1);
        assert_eq!(
            report.conflict_proposals[0].recommended_primary_key,
            "build_a"
        );

        // Analysis must not archive or merge anything.
        let stale = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "stale")
            .await
            .unwrap()
            .expect("stale entry remains present");
        assert_eq!(stale.meta.status, MemoryStatus::Active);
        let build_b = typed
            .get_typed(crate::evolution::layer::WARM_NAMESPACE, "build_b")
            .await
            .unwrap()
            .expect("conflict member remains present");
        assert_eq!(build_b.meta.status, MemoryStatus::Active);
        assert_eq!(build_b.content, "Build uses cargo 1.70");
    }

    #[tokio::test]
    async fn review_omits_oversized_conflict_proposals() -> crate::error::Result<()> {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let meta = MemoryMeta::new(
            MemoryType::ProjectFact,
            MemorySource::AutoExtracted,
            "oversized",
        );
        for index in 0..17 {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &format!("member_{index}"),
                    &format!("value_{index}"),
                    meta.clone(),
                )
                .await?;
        }
        let report = MemoryReviewer::new()
            .review(&typed, &ReviewConfig::default())
            .await?;

        assert_eq!(report.conflict_groups, 1);
        assert!(report.conflict_proposals.is_empty());
        Ok(())
    }
}
