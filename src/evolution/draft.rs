//! Skill draft generation — creates SKILL.md files from detected candidates.
//!
//! Takes a [`SkillCandidate`] (produced by [`super::candidate::SkillCandidateDetector`]) and
//! generates a draft SKILL.md file at `.echo-agent/skills/_drafts/<name>/SKILL.md`.
//! The draft is a *proposal* — a human reviews it before promoting to Active.
//!
//! # Template-based generation
//!
//! Phase 3 uses deterministic templates (no LLM call). This keeps the system
//! fast, free, and reproducible. A future iteration can add LLM-assisted
//! refinement through the optional eval-driven improvement pipeline.

use std::path::PathBuf;

use echo_state::memory::typed_store::TypedMemoryStore;

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
use super::candidate::{CANDIDATE_NAMESPACE, SkillCandidate};
use super::curator::{Curator, CuratorConfig};
use crate::error::Result;
use echo_core::error::ReactError;

// ── Constants ──────────────────────────────────────────────────────────

/// Where draft SKILL.md files are saved, relative to echo_agent_dir.
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

// ── SkillDraftGenerator ────────────────────────────────────────────────

/// Generates draft SKILL.md files from skill candidates.
///
/// The generator writes template-based SKILL.md files to the `_drafts`
/// directory and promotes the candidate to `Draft` lifecycle state via
/// the [`Curator`].
pub struct SkillDraftGenerator<'a> {
    /// Path to the `.echo-agent/` directory.
    echo_agent_dir: PathBuf,
    /// ChangeLog for recording mutations.
    change_log: &'a dyn ChangeLog,
    curator: Curator,
    require_curator_transition: bool,
}

impl<'a> SkillDraftGenerator<'a> {
    /// Create a new generator.
    pub fn new(echo_agent_dir: PathBuf, change_log: &'a dyn ChangeLog) -> Self {
        let curator = Curator::new(
            CuratorConfig::default(),
            echo_agent_dir.join("curator_state.json"),
        );
        Self {
            echo_agent_dir,
            change_log,
            curator,
            require_curator_transition: false,
        }
    }

    /// Use a consumer-supplied curator state file.
    pub fn with_curator(mut self, curator: Curator) -> Self {
        self.curator = curator;
        self.require_curator_transition = true;
        self
    }

    /// Generate a draft SKILL.md from a named candidate.
    ///
    /// Reads the candidate from the Store, generates the SKILL.md file,
    /// and promotes the candidate from `Candidate` to `Draft` lifecycle state.
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

        self.generate_from_candidate(&candidate).await
    }

    /// Generate a draft SKILL.md directly from a [`SkillCandidate`] struct.
    pub async fn generate_from_candidate(&self, candidate: &SkillCandidate) -> Result<DraftResult> {
        let name = &candidate.name;
        let drafts_root = self.echo_agent_dir.join(DRAFTS_DIR);
        let dir = echo_core::utils::fs::join_path_segment(&drafts_root, name).map_err(|error| {
            ReactError::Other(format!("Unsafe skill candidate name {name:?}: {error}"))
        })?;
        let skill_md_path = dir.join("SKILL.md");

        // 2. Generate SKILL.md content.
        let content = render_skill_md(candidate);

        // 3. Write to disk.
        let previous = match std::fs::read(&skill_md_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let created = previous.is_none();
        echo_core::utils::fs::atomic_write(&skill_md_path, content.as_bytes())?;

        // 4. Promote in the evolution-owned Curator lifecycle.
        match self.curator.promote_to_draft_at(name, Some(&skill_md_path)) {
            Ok(true) => {}
            Ok(false) if self.require_curator_transition => {
                restore_draft_file(&skill_md_path, previous.as_deref())?;
                return Err(ReactError::Other(format!(
                    "candidate '{name}' is not in Candidate lifecycle state"
                )));
            }
            Err(error) if self.require_curator_transition => {
                restore_draft_file(&skill_md_path, previous.as_deref())?;
                return Err(error);
            }
            Ok(false) => tracing::warn!("Candidate '{}' was not registered with Curator", name),
            Err(error) => {
                tracing::warn!("Failed to promote '{}' to draft: {}", name, error);
            }
        }

        // 5. Record in audit log.
        let action = if created { "created" } else { "updated" };
        let entry = ChangeEntryBuilder::new(EntityType::Skill, name, ChangeType::Create)
            .reason(format!(
                "draft SKILL.md {} for candidate '{}' from {} observations",
                action, name, candidate.sample_count
            ))
            .trigger("skill_draft_generator".to_string())
            .build(self.change_log);
        if let Err(error) = self.change_log.record(entry) {
            let _ = self.curator.revert_draft_to_candidate(name);
            restore_draft_file(&skill_md_path, previous.as_deref())?;
            return Err(error);
        }

        Ok(DraftResult {
            name: name.clone(),
            skill_md_path,
            created,
        })
    }
}

fn restore_draft_file(path: &std::path::Path, previous: Option<&[u8]>) -> Result<()> {
    if let Some(bytes) = previous {
        echo_core::utils::fs::atomic_write(path, bytes)?;
    } else if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.into());
    }
    Ok(())
}

// ── Template rendering ─────────────────────────────────────────────────

/// Render a SKILL.md file from a candidate using a deterministic template.
fn render_skill_md(candidate: &SkillCandidate) -> String {
    let SkillCandidate {
        name,
        description,
        trigger_patterns,
        tool_sequence,
        sample_count,
        confidence,
        topic,
        source_type,
        created_at,
    } = candidate;

    let triggers_yaml = trigger_patterns
        .iter()
        .map(|t| format!("    - \"{}\"", yaml_escape(t)))
        .collect::<Vec<_>>()
        .join("\n");

    let tools_yaml = if tool_sequence.is_empty() {
        "    - Bash(*)".to_string()
    } else {
        tool_sequence
            .iter()
            .map(|t| format!("    - \"{}\"", yaml_escape(t)))
            .collect::<Vec<_>>()
            .join("\n")
    };

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
triggers:
{triggers_yaml}
allowed-tools:
{tools_yaml}
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
        triggers_yaml = triggers_yaml,
        tools_yaml = tools_yaml,
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
    use chrono::Utc;
    use echo_core::memory::types::MemoryType;

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

    #[tokio::test]
    async fn test_draft_generation_creates_file() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let log = NullChangeLog;
        let generator = SkillDraftGenerator::new(dir.clone(), &log);

        let candidate = sample_candidate();
        let result = generator.generate_from_candidate(&candidate).await.unwrap();

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
        let log = NullChangeLog;
        let generator = SkillDraftGenerator::new(dir, &log);
        let mut candidate = sample_candidate();
        candidate.name = "../escaped-skill".to_string();
        assert!(generator.generate_from_candidate(&candidate).await.is_err());
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
        let log = NullChangeLog;
        let generator = SkillDraftGenerator::new(root.path().to_path_buf(), &log);

        assert!(
            generator
                .generate_from_candidate(&sample_candidate())
                .await
                .is_err()
        );
        assert!(!outside.path().join("SKILL.md").exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_draft_yaml_frontmatter_valid() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let log = NullChangeLog;
        let generator = SkillDraftGenerator::new(dir, &log);

        let candidate = sample_candidate();
        let result = generator.generate_from_candidate(&candidate).await.unwrap();

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
        assert!(parsed["triggers"].as_sequence().is_some());
    }

    #[tokio::test]
    async fn test_draft_idempotent_update() {
        let dir = tempfile::tempdir().expect("tempdir").keep();
        let log = NullChangeLog;
        let generator = SkillDraftGenerator::new(dir, &log);

        let candidate = sample_candidate();

        // First generation: created.
        let result1 = generator.generate_from_candidate(&candidate).await.unwrap();
        assert!(result1.created);

        // Second generation: updated, not created.
        let mut candidate2 = candidate.clone();
        candidate2.sample_count = 7;
        let result2 = generator
            .generate_from_candidate(&candidate2)
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
