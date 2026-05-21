//! Skill Loader -- multi-scope discovery and agentskills.io-compliant parsing.
//!
//! Supports the standard [agentskills.io](https://agentskills.io/specification) directory
//! convention as well as the legacy echo-agent SKILL.md format (auto-detected with fallback).
//!
//! # Discovery scopes
//!
//! | Scope | Paths scanned |
//! |-------|--------------|
//! | Project | `./skills/`, `./.agents/skills/` |
//! | User | `~/.agents/skills/` |
//! | Custom | Any user-specified path |
//!
//! Project-level skills override user-level skills when names collide.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use echo_core::error::{ReactError, Result};

use super::types::{RawFrontmatter, SkillDescriptor};

const SKILL_FILE: &str = "SKILL.md";
const MAX_SCAN_DEPTH: usize = 4;

/// Directories to skip during scanning.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "dist",
    "build",
];

// -- DiscoveryScope --

/// Where to scan for skills.
#[derive(Debug, Clone)]
pub enum DiscoveryScope {
    /// Project-level: `<root>/skills/` and `<root>/.agents/skills/`
    Project(PathBuf),
    /// User-level: `~/.agents/skills/`
    User,
    /// Custom path (scanned as-is)
    Custom(PathBuf),
}

// -- SkillLoader --

/// Multi-scope skill loader with agentskills.io-compliant parsing.
///
/// # Parsing behavior
///
/// - **Standard format**: YAML frontmatter (`name`, `description` required),
///   Markdown body = instructions.
/// - **Legacy format**: If frontmatter contains `instructions:` or `resources:`,
///   those are used instead of the body. A deprecation warning is logged.
/// - **Lenient validation**: Name/description issues produce warnings but don't
///   block loading (except missing `description`, which skips the skill).
pub struct SkillLoader {
    /// Discovered descriptors keyed by skill name.
    descriptors: HashMap<String, SkillDescriptor>,
    /// Legacy instructions from frontmatter, keyed by skill name.
    /// Preserved for activation when SKILL.md body is empty.
    legacy_instructions: HashMap<String, String>,
}

impl SkillLoader {
    pub fn new() -> Self {
        Self {
            descriptors: HashMap::new(),
            legacy_instructions: HashMap::new(),
        }
    }

    /// Discover skills from multiple scopes.
    ///
    /// Returns all successfully parsed `SkillDescriptor`s. Name collisions
    /// are resolved by order: earlier scopes take precedence. A warning is
    /// logged when a skill is shadowed.
    pub async fn discover(&mut self, scopes: &[DiscoveryScope]) -> Result<Vec<SkillDescriptor>> {
        let mut results = Vec::new();

        for scope in scopes {
            let dirs = scope_to_dirs(scope);
            for dir in dirs {
                if !dir.exists() {
                    debug!(
                        "Skill directory does not exist, skipping: {}",
                        dir.display()
                    );
                    continue;
                }
                let found = self.scan_directory(&dir, 0).await?;
                for (desc, legacy_instr) in found {
                    if let Some(existing) = self.descriptors.get(&desc.name) {
                        warn!(
                            "Skill '{}' at '{}' shadowed by existing at '{}'",
                            desc.name,
                            desc.location.display(),
                            existing.location.display()
                        );
                    } else {
                        if !legacy_instr.is_empty() {
                            self.legacy_instructions
                                .insert(desc.name.clone(), legacy_instr);
                        }
                        self.descriptors.insert(desc.name.clone(), desc.clone());
                        results.push(desc);
                    }
                }
            }
        }

        info!("Skill discovery complete: {} skills found", results.len());
        Ok(results)
    }

    /// Convenience: discover from a single directory path (backward-compatible).
    pub async fn discover_from_dir(
        &mut self,
        dir: impl Into<PathBuf>,
    ) -> Result<Vec<SkillDescriptor>> {
        self.discover(&[DiscoveryScope::Custom(dir.into())]).await
    }

    /// Scan a single directory for SKILL.md files.
    async fn scan_directory(
        &self,
        dir: &Path,
        depth: usize,
    ) -> Result<Vec<(SkillDescriptor, String)>> {
        if depth > MAX_SCAN_DEPTH {
            return Ok(vec![]);
        }

        let mut found = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| {
            ReactError::Other(format!("Cannot read directory '{}': {}", dir.display(), e))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ReactError::Other(format!("Error reading directory entry: {}", e)))?
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if SKIP_DIRS.contains(&dir_name.as_str()) {
                continue;
            }

            let skill_file = path.join(SKILL_FILE);
            if skill_file.exists() {
                match parse_skill_file(&skill_file, &dir_name).await {
                    Ok((desc, legacy_instr)) => {
                        info!(
                            "Discovered skill '{}' at {}",
                            desc.name,
                            skill_file.display()
                        );
                        found.push((desc, legacy_instr));
                    }
                    Err(e) => {
                        warn!(
                            "Failed to parse '{}', skipping: {}",
                            skill_file.display(),
                            e
                        );
                    }
                }
            }
        }

        Ok(found)
    }

    /// Get a descriptor by name.
    pub fn get_descriptor(&self, name: &str) -> Option<&SkillDescriptor> {
        self.descriptors.get(name)
    }

    /// List all discovered descriptors.
    pub fn list_descriptors(&self) -> Vec<&SkillDescriptor> {
        let mut descs: Vec<&SkillDescriptor> = self.descriptors.values().collect();
        descs.sort_by_key(|d| &d.name);
        descs
    }

    /// Consume the loader and return all descriptors.
    pub fn into_descriptors(self) -> Vec<SkillDescriptor> {
        let mut descs: Vec<SkillDescriptor> = self.descriptors.into_values().collect();
        descs.sort_by(|a, b| a.name.cmp(&b.name));
        descs
    }

    /// Number of discovered skills.
    pub fn skill_count(&self) -> usize {
        self.descriptors.len()
    }

    /// Get legacy instructions for a skill by name, if any.
    pub fn get_legacy_instructions(&self, name: &str) -> Option<&String> {
        self.legacy_instructions.get(name)
    }
}

impl Default for SkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

// -- Parsing --

/// Parse a single SKILL.md file into a `SkillDescriptor` and optional legacy instructions.
///
/// Implements lenient validation per agentskills.io integration guide:
/// - Name mismatch with parent directory -> warn, load anyway
/// - Name exceeds 64 chars -> warn, load anyway
/// - Description missing/empty -> skip (return error)
/// - Unparseable YAML -> skip (return error)
///
/// Returns `(descriptor, legacy_instructions)` where `legacy_instructions`
/// is empty if the skill uses the standard format.
async fn parse_skill_file(path: &Path, parent_dir_name: &str) -> Result<(SkillDescriptor, String)> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ReactError::Other(format!("Failed to read '{}': {}", path.display(), e)))?;

    let raw = parse_frontmatter(&content)?;

    // Lenient validation
    if raw.description.trim().is_empty() {
        return Err(ReactError::Other(format!(
            "Skill at '{}': description is empty (required per spec)",
            path.display()
        )));
    }

    // Extract legacy instructions before consuming raw
    let legacy_instr = raw.instructions.clone().unwrap_or_default();

    let descriptor = raw.clone().into_descriptor(
        path.to_path_buf()
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf()),
    );

    // Warn on name issues
    if descriptor.name != parent_dir_name {
        warn!(
            "Skill '{}' name does not match directory '{}' (loading anyway)",
            descriptor.name, parent_dir_name
        );
    }

    for warning in descriptor.validate_name() {
        warn!("Skill '{}': {}", descriptor.name, warning);
    }

    if raw.is_legacy_format() {
        warn!(
            "Skill '{}' uses legacy SKILL.md format (instructions/resources in frontmatter). \
             Consider migrating to agentskills.io format where the body is the instructions.",
            descriptor.name
        );
    }

    Ok((descriptor, legacy_instr))
}

/// Parse YAML frontmatter from a SKILL.md file.
///
/// Handles the common edge case of unquoted colons in values by retrying
/// with the problematic value wrapped in quotes.
/// Parse YAML frontmatter from a SKILL.md string into a `SkillDescriptor`.
///
/// Useful for manual/programmatic parsing of skill files.
pub fn parse_skill_md(content: &str) -> Result<SkillDescriptor> {
    let raw = parse_frontmatter(content)?;
    Ok(raw.into_descriptor(std::path::PathBuf::new()))
}

fn parse_frontmatter(content: &str) -> Result<RawFrontmatter> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err(ReactError::Other(
            "SKILL.md must begin with YAML frontmatter (---)".to_string(),
        ));
    }

    // Skip the opening --- and the newline after it
    let after_open = trimmed
        .get(3..)
        .unwrap_or("")
        .trim_start_matches('\r')
        .trim_start_matches('\n');

    // Find the closing --- which must be on its own line.
    // This prevents markdown horizontal rules (e.g., `---` mid-document)
    // from being mistaken for the frontmatter terminator.
    // The closing --- must appear at the start of a line.
    let close_idx = after_open
        .find("\n---")
        .ok_or_else(|| ReactError::Other("SKILL.md frontmatter missing closing ---".to_string()))?;

    // Verify the closing --- is actually at the start of a line (not mid-line)
    let yaml_str = &after_open[..close_idx];

    // Ensure there's no trailing content on the closing --- line
    // (the --- should be followed only by whitespace, \r, or \n)
    let after_close_start = &after_open[close_idx + 4..]; // skip "\n---"
    // The first non-whitespace after "---" should be the markdown body or end of file
    // If there's text on the same line as "---", it's not a proper separator
    let close_line_remainder = &after_close_start[..after_close_start
        .find('\n')
        .unwrap_or(after_close_start.len())];
    if !close_line_remainder.trim().is_empty() {
        return Err(ReactError::Other(
            "SKILL.md frontmatter closing --- has trailing content on same line".to_string(),
        ));
    }

    serde_yaml_ng::from_str(yaml_str)
        .map_err(|e| ReactError::Other(format!("SKILL.md YAML parse error: {}", e)))
}

/// Extract the Markdown body from a SKILL.md file (strip frontmatter).
///
/// If the frontmatter contains a legacy `instructions` field, returns that
/// instead of the body.
pub fn extract_instructions(content: &str) -> String {
    if let Ok(raw) = parse_frontmatter(content)
        && let Some(instructions) = raw.instructions
    {
        return instructions;
    }

    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }

    let after_open = trimmed
        .get(3..)
        .unwrap_or("")
        .trim_start_matches('\r')
        .trim_start_matches('\n');

    if let Some(close_idx) = after_open.find("\n---") {
        let after_close = &after_open[close_idx + 4..];
        after_close
            .trim_start_matches('\r')
            .trim_start_matches('\n')
            .to_string()
    } else {
        content.to_string()
    }
}

// -- Scope resolution --

/// Resolve a `DiscoveryScope` into concrete directory paths to scan.
fn scope_to_dirs(scope: &DiscoveryScope) -> Vec<PathBuf> {
    match scope {
        DiscoveryScope::Project(root) => {
            vec![root.join("skills"), root.join(".agents").join("skills")]
        }
        DiscoveryScope::User => {
            if let Some(home) = dirs::home_dir() {
                vec![home.join(".agents").join("skills")]
            } else {
                warn!("Cannot determine home directory for user-level skill discovery");
                vec![]
            }
        }
        DiscoveryScope::Custom(path) => {
            vec![path.clone()]
        }
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_standard() {
        let content = r#"---
name: pdf-processing
description: Extract PDF text, fill forms, merge files. Use when handling PDFs.
license: Apache-2.0
metadata:
  author: example-org
  version: "1.0"
---

# PDF Processing

Instructions here.
"#;
        let raw = parse_frontmatter(content).unwrap();
        assert_eq!(raw.name, "pdf-processing");
        assert_eq!(raw.license, Some("Apache-2.0".into()));
        assert!(!raw.is_legacy_format());
    }

    #[test]
    fn test_parse_frontmatter_legacy() {
        let content = r#"---
name: code_review
version: "1.0.0"
description: "Code review skill"
author: "team"
tags: [code, review]
instructions: |
  Review the code carefully.
resources:
  - name: checklist
    path: checklist.md
    description: "Review checklist"
---
"#;
        let raw = parse_frontmatter(content).unwrap();
        assert_eq!(raw.name, "code_review");
        assert!(raw.is_legacy_format());
        assert!(raw.instructions.is_some());
    }

    #[test]
    fn test_parse_frontmatter_missing_description() {
        let content = "---\nname: test\ndescription: \"\"\n---\n";
        let raw = parse_frontmatter(content).unwrap();
        assert!(raw.description.is_empty());
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# Just markdown";
        assert!(parse_frontmatter(content).is_err());
    }

    #[test]
    fn test_parse_frontmatter_unclosed() {
        let content = "---\nname: test\ndescription: Test\n";
        assert!(parse_frontmatter(content).is_err());
    }

    #[test]
    fn test_extract_instructions_body() {
        let content = "---\nname: test\ndescription: Test\n---\n\n# Instructions\n\nDo stuff.";
        let body = extract_instructions(content);
        assert_eq!(body, "# Instructions\n\nDo stuff.");
    }

    #[test]
    fn test_extract_instructions_legacy() {
        let content =
            "---\nname: test\ndescription: Test\ninstructions: |\n  Do stuff.\n---\n\n# Body";
        let body = extract_instructions(content);
        assert_eq!(body.trim(), "Do stuff.");
    }

    #[test]
    fn test_scope_to_dirs_project() {
        let dirs = scope_to_dirs(&DiscoveryScope::Project(PathBuf::from("/my/project")));
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0], PathBuf::from("/my/project/skills"));
        assert_eq!(dirs[1], PathBuf::from("/my/project/.agents/skills"));
    }

    #[test]
    fn test_scope_to_dirs_custom() {
        let dirs = scope_to_dirs(&DiscoveryScope::Custom(PathBuf::from("/custom/path")));
        assert_eq!(dirs, vec![PathBuf::from("/custom/path")]);
    }

    #[test]
    fn test_allowed_tools_string() {
        let content = "---\nname: test\ndescription: Test\nallowed-tools: Bash(git:*) Read\n---\n";
        let raw = parse_frontmatter(content).unwrap();
        let desc = raw.into_descriptor(PathBuf::from("/test/SKILL.md"));
        assert_eq!(desc.allowed_tools, vec!["Bash(git:*)", "Read"]);
    }
}
