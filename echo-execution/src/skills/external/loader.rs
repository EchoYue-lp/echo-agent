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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{debug, info, warn};

use echo_core::error::{ReactError, Result};

use super::types::{RawFrontmatter, SkillDescriptor};
use crate::skills::hooks::HooksDefinition;

const SKILL_FILE: &str = "SKILL.md";
/// Hook definition file (EKO format) alongside SKILL.md.
/// Distinct from superpowers' Claude-Code-format hooks.json; assets are
/// transcribed to this format at integration time.
const HOOKS_FILE: &str = "hooks.json";
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

/// Product-supplied authority for deciding whether a discovered skill may load.
///
/// The framework intentionally knows nothing about product lifecycle files. A
/// consumer can implement this trait using its own enabled/disabled or curator
/// state while the loader remains a reusable discovery primitive.
pub trait SkillLoadPolicy: Send + Sync {
    /// Return `true` when this descriptor may enter the runtime catalog.
    fn allows(&self, descriptor: &SkillDescriptor) -> bool;
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
    /// Optional authority consulted after parsing and before registration.
    policy: Option<Arc<dyn SkillLoadPolicy>>,
    /// Optional plugin variable context applied before SKILL.md frontmatter
    /// and adjacent hooks.json are parsed.
    plugin_variables: Option<echo_core::plugin::PluginVariables>,
}

impl SkillLoader {
    pub fn new() -> Self {
        Self {
            descriptors: HashMap::new(),
            legacy_instructions: HashMap::new(),
            policy: None,
            plugin_variables: None,
        }
    }

    /// Install a product-owned skill loading policy.
    pub fn with_policy(mut self, policy: Arc<dyn SkillLoadPolicy>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Apply plugin variables to skill metadata and hooks during discovery.
    pub fn with_plugin_variables(mut self, variables: echo_core::plugin::PluginVariables) -> Self {
        self.plugin_variables = Some(variables);
        self
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
                    if self
                        .policy
                        .as_ref()
                        .is_some_and(|policy| !policy.allows(&desc))
                    {
                        info!(
                            skill = %desc.name,
                            path = %desc.location.display(),
                            "Skill excluded by load policy"
                        );
                        continue;
                    }
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

        // Validate dependencies and topological sort
        validate_and_sort_dependencies(&results);

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
                match parse_skill_file_with_variables(
                    &skill_file,
                    &dir_name,
                    self.plugin_variables.as_ref(),
                )
                .await
                {
                    Ok((mut desc, legacy_instr)) => {
                        // Merge external hooks.json (EKO format) if present alongside SKILL.md.
                        let hooks_path = path.join(HOOKS_FILE);
                        if hooks_path.exists() {
                            match tokio::fs::read_to_string(&hooks_path).await {
                                Ok(text) => {
                                    let text = match &self.plugin_variables {
                                        Some(variables) => variables.substitute(&text),
                                        None => text,
                                    };
                                    match serde_json::from_str::<HooksDefinition>(&text) {
                                        Ok(extra) => {
                                            info!(
                                                "Merged hooks.json for skill '{}' from {}",
                                                desc.name,
                                                hooks_path.display()
                                            );
                                            match &mut desc.hooks {
                                                Some(existing) => existing.merge(extra),
                                                None => desc.hooks = Some(extra),
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to parse '{}' for skill '{}': {}",
                                                hooks_path.display(),
                                                desc.name,
                                                e
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Cannot read '{}': {}", hooks_path.display(), e);
                                }
                            }
                        }
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
            } else {
                // No SKILL.md in this direct subdir: recurse into it to support
                // nested category layouts (skills/<category>/<name>/SKILL.md).
                // Without this, only flat skills/<name>/SKILL.md would be found.
                // MAX_SCAN_DEPTH caps recursion to avoid infinite loops / huge trees.
                // Box::pin because async fn cannot recurse directly (Rust constraint).
                let nested = Box::pin(self.scan_directory(&path, depth + 1)).await?;
                found.extend(nested);
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
async fn parse_skill_file_with_variables(
    path: &Path,
    parent_dir_name: &str,
    variables: Option<&echo_core::plugin::PluginVariables>,
) -> Result<(SkillDescriptor, String)> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ReactError::Other(format!("Failed to read '{}': {}", path.display(), e)))?;
    let content = match variables {
        Some(variables) => variables.substitute(&content),
        None => content,
    };

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

// -- Dependency validation --

/// Validate skill dependencies and log warnings for issues.
///
/// Performs DFS-based cycle detection across all discovered skills.
/// Circular dependencies and missing dependencies are logged as warnings
/// but do not prevent skill loading (they are handled at activation time).
fn validate_and_sort_dependencies(skills: &[SkillDescriptor]) {
    let name_set: HashSet<&str> = skills.iter().map(|s| s.name.as_str()).collect();

    // Check for missing dependencies
    for skill in skills {
        for dep in &skill.depends_on {
            if !name_set.contains(dep.as_str()) {
                warn!(
                    "Skill '{}' depends on '{}' which is not available",
                    skill.name, dep
                );
            }
        }
    }

    // Detect circular dependencies via DFS
    let skill_deps: HashMap<&str, &[String]> = skills
        .iter()
        .map(|s| (s.name.as_str(), s.depends_on.as_slice()))
        .collect();

    let mut visited: HashSet<&str> = HashSet::new();
    let mut temp_visited: HashSet<&str> = HashSet::new();

    for skill in skills {
        detect_cycle(
            skill.name.as_str(),
            &skill_deps,
            &mut visited,
            &mut temp_visited,
        );
    }
}

/// DFS cycle detection. Logs a warning and skips the cycle edge if detected.
fn detect_cycle<'a>(
    name: &'a str,
    deps: &HashMap<&'a str, &'a [String]>,
    visited: &mut HashSet<&'a str>,
    temp_visited: &mut HashSet<&'a str>,
) {
    if visited.contains(name) {
        return;
    }
    if temp_visited.contains(name) {
        warn!("Circular dependency detected involving skill: {}", name);
        return;
    }

    temp_visited.insert(name);

    if let Some(skill_deps) = deps.get(name) {
        for dep in *skill_deps {
            detect_cycle(dep.as_str(), deps, visited, temp_visited);
        }
    }

    temp_visited.remove(name);
    visited.insert(name);
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

    /// 验证 scan_directory 递归:嵌套布局 skills/<category>/<name>/SKILL.md
    /// 必须能被发现(B2 修复前只能找到扁平 skills/<name>/SKILL.md)。
    #[tokio::test]
    async fn scan_directory_finds_nested_category_skills() {
        // 用 std 临时目录避免为单个测试增加 dev-dependency。用进程 id + 原子计数
        // 保证唯一,测试结束清理。
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let uid = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "echo_b2_test_{}_{}_{}",
            std::process::id(),
            uid,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // cleanup guard
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());

        // 扁平技能(直接 root/<name>/SKILL.md)
        let flat = root.join("coding");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(
            flat.join("SKILL.md"),
            "---\nname: coding\ndescription: flat skill\n---\nbody",
        )
        .unwrap();

        // 嵌套技能(root/<category>/<name>/SKILL.md)——B2 修复前扫不到
        let nested = root.join("methodology").join("brainstorming");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("SKILL.md"),
            "---\nname: brainstorming\ndescription: nested skill\n---\nbody",
        )
        .unwrap();
        // 第二个嵌套,确保多个都能扫到
        let nested2 = root.join("methodology").join("writing-plans");
        std::fs::create_dir_all(&nested2).unwrap();
        std::fs::write(
            nested2.join("SKILL.md"),
            "---\nname: writing-plans\ndescription: nested skill 2\n---\nbody",
        )
        .unwrap();

        let mut loader = SkillLoader::new();
        let descs = loader.discover_from_dir(root.clone()).await.unwrap();

        let names: Vec<String> = descs.iter().map(|d| d.name.clone()).collect();
        assert!(
            names.contains(&"coding".to_string()),
            "flat skill should be found: {:?}",
            names
        );
        assert!(
            names.contains(&"brainstorming".to_string()),
            "nested methodology skill must be found (B2 recursion): {:?}",
            names
        );
        assert!(
            names.contains(&"writing-plans".to_string()),
            "second nested skill must be found: {:?}",
            names
        );
        assert_eq!(descs.len(), 3, "expected 3 skills (1 flat + 2 nested)");
    }

    #[tokio::test]
    async fn load_policy_excludes_disallowed_descriptor()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        struct DenyBlocked;
        impl SkillLoadPolicy for DenyBlocked {
            fn allows(&self, descriptor: &SkillDescriptor) -> bool {
                descriptor.name != "blocked"
            }
        }

        let root = std::env::temp_dir().join(format!("echo_skill_policy_{}", uuid::Uuid::new_v4()));
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());
        for name in ["allowed", "blocked"] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name} skill\n---\nbody"),
            )?;
        }

        let mut loader = SkillLoader::new().with_policy(Arc::new(DenyBlocked));
        let descriptors = loader.discover_from_dir(&root).await?;
        assert_eq!(descriptors.len(), 1);
        assert_eq!(
            descriptors.first().map(|value| value.name.as_str()),
            Some("allowed")
        );
        Ok(())
    }

    #[tokio::test]
    async fn plugin_variables_are_applied_before_frontmatter_hooks_are_parsed()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "echo_plugin_skill_variables_{}",
            uuid::Uuid::new_v4()
        ));
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());
        let skill_dir = root.join("configured-skill");
        std::fs::create_dir_all(&skill_dir)?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: configured-skill\ndescription: Uses ${user_config.endpoint}\nhooks:\n  PreToolUse:\n    - matcher: Bash\n      hooks:\n        - type: command\n          command: notify ${user_config.endpoint}\n---\nBody ${user_config.endpoint}\n",
        )?;

        let variables = echo_core::plugin::PluginVariables::new(
            "configured-plugin",
            root.clone(),
            root.join("project"),
        )
        .with_user_config(std::collections::HashMap::from([(
            "endpoint".to_string(),
            "http://localhost:9100".to_string(),
        )]));
        let mut loader = SkillLoader::new().with_plugin_variables(variables);
        let descriptors = loader.discover_from_dir(&root).await?;
        let descriptor = descriptors
            .first()
            .ok_or("plugin skill was not discovered")?;
        assert_eq!(descriptor.description, "Uses http://localhost:9100");
        let action = descriptor
            .hooks
            .as_ref()
            .and_then(|definition| {
                definition
                    .rules_for(echo_core::hooks::HookEvent::PreToolUse)
                    .first()
            })
            .and_then(|rule| rule.hooks.first())
            .ok_or("frontmatter hook was not parsed")?;
        match action {
            crate::skills::hooks::HookAction::Command { command, .. } => {
                assert_eq!(command, "notify http://localhost:9100");
            }
            _ => return Err("expected command hook".into()),
        }
        Ok(())
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
