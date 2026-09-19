//! Skill draft generation — creates SKILL.md files from detected candidates.
//!
//! Takes a [`SkillCandidate`] (produced by [`super::candidate::SkillCandidateDetector`]) and
//! generates a draft SKILL.md file at `<consumer-root>/skills/_drafts/<name>/SKILL.md`.
//! The draft is a *proposal* — a human reviews it before promoting to Active.
//!
//! # Template-based generation
//!
//! Phase 3 uses deterministic templates (no LLM call). This keeps the system
//! fast, free, and reproducible. A future iteration can add LLM-assisted
//! refinement through the optional eval-driven improvement pipeline.

use std::path::PathBuf;
use std::sync::Arc;

use echo_state::memory::typed_store::TypedMemoryStore;

use super::candidate::{CANDIDATE_NAMESPACE, SkillCandidate};
use super::curator::SkillLifecycle;
use super::skill_mutation::{
    SkillApprovalArtifact, SkillFileMutation, SkillMutationAuthority, SkillMutationKind,
    SkillMutationOutcome, SkillMutationPreview, SkillMutationRequest,
};
use crate::error::Result;
use echo_core::error::ReactError;

// ── Constants ──────────────────────────────────────────────────────────

/// Where draft SKILL.md files are saved, relative to the consumer root.
const DRAFTS_DIR: &str = "skills/_drafts";

// ── DraftResult ────────────────────────────────────────────────────────

/// Result of generating a skill draft.
#[derive(Debug, Clone)]
pub struct DraftResult {
    /// Name of the skill.
    pub name: String,
    /// Path to the created SKILL.md file.
    pub skill_md_path: PathBuf,
    /// Whether this was a new creation (true) or an update (false).
    pub created: bool,
}

/// Exact reviewed draft request and its digest-bound preview.
pub struct SkillDraftPreview {
    pub result: DraftResult,
    pub request: SkillMutationRequest,
    pub preview: SkillMutationPreview,
}

// ── SkillDraftGenerator ────────────────────────────────────────────────

/// Generates draft SKILL.md files from skill candidates.
///
/// The generator writes template-based SKILL.md files to the `_drafts`
/// directory and promotes the candidate to `Draft` lifecycle state via
/// the [`Curator`].
pub struct SkillDraftGenerator {
    /// Consumer-supplied evolution storage root.
    storage_root: PathBuf,
    authority: Arc<SkillMutationAuthority>,
}

impl SkillDraftGenerator {
    /// Create a new generator.
    pub fn new(storage_root: PathBuf, authority: Arc<SkillMutationAuthority>) -> Self {
        Self {
            storage_root,
            authority,
        }
    }

    /// Legacy named-candidate entry point. It reads the candidate but fails
    /// closed because no reviewed approval artifact is available.
    #[deprecated(
        note = "load the candidate, call preview_generate_from_candidate, then generate_from_preview with approval"
    )]
    pub async fn generate(
        &self,
        name: &str,
        typed_store: &TypedMemoryStore,
    ) -> Result<DraftResult> {
        // 1. Read candidate from Store.
        let entry = typed_store
            .get_typed(CANDIDATE_NAMESPACE, name)
            .await?
            .ok_or_else(|| {
                ReactError::Other(format!("Skill candidate '{}' not found in store", name))
            })?;

        let candidate: SkillCandidate = serde_json::from_str(&entry.content).map_err(|e| {
            ReactError::Other(format!("Failed to parse candidate '{}': {}", name, e))
        })?;

        Err(ReactError::Other(format!(
            "draft mutation for {:?} requires preview_generate_from_candidate followed by generate_from_preview with approval",
            candidate.name
        )))
    }

    /// Legacy direct entry point; it fails closed without an approval artifact.
    #[deprecated(note = "use preview_generate_from_candidate followed by generate_from_preview")]
    pub async fn generate_from_candidate(&self, candidate: &SkillCandidate) -> Result<DraftResult> {
        Err(ReactError::Other(format!(
            "draft mutation for {:?} requires preview_generate_from_candidate followed by generate_from_preview with approval",
            candidate.name
        )))
    }

    /// Build the exact file/lifecycle mutation reviewed by the host.
    pub async fn preview_generate_from_candidate(
        &self,
        candidate: &SkillCandidate,
        request_id: impl Into<String>,
    ) -> Result<SkillDraftPreview> {
        let name = &candidate.name;
        let drafts_root = self.storage_root.join(DRAFTS_DIR);
        let dir = echo_core::utils::fs::join_path_segment(&drafts_root, name).map_err(|error| {
            ReactError::Other(format!("Unsafe skill candidate name {name:?}: {error}"))
        })?;
        let skill_md_path = dir.join("SKILL.md");

        let content = render_skill_md(candidate);
        let previous = match std::fs::read(&skill_md_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let created = previous.is_none();
        let file_mutation =
            SkillFileMutation::new(&skill_md_path, previous, Some(content.into_bytes()))?;
        let canonical_skill_path = file_mutation.path.clone();
        let canonical_drafts_root = SkillFileMutation::canonical_path(&drafts_root)?;
        if !canonical_skill_path.starts_with(&canonical_drafts_root) {
            return Err(ReactError::Other(format!(
                "skill draft path escapes the drafts root: {}",
                canonical_skill_path.display()
            )));
        }
        let curator_before = self.authority.curator().load_state()?;
        let mut curator_after = curator_before.clone();
        let meta = curator_after.skills.get_mut(name).ok_or_else(|| {
            ReactError::Other(format!("candidate '{name}' is not registered with Curator"))
        })?;
        if !matches!(
            meta.lifecycle,
            SkillLifecycle::Candidate | SkillLifecycle::Draft
        ) {
            return Err(ReactError::Other(format!(
                "candidate '{name}' is not in Candidate lifecycle state"
            )));
        }
        meta.lifecycle = SkillLifecycle::Draft;
        meta.path = Some(canonical_skill_path.clone());
        meta.last_modified_at = chrono::Utc::now();
        let request = SkillMutationRequest {
            request_id: request_id.into(),
            entity_key: name.clone(),
            kind: SkillMutationKind::Draft,
            reason: format!(
                "draft SKILL.md for candidate '{}' from {} observations",
                name, candidate.sample_count
            ),
            files: vec![file_mutation],
            curator_before,
            curator_after,
            rollback_of: None,
        };
        let preview = self.authority.preview(&request)?;
        Ok(SkillDraftPreview {
            result: DraftResult {
                name: name.clone(),
                skill_md_path: canonical_skill_path,
                created,
            },
            request,
            preview,
        })
    }

    /// Apply an exact preview using a one-use digest-bound approval artifact.
    pub async fn generate_from_preview(
        &self,
        preview: SkillDraftPreview,
        approval: SkillApprovalArtifact,
    ) -> Result<DraftResult> {
        match self.authority.apply(preview.request, approval).await? {
            SkillMutationOutcome::Applied(_) | SkillMutationOutcome::AlreadyApplied(_) => {
                Ok(preview.result)
            }
            outcome => Err(ReactError::Other(format!(
                "draft mutation did not apply: {outcome:?}"
            ))),
        }
    }
}

// ── Template rendering ─────────────────────────────────────────────────

/// Render a SKILL.md file from a candidate using a deterministic template.
fn render_skill_md(candidate: &SkillCandidate) -> String {
    let SkillCandidate {
        name,
        description,
        // Trigger patterns stay in curator state: the standard SKILL.md
        // format has no field for routing triggers.
        trigger_patterns: _,
        tool_sequence,
        sample_count,
        confidence,
        topic,
        source_type,
        created_at,
    } = candidate;

    // Official agentskills.io format: one space-separated plain string.
    // Routing is description-driven; trigger patterns stay in curator state
    // because the standard format has no field for them.
    let tools_inline = (!tool_sequence.is_empty()).then(|| {
        tool_sequence
            .iter()
            .map(|t| yaml_escape(t))
            .collect::<Vec<_>>()
            .join(" ")
    });

    let workflow_steps = if tool_sequence.is_empty() {
        "1. Analyze the user's request\n2. Apply the relevant tools\n3. Verify the result"
            .to_string()
    } else {
        tool_sequence
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "{}. Use `{}` to accomplish the task step",
                    i + 1,
                    yaml_escape(t)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let source_label = match source_type {
        echo_core::memory::types::MemoryType::WorkflowPattern => "workflow",
        echo_core::memory::types::MemoryType::DebuggingLesson => "debugging",
        _ => "usage",
    };

    // Escape values that go into YAML frontmatter to prevent injection.
    let safe_name = yaml_escape(name);
    let safe_description = yaml_escape(description);
    let safe_topic = yaml_escape(topic);

    format!(
        r#"---
name: {safe_name}
description: >-
    {safe_description}
{allowed_tools_line}
metadata:
    author: echo-agent
    source: auto-candidate
    confidence: "{confidence:.2}"
    sample_count: "{sample_count}"
    lifecycle: draft
    topic: "{safe_topic}"
    created_at: "{created_at}"
---

## {safe_name}

Auto-generated skill from {sample_count} observed {source_label} patterns on topic `{safe_topic}`.

### Workflow

{workflow_steps}

### Common Patterns

This skill was proposed based on {sample_count} repeated observations.
Confidence: {confidence:.0}%.

### Safety

- Always verify the result before presenting to the user.
- Do not apply destructive operations without confirmation.
"#,
        safe_name = safe_name,
        safe_description = safe_description,
        allowed_tools_line = tools_inline
            .map(|tools| format!("allowed-tools: {tools}"))
            .unwrap_or_default(),
        sample_count = sample_count,
        confidence = confidence,
        safe_topic = safe_topic,
        source_label = source_label,
        created_at = crate::utils::time::to_local(*created_at).to_rfc3339(),
        workflow_steps = workflow_steps,
    )
}

/// Escape a string value for safe inclusion in YAML double-quoted or unquoted context.
///
/// Replaces characters that would break YAML parsing or inject additional frontmatter
/// fields (`: `, `"`, `\`, newlines, `---`).
/// Escape a value for inclusion inside a **double-quoted** YAML string.
///
/// The YAML template wraps values in `"..."`, so escaping `\`, `"`, newlines,
/// `\r`, and `---` (frontmatter delimiter) is sufficient. Additional characters
/// (`:`, `#`, `[`, `]`, `{`, `}`) are harmless inside double quotes per the
/// YAML 1.2 spec §7.3.1.
fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', "")
        .replace("---", "-\\-\\-")
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::{Curator, CuratorConfig, JsonlChangeLog};
    use chrono::Utc;
    use echo_core::memory::types::MemoryType;

    fn sample_candidate() -> SkillCandidate {
        SkillCandidate {
            name: "cargo-build".to_string(),
            description: "Auto-detected workflow pattern for 'cargo-build'.".to_string(),
            trigger_patterns: vec![
                "cargo-build".to_string(),
                "build".to_string(),
                "compile".to_string(),
            ],
            tool_sequence: vec!["Bash(*)".to_string(), "Read".to_string()],
            sample_count: 5,
            confidence: 0.85,
            topic: "cargo-build".to_string(),
            source_type: MemoryType::WorkflowPattern,
            created_at: Utc::now(),
        }
    }

    fn make_generator(root: PathBuf) -> Result<SkillDraftGenerator> {
        let curator = Curator::new(CuratorConfig::default(), root.join("curator_state.json"));
        let authority = Arc::new(SkillMutationAuthority::open(
            curator,
            Arc::new(JsonlChangeLog::new(root.join("draft-changes.jsonl"))?),
        )?);
        Ok(SkillDraftGenerator::new(root, authority))
    }

    async fn generate_approved(
        generator: &SkillDraftGenerator,
        candidate: &SkillCandidate,
    ) -> Result<DraftResult> {
        generator
            .authority
            .curator()
            .register_candidate(&candidate.name)?;
        let preview = generator
            .preview_generate_from_candidate(candidate, uuid::Uuid::new_v4().to_string())
            .await?;
        let approval = SkillApprovalArtifact::new(
            uuid::Uuid::new_v4().to_string(),
            &preview.preview.operation_digest,
            "test-reviewer",
            Utc::now(),
        );
        generator.generate_from_preview(preview, approval).await
    }

    #[tokio::test]
    async fn test_draft_generation_creates_file() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let generator = make_generator(dir.clone()).unwrap();

        let candidate = sample_candidate();
        let result = generate_approved(&generator, &candidate).await.unwrap();

        assert_eq!(result.name, "cargo-build");
        assert!(result.created);
        assert!(result.skill_md_path.exists());

        let content = std::fs::read_to_string(&result.skill_md_path).unwrap();
        assert!(content.contains("name: cargo-build"));
        assert!(content.contains("lifecycle: draft"));
    }

    #[tokio::test]
    async fn draft_generation_rejects_path_escape_name() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let outside = dir.parent().map(|parent| parent.join("escaped-skill"));
        let generator = make_generator(dir).unwrap();
        let mut candidate = sample_candidate();
        candidate.name = "../escaped-skill".to_string();
        assert!(
            generator
                .preview_generate_from_candidate(&candidate, "escape-test")
                .await
                .is_err()
        );
        assert!(outside.is_none_or(|path| !path.exists()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn draft_generation_rejects_symlink_candidate_directory() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let drafts = root.path().join(DRAFTS_DIR);
        std::fs::create_dir_all(&drafts)?;
        symlink(outside.path(), drafts.join("cargo-build"))?;
        let generator = make_generator(root.path().to_path_buf())?;

        assert!(
            generator
                .preview_generate_from_candidate(&sample_candidate(), "symlink-test")
                .await
                .is_err()
        );
        assert!(!outside.path().join("SKILL.md").exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_draft_yaml_frontmatter_valid() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let generator = make_generator(dir).unwrap();

        let candidate = sample_candidate();
        let result = generate_approved(&generator, &candidate).await.unwrap();

        let content = std::fs::read_to_string(&result.skill_md_path).unwrap();

        // Extract YAML frontmatter.
        let yaml = if let Some(rest) = content.strip_prefix("---") {
            let end = rest.find("---").unwrap_or(rest.len());
            &rest[..end]
        } else {
            panic!("Missing frontmatter delimiter");
        };

        // Should be valid YAML.
        let parsed: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(yaml).expect("YAML should parse");
        assert_eq!(parsed["name"].as_str(), Some("cargo-build"));
        assert_eq!(parsed["metadata"]["lifecycle"].as_str(), Some("draft"));
        // Standard-only layout: no routing namespace, string allowed-tools.
        assert!(parsed.get("triggers").is_none());
        assert!(parsed["metadata"].get("echo-agent").is_none());
        assert!(parsed["allowed-tools"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_draft_idempotent_update() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let generator = make_generator(dir).unwrap();

        let candidate = sample_candidate();

        // First generation: created.
        let result1 = generate_approved(&generator, &candidate).await.unwrap();
        assert!(result1.created);

        // Second generation: updated, not created.
        let mut candidate2 = candidate.clone();
        candidate2.sample_count = 7;
        let preview = generator
            .preview_generate_from_candidate(&candidate2, uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        let approval = SkillApprovalArtifact::new(
            uuid::Uuid::new_v4().to_string(),
            &preview.preview.operation_digest,
            "test-reviewer",
            Utc::now(),
        );
        let result2 = generator
            .generate_from_preview(preview, approval)
            .await
            .unwrap();
        assert!(!result2.created);

        // Content should reflect the updated sample count.
        let content = std::fs::read_to_string(&result2.skill_md_path).unwrap();
        assert!(content.contains("7 observed"));
    }

    #[test]
    fn test_render_skill_md_content() {
        let candidate = sample_candidate();
        let md = render_skill_md(&candidate);

        // Basic structure checks.
        assert!(md.starts_with("---"));
        assert!(md.contains("name: cargo-build"));
        assert!(md.contains("lifecycle: draft"));
        assert!(md.contains("## cargo-build"));
        assert!(md.contains("### Workflow"));
        assert!(md.contains("### Safety"));
        assert!(md.contains("5 observed"));
        assert!(md.contains("confidence: \"0.85\""));
    }
}
