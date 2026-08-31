//! Skill Loader -- multi-scope discovery and agentskills.io-compliant parsing.
//!
//! Supports the [agentskills.io](https://agentskills.io/specification) directory
//! convention: official frontmatter fields only; routing is description-driven
//! and per-skill files do not define private hook extensions.
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

use serde_yaml_ng::Value;
use tracing::{debug, info, warn};

use echo_core::error::{ReactError, Result};

use super::types::{RawFrontmatter, SkillDescriptor, SkillDocument};
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
/// - **Current format**: YAML frontmatter (`name`, `description` required),
///   Markdown body = instructions.
/// - **Strict file validation**: unknown fields, invalid types, name mismatch,
///   and empty descriptions reject the document before registration.
pub struct SkillLoader {
    /// Discovered descriptors keyed by skill name.
    descriptors: HashMap<String, SkillDescriptor>,
    documents: HashMap<String, SkillDocument>,
    prepared_identity_documents: HashMap<String, Vec<(PathBuf, String)>>,
    discovery_diagnostics: Vec<SkillDiscoveryDiagnostic>,
    /// Optional authority consulted after parsing and before registration.
    policy: Option<Arc<dyn SkillLoadPolicy>>,
    /// Optional plugin variable context applied before SKILL.md metadata and
    /// body are parsed.
    plugin_variables: Option<echo_core::plugin::PluginVariables>,
}

impl SkillLoader {
    pub fn new() -> Self {
        Self {
            descriptors: HashMap::new(),
            documents: HashMap::new(),
            prepared_identity_documents: HashMap::new(),
            discovery_diagnostics: Vec::new(),
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
                if !tokio::fs::metadata(&dir)
                    .await
                    .is_ok_and(|metadata| metadata.is_dir())
                {
                    debug!(
                        "Skill directory does not exist, skipping: {}",
                        dir.display()
                    );
                    continue;
                }
                let found = self.scan_directory(&dir, 0, true).await?;
                for (document, identity_documents) in found {
                    let desc = document.descriptor().clone();
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
                        let name = desc.name.clone();
                        self.descriptors.insert(name.clone(), desc.clone());
                        self.documents.insert(name.clone(), document);
                        self.prepared_identity_documents
                            .insert(name, identity_documents);
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

    /// Discover Agent Plugin Skills from immediate child directories only.
    ///
    /// Agent Plugins 1.0 fixes the component at `skills/<name>/SKILL.md` and
    /// does not recursively search category directories.
    pub async fn discover_agent_plugin_skills(
        &mut self,
        dir: impl Into<PathBuf>,
    ) -> Result<Vec<SkillDescriptor>> {
        let dir = dir.into();
        if !tokio::fs::metadata(&dir)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return Ok(Vec::new());
        }
        let found = self.scan_directory(&dir, 0, false).await?;
        let mut results = Vec::new();
        for (document, identity_documents) in found {
            let desc = document.descriptor().clone();
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
                continue;
            }
            let name = desc.name.clone();
            self.descriptors.insert(name.clone(), desc.clone());
            self.documents.insert(name.clone(), document);
            self.prepared_identity_documents
                .insert(name, identity_documents);
            results.push(desc);
        }
        validate_and_sort_dependencies(&results);
        Ok(results)
    }

    /// Discover Skills below one explicit directory.
    pub async fn discover_directory(
        &mut self,
        dir: impl Into<PathBuf>,
    ) -> Result<Vec<SkillDescriptor>> {
        self.discover(&[DiscoveryScope::Custom(dir.into())]).await
    }

    /// Scan a single directory for SKILL.md files.
    async fn scan_directory(
        &mut self,
        dir: &Path,
        depth: usize,
        recursive: bool,
    ) -> Result<Vec<(SkillDocument, Vec<(PathBuf, String)>)>> {
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
            if !entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
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
            if tokio::fs::metadata(&skill_file)
                .await
                .is_ok_and(|metadata| metadata.is_file())
            {
                let private_hook_file = path.join("hooks.json");
                if tokio::fs::metadata(&private_hook_file)
                    .await
                    .is_ok_and(|metadata| metadata.is_file())
                {
                    self.discovery_diagnostics.push(SkillDiscoveryDiagnostic {
                        path: private_hook_file.clone(),
                        message: "hooks.json is not part of the official Agent Skills file format"
                            .to_string(),
                        is_error: true,
                    });
                    warn!(
                        "Skipping skill '{}' because hooks.json is a private extension",
                        path.display()
                    );
                    continue;
                }
                match parse_skill_file_with_variables(
                    &skill_file,
                    &dir_name,
                    self.plugin_variables.as_ref(),
                )
                .await
                {
                    Ok(document) => {
                        let source = document.source().to_string();
                        info!(
                            "Discovered skill '{}' at {}",
                            document.descriptor().name,
                            skill_file.display()
                        );
                        found.push((document, vec![(skill_file.clone(), source)]));
                    }
                    Err(e) => {
                        self.discovery_diagnostics.push(SkillDiscoveryDiagnostic {
                            path: skill_file.clone(),
                            message: e.to_string(),
                            is_error: true,
                        });
                        warn!(
                            "Failed to parse '{}', skipping: {}",
                            skill_file.display(),
                            e
                        );
                    }
                }
            } else if recursive {
                // No SKILL.md in this direct subdir: recurse into it to support
                // nested category layouts (skills/<category>/<name>/SKILL.md).
                // Without this, only flat skills/<name>/SKILL.md would be found.
                // MAX_SCAN_DEPTH caps recursion to avoid infinite loops / huge trees.
                // Box::pin because async fn cannot recurse directly (Rust constraint).
                let nested = Box::pin(self.scan_directory(&path, depth + 1, true)).await?;
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

    pub fn get_document(&self, name: &str) -> Option<&SkillDocument> {
        self.documents.get(name)
    }

    pub fn get_prepared_identity_documents(&self, name: &str) -> Option<&[(PathBuf, String)]> {
        self.prepared_identity_documents
            .get(name)
            .map(Vec::as_slice)
    }

    pub fn discovery_diagnostics(&self) -> &[SkillDiscoveryDiagnostic] {
        &self.discovery_diagnostics
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiscoveryDiagnostic {
    pub path: PathBuf,
    pub message: String,
    pub is_error: bool,
}

impl Default for SkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

// -- Parsing --

/// Parse a single SKILL.md file into catalog metadata and its prepared document.
///
/// Enforces the file-format parts of agentskills.io validation:
/// - Name must match the containing skill directory
/// - Name must satisfy the lowercase kebab-case constraints
/// - Description must be present and non-empty
/// - Unknown fields and invalid YAML are rejected
///
async fn parse_skill_file_with_variables(
    path: &Path,
    parent_dir_name: &str,
    variables: Option<&echo_core::plugin::PluginVariables>,
) -> Result<SkillDocument> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| ReactError::Other(format!("Failed to read '{}': {}", path.display(), e)))?;
    let content = match variables {
        Some(variables) => variables.substitute(&content),
        None => content,
    };

    let location = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let document = SkillDocument::parse_at(&content, location)?;
    let descriptor = document.descriptor();

    if descriptor.name != parent_dir_name {
        return Err(ReactError::Other(format!(
            "Skill name '{}' must match directory '{}' per agentskills.io",
            descriptor.name, parent_dir_name
        )));
    }

    Ok(document)
}

impl SkillDocument {
    /// Parse and validate one in-memory `SKILL.md` document.
    pub fn parse(content: &str) -> Result<Self> {
        Self::parse_at(content, PathBuf::new())
    }

    /// Parse and validate one `SKILL.md` document with its source location.
    pub fn parse_at(content: &str, location: impl Into<PathBuf>) -> Result<Self> {
        let (raw, instructions) = parse_document(content)?;
        let descriptor = raw.into_descriptor(location.into());
        validate_official_descriptor(&descriptor)?;
        Ok(Self::new(descriptor, instructions, content.to_string()))
    }
}

/// Validate official agentskills.io limits shared by every file-based entry
/// point. Keeping these checks here prevents runtime discovery, SkillsHub,
/// install, and the standalone validator from accepting different formats.
fn validate_official_descriptor(descriptor: &SkillDescriptor) -> Result<()> {
    let name_errors = descriptor.validate_name();
    if !name_errors.is_empty() {
        return Err(ReactError::Other(format!(
            "Skill '{}' violates agentskills.io name rules: {}",
            descriptor.name,
            name_errors.join("; ")
        )));
    }
    if descriptor.description.trim().is_empty() {
        return Err(ReactError::Other(
            "SKILL.md description is empty (required per spec)".to_string(),
        ));
    }
    if descriptor.description.chars().count() > 1024 {
        return Err(ReactError::Other(
            "SKILL.md description exceeds the 1024-character limit".to_string(),
        ));
    }
    if descriptor
        .compatibility
        .as_ref()
        .is_some_and(|value| value.chars().count() > 500)
    {
        return Err(ReactError::Other(
            "SKILL.md compatibility exceeds the 500-character limit".to_string(),
        ));
    }
    Ok(())
}

fn parse_document(content: &str) -> Result<(RawFrontmatter, String)> {
    if !content.starts_with("---") {
        return Err(ReactError::Other(
            "SKILL.md must begin with YAML frontmatter (---)".to_string(),
        ));
    }
    let trimmed = content;

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
    let yaml_str = after_open.get(..close_idx).unwrap_or_default();

    // Ensure there's no trailing content on the closing --- line
    // (the --- should be followed only by whitespace, \r, or \n)
    let after_close_start = after_open
        .get(close_idx.saturating_add(4)..)
        .unwrap_or_default(); // skip "\n---"
    // The first non-whitespace after "---" should be the markdown body or end of file
    // If there's text on the same line as "---", it's not a proper separator
    let close_line_end = after_close_start
        .find('\n')
        .unwrap_or(after_close_start.len());
    let close_line_remainder = after_close_start.get(..close_line_end).unwrap_or_default();
    if !close_line_remainder.trim().is_empty() {
        return Err(ReactError::Other(
            "SKILL.md frontmatter closing --- has trailing content on same line".to_string(),
        ));
    }

    validate_official_frontmatter_types(yaml_str)?;

    let raw = serde_yaml_ng::from_str(yaml_str)
        .map_err(|e| ReactError::Other(format!("SKILL.md YAML parse error: {}", e)))?;
    let instructions = after_close_start
        .get(close_line_end..)
        .unwrap_or_default()
        .trim_start_matches('\r')
        .trim_start_matches('\n')
        .to_string();
    Ok((raw, instructions))
}

/// Reject present-but-null or otherwise non-string values before serde's
/// `Option<T>` defaults can erase the distinction between omission and an
/// invalid explicit value.
fn validate_official_frontmatter_types(yaml: &str) -> Result<()> {
    let value: Value = serde_yaml_ng::from_str(yaml)
        .map_err(|error| ReactError::Other(format!("SKILL.md YAML parse error: {error}")))?;
    let Some(mapping) = value.as_mapping() else {
        return Err(ReactError::Other(
            "SKILL.md frontmatter must be a YAML mapping".to_string(),
        ));
    };

    for field in [
        "name",
        "description",
        "license",
        "compatibility",
        "allowed-tools",
    ] {
        let key = Value::String(field.to_string());
        if let Some(value) = mapping.get(&key)
            && !matches!(value, Value::String(_))
        {
            return Err(ReactError::Other(format!(
                "SKILL.md frontmatter field '{field}' must be a string"
            )));
        }
    }

    if let Some(metadata) = mapping.get(Value::String("metadata".to_string())) {
        let Some(metadata) = metadata.as_mapping() else {
            return Err(ReactError::Other(
                "SKILL.md frontmatter field 'metadata' must be a mapping".to_string(),
            ));
        };
        for (key, value) in metadata {
            if key.as_str().is_none() || !matches!(value, Value::String(_)) {
                return Err(ReactError::Other(
                    "SKILL.md metadata keys and values must be strings".to_string(),
                ));
            }
        }
    }
    Ok(())
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
    fn test_skill_document_standard() -> std::result::Result<(), String> {
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
        let document = SkillDocument::parse(content).map_err(|error| error.to_string())?;
        assert_eq!(document.descriptor().name, "pdf-processing");
        assert_eq!(document.descriptor().license, Some("Apache-2.0".into()));
        assert_eq!(
            document.instructions().trim(),
            "# PDF Processing\n\nInstructions here."
        );
        Ok(())
    }

    #[test]
    fn test_skill_document_rejects_removed_echo_agent_fields() {
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
        assert!(SkillDocument::parse(content).is_err());
    }

    #[test]
    fn test_skill_document_rejects_empty_description() {
        let content = "---\nname: test\ndescription: \"\"\n---\n";
        assert!(SkillDocument::parse(content).is_err());
    }

    #[test]
    fn test_skill_document_enforces_official_limits_and_types() {
        let long_description = "x".repeat(1025);
        let content = format!("---\nname: test\ndescription: {long_description}\n---\nBody");
        assert!(SkillDocument::parse(&content).is_err());

        let long_compatibility = "x".repeat(501);
        let content = format!(
            "---\nname: test\ndescription: Test\ncompatibility: {long_compatibility}\n---\nBody"
        );
        assert!(SkillDocument::parse(&content).is_err());

        for field in ["license", "compatibility", "allowed-tools"] {
            let content = format!("---\nname: test\ndescription: Test\n{field}: null\n---\nBody");
            assert!(SkillDocument::parse(&content).is_err());
        }
        assert!(
            SkillDocument::parse("---\nname: test\ndescription: Test\nmetadata: null\n---\nBody")
                .is_err()
        );
    }

    #[test]
    fn test_skill_document_rejects_missing_frontmatter() {
        let content = "# Just markdown";
        assert!(SkillDocument::parse(content).is_err());
    }

    #[test]
    fn test_skill_document_rejects_unclosed_frontmatter() {
        let content = "---\nname: test\ndescription: Test\n";
        assert!(SkillDocument::parse(content).is_err());
    }

    #[test]
    fn test_skill_document_uses_markdown_body_as_instructions() -> std::result::Result<(), String> {
        let content = "---\nname: test\ndescription: Test\n---\n\n# Instructions\n\nDo stuff.";
        let document = SkillDocument::parse(content).map_err(|error| error.to_string())?;
        assert_eq!(document.instructions(), "# Instructions\n\nDo stuff.");
        Ok(())
    }

    /// 验证 scan_directory 递归:嵌套布局 skills/<category>/<name>/SKILL.md
    /// 必须能被发现(B2 修复前只能找到扁平 skills/<name>/SKILL.md)。
    #[tokio::test]
    async fn scan_directory_finds_nested_category_skills()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
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
        std::fs::create_dir_all(&flat)?;
        std::fs::write(
            flat.join("SKILL.md"),
            "---\nname: coding\ndescription: flat skill\n---\nbody",
        )?;

        // 嵌套技能(root/<category>/<name>/SKILL.md)——B2 修复前扫不到
        let nested = root.join("methodology").join("brainstorming");
        std::fs::create_dir_all(&nested)?;
        std::fs::write(
            nested.join("SKILL.md"),
            "---\nname: brainstorming\ndescription: nested skill\n---\nbody",
        )?;
        // 第二个嵌套,确保多个都能扫到
        let nested2 = root.join("methodology").join("writing-plans");
        std::fs::create_dir_all(&nested2)?;
        std::fs::write(
            nested2.join("SKILL.md"),
            "---\nname: writing-plans\ndescription: nested skill 2\n---\nbody",
        )?;

        let mut loader = SkillLoader::new();
        let descs = loader.discover_directory(root.clone()).await?;

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
        Ok(())
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
        let descriptors = loader.discover_directory(&root).await?;
        assert_eq!(descriptors.len(), 1);
        assert_eq!(
            descriptors.first().map(|value| value.name.as_str()),
            Some("allowed")
        );
        Ok(())
    }

    #[tokio::test]
    async fn agent_plugin_discovery_uses_only_immediate_children()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("echo_agent_plugin_skills_{}", uuid::Uuid::new_v4()));
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());
        let direct = root.join("direct");
        let nested = root.join("category/nested");
        std::fs::create_dir_all(&direct)?;
        std::fs::create_dir_all(&nested)?;
        std::fs::write(
            direct.join("SKILL.md"),
            "---\nname: direct\ndescription: direct skill\n---\nbody",
        )?;
        std::fs::write(
            nested.join("SKILL.md"),
            "---\nname: nested\ndescription: nested skill\n---\nbody",
        )?;

        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_agent_plugin_skills(&root).await?;
        assert_eq!(descriptors.len(), 1);
        assert_eq!(
            descriptors
                .first()
                .map(|descriptor| descriptor.name.as_str()),
            Some("direct")
        );
        Ok(())
    }

    #[tokio::test]
    async fn plugin_variables_apply_to_frontmatter_and_body()
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
            "---\nname: configured-skill\ndescription: Uses ${user_config.endpoint}\n---\nBody ${user_config.endpoint}\n",
        )?;
        let variables = echo_core::plugin::PluginVariables::new(
            root.clone(),
            root.join("plugin-data/configured-plugin"),
            root.join("project"),
        )
        .with_user_config(std::collections::HashMap::from([(
            "endpoint".to_string(),
            "http://localhost:9100".to_string(),
        )]));
        let mut loader = SkillLoader::new().with_plugin_variables(variables);
        let descriptors = loader.discover_directory(&root).await?;
        let descriptor = descriptors
            .first()
            .ok_or("plugin skill was not discovered")?;
        assert_eq!(descriptor.description, "Uses http://localhost:9100");
        assert!(descriptor.hooks.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn standard_frontmatter_parses_without_private_extensions()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("echo_standard_skill_{}", uuid::Uuid::new_v4()));
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());
        let skill_dir = root.join("routed-skill");
        std::fs::create_dir_all(&skill_dir)?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: routed-skill\ndescription: Official layout with routing keywords.\nmetadata:\n  category: demo\nallowed-tools: shell read_file\n---\nBody",
        )?;
        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_directory(&root).await?;
        let descriptor = descriptors
            .first()
            .ok_or("standard skill was not discovered")?;
        assert!(descriptor.triggers.is_empty());
        assert_eq!(
            descriptor.allowed_tools,
            vec!["shell".to_string(), "read_file".to_string()]
        );
        assert!(descriptor.hooks.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn private_hook_sidecar_is_rejected_during_discovery()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("echo_private_hook_{}", uuid::Uuid::new_v4()));
        struct Guard(std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(root.clone());
        let skill_dir = root.join("private-hook-skill");
        std::fs::create_dir_all(&skill_dir)?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: private-hook-skill\ndescription: Invalid private extension.\n---\nBody",
        )?;
        std::fs::write(skill_dir.join("hooks.json"), "{}\n")?;

        let mut loader = SkillLoader::new();
        let descriptors = loader.discover_directory(&root).await?;
        assert!(descriptors.is_empty());
        assert!(
            loader
                .discovery_diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.is_error && diagnostic.path.ends_with("hooks.json"))
        );
        Ok(())
    }

    #[test]
    fn test_scope_to_dirs_project() {
        let dirs = scope_to_dirs(&DiscoveryScope::Project(PathBuf::from("/my/project")));
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs.first(), Some(&PathBuf::from("/my/project/skills")));
        assert_eq!(
            dirs.get(1),
            Some(&PathBuf::from("/my/project/.agents/skills"))
        );
    }

    #[test]
    fn test_scope_to_dirs_custom() {
        let dirs = scope_to_dirs(&DiscoveryScope::Custom(PathBuf::from("/custom/path")));
        assert_eq!(dirs, vec![PathBuf::from("/custom/path")]);
    }

    #[test]
    fn test_allowed_tools_string() -> std::result::Result<(), String> {
        let content = "---\nname: test\ndescription: Test\nallowed-tools: Bash(git:*) Read\n---\n";
        let document = SkillDocument::parse_at(content, PathBuf::from("/test/SKILL.md"))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            document.descriptor().allowed_tools,
            vec!["Bash(git:*)", "Read"]
        );
        Ok(())
    }
}
