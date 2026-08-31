//! Skill catalog validation — echo-agent's equivalent of `skills-ref validate`.
//!
//! The [agentskills.io specification](https://agentskills.io/specification)
//! ships a reference validator (`skills-ref validate`). This module provides
//! the same gate as an in-process API so embedding applications and CI can
//! enforce spec compliance without an external tool:
//!
//! - frontmatter uses only official top-level fields;
//! - `allowed-tools` is a space-separated string (not a YAML sequence);
//! - `name` follows the spec pattern and matches the skill directory;
//! - `description` is present and within the 1024-character limit;
//! - `metadata` maps string keys to string values;
//! - Skill files contain only the official format; Hooks are
//!   application/runtime configuration rather than part of the Agent Skills
//!   file format.
//!
//! Violations are hard failures for a catalog gate; warnings are advisory.

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use super::types::SkillDocument;

/// Official agentskills.io top-level frontmatter fields.
pub const OFFICIAL_FRONTMATTER_FIELDS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// Maximum `description` length in Unicode scalar values (spec: 1024).
const MAX_DESCRIPTION_CHARS: usize = 1024;

/// Recommended `SKILL.md` body length (spec guidance, not a hard limit).
const RECOMMENDED_MAX_BODY_LINES: usize = 500;

/// Result of validating one skill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillValidationReport {
    /// The skill directory (or file) that was validated.
    pub path: PathBuf,
    /// Spec violations — a catalog gate fails when any is present.
    pub violations: Vec<String>,
    /// Advisory findings that do not block the gate.
    pub warnings: Vec<String>,
}

impl SkillValidationReport {
    /// Whether the skill passes the spec gate.
    pub fn is_valid(&self) -> bool {
        self.violations.is_empty()
    }

    fn violation(&mut self, message: impl Into<String>) {
        self.violations.push(message.into());
    }

    fn warning(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

/// Validate one `SKILL.md` document against the official spec.
///
/// `dir_name` is the parent directory name the spec requires `name` to match;
/// pass an empty string to skip that check for in-memory documents.
pub fn validate_skill_markdown(content: &str, dir_name: &str) -> SkillValidationReport {
    let mut report = SkillValidationReport::default();

    let yaml = match extract_frontmatter_yaml(content) {
        Ok(yaml) => yaml,
        Err(error) => {
            report.violation(error);
            return report;
        }
    };

    // Full typed parse: enforces required fields, empty description, string
    // metadata, and unknown top-level fields (deny_unknown_fields).
    let parsed = SkillDocument::parse(content);
    if let Err(error) = &parsed {
        report.violation(format!("SKILL.md does not parse: {error}"));
    }

    let root: Value = match serde_yaml_ng::from_str(yaml) {
        Ok(value) => value,
        Err(error) => {
            report.violation(format!("frontmatter YAML error: {error}"));
            return report;
        }
    };

    let Some(mapping) = root.as_mapping() else {
        report.violation("frontmatter must be a YAML mapping");
        return report;
    };

    let descriptor = parsed.as_ref().map(SkillDocument::descriptor).ok();
    for (key, value) in mapping.iter() {
        let Some(field) = key.as_str() else {
            report.violation("frontmatter contains a non-string field name");
            continue;
        };
        if !OFFICIAL_FRONTMATTER_FIELDS.contains(&field) {
            report.violation(format!(
                "non-official top-level field '{field}'; use only standard fields — routing \
                 intent belongs in description and hooks belong to application configuration"
            ));
            continue;
        }
        validate_field_value(&mut report, field, value);
    }

    if let Some(Value::String(description)) = mapping.get(Value::String("description".into()))
        && description.chars().count() > MAX_DESCRIPTION_CHARS
    {
        report.violation(format!(
            "description is {} chars, exceeding the {MAX_DESCRIPTION_CHARS}-char limit",
            description.chars().count()
        ));
    }
    if let Some(Value::String(compatibility)) = mapping.get(Value::String("compatibility".into()))
        && compatibility.chars().count() > 500
    {
        report.violation("compatibility exceeds the 500-character limit".to_string());
    }

    if let Some(descriptor) = descriptor {
        for warning in descriptor.validate_name() {
            report.violation(format!("name: {warning}"));
        }
        if !dir_name.is_empty() && descriptor.name != dir_name {
            report.violation(format!(
                "name '{}' must match parent directory '{}' per spec",
                descriptor.name, dir_name
            ));
        }
        let description_chars = descriptor.description.chars().count();
        if description_chars > MAX_DESCRIPTION_CHARS {
            report.violation(format!(
                "description is {description_chars} chars, exceeding the {MAX_DESCRIPTION_CHARS}-char limit"
            ));
        }
        if let Some(compatibility) = &descriptor.compatibility
            && compatibility.chars().count() > 500
        {
            report.violation("compatibility exceeds the 500-character limit".to_string());
        }
    }

    let trimmed_start = content;
    let body = trimmed_start
        .find("\n---")
        .and_then(|idx| trimmed_start.get(idx + 4..))
        .unwrap_or("");
    let body_lines = body.lines().count();
    if body_lines > RECOMMENDED_MAX_BODY_LINES {
        report.warning(format!(
            "SKILL.md is {body_lines} lines; spec recommends keeping it under {RECOMMENDED_MAX_BODY_LINES}"
        ));
    }

    report
}

/// Validate one skill directory and its required `SKILL.md` file.
pub fn validate_skill_dir(dir: &Path) -> SkillValidationReport {
    let mut report = SkillValidationReport {
        path: dir.to_path_buf(),
        ..SkillValidationReport::default()
    };

    let skill_file = dir.join("SKILL.md");
    if dir.join("hooks.json").is_file() {
        report.violation(
            "hooks.json is not an official Agent Skills file; configure Hooks in the host or plugin"
                .to_string(),
        );
    }
    let Some(dir_name) = dir.file_name().and_then(|name| name.to_str()) else {
        report.violation("skill directory name is not valid UTF-8");
        return report;
    };
    let content = match std::fs::read_to_string(&skill_file) {
        Ok(content) => content,
        Err(error) => {
            report.violation(format!("cannot read SKILL.md: {error}"));
            return report;
        }
    };

    let markdown_report = validate_skill_markdown(&content, dir_name);
    report.violations.extend(markdown_report.violations);
    report.warnings.extend(markdown_report.warnings);

    report
}

/// Field-level format checks that need the raw YAML value.
fn validate_field_value(report: &mut SkillValidationReport, field: &str, value: &Value) {
    match field {
        "allowed-tools" => {
            // Official format is a space-separated string. A YAML sequence is
            // the legacy layout and must be migrated.
            if value.is_sequence() {
                report.violation(
                    "allowed-tools must be a space-separated string, not a list \
                     (e.g. `allowed-tools: shell read_file apply_patch`)"
                        .to_string(),
                );
            } else if !value.is_string() {
                report.violation("allowed-tools must be a string".to_string());
            }
        }
        "metadata" => {
            let Some(mapping) = value.as_mapping() else {
                report.violation("metadata must be a mapping".to_string());
                return;
            };
            for (key, entry) in mapping.iter() {
                let Some(key) = key.as_str() else {
                    report.violation("metadata contains a non-string key".to_string());
                    continue;
                };
                if !matches!(entry, Value::String(_)) {
                    report.violation(format!(
                        "metadata.{key} must be a string value (spec maps strings to strings; \
                         no vendor namespaces)"
                    ));
                }
            }
        }
        _ => {}
    }
}

/// Extract the raw YAML text between the frontmatter delimiters.
fn extract_frontmatter_yaml(content: &str) -> Result<&str, String> {
    if !content.starts_with("---") {
        return Err("SKILL.md must begin with YAML frontmatter (---)".to_string());
    }
    let trimmed = content;
    let after_open = trimmed
        .get(3..)
        .unwrap_or("")
        .trim_start_matches('\r')
        .trim_start_matches('\n');
    let close_idx = after_open
        .find("\n---")
        .ok_or_else(|| "SKILL.md frontmatter missing closing ---".to_string())?;
    Ok(after_open.get(..close_idx).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_skill() -> String {
        "---\nname: demo-skill\ndescription: Demonstrates the official layout.\nmetadata:\n  category: demo\nallowed-tools: shell read_file\n---\n# Demo\n\nBody.\n".to_string()
    }

    #[test]
    fn official_layout_passes() {
        let report = validate_skill_markdown(&valid_skill(), "demo-skill");
        assert!(report.is_valid(), "violations: {:?}", report.violations);
    }

    #[test]
    fn legacy_top_level_fields_fail_with_migration_hint() {
        let content = "---\nname: demo-skill\ndescription: Legacy.\ntriggers:\n  - demo\n---\nBody";
        let report = validate_skill_markdown(content, "demo-skill");
        assert!(!report.is_valid());
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("non-official top-level field 'triggers'"))
        );
    }

    #[test]
    fn list_shaped_allowed_tools_fails() {
        let content = "---\nname: demo-skill\ndescription: Legacy tools.\nallowed-tools: [shell, read_file]\n---\nBody";
        let report = validate_skill_markdown(content, "demo-skill");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("space-separated"))
        );
    }

    #[test]
    fn name_directory_mismatch_fails() {
        let report = validate_skill_markdown(&valid_skill(), "other-dir");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("must match parent directory"))
        );
    }

    #[test]
    fn overlong_description_fails() {
        let long: String = "x".repeat(1025);
        let content = format!("---\nname: demo-skill\ndescription: {long}\n---\nBody");
        let report = validate_skill_markdown(&content, "demo-skill");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("1024-char limit"))
        );
    }

    #[test]
    fn frontmatter_hooks_block_fails() {
        let content = "---\nname: demo-skill\ndescription: Hooked.\nhooks:\n  Stop:\n    - hooks:\n        - type: command\n          command: echo done\n---\nBody";
        let report = validate_skill_markdown(content, "demo-skill");
        assert!(!report.is_valid());
    }

    #[test]
    fn vendor_namespace_in_metadata_fails() {
        let content = "---\nname: demo-skill\ndescription: Namespaced.\nmetadata:\n  echo-agent:\n    triggers: [demo]\n---\nBody";
        let report = validate_skill_markdown(content, "demo-skill");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("metadata.echo-agent must be a string value"))
        );
    }

    #[test]
    fn non_scalar_foreign_metadata_fails() {
        let content = "---\nname: demo-skill\ndescription: Nested.\nmetadata:\n  vendor:\n    nested: true\n---\nBody";
        let report = validate_skill_markdown(content, "demo-skill");
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.contains("metadata.vendor"))
        );
    }

    #[test]
    fn skill_dir_validates_required_skill_file() -> std::result::Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "echo_skill_validate_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let dir = root.join("demo-skill");
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());

        std::fs::write(dir.join("SKILL.md"), valid_skill()).map_err(|error| error.to_string())?;
        let ok = validate_skill_dir(&dir);
        assert!(ok.is_valid(), "violations: {:?}", ok.violations);

        std::fs::write(dir.join("hooks.json"), "{}\n").map_err(|error| error.to_string())?;
        let sidecar = validate_skill_dir(&dir);
        assert!(
            sidecar
                .violations
                .iter()
                .any(|violation| violation.contains("hooks.json"))
        );
        std::fs::remove_file(dir.join("hooks.json")).map_err(|error| error.to_string())?;
        std::fs::remove_file(dir.join("SKILL.md")).map_err(|error| error.to_string())?;
        let broken = validate_skill_dir(&dir);
        assert!(broken.violations.iter().any(|v| v.contains("SKILL.md")));
        Ok(())
    }
}
