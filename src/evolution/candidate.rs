//! Skill candidate detection — discovers reusable patterns in typed memory.
//!
//! Scans `TypedMemoryStore` for `WorkflowPattern` and `DebuggingLesson` entries
//! and proposes [`SkillCandidate`]s when enough observations accumulate. This is
//! the first step in the skill creation pipeline:
//!
//! ```text
//! observations → SkillCandidateDetector → SkillCandidate → SkillDraftGenerator → SKILL.md
//! ```
//!
//! # Detection logic
//!
//! - Group `WorkflowPattern` / `DebuggingLesson` memories by `(topic, memory_type)`.
//! - When a group has ≥ `min_observations` entries (default 3), propose a candidate.
//! - Existing candidates in `["agent", "skill_candidates"]` are checked to avoid
//!   duplicates; reinforced candidates get their sample count updated.
//! - Each new candidate is registered with the `Curator` lifecycle system.

use chrono::{DateTime, Utc};
use echo_core::memory::types::{MemorySource, MemoryType};
use echo_state::memory::typed_store::{MemoryFilter, TypedMemoryEntry, TypedMemoryStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
use super::curator::{Curator, CuratorConfig};
use crate::error::Result;

// ── Constants ──────────────────────────────────────────────────────────

/// Namespace for skill candidate proposals in the Store.
pub const CANDIDATE_NAMESPACE: &[&str] = &["agent", "skill_candidates"];

// ── SkillCandidate ─────────────────────────────────────────────────────

/// A proposed skill candidate, derived from repeated memory observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCandidate {
    /// Auto-generated skill name (sanitized topic).
    pub name: String,
    /// Human-readable description of the pattern.
    pub description: String,
    /// Keywords that should trigger this skill.
    pub trigger_patterns: Vec<String>,
    /// Tools commonly used in this pattern.
    pub tool_sequence: Vec<String>,
    /// Number of memory observations that led to this candidate.
    pub sample_count: usize,
    /// Confidence derived from observation quality (0.0–1.0).
    pub confidence: f32,
    /// The shared topic of the source memories.
    pub topic: String,
    /// The memory type of the source observations.
    pub source_type: MemoryType,
    /// When this candidate was created.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
}

impl SkillCandidate {
    /// Build a candidate from a group of memory entries sharing the same topic.
    fn from_group(topic: &str, source_type: MemoryType, entries: &[TypedMemoryEntry]) -> Self {
        let name = sanitize_name(topic);
        let sample_count = entries.len();

        // Confidence: average of all entry confidences, with a bonus for more observations.
        let avg_confidence =
            entries.iter().map(|e| e.meta.confidence).sum::<f32>() / sample_count.max(1) as f32;
        let observation_bonus = (sample_count as f32 / 10.0).min(0.15);
        let confidence = (avg_confidence + observation_bonus).min(1.0);

        // Description: synthesize from the first entry's content.
        let description = format!(
            "Auto-detected {} pattern for '{}'. Based on {} observations.",
            match source_type {
                MemoryType::WorkflowPattern => "workflow",
                MemoryType::DebuggingLesson => "debugging",
                _ => "usage",
            },
            topic,
            sample_count
        );

        // Trigger patterns: topic words + content keywords.
        let mut trigger_patterns = vec![topic.to_string()];
        // Extract short keywords from content (first 3 unique words > 3 chars).
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            for word in entry.content.split_whitespace() {
                let w = word.to_lowercase();
                if w.len() > 3 && seen.insert(w.clone()) && trigger_patterns.len() < 5 {
                    trigger_patterns.push(w);
                }
            }
        }

        // Tool sequence: extract tool names from content patterns like "tool 'X'".
        let tool_sequence = extract_tool_names(entries);

        Self {
            name,
            description,
            trigger_patterns,
            tool_sequence,
            sample_count,
            confidence,
            topic: topic.to_string(),
            source_type,
            created_at: Utc::now(),
        }
    }
}

// ── CandidateReport ────────────────────────────────────────────────────

/// Result of a candidate detection pass.
#[derive(Debug, Clone, Default)]
pub struct CandidateReport {
    /// New candidates proposed in this pass.
    pub new_candidates: Vec<SkillCandidate>,
    /// Existing candidates that were reinforced (more observations).
    pub reinforced: Vec<String>,
    /// Total groups examined.
    pub groups_scanned: usize,
}

// ── SkillCandidateDetector ─────────────────────────────────────────────

/// Detects reusable skill candidates from accumulated memory observations.
///
/// Non-LLM, pure pattern matching — fast and free. The detected candidates
/// are stored in `["agent", "skill_candidates"]` and registered with the
/// [`Curator`] lifecycle system.
pub struct SkillCandidateDetector {
    /// Minimum observations to propose a candidate. Default: 3.
    pub min_observations: usize,
    /// Maximum candidates to propose per scan. Default: 5.
    pub max_candidates_per_scan: usize,
}

impl Default for SkillCandidateDetector {
    fn default() -> Self {
        Self {
            min_observations: 3,
            max_candidates_per_scan: 5,
        }
    }
}

impl SkillCandidateDetector {
    /// Create a new detector with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a detector with custom thresholds.
    pub fn with_thresholds(min_observations: usize, max_candidates_per_scan: usize) -> Self {
        Self {
            min_observations,
            max_candidates_per_scan,
        }
    }

    /// Run a detection pass against the typed memory store.
    ///
    /// Scans for `WorkflowPattern` and `DebuggingLesson` entries, groups by
    /// `(topic, memory_type)`, and proposes candidates for groups exceeding
    /// the observation threshold.
    ///
    /// New candidates are persisted to the Store and registered with the
    /// `Curator`. Existing candidates are updated if their observation count
    /// has increased.
    pub async fn detect(
        &self,
        typed_store: &TypedMemoryStore,
        change_log: &dyn ChangeLog,
    ) -> Result<CandidateReport> {
        let mut report = CandidateReport::default();

        // 1. Query for WorkflowPattern entries.
        let wf_filter = MemoryFilter::new()
            .with_type(MemoryType::WorkflowPattern)
            .with_source(MemorySource::RepeatedWorkflow);
        let workflow_entries = typed_store
            .list_typed(crate::evolution::layer::WARM_NAMESPACE, &wf_filter)
            .await?;

        // 2. Query for DebuggingLesson entries (any source).
        let dl_filter = MemoryFilter::new().with_type(MemoryType::DebuggingLesson);
        let debugging_entries = typed_store
            .list_typed(crate::evolution::layer::WARM_NAMESPACE, &dl_filter)
            .await?;

        // 3. Group by (topic, memory_type).
        let mut groups: HashMap<(String, MemoryType), Vec<TypedMemoryEntry>> = HashMap::new();
        for entry in &workflow_entries {
            groups
                .entry((entry.meta.topic.clone(), MemoryType::WorkflowPattern))
                .or_default()
                .push(entry.clone());
        }
        for entry in &debugging_entries {
            groups
                .entry((entry.meta.topic.clone(), MemoryType::DebuggingLesson))
                .or_default()
                .push(entry.clone());
        }
        report.groups_scanned = groups.len();

        // 4. Load existing candidates to check for duplicates.
        let existing_filter = MemoryFilter::new();
        let existing_candidates = typed_store
            .list_typed(CANDIDATE_NAMESPACE, &existing_filter)
            .await
            .unwrap_or_default();
        let existing_names: std::collections::HashSet<String> =
            existing_candidates.iter().map(|e| e.key.clone()).collect();

        // 5. For each group above threshold, propose or reinforce.
        let mut proposed = 0usize;
        for ((topic, source_type), entries) in &groups {
            if entries.len() < self.min_observations {
                continue;
            }
            if proposed >= self.max_candidates_per_scan {
                break;
            }

            let candidate = SkillCandidate::from_group(topic, *source_type, entries);
            let key = candidate.name.clone();

            if existing_names.contains(&key) {
                // Reinforce existing candidate — update sample count.
                if let Some(existing) = typed_store
                    .get_typed(CANDIDATE_NAMESPACE, &key)
                    .await
                    .ok()
                    .flatten()
                {
                    let old_count = candidate_sample_count(&existing);
                    if candidate.sample_count > old_count {
                        // Update with new sample count and confidence.
                        let updated = SkillCandidate {
                            created_at: parse_candidate_created_at(&existing)
                                .unwrap_or(candidate.created_at),
                            ..candidate.clone()
                        };
                        let value =
                            serde_json::to_value(&updated).unwrap_or(serde_json::Value::Null);
                        let content = serde_json::to_string(&value).unwrap_or_default();
                        typed_store
                            .put_typed(CANDIDATE_NAMESPACE, &key, &content, existing.meta.clone())
                            .await?;
                        report.reinforced.push(key);
                    }
                }
            } else {
                // New candidate — persist and register.
                let value = serde_json::to_value(&candidate).unwrap_or(serde_json::Value::Null);
                let content = serde_json::to_string(&value).unwrap_or_default();
                typed_store
                    .put_typed(
                        CANDIDATE_NAMESPACE,
                        &key,
                        &content,
                        default_candidate_meta(),
                    )
                    .await?;

                // Register with the evolution-owned Curator lifecycle.
                let curator = Curator::default_path(CuratorConfig::default());
                if let Err(e) = curator.register_candidate(&key) {
                    tracing::warn!("Failed to register candidate '{}': {}", key, e);
                }

                // Record in audit log.
                let entry = ChangeEntryBuilder::new(EntityType::Skill, &key, ChangeType::Create)
                    .reason(format!(
                        "skill candidate proposed from {} observations on topic '{}'",
                        candidate.sample_count, candidate.topic
                    ))
                    .trigger("skill_candidate_detector".to_string())
                    .build(change_log);
                change_log.record(entry)?;

                report.new_candidates.push(candidate);
                proposed += 1;
            }
        }

        Ok(report)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Sanitize a topic string into a valid skill name.
///
/// Lowercase, replace non-alphanumeric chars with `-`, collapse runs of `-`,
/// strip leading/trailing `-`, truncate to 64 chars.
fn sanitize_name(topic: &str) -> String {
    let mut name: String = topic
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse runs of `-`.
    let mut prev_dash = false;
    name = name
        .chars()
        .filter(|&c| {
            if c == '-' {
                if prev_dash {
                    false
                } else {
                    prev_dash = true;
                    true
                }
            } else {
                prev_dash = false;
                true
            }
        })
        .collect();
    // Strip leading/trailing `-`.
    let name = name.trim_matches('-');
    // Fallback: if sanitization produced an empty string, use a stable hash-based name.
    if name.is_empty() {
        let hash = echo_core::utils::hash::fnv1a_64(topic.as_bytes());
        return format!("candidate-{:x}", hash);
    }
    // Truncate.
    name.chars().take(64).collect()
}

/// Extract tool names from memory content.
///
/// Looks for patterns like `tool 'name'` (with space-quote), `tool:name`,
/// `Tool:name`, or `tool "name"` in the content.
fn extract_tool_names(entries: &[TypedMemoryEntry]) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let content = &entry.content;
        let lower = content.to_lowercase();
        // Scan for "tool" mentions and extract the following name.
        for (idx, _) in lower.match_indices("tool") {
            let after = lower.get(idx.saturating_add(4)..).unwrap_or_default();
            let name = extract_tool_name_after_keyword(after);
            if !name.is_empty() && seen.insert(name.to_string()) && tools.len() < 8 {
                tools.push(name.to_string());
            }
        }
    }
    tools
}

/// Given the text immediately after "tool", extract the tool name.
///
/// Handles: `:cargo`, `: 'cargo'`, ` 'cargo'`, ` "cargo"`, `: cargo`, etc.
fn extract_tool_name_after_keyword(after: &str) -> String {
    let s = after.trim_start();
    // Strip leading colon and optional whitespace: "tool: cargo" or "tool:cargo"
    let s = s.strip_prefix(':').unwrap_or(s).trim_start();
    // Strip surrounding quotes: "tool 'cargo'" or "tool \"cargo\""
    if let Some(rest) = s.strip_prefix('\'')
        && let Some(end) = rest.find('\'')
    {
        return rest.get(..end).unwrap_or_default().to_string();
    }
    if let Some(rest) = s.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return rest.get(..end).unwrap_or_default().to_string();
    }
    // Unquoted: take the first word (alphanumeric + dashes/underscores).
    s.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Extract the sample_count from an existing candidate entry.
///
/// The content is stored as a JSON-serialized string of the candidate object,
/// so we use `from_str` to deserialize it directly.
fn candidate_sample_count(entry: &TypedMemoryEntry) -> usize {
    serde_json::from_str::<SkillCandidate>(&entry.content)
        .map(|c| c.sample_count)
        .unwrap_or(0)
}

/// Extract the created_at timestamp from an existing candidate entry.
fn parse_candidate_created_at(entry: &TypedMemoryEntry) -> Option<DateTime<Utc>> {
    serde_json::from_str::<SkillCandidate>(&entry.content)
        .ok()
        .map(|c| c.created_at)
}

/// Create a default MemoryMeta for a skill candidate.
fn default_candidate_meta() -> echo_core::memory::types::MemoryMeta {
    echo_core::memory::types::MemoryMeta::new(
        MemoryType::SkillCandidate,
        MemorySource::AutoExtracted,
        "skill_candidate",
    )
    .with_confidence(0.75)
    .with_stability(0.60)
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

    fn make_entry(
        key: &str,
        content: &str,
        meta: echo_core::memory::types::MemoryMeta,
    ) -> TypedMemoryEntry {
        let raw = StoreItem::new(
            vec!["agent".to_string(), "typed_memories".to_string()],
            key.to_string(),
            serde_json::Value::Null,
        );
        TypedMemoryEntry {
            key: key.to_string(),
            content: content.to_string(),
            meta,
            raw,
        }
    }

    fn wf_meta(topic: &str) -> echo_core::memory::types::MemoryMeta {
        echo_core::memory::types::MemoryMeta::new(
            MemoryType::WorkflowPattern,
            MemorySource::RepeatedWorkflow,
            topic,
        )
        .with_confidence(0.75)
    }

    fn dl_meta(topic: &str) -> echo_core::memory::types::MemoryMeta {
        echo_core::memory::types::MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            topic,
        )
        .with_confidence(0.80)
    }

    #[tokio::test]
    async fn test_candidate_from_repeated_workflow() {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let log = NullChangeLog;

        // Insert 3 WorkflowPattern entries with same topic.
        for i in 0..3 {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &format!("wf_{}", i),
                    &format!(
                        "Repeated workflow pattern: tool 'cargo' used {} times across sessions",
                        i + 1
                    ),
                    wf_meta("cargo-build"),
                )
                .await
                .unwrap();
        }

        let detector = SkillCandidateDetector::new();
        let report = detector.detect(&typed, &log).await.unwrap();

        assert_eq!(report.new_candidates.len(), 1);
        assert_eq!(report.new_candidates[0].topic, "cargo-build");
        assert!(report.new_candidates[0].sample_count >= 3);
        assert_eq!(
            report.new_candidates[0].source_type,
            MemoryType::WorkflowPattern
        );
    }

    #[tokio::test]
    async fn test_no_candidate_below_threshold() {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let log = NullChangeLog;

        // Insert only 2 entries — below threshold.
        for i in 0..2 {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &format!("wf_{}", i),
                    "Some workflow",
                    wf_meta("build"),
                )
                .await
                .unwrap();
        }

        let detector = SkillCandidateDetector::new();
        let report = detector.detect(&typed, &log).await.unwrap();

        assert!(report.new_candidates.is_empty());
    }

    #[tokio::test]
    async fn test_no_duplicate_candidates() {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let log = NullChangeLog;

        // Insert 3 entries.
        for i in 0..3 {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &format!("wf_{}", i),
                    "Build pattern",
                    wf_meta("build"),
                )
                .await
                .unwrap();
        }

        let detector = SkillCandidateDetector::new();
        // First detection: should create candidate.
        let report1 = detector.detect(&typed, &log).await.unwrap();
        assert_eq!(report1.new_candidates.len(), 1);

        // Second detection: should NOT create duplicate.
        let report2 = detector.detect(&typed, &log).await.unwrap();
        assert!(report2.new_candidates.is_empty());
    }

    #[test]
    fn test_name_sanitization() {
        assert_eq!(sanitize_name("Cargo Build"), "cargo-build");
        assert_eq!(sanitize_name("test/deploy:ci"), "test-deploy-ci");
        assert_eq!(sanitize_name("  leading  spaces  "), "leading-spaces");
        assert_eq!(sanitize_name("---dashes---"), "dashes");
        assert_eq!(sanitize_name("a/b/c/d/e"), "a-b-c-d-e");
    }

    #[tokio::test]
    async fn test_candidate_with_debugging_lesson() {
        let store = Arc::new(InMemoryStore::new());
        let typed = TypedMemoryStore::new(store);
        let log = NullChangeLog;

        // Insert 3 DebuggingLesson entries.
        for i in 0..3 {
            typed
                .put_typed(
                    crate::evolution::layer::WARM_NAMESPACE,
                    &format!("dl_{}", i),
                    &format!(
                        "Lesson: always run cargo check before cargo build (attempt {})",
                        i + 1
                    ),
                    dl_meta("cargo-check"),
                )
                .await
                .unwrap();
        }

        let detector = SkillCandidateDetector::new();
        let report = detector.detect(&typed, &log).await.unwrap();

        assert_eq!(report.new_candidates.len(), 1);
        assert_eq!(
            report.new_candidates[0].source_type,
            MemoryType::DebuggingLesson
        );
        assert_eq!(report.new_candidates[0].topic, "cargo-check");
    }

    #[test]
    fn test_extract_tool_names() {
        let entries = vec![
            make_entry("a", "Used tool:cargo to build project", wf_meta("t")),
            make_entry("b", "Used tool:rustfmt for formatting", wf_meta("t")),
        ];
        let tools = extract_tool_names(&entries);
        assert!(tools.contains(&"cargo".to_string()));
    }
}
