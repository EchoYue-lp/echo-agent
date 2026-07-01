//! Skill patch generation based on failure patterns.
//!
//! Analyzes skill telemetry to identify common failure modes and generates
//! patch proposals to improve skill instructions with better error handling.

use chrono::{DateTime, Utc};
use std::sync::Arc;

use echo_core::memory::store::Store;
use echo_state::skill_telemetry::SkillTelemetryStore;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::evolution::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};

// Re-export SkillDescriptor for use in this module.
pub use echo_execution::skills::external::SkillDescriptor;

/// Type of patch to apply to a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchType {
    /// Add error handling instructions for a specific failure pattern.
    ErrorHandling {
        /// The failure pattern to handle.
        failure_pattern: String,
        /// Suggested error handling instructions.
        handling_instructions: String,
    },
    /// Add prerequisite checks before execution.
    PrerequisiteCheck {
        /// Description of the prerequisite.
        check_description: String,
        /// How to verify the prerequisite.
        verification_steps: Vec<String>,
    },
    /// Add fallback strategy for when primary approach fails.
    FallbackStrategy {
        /// When to use the fallback.
        trigger_condition: String,
        /// Fallback instructions.
        fallback_instructions: String,
    },
    /// Improve existing instructions with more detail.
    InstructionEnhancement {
        /// Which part of the instructions to enhance.
        target_section: String,
        /// Enhancement details.
        enhancement: String,
    },
}

impl PatchType {
    /// Human-readable label for this patch type (used in markdown headings).
    pub fn label(&self) -> &'static str {
        match self {
            PatchType::ErrorHandling { .. } => "Error Handling",
            PatchType::PrerequisiteCheck { .. } => "Prerequisite Check",
            PatchType::FallbackStrategy { .. } => "Fallback Strategy",
            PatchType::InstructionEnhancement { .. } => "Instruction Enhancement",
        }
    }
}

/// A proposed patch for a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPatch {
    /// Skill name.
    pub skill_name: String,
    /// Type of patch.
    pub patch_type: PatchType,
    /// Rationale for this patch.
    pub rationale: String,
    /// Confidence score (0.0-1.0) based on failure frequency.
    pub confidence: f64,
    /// Priority score (0-10) for ordering patches.
    pub priority: u8,
    /// When this patch was proposed.
    pub proposed_at: DateTime<Utc>,
}

impl SkillPatch {
    /// Generate a human-readable summary of the patch.
    pub fn summary(&self) -> String {
        match &self.patch_type {
            PatchType::ErrorHandling {
                failure_pattern,
                handling_instructions,
            } => {
                format!(
                    "Add error handling for '{}' (confidence: {:.0}%):\n  {}",
                    failure_pattern,
                    self.confidence * 100.0,
                    handling_instructions
                )
            }
            PatchType::PrerequisiteCheck {
                check_description,
                verification_steps,
            } => {
                format!(
                    "Add prerequisite check: {} (priority: {})\n  Steps:\n    - {}",
                    check_description,
                    self.priority,
                    verification_steps.join("\n    - ")
                )
            }
            PatchType::FallbackStrategy {
                trigger_condition,
                fallback_instructions,
            } => {
                format!(
                    "Add fallback when: {} (confidence: {:.0}%)\n  Fallback: {}",
                    trigger_condition,
                    self.confidence * 100.0,
                    fallback_instructions
                )
            }
            PatchType::InstructionEnhancement {
                target_section,
                enhancement,
            } => {
                format!(
                    "Enhance section '{}' (priority: {}):\n  {}",
                    target_section, self.priority, enhancement
                )
            }
        }
    }

    /// Render the patch as a markdown section suitable for appending to SKILL.md.
    ///
    /// The section is placed after the frontmatter and existing body content.
    /// It includes a timestamped heading, the rationale, and the structured
    /// instructions derived from the patch type.
    pub fn to_markdown(&self) -> String {
        let ts = self.proposed_at.format("%Y-%m-%d").to_string();
        let mut lines = vec![
            format!(
                "\n---\n\n## Auto-patch: {} ({})\n",
                self.patch_type.label(),
                ts
            ),
            format!("**Rationale:** {}\n", self.rationale),
            format!("**Confidence:** {:.0}%\n", self.confidence * 100.0),
        ];

        match &self.patch_type {
            PatchType::ErrorHandling {
                failure_pattern,
                handling_instructions,
            } => {
                lines.push(format!("**Failure pattern:** `{failure_pattern}`\n"));
                lines.push(format!("**Instructions:**\n{handling_instructions}\n"));
            }
            PatchType::PrerequisiteCheck {
                check_description,
                verification_steps,
            } => {
                lines.push(format!("**Check:** {check_description}\n"));
                lines.push("**Steps:**\n".into());
                for step in verification_steps {
                    lines.push(format!("- {step}"));
                }
                lines.push(String::new());
            }
            PatchType::FallbackStrategy {
                trigger_condition,
                fallback_instructions,
            } => {
                lines.push(format!("**Trigger:** {trigger_condition}\n"));
                lines.push(format!("**Fallback:**\n{fallback_instructions}\n"));
            }
            PatchType::InstructionEnhancement {
                target_section,
                enhancement,
            } => {
                lines.push(format!("**Section:** {target_section}\n"));
                lines.push(format!("**Enhancement:**\n{enhancement}\n"));
            }
        }

        lines.join("\n")
    }
}

/// Generates skill patches based on telemetry analysis.
pub struct SkillPatcher {
    telemetry_store: SkillTelemetryStore,
}

impl SkillPatcher {
    /// Create a new skill patcher.
    pub fn new(store: Arc<dyn Store>) -> Self {
        let telemetry_store = SkillTelemetryStore::new(store);
        Self { telemetry_store }
    }

    /// Analyze a skill's failure patterns and generate patch proposals.
    pub async fn analyze_and_propose(&self, skill_name: &str) -> Result<Vec<SkillPatch>> {
        let telemetry = match self.telemetry_store.get_telemetry(skill_name).await? {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };

        let mut patches = Vec::new();

        // Analyze common failures
        for failure in &telemetry.common_failures {
            if let Some(patch) = self.generate_patch_for_failure(skill_name, failure) {
                patches.push(patch);
            }
        }

        // Sort by priority (descending) then confidence (descending)
        patches.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });

        Ok(patches)
    }

    /// Generate patches for all skills with failure patterns.
    pub async fn analyze_all_skills(&self) -> Result<Vec<SkillPatch>> {
        let all_telemetry = self.telemetry_store.list_all().await?;
        let mut all_patches = Vec::new();

        for telemetry in all_telemetry {
            if !telemetry.common_failures.is_empty() {
                for failure in &telemetry.common_failures {
                    if let Some(patch) =
                        self.generate_patch_for_failure(&telemetry.skill_name, failure)
                    {
                        all_patches.push(patch);
                    }
                }
            }
        }

        // Sort by priority then confidence
        all_patches.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });

        Ok(all_patches)
    }

    /// Apply a patch to a skill's SKILL.md file.
    ///
    /// Appends the patch instructions as a markdown section to the body
    /// (after any existing frontmatter). Does NOT modify the YAML frontmatter.
    /// Records the change in the audit log.
    ///
    /// # Arguments
    /// * `patch` — The patch to apply (from `analyze_and_propose`).
    /// * `descriptor` — The skill descriptor (provides `.location` = SKILL.md path).
    /// * `change_log` — Audit log to record the mutation.
    pub async fn apply_patch(
        &self,
        patch: &SkillPatch,
        descriptor: &SkillDescriptor,
        change_log: &dyn ChangeLog,
    ) -> Result<()> {
        let path = &descriptor.location;
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::ReactError::Other(format!(
                "Failed to read SKILL.md at {}: {e}",
                path.display()
            ))
        })?;

        // Split frontmatter (--- ... ---) from body. If no frontmatter, treat
        // the entire content as body.
        let (frontmatter, body) = if content.starts_with("---\n") {
            if let Some(end) = content[4..].find("\n---\n") {
                let fm_end = end + 8; // 4 (prefix) + content + 4 (\n---\n)
                let (fm, rest) = content.split_at(fm_end);
                (fm.to_string(), rest.to_string())
            } else {
                (String::new(), content.clone())
            }
        } else {
            (String::new(), content.clone())
        };

        // Append the patch markdown section to the body.
        let patch_md = patch.to_markdown();
        let new_body = format!("{}\n{}", body.trim_end(), patch_md);
        let new_content = if frontmatter.is_empty() {
            new_body
        } else {
            format!("{frontmatter}\n{new_body}\n")
        };

        // Write back atomically (write to tmp, then rename — same pattern as FileStore).
        let tmp_path = path.with_extension("md.tmp");
        tokio::fs::write(&tmp_path, &new_content)
            .await
            .map_err(|e| {
                crate::error::ReactError::Other(format!("Failed to write patched SKILL.md: {e}"))
            })?;
        tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
            crate::error::ReactError::Other(format!("Failed to rename patched SKILL.md: {e}"))
        })?;

        // Record in audit log.
        let entry =
            ChangeEntryBuilder::new(EntityType::Skill, &patch.skill_name, ChangeType::Update)
                .reason(format!(
                    "Applied patch: {} (confidence: {:.0}%)",
                    patch.patch_type.label(),
                    patch.confidence * 100.0
                ))
                .trigger("skill_patcher".to_string())
                .build(change_log);
        change_log.record(entry)?;

        tracing::info!(
            skill = %patch.skill_name,
            patch_type = patch.patch_type.label(),
            "Patch applied to {}",
            path.display()
        );

        Ok(())
    }

    /// Generate a patch proposal for a specific failure pattern.
    fn generate_patch_for_failure(
        &self,
        skill_name: &str,
        failure: &echo_state::skill_telemetry::FailurePattern,
    ) -> Option<SkillPatch> {
        let snippet_lower = failure.error_snippet.to_lowercase();

        // Calculate confidence based on failure frequency
        let confidence = if failure.count >= 10 {
            0.95
        } else if failure.count >= 5 {
            0.85
        } else if failure.count >= 3 {
            0.75
        } else {
            0.60
        };

        // Determine patch type based on error pattern analysis
        let (patch_type, priority) = if snippet_lower.contains("permission denied")
            || snippet_lower.contains("access denied")
        {
            (
                PatchType::PrerequisiteCheck {
                    check_description: "Verify user has required permissions".to_string(),
                    verification_steps: vec![
                        "Check if target path is writable".to_string(),
                        "Verify file/directory ownership".to_string(),
                        "Confirm user has necessary privileges".to_string(),
                    ],
                },
                9,
            )
        } else if snippet_lower.contains("not found")
            || snippet_lower.contains("no such file")
            || snippet_lower.contains("does not exist")
        {
            (
                PatchType::PrerequisiteCheck {
                    check_description: "Verify required files/resources exist".to_string(),
                    verification_steps: vec![
                        "Check if target file or directory exists".to_string(),
                        "Verify path is correct and accessible".to_string(),
                        "Confirm dependencies are installed".to_string(),
                    ],
                },
                8,
            )
        } else if snippet_lower.contains("timeout") || snippet_lower.contains("timed out") {
            (
                PatchType::FallbackStrategy {
                    trigger_condition: "Operation times out or takes too long".to_string(),
                    fallback_instructions: "Consider breaking the operation into smaller chunks, \
                        increasing timeout limits, or using a more efficient approach. \
                        For network operations, implement retry logic with exponential backoff."
                        .to_string(),
                },
                7,
            )
        } else if snippet_lower.contains("connection refused")
            || snippet_lower.contains("network")
            || snippet_lower.contains("unreachable")
        {
            (
                PatchType::FallbackStrategy {
                    trigger_condition: "Network connectivity issues".to_string(),
                    fallback_instructions:
                        "Check network connectivity before attempting network operations. \
                        Implement retry logic with delays. Provide clear error messages about \
                        network requirements."
                            .to_string(),
                },
                7,
            )
        } else if snippet_lower.contains("syntax error")
            || snippet_lower.contains("parse error")
            || snippet_lower.contains("invalid")
        {
            (
                PatchType::ErrorHandling {
                    failure_pattern: failure.error_snippet.clone(),
                    handling_instructions: "Validate input format before processing. \
                        Provide clear examples of expected input format. \
                        Add detailed error messages that explain what went wrong and how to fix it."
                        .to_string(),
                },
                6,
            )
        } else if snippet_lower.contains("memory")
            || snippet_lower.contains("oom")
            || snippet_lower.contains("out of memory")
        {
            (
                PatchType::FallbackStrategy {
                    trigger_condition: "Memory exhaustion or resource limits".to_string(),
                    fallback_instructions: "Process data in smaller batches. \
                        Use streaming where possible. \
                        Monitor memory usage and provide warnings before hitting limits."
                        .to_string(),
                },
                8,
            )
        } else {
            // Generic error handling for unrecognized patterns
            (
                PatchType::ErrorHandling {
                    failure_pattern: failure.error_snippet.clone(),
                    handling_instructions: format!(
                        "Add error handling for: '{}'. \
                        Check common causes, provide troubleshooting steps, \
                        and suggest alternative approaches.",
                        failure.error_snippet
                    ),
                },
                5,
            )
        };

        let rationale = format!(
            "This failure has occurred {} time{}. Recent occurrence: {}",
            failure.count,
            if failure.count == 1 { "" } else { "s" },
            failure.error_snippet
        );

        Some(SkillPatch {
            skill_name: skill_name.to_string(),
            patch_type,
            rationale,
            confidence,
            priority,
            proposed_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_state::memory::store::InMemoryStore;
    use echo_state::skill_telemetry::SkillExecutionRecord;

    fn make_record(skill_name: &str, success: bool, error: Option<&str>) -> SkillExecutionRecord {
        SkillExecutionRecord {
            skill_name: skill_name.to_string(),
            session_id: "test-session".to_string(),
            activated_at: Utc::now().timestamp_millis() as u64,
            duration_ms: 1000,
            tools_used: vec!["Bash".to_string()],
            tool_calls_count: 1,
            success,
            error_message: error.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn test_patch_for_permission_denied() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let patcher = SkillPatcher::new(store.clone());

        // Record permission denied errors
        let telemetry_store = SkillTelemetryStore::new(store.clone());
        for _ in 0..5 {
            telemetry_store
                .record_execution(&make_record(
                    "test-skill",
                    false,
                    Some("Permission denied: cannot write to /etc/config"),
                ))
                .await
                .unwrap();
        }

        let patches = patcher.analyze_and_propose("test-skill").await.unwrap();
        assert!(!patches.is_empty());

        let patch = &patches[0];
        assert!(matches!(
            patch.patch_type,
            PatchType::PrerequisiteCheck { .. }
        ));
        assert!(patch.priority >= 8);
        assert!(patch.confidence >= 0.8);
    }

    #[tokio::test]
    async fn test_patch_for_timeout() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let patcher = SkillPatcher::new(store.clone());

        let telemetry_store = SkillTelemetryStore::new(store.clone());
        for _ in 0..3 {
            telemetry_store
                .record_execution(&make_record(
                    "test-skill",
                    false,
                    Some("Operation timed out after 30 seconds"),
                ))
                .await
                .unwrap();
        }

        let patches = patcher.analyze_and_propose("test-skill").await.unwrap();
        assert!(!patches.is_empty());

        let patch = &patches[0];
        assert!(matches!(
            patch.patch_type,
            PatchType::FallbackStrategy { .. }
        ));
    }

    #[tokio::test]
    async fn test_patch_for_network_error() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let patcher = SkillPatcher::new(store.clone());

        let telemetry_store = SkillTelemetryStore::new(store.clone());
        telemetry_store
            .record_execution(&make_record(
                "test-skill",
                false,
                Some("Connection refused: network unreachable"),
            ))
            .await
            .unwrap();

        let patches = patcher.analyze_and_propose("test-skill").await.unwrap();
        assert!(!patches.is_empty());

        let patch = &patches[0];
        assert!(matches!(
            patch.patch_type,
            PatchType::FallbackStrategy { .. }
        ));
    }

    #[tokio::test]
    async fn test_no_patches_for_successful_skill() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let patcher = SkillPatcher::new(store.clone());

        let telemetry_store = SkillTelemetryStore::new(store.clone());
        for _ in 0..10 {
            telemetry_store
                .record_execution(&make_record("test-skill", true, None))
                .await
                .unwrap();
        }

        let patches = patcher.analyze_and_propose("test-skill").await.unwrap();
        assert!(patches.is_empty());
    }

    #[tokio::test]
    async fn test_patches_sorted_by_priority() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let patcher = SkillPatcher::new(store.clone());

        let telemetry_store = SkillTelemetryStore::new(store.clone());

        // Low priority error
        telemetry_store
            .record_execution(&make_record(
                "test-skill",
                false,
                Some("Unknown error occurred"),
            ))
            .await
            .unwrap();

        // High priority error
        for _ in 0..5 {
            telemetry_store
                .record_execution(&make_record("test-skill", false, Some("Permission denied")))
                .await
                .unwrap();
        }

        let patches = patcher.analyze_and_propose("test-skill").await.unwrap();
        assert!(patches.len() >= 2);

        // First patch should have higher or equal priority
        assert!(patches[0].priority >= patches[1].priority);
    }

    #[test]
    fn test_patch_summary_error_handling() {
        let patch = SkillPatch {
            skill_name: "test".to_string(),
            patch_type: PatchType::ErrorHandling {
                failure_pattern: "file not found".to_string(),
                handling_instructions: "Check file exists first".to_string(),
            },
            rationale: "Common failure".to_string(),
            confidence: 0.85,
            priority: 7,
            proposed_at: Utc::now(),
        };

        let summary = patch.summary();
        assert!(summary.contains("file not found"));
        assert!(summary.contains("85%"));
        assert!(summary.contains("Check file exists first"));
    }

    #[test]
    fn test_patch_summary_prerequisite() {
        let patch = SkillPatch {
            skill_name: "test".to_string(),
            patch_type: PatchType::PrerequisiteCheck {
                check_description: "Verify permissions".to_string(),
                verification_steps: vec![
                    "Check ownership".to_string(),
                    "Check write access".to_string(),
                ],
            },
            rationale: "Permission errors".to_string(),
            confidence: 0.9,
            priority: 9,
            proposed_at: Utc::now(),
        };

        let summary = patch.summary();
        assert!(summary.contains("Verify permissions"));
        assert!(summary.contains("Check ownership"));
        assert!(summary.contains("Check write access"));
    }
}
