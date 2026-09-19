//! Skill similarity detection and merging.
//!
//! Detects overlapping skills based on triggers, paths, tools, and descriptions,
//! then proposes or executes merges to reduce redundancy.

use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;

use echo_core::memory::store::Store;
use echo_state::skill_telemetry::{SkillTelemetry, SkillTelemetryStore};
use serde::{Deserialize, Serialize};

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
use super::curator::SkillLifecycle;
use super::skill_mutation::{
    SkillApprovalArtifact, SkillFileMutation, SkillMutationAuthority, SkillMutationKind,
    SkillMutationOutcome, SkillMutationPreview, SkillMutationRequest,
};
use crate::error::Result;

// Re-export SkillDescriptor from echo-execution for use in this module.
// The actual type lives in echo_execution::skills::external::types.
pub use echo_execution::skills::external::SkillDescriptor;

/// Threshold above which a merge proposal is generated.
const MERGE_THRESHOLD: f64 = 0.75;

/// Namespace for storing merge proposals.
const MERGE_NAMESPACE: &[&str] = &["agent", "evolution", "merges"];

/// A proposal to merge two similar skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMergeProposal {
    /// Name of the first skill.
    pub skill_a: String,
    /// Name of the second skill.
    pub skill_b: String,
    /// Overall similarity score (0.0 to 1.0).
    pub similarity_score: f64,
    /// Breakdown of similarity by dimension.
    pub breakdown: SimilarityBreakdown,
    /// Which skill should be kept (the one with more telemetry data).
    pub primary_skill: String,
    /// Which skill should be deprecated.
    pub deprecated_skill: String,
    /// When this proposal was created.
    #[serde(with = "crate::utils::time::local_rfc3339")]
    pub created_at: DateTime<Utc>,
}

/// Detailed breakdown of similarity across different dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarityBreakdown {
    /// Overlap in trigger keywords (Jaccard).
    pub trigger_overlap: f64,
    /// Overlap in file paths patterns (Jaccard).
    pub path_overlap: f64,
    /// Overlap in allowed tools (Jaccard).
    pub tool_overlap: f64,
    /// Text similarity in descriptions (word-level Jaccard).
    pub description_similarity: f64,
}

/// Detects similar skills and generates merge proposals.
pub struct SkillSimilarityDetector {
    store: Arc<dyn Store>,
    telemetry_store: SkillTelemetryStore,
}

impl SkillSimilarityDetector {
    /// Create a new detector.
    pub fn new(store: Arc<dyn Store>) -> Self {
        let telemetry_store = SkillTelemetryStore::new(store.clone());
        Self {
            store,
            telemetry_store,
        }
    }

    /// Analyze two skills and compute their similarity score.
    pub fn compute_similarity(
        desc_a: &SkillDescriptor,
        desc_b: &SkillDescriptor,
    ) -> SimilarityBreakdown {
        let trigger_overlap = jaccard_overlap(&desc_a.triggers, &desc_b.triggers);
        let path_overlap = jaccard_overlap(&desc_a.paths, &desc_b.paths);
        let tool_overlap = jaccard_overlap(&desc_a.allowed_tools, &desc_b.allowed_tools);
        let description_similarity = word_similarity(&desc_a.description, &desc_b.description);

        SimilarityBreakdown {
            trigger_overlap,
            path_overlap,
            tool_overlap,
            description_similarity,
        }
    }

    /// Compute overall similarity score from breakdown.
    /// Weights: triggers 0.4, paths 0.2, tools 0.2, description 0.2
    pub fn overall_score(breakdown: &SimilarityBreakdown) -> f64 {
        breakdown.trigger_overlap * 0.4
            + breakdown.path_overlap * 0.2
            + breakdown.tool_overlap * 0.2
            + breakdown.description_similarity * 0.2
    }

    /// Scan all registered skills and generate merge proposals for similar pairs.
    pub async fn scan_and_propose(
        &self,
        descriptors: &[SkillDescriptor],
        change_log: &dyn ChangeLog,
    ) -> Result<Vec<SkillMergeProposal>> {
        let mut proposals = Vec::new();
        let mut seen_pairs = HashSet::new();

        for (i, desc_a) in descriptors.iter().enumerate() {
            for desc_b in descriptors.iter().skip(i + 1) {
                // Normalize pair key to avoid duplicates (A-B vs B-A).
                let (first, second) = if desc_a.name < desc_b.name {
                    (&desc_a.name, &desc_b.name)
                } else {
                    (&desc_b.name, &desc_a.name)
                };
                let pair_key = format!("{}__{}", first, second);
                if seen_pairs.contains(&pair_key) {
                    continue;
                }
                seen_pairs.insert(pair_key);

                let breakdown = Self::compute_similarity(desc_a, desc_b);
                let score = Self::overall_score(&breakdown);

                if score >= MERGE_THRESHOLD {
                    let (primary, deprecated) = self.determine_primary(desc_a, desc_b).await;

                    let proposal = SkillMergeProposal {
                        skill_a: desc_a.name.clone(),
                        skill_b: desc_b.name.clone(),
                        similarity_score: score,
                        breakdown,
                        primary_skill: primary,
                        deprecated_skill: deprecated,
                        created_at: Utc::now(),
                    };

                    // Store the proposal.
                    let store_key = format!("{}__{}", proposal.skill_a, proposal.skill_b);
                    let value = serde_json::to_value(&proposal).map_err(|e| {
                        echo_core::error::ReactError::Other(format!(
                            "Failed to serialize merge proposal: {}",
                            e
                        ))
                    })?;
                    self.store.put(MERGE_NAMESPACE, &store_key, value).await?;

                    // Record in audit log.
                    let entry =
                        ChangeEntryBuilder::new(EntityType::Skill, &store_key, ChangeType::Create)
                            .reason(format!(
                                "Merge proposal: {} <-> {} (score: {:.2})",
                                proposal.skill_a, proposal.skill_b, score
                            ))
                            .trigger("skill_similarity_detector".to_string())
                            .build(change_log);
                    change_log.record(entry)?;

                    proposals.push(proposal);
                }
            }
        }

        Ok(proposals)
    }

    /// Determine which skill should be primary based on telemetry data.
    /// The one with more activations or more recent usage becomes primary.
    async fn determine_primary(
        &self,
        desc_a: &SkillDescriptor,
        desc_b: &SkillDescriptor,
    ) -> (String, String) {
        let telem_a = self
            .telemetry_store
            .get_telemetry(&desc_a.name)
            .await
            .ok()
            .flatten();
        let telem_b = self
            .telemetry_store
            .get_telemetry(&desc_b.name)
            .await
            .ok()
            .flatten();

        // Normalised scoring: 50% activation_count (≤1.0) + 50% recency
        // (last_used normalised to [0, 1] via approximate epoch→now window).
        // Previously `last_used / 1e12` ≈ 1.77 dominated activation_count ≤ 1.0.
        fn usage_score(t: &SkillTelemetry) -> f64 {
            let recency = (t.last_used as f64 / 1.7e12 - 1.0).clamp(0.0, 1.0);
            t.activation_count as f64 * 0.5 + recency * 0.5
        }
        let score_a = telem_a.as_ref().map(usage_score).unwrap_or(0.0);
        let score_b = telem_b.as_ref().map(usage_score).unwrap_or(0.0);

        if score_a >= score_b {
            (desc_a.name.clone(), desc_b.name.clone())
        } else {
            (desc_b.name.clone(), desc_a.name.clone())
        }
    }
}

/// Executes skill merges by updating descriptors and marking skills as deprecated.
pub struct SkillMerger {
    authority: Arc<SkillMutationAuthority>,
}

pub struct SkillMergePreview {
    pub request: SkillMutationRequest,
    pub preview: SkillMutationPreview,
    pub merged_descriptor: SkillDescriptor,
}

impl SkillMerger {
    /// Create a new merger.
    pub fn new(authority: Arc<SkillMutationAuthority>) -> Self {
        Self { authority }
    }

    /// Legacy direct merge entry point. It fails closed because no reviewed
    /// approval artifact is available; use `preview_merge` + `execute_preview`.
    ///
    /// The `primary_descriptor` is updated in-place with merged triggers, paths,
    /// and tools. The deprecated skill is marked via the Curator.
    pub async fn execute_merge(
        &self,
        proposal: &SkillMergeProposal,
        primary_descriptor: &mut SkillDescriptor,
        deprecated_descriptor: Option<&SkillDescriptor>,
    ) -> Result<()> {
        let _ = (proposal, primary_descriptor, deprecated_descriptor);
        Err(echo_core::error::ReactError::Other(
            "skill merge requires preview_merge followed by execute_preview with approval".into(),
        ))
    }

    pub async fn preview_merge(
        &self,
        request_id: impl Into<String>,
        proposal: &SkillMergeProposal,
        primary_descriptor: &SkillDescriptor,
        deprecated_descriptor: Option<&SkillDescriptor>,
    ) -> Result<SkillMergePreview> {
        let mut merged = primary_descriptor.clone();
        // Merge triggers from deprecated skill into primary.
        if let Some(dep_desc) = deprecated_descriptor {
            let mut merged_triggers: HashSet<String> = merged.triggers.iter().cloned().collect();
            for trigger in &dep_desc.triggers {
                merged_triggers.insert(trigger.clone());
            }
            merged.triggers = merged_triggers.into_iter().collect();

            // Merge paths.
            let mut merged_paths: HashSet<String> = merged.paths.iter().cloned().collect();
            for path in &dep_desc.paths {
                merged_paths.insert(path.clone());
            }
            merged.paths = merged_paths.into_iter().collect();

            // Merge allowed_tools.
            let mut merged_tools: HashSet<String> = merged.allowed_tools.iter().cloned().collect();
            for tool in &dep_desc.allowed_tools {
                merged_tools.insert(tool.clone());
            }
            merged.allowed_tools = merged_tools.into_iter().collect();
        }

        merged.triggers.sort();
        merged.paths.sort();
        merged.allowed_tools.sort();

        let original = tokio::fs::read(&merged.location).await?;
        let original_text = String::from_utf8(original.clone()).map_err(|error| {
            echo_core::error::ReactError::Other(format!("SKILL.md is not UTF-8: {error}"))
        })?;
        let updated = update_skill_frontmatter(&original_text, &merged)?;
        let curator_before = self.authority.curator().load_state()?;
        let mut curator_after = curator_before.clone();
        let meta = curator_after
            .skills
            .get_mut(&proposal.deprecated_skill)
            .ok_or_else(|| {
                echo_core::error::ReactError::Other(format!(
                    "skill {:?} is not tracked by Curator",
                    proposal.deprecated_skill
                ))
            })?;
        if !matches!(
            meta.lifecycle,
            SkillLifecycle::Active | SkillLifecycle::Stale
        ) {
            return Err(echo_core::error::ReactError::Other(format!(
                "skill {:?} is not in a lifecycle state that can be deprecated",
                proposal.deprecated_skill
            )));
        }
        meta.lifecycle = SkillLifecycle::Deprecated;
        meta.superseded_by = Some(proposal.primary_skill.clone());
        if let Some(path) = &meta.path {
            meta.path = Some(SkillFileMutation::canonical_path(path)?);
        }
        meta.last_modified_at = Utc::now();
        let merge_key = format!("{}__{}", proposal.skill_a, proposal.skill_b);
        let file_mutation =
            SkillFileMutation::new(&merged.location, Some(original), Some(updated.into_bytes()))?;
        merged.location = file_mutation.path.clone();
        let request = SkillMutationRequest {
            request_id: request_id.into(),
            entity_key: merge_key,
            kind: SkillMutationKind::Merge,
            reason: format!(
                "Merged {} into {}",
                proposal.deprecated_skill, proposal.primary_skill
            ),
            files: vec![file_mutation],
            curator_before,
            curator_after,
            rollback_of: None,
        };
        let preview = self.authority.preview(&request)?;
        Ok(SkillMergePreview {
            request,
            preview,
            merged_descriptor: merged,
        })
    }

    pub async fn execute_preview(
        &self,
        preview: SkillMergePreview,
        approval: SkillApprovalArtifact,
        primary_descriptor: &mut SkillDescriptor,
    ) -> Result<crate::evolution::SkillMutationReceipt> {
        match self.authority.apply(preview.request, approval).await? {
            SkillMutationOutcome::Applied(receipt)
            | SkillMutationOutcome::AlreadyApplied(receipt) => {
                *primary_descriptor = preview.merged_descriptor;
                Ok(receipt)
            }
            outcome => Err(echo_core::error::ReactError::Other(format!(
                "skill merge did not apply: {outcome:?}"
            ))),
        }
    }
}

fn update_skill_frontmatter(content: &str, descriptor: &SkillDescriptor) -> Result<String> {
    let document = echo_execution::skills::external::SkillDocument::parse(content)
        .map_err(|error| echo_core::error::ReactError::Other(error.to_string()))?;
    document
        .render_with_allowed_tools(&descriptor.allowed_tools)
        .map_err(echo_core::error::ReactError::Other)
}

// ── Similarity helpers ──────────────────────────────────────────────────

/// Compute Jaccard similarity between two string lists.
fn jaccard_overlap(list_a: &[String], list_b: &[String]) -> f64 {
    if list_a.is_empty() && list_b.is_empty() {
        return 1.0;
    }
    if list_a.is_empty() || list_b.is_empty() {
        return 0.0;
    }

    let set_a: HashSet<&str> = list_a.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = list_b.iter().map(|s| s.as_str()).collect();

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Compute word-level Jaccard similarity between two text strings.
fn word_similarity(text_a: &str, text_b: &str) -> f64 {
    let words_a: HashSet<&str> = text_a.split_whitespace().collect();
    let words_b: HashSet<&str> = text_b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }
    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::{JsonlChangeLog, SkillRollbackPreviewOutcome, SkillRollbackTarget};
    use echo_state::memory::store::InMemoryStore;
    use std::collections::HashMap;

    fn make_descriptor(
        name: &str,
        triggers: Vec<&str>,
        paths: Vec<&str>,
        tools: Vec<&str>,
        description: &str,
    ) -> SkillDescriptor {
        SkillDescriptor {
            source: None,
            name: name.to_string(),
            description: description.to_string(),
            triggers: triggers.into_iter().map(|s| s.to_string()).collect(),
            paths: paths.into_iter().map(|s| s.to_string()).collect(),
            allowed_tools: tools.into_iter().map(|s| s.to_string()).collect(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            shell: None,
            hooks: None,
            sandbox: None,
            depends_on: vec![],
            location: std::path::PathBuf::new(),
        }
    }

    /// A no-op ChangeLog for testing.
    struct NullChangeLog;
    impl ChangeLog for NullChangeLog {
        fn record(&self, _entry: super::super::audit::ChangeEntry) -> Result<()> {
            Ok(())
        }
        fn record_idempotent(
            &self,
            _entry: super::super::audit::ChangeEntry,
        ) -> Result<super::super::audit::ChangeRecordOutcome> {
            Ok(super::super::audit::ChangeRecordOutcome::AlreadyRecorded)
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

    #[test]
    fn test_jaccard_overlap_identical() {
        let list_a = vec!["git".to_string(), "commit".to_string(), "push".to_string()];
        let list_b = vec!["git".to_string(), "commit".to_string(), "push".to_string()];
        let overlap = jaccard_overlap(&list_a, &list_b);
        assert!((overlap - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_jaccard_overlap_disjoint() {
        let list_a = vec!["git".to_string(), "commit".to_string()];
        let list_b = vec!["docker".to_string(), "build".to_string()];
        let overlap = jaccard_overlap(&list_a, &list_b);
        assert!((overlap - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_jaccard_overlap_partial() {
        let list_a = vec!["git".to_string(), "commit".to_string(), "push".to_string()];
        let list_b = vec!["git".to_string(), "commit".to_string(), "pull".to_string()];
        // Intersection: {git, commit} = 2
        // Union: {git, commit, push, pull} = 4
        let overlap = jaccard_overlap(&list_a, &list_b);
        assert!((overlap - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_jaccard_overlap_both_empty() {
        let list_a: Vec<String> = vec![];
        let list_b: Vec<String> = vec![];
        let overlap = jaccard_overlap(&list_a, &list_b);
        assert!((overlap - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_word_similarity() {
        let text_a = "Git workflow for version control";
        let text_b = "Git workflow management";
        let similarity = word_similarity(text_a, text_b);
        // Intersection: {Git, workflow} = 2
        // Union: {Git, workflow, for, version, control, management} = 6
        assert!((similarity - 2.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn test_compute_similarity_high() {
        let desc_a = make_descriptor(
            "git-workflow",
            vec!["git", "commit", "push"],
            vec!["*.rs", "*.toml"],
            vec!["Bash", "Read", "Write"],
            "Git version control workflow",
        );
        let desc_b = make_descriptor(
            "git-ops",
            vec!["git", "commit", "push", "pull"],
            vec!["*.rs", "*.toml", "*.md"],
            vec!["Bash", "Read", "Write"],
            "Git operations workflow",
        );
        let breakdown = SkillSimilarityDetector::compute_similarity(&desc_a, &desc_b);
        let score = SkillSimilarityDetector::overall_score(&breakdown);
        assert!(score > 0.6, "Expected high similarity, got {}", score);
    }

    #[test]
    fn test_compute_similarity_low() {
        let desc_a = make_descriptor(
            "git-workflow",
            vec!["git", "commit"],
            vec!["*.rs"],
            vec!["Bash"],
            "Git version control",
        );
        let desc_b = make_descriptor(
            "docker-build",
            vec!["docker", "build"],
            vec!["Dockerfile"],
            vec!["Bash"],
            "Docker container building",
        );
        let breakdown = SkillSimilarityDetector::compute_similarity(&desc_a, &desc_b);
        let score = SkillSimilarityDetector::overall_score(&breakdown);
        assert!(score < 0.3, "Expected low similarity, got {}", score);
    }

    #[tokio::test]
    async fn test_scan_and_propose() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let detector = SkillSimilarityDetector::new(store);

        let descriptors = vec![
            make_descriptor(
                "git-workflow",
                vec!["git", "commit", "push"],
                vec!["*.rs"],
                vec!["Bash", "Read"],
                "Git workflow for commits",
            ),
            make_descriptor(
                "git-ops",
                vec!["git", "commit", "push"],
                vec!["*.rs"],
                vec!["Bash", "Read"],
                "Git operations for commits",
            ),
            make_descriptor(
                "docker-build",
                vec!["docker", "build"],
                vec!["Dockerfile"],
                vec!["Bash"],
                "Docker container building",
            ),
        ];

        let change_log = NullChangeLog;
        let proposals = detector
            .scan_and_propose(&descriptors, &change_log)
            .await
            .unwrap();

        // Should find the git-workflow <-> git-ops pair.
        assert!(!proposals.is_empty());
        let proposal = &proposals[0];
        assert!(
            (proposal.skill_a == "git-workflow" && proposal.skill_b == "git-ops")
                || (proposal.skill_a == "git-ops" && proposal.skill_b == "git-workflow")
        );
        assert!(proposal.similarity_score >= MERGE_THRESHOLD);
    }

    fn make_merger(_store: Arc<dyn Store>) -> SkillMerger {
        use crate::evolution::curator::{Curator, CuratorConfig};
        let config = CuratorConfig::default();
        let state_path = std::env::temp_dir().join(format!(
            "echo-agent-test-curator-state-{}.json",
            uuid::Uuid::new_v4()
        ));
        let change_path = state_path.with_extension("changes.jsonl");
        let curator = Curator::new(config, state_path);
        let authority = Arc::new(
            SkillMutationAuthority::open(
                curator,
                Arc::new(JsonlChangeLog::new(change_path).unwrap()),
            )
            .unwrap(),
        );
        SkillMerger::new(authority)
    }

    #[tokio::test]
    async fn test_execute_merge() -> std::result::Result<(), String> {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let merger = make_merger(store);
        let root = std::env::temp_dir().join(format!("echo-agent-merge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let primary_path = root.join("primary-SKILL.md");
        std::fs::write(
            &primary_path,
            "---\nname: git-workflow\ndescription: Git workflow\n---\nBody\n",
        )
        .map_err(|error| error.to_string())?;

        let mut primary = make_descriptor(
            "git-workflow",
            vec!["git", "commit"],
            vec!["*.rs"],
            vec!["Bash"],
            "Git workflow",
        );
        let deprecated = make_descriptor(
            "git-ops",
            vec!["git", "push", "pull"],
            vec!["*.toml"],
            vec!["Read", "Write"],
            "Git operations",
        );
        primary.location = primary_path.clone();

        merger
            .authority
            .curator()
            .touch_skill("git-ops", true)
            .map_err(|error| error.to_string())?;

        let proposal = SkillMergeProposal {
            skill_a: "git-workflow".to_string(),
            skill_b: "git-ops".to_string(),
            similarity_score: 0.85,
            breakdown: SimilarityBreakdown {
                trigger_overlap: 0.5,
                path_overlap: 0.0,
                tool_overlap: 0.0,
                description_similarity: 0.5,
            },
            primary_skill: "git-workflow".to_string(),
            deprecated_skill: "git-ops".to_string(),
            created_at: Utc::now(),
        };

        let preview = merger
            .preview_merge(
                uuid::Uuid::new_v4().to_string(),
                &proposal,
                &primary,
                Some(&deprecated),
            )
            .await
            .map_err(|error| error.to_string())?;
        let approval = SkillApprovalArtifact::new(
            uuid::Uuid::new_v4().to_string(),
            &preview.preview.operation_digest,
            "test-reviewer",
            Utc::now(),
        );
        let receipt = merger
            .execute_preview(preview, approval, &mut primary)
            .await
            .map_err(|error| error.to_string())?;

        // Primary should now have merged triggers.
        assert!(primary.triggers.contains(&"git".to_string()));
        assert!(primary.triggers.contains(&"commit".to_string()));
        assert!(primary.triggers.contains(&"push".to_string()));
        assert!(primary.triggers.contains(&"pull".to_string()));

        // Primary should have merged paths.
        assert!(primary.paths.contains(&"*.rs".to_string()));
        assert!(primary.paths.contains(&"*.toml".to_string()));

        // Primary should have merged tools.
        assert!(primary.allowed_tools.contains(&"Bash".to_string()));
        assert!(primary.allowed_tools.contains(&"Read".to_string()));
        assert!(primary.allowed_tools.contains(&"Write".to_string()));
        // The persisted file is standard-format: the merged tool surface is
        // written as a space-separated string, while routing triggers have
        // no standard field and stay in the in-memory descriptor only.
        let persisted =
            std::fs::read_to_string(&primary_path).map_err(|error| error.to_string())?;
        assert!(persisted.contains("allowed-tools: Bash Read Write"));
        assert!(!persisted.contains("\ntriggers:"));
        let authority = merger.authority.clone();
        let target = SkillRollbackTarget::BatchId(receipt.batch_id);
        let SkillRollbackPreviewOutcome::Ready(rollback_preview) = authority
            .preview_rollback("merge-rollback", &target)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Err("merge rollback preview was not ready".into());
        };
        let rollback_approval = SkillApprovalArtifact::new(
            "merge-rollback-approval",
            &rollback_preview.operation_digest,
            "test-reviewer",
            Utc::now(),
        );
        authority
            .rollback("merge-rollback", target, rollback_approval)
            .await
            .map_err(|error| error.to_string())?;
        let restored = std::fs::read_to_string(&primary_path).map_err(|error| error.to_string())?;
        assert_eq!(
            restored,
            "---\nname: git-workflow\ndescription: Git workflow\n---\nBody\n"
        );
        assert_eq!(
            merger
                .authority
                .curator()
                .load_state()
                .map_err(|error| error.to_string())?
                .skills
                .get("git-ops")
                .map(|meta| meta.lifecycle),
            Some(SkillLifecycle::Active)
        );
        std::fs::remove_dir_all(root).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    }
}
