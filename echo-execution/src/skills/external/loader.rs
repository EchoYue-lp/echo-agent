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

use futures::future::BoxFuture;
use serde_yaml_ng::Value;
use tracing::{debug, info, warn};

use echo_core::error::{ReactError, Result};
use echo_core::sandbox::SandboxExecutor;
use echo_core::tools::filesystem::{FileEntry, FileMetadata, FileSystem, FileType};

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
    ///
    /// The decision is asynchronous so an embedding application or source SDK
    /// can remain the live policy authority without blocking the Agent runtime.
    fn allows<'a>(&'a self, descriptor: &'a SkillDescriptor) -> BoxFuture<'a, bool>;
}

/// The immutable file and execution origin of one discovered Skill.
///
/// This is runtime definition data, not a serialized Skill field or another
/// activation authority. A user/global Skill can remain local while a project
/// Skill with the same path text uses another filesystem and executor.
#[derive(Clone, Default)]
pub struct SkillFileSource {
    file_system: Option<Arc<dyn FileSystem>>,
    sandbox: Option<Arc<dyn SandboxExecutor>>,
    resource_guards: Vec<echo_core::tools::InvocationResourceGuard>,
}

impl SkillFileSource {
    /// Preserve the existing local I/O path and its optional local executor.
    pub fn local(sandbox: Option<Arc<dyn SandboxExecutor>>) -> Self {
        Self {
            file_system: None,
            sandbox,
            resource_guards: Vec::new(),
        }
    }

    /// Bind file reads and commands to resources for the same target.
    /// A missing executor never authorizes executing a target script locally.
    pub fn new(
        file_system: Arc<dyn FileSystem>,
        sandbox: Option<Arc<dyn SandboxExecutor>>,
    ) -> Self {
        Self {
            file_system: Some(file_system),
            sandbox,
            resource_guards: Vec::new(),
        }
    }

    pub fn sandbox(&self) -> Option<Arc<dyn SandboxExecutor>> {
        self.sandbox.clone()
    }

    /// Whether this source uses the original local filesystem path.
    pub fn is_local(&self) -> bool {
        self.file_system.is_none()
    }

    pub(crate) fn requires_executor(&self) -> bool {
        !self.is_local()
    }

    /// The original methodology projection is synchronous. Custom providers
    /// require the caller's existing blocking preparation boundary; this does
    /// not create a runtime or call Handle::block_on from an async thread.
    pub(crate) fn read_to_string_blocking(&self, path: &Path) -> Result<String> {
        match &self.file_system {
            Some(file_system) => file_system.read_to_string(path),
            None => Ok(std::fs::read_to_string(path)?),
        }
    }

    pub(crate) fn for_invocation(&self, context: Option<&echo_core::tools::ToolContext>) -> Self {
        let mut source = self.clone();
        source.resource_guards = context
            .map(|ctx| ctx.resource_guards.clone())
            .unwrap_or_default();
        source
    }

    async fn blocking_operation<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        // Cancelling a waiter does not cancel already submitted file I/O.
        let guards = self.resource_guards.clone();
        tokio::task::spawn_blocking(move || {
            let _guards = guards;
            operation()
        })
        .await
        .map_err(|error| ReactError::Other(format!("Skill file operation failed: {error}")))?
    }

    async fn on_file_system<T: Send + 'static>(
        &self,
        file_system: Arc<dyn FileSystem>,
        operation: impl FnOnce(&dyn FileSystem) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.blocking_operation(move || operation(file_system.as_ref()))
            .await
    }

    pub(crate) async fn metadata(&self, path: &Path) -> Result<FileMetadata> {
        if let Some(file_system) = &self.file_system {
            let path = path.to_path_buf();
            return self
                .on_file_system(file_system.clone(), move |fs| fs.metadata(&path))
                .await;
        }
        let path = path.to_path_buf();
        self.blocking_operation(move || {
            let metadata = std::fs::metadata(path)?;
            let file_type = if metadata.is_dir() {
                FileType::Directory
            } else if metadata.is_file() {
                FileType::File
            } else {
                FileType::Other
            };
            Ok(FileMetadata {
                file_type,
                len: metadata.len(),
                mode: None,
            })
        })
        .await
    }

    pub(crate) async fn is_file(&self, path: &Path) -> Result<bool> {
        match self.metadata(path).await {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn is_dir(&self, path: &Path) -> Result<bool> {
        match self.metadata(path).await {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(ReactError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn read_to_string(&self, path: &Path) -> Result<String> {
        if let Some(file_system) = &self.file_system {
            let path = path.to_path_buf();
            return self
                .on_file_system(file_system.clone(), move |fs| fs.read_to_string(&path))
                .await;
        }
        let path = path.to_path_buf();
        self.blocking_operation(move || Ok(std::fs::read_to_string(path)?))
            .await
    }

    pub(crate) async fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        if let Some(file_system) = &self.file_system {
            let path = path.to_path_buf();
            return self
                .on_file_system(file_system.clone(), move |fs| fs.canonicalize(&path))
                .await;
        }
        let path = path.to_path_buf();
        self.blocking_operation(move || Ok(std::fs::canonicalize(path)?))
            .await
    }

    pub(crate) async fn read_dir(&self, path: &Path) -> Result<Vec<FileEntry>> {
        if let Some(file_system) = &self.file_system {
            let path = path.to_path_buf();
            return self
                .on_file_system(file_system.clone(), move |fs| fs.read_dir(&path))
                .await;
        }
        let path = path.to_path_buf();
        self.blocking_operation(move || {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                let file_type = if kind.is_dir() {
                    FileType::Directory
                } else if kind.is_file() {
                    FileType::File
                } else if kind.is_symlink() {
                    FileType::Symlink
                } else {
                    FileType::Other
                };
                entries.push(FileEntry {
                    path: entry.path(),
                    file_name: entry.file_name(),
                    file_type,
                });
            }
            Ok(entries)
        })
        .await
    }
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
    local_source: SkillFileSource,
    project_source: Option<SkillFileSource>,
    custom_source: Option<SkillFileSource>,
    file_sources: HashMap<String, SkillFileSource>,
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
            local_source: SkillFileSource::default(),
            project_source: None,
            custom_source: None,
            file_sources: HashMap::new(),
        }
    }

    /// Keep User/default Custom I/O local, with its own optional local executor.
    pub fn with_local_sandbox(mut self, sandbox: Option<Arc<dyn SandboxExecutor>>) -> Self {
        self.local_source = SkillFileSource::local(sandbox);
        self
    }

    /// Project scopes use this source; User and ordinary Custom stay local.
    pub fn with_project_source(mut self, source: SkillFileSource) -> Self {
        self.project_source = Some(source);
        self
    }

    /// Explicitly bind Custom/plugin directories without changing User scope.
    pub fn with_custom_source(mut self, source: SkillFileSource) -> Self {
        self.custom_source = Some(source);
        self
    }

    pub fn get_file_source(&self, name: &str) -> Option<&SkillFileSource> {
        self.file_sources.get(name)
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
            let source = match scope {
                DiscoveryScope::Project(_) => {
                    self.project_source.as_ref().unwrap_or(&self.local_source)
                }
                DiscoveryScope::Custom(_) => {
                    self.custom_source.as_ref().unwrap_or(&self.local_source)
                }
                DiscoveryScope::User => &self.local_source,
            }
            .clone();
            let dirs = scope_to_dirs(scope);
            for dir in dirs {
                if !source.is_dir(&dir).await? {
                    debug!(
                        "Skill directory does not exist, skipping: {}",
                        dir.display()
                    );
                    continue;
                }
                let found = self.scan_directory(&dir, 0, true, &source).await?;
                for (document, identity_documents) in found {
                    let desc = document.descriptor().clone();
                    if let Some(policy) = &self.policy
                        && !policy.allows(&desc).await
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
                            .insert(name.clone(), identity_documents);
                        self.file_sources.insert(name, source.clone());
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
        let source = self
            .custom_source
            .as_ref()
            .unwrap_or(&self.local_source)
            .clone();
        if !source.is_dir(&dir).await? {
            return Ok(Vec::new());
        }
        let found = self.scan_directory(&dir, 0, false, &source).await?;
        let mut results = Vec::new();
        for (document, identity_documents) in found {
            let desc = document.descriptor().clone();
            if let Some(policy) = &self.policy
                && !policy.allows(&desc).await
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
                .insert(name.clone(), identity_documents);
            self.file_sources.insert(name, source.clone());
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
        source: &SkillFileSource,
    ) -> Result<Vec<(SkillDocument, Vec<(PathBuf, String)>)>> {
        if depth > MAX_SCAN_DEPTH {
            return Ok(vec![]);
        }

        let mut found = Vec::new();

        let entries = source.read_dir(dir).await?;

        for entry in entries {
            let path = entry.path();
            if !entry.file_type().is_dir() {
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
            if source.is_file(&skill_file).await? {
                let private_hook_file = path.join("hooks.json");
                if source.is_file(&private_hook_file).await? {
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
                // Source failures abort discovery before a later scope can
                // replace this target with a same-name local Skill. Only an
                // actually read document's format/name errors are skippable.
                let content = source.read_to_string(&skill_file).await?;
                let location = source.canonicalize(&skill_file).await?;
                match parse_skill_content_with_variables(
                    &content,
                    location,
                    &dir_name,
                    self.plugin_variables.as_ref(),
                ) {
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
                let nested = Box::pin(self.scan_directory(&path, depth + 1, true, source)).await?;
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

/// Parse already-read SKILL.md content into metadata and its prepared document.
///
/// Enforces the file-format parts of agentskills.io validation:
/// - Name must match the containing skill directory
/// - Name must satisfy the lowercase kebab-case constraints
/// - Description must be present and non-empty
/// - Unknown fields and invalid YAML are rejected
///
fn parse_skill_content_with_variables(
    content: &str,
    location: PathBuf,
    parent_dir_name: &str,
    variables: Option<&echo_core::plugin::PluginVariables>,
) -> Result<SkillDocument> {
    let content = match variables {
        Some(variables) => variables.substitute(content),
        None => content.to_string(),
    };
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
            fn allows<'a>(&'a self, descriptor: &'a SkillDescriptor) -> BoxFuture<'a, bool> {
                Box::pin(async move { descriptor.name != "blocked" })
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

#[cfg(test)]
mod file_source_tests {
    use super::*;
    use crate::skills::external::activate_tool::ActivateSkillTool;
    use crate::skills::external::resource_tool::ReadSkillResourceTool;
    use crate::skills::external::run_script_tool::RunSkillScriptTool;
    use crate::skills::registry::SkillRegistry;
    use echo_core::sandbox::{CommandKind, ExecutionResult, IsolationLevel, SandboxCommand};
    use echo_core::tools::{InvocationResourceGuard, Tool, ToolContext, ToolParameters};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::RwLock;

    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new() -> Result<Self> {
            let root =
                std::env::temp_dir().join(format!("echo-skill-source-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root)?;
            Ok(Self(root))
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An in-memory target with the same path text as a different local file.
    /// No test transport, production provider or filesystem algorithm is added.
    struct TargetFiles {
        root: PathBuf,
        files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
        fail_reads: AtomicBool,
        fail_canonicalize: AtomicBool,
        body_reads: AtomicUsize,
    }
    impl TargetFiles {
        fn new(root: PathBuf) -> Self {
            Self {
                root,
                files: Mutex::new(BTreeMap::new()),
                fail_reads: AtomicBool::new(false),
                fail_canonicalize: AtomicBool::new(false),
                body_reads: AtomicUsize::new(0),
            }
        }
        fn put(&self, path: PathBuf, text: &str) {
            self.files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path, text.as_bytes().to_vec());
        }
        fn disconnected() -> ReactError {
            echo_core::error::ToolError::RemoteFailure {
                tool: "target-file-source".into(),
                message: "target disconnected".into(),
                http_status: None,
                failure: Box::new(echo_core::tools::ToolFailure::new(
                    echo_core::tools::ToolFailureCategory::Unavailable,
                )),
            }
            .into()
        }
        fn absent() -> ReactError {
            std::io::Error::new(std::io::ErrorKind::NotFound, "not in target").into()
        }
        fn readonly() -> ReactError {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "read-only test target",
            )
            .into()
        }
    }
    impl FileSystem for TargetFiles {
        fn current_dir(&self) -> Result<PathBuf> {
            Ok(self.root.clone())
        }
        fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
            self.metadata(path)?;
            if self.fail_canonicalize.load(Ordering::SeqCst) {
                return Err(Self::disconnected());
            }
            Ok(path.to_path_buf())
        }
        fn metadata(&self, path: &Path) -> Result<FileMetadata> {
            let files = self.files.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(bytes) = files.get(path) {
                return Ok(FileMetadata {
                    file_type: FileType::File,
                    len: bytes.len() as u64,
                    mode: None,
                });
            }
            if files.keys().any(|entry| entry.starts_with(path)) {
                return Ok(FileMetadata {
                    file_type: FileType::Directory,
                    len: 0,
                    mode: None,
                });
            }
            Err(Self::absent())
        }
        fn symlink_metadata(&self, path: &Path) -> Result<FileMetadata> {
            self.metadata(path)
        }
        fn read(&self, path: &Path) -> Result<Vec<u8>> {
            self.body_reads.fetch_add(1, Ordering::SeqCst);
            if self.fail_reads.load(Ordering::SeqCst) {
                return Err(Self::disconnected());
            }
            self.files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(path)
                .cloned()
                .ok_or_else(Self::absent)
        }
        fn read_dir(&self, path: &Path) -> Result<Vec<FileEntry>> {
            if !self.metadata(path)?.is_dir() {
                return Err(Self::absent());
            }
            let paths = self
                .files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let mut entries = BTreeMap::new();
            for file in paths {
                let Ok(relative) = file.strip_prefix(path) else {
                    continue;
                };
                let Some(name) = relative.components().next() else {
                    continue;
                };
                let entry_path = path.join(name.as_os_str());
                let metadata = self.metadata(&entry_path)?;
                entries.insert(
                    entry_path.clone(),
                    FileEntry {
                        path: entry_path,
                        file_name: name.as_os_str().to_os_string(),
                        file_type: metadata.file_type,
                    },
                );
            }
            Ok(entries.into_values().collect())
        }
        fn create_dir_all(&self, _: &Path) -> Result<()> {
            Err(Self::readonly())
        }
        fn create_file(&self, _: &Path) -> Result<()> {
            Err(Self::readonly())
        }
        fn write(&self, _: &Path, _: &[u8]) -> Result<()> {
            Err(Self::readonly())
        }
        fn append(&self, _: &Path, _: &[u8]) -> Result<()> {
            Err(Self::readonly())
        }
        fn rename(&self, _: &Path, _: &Path) -> Result<()> {
            Err(Self::readonly())
        }
        fn remove_file(&self, _: &Path) -> Result<()> {
            Err(Self::readonly())
        }
        fn remove_dir(&self, _: &Path) -> Result<()> {
            Err(Self::readonly())
        }
        fn remove_dir_all(&self, _: &Path) -> Result<()> {
            Err(Self::readonly())
        }
        fn set_mode(&self, _: &Path, _: u32) -> Result<()> {
            Err(Self::readonly())
        }
        fn atomic_write(&self, _: &Path, _: &[u8]) -> Result<()> {
            Err(Self::readonly())
        }
        fn checkpoint(&self, _: &Path) -> Result<Option<String>> {
            Err(Self::readonly())
        }
    }

    struct TargetExecutor {
        label: &'static str,
        calls: Mutex<Vec<SandboxCommand>>,
        limits: Mutex<Vec<Option<echo_core::sandbox::ResourceLimits>>>,
    }
    impl TargetExecutor {
        fn new(label: &'static str) -> Self {
            Self {
                label,
                calls: Mutex::new(Vec::new()),
                limits: Mutex::new(Vec::new()),
            }
        }
        fn record(
            &self,
            command: SandboxCommand,
            limits: Option<echo_core::sandbox::ResourceLimits>,
        ) -> ExecutionResult {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(command);
            self.limits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(limits);
            ExecutionResult {
                exit_code: 0,
                stdout: self.label.to_string(),
                stderr: String::new(),
                duration: std::time::Duration::from_millis(1),
                sandbox_type: self.label.to_string(),
                timed_out: false,
                cancelled: false,
                output_truncated: false,
                stdout_bytes: self.label.len() as u64,
                stderr_bytes: 0,
            }
        }
    }
    impl SandboxExecutor for TargetExecutor {
        fn name(&self) -> &str {
            self.label
        }
        fn isolation_level(&self) -> IsolationLevel {
            IsolationLevel::Process
        }
        fn is_available(&self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }
        fn execute(&self, command: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
            Box::pin(async move { Ok(self.record(command, None)) })
        }
        fn execute_with_limits(
            &self,
            command: SandboxCommand,
            limits: echo_core::sandbox::ResourceLimits,
        ) -> BoxFuture<'_, Result<ExecutionResult>> {
            Box::pin(async move { Ok(self.record(command, Some(limits))) })
        }
    }

    fn document(name: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: actual source\n---\n{body}\n")
    }

    #[tokio::test]
    async fn discovered_origin_survives_late_activation_resource_and_script_calls() -> Result<()> {
        let root = TestRoot::new()?;
        let project = root.0.join("project");
        let skill_dir = project.join("skills/shared-skill");
        std::fs::create_dir_all(skill_dir.join("references"))?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            document("shared-skill", "LOCAL_WRONG_BODY"),
        )?;
        std::fs::write(
            skill_dir.join("references/guide.md"),
            "LOCAL_WRONG_RESOURCE",
        )?;
        let user_dir = root.0.join("user-skills/local-skill");
        std::fs::create_dir_all(user_dir.join("scripts"))?;
        std::fs::write(
            user_dir.join("SKILL.md"),
            document("local-skill", "local instructions"),
        )?;
        std::fs::write(user_dir.join("scripts/run.sh"), "printf LOCAL_SCRIPT")?;

        let files = Arc::new(TargetFiles::new(project.clone()));
        files.put(
            skill_dir.join("SKILL.md"),
            &document("shared-skill", "DISCOVERY_BODY"),
        );
        files.put(
            skill_dir.join("references/guide.md"),
            "TARGET_RESOURCE 中文",
        );
        files.put(skill_dir.join("scripts/run.sh"), "printf TARGET_SCRIPT");
        let target = Arc::new(TargetExecutor::new("TARGET_EXECUTED"));
        let local = Arc::new(TargetExecutor::new("LOCAL_EXECUTED"));
        let wrong = Arc::new(TargetExecutor::new("WRONG_HOST_EXECUTOR"));
        let mut loader = SkillLoader::new()
            .with_local_sandbox(Some(local.clone()))
            .with_project_source(SkillFileSource::new(files.clone(), Some(target.clone())));
        let descriptors = loader
            .discover(&[
                DiscoveryScope::Project(project),
                DiscoveryScope::Custom(root.0.join("user-skills")),
            ])
            .await?;
        assert_eq!(descriptors.len(), 2);
        let mut registry = SkillRegistry::new();
        registry.set_sandbox_manager(wrong.clone());
        for descriptor in descriptors {
            let source = loader
                .get_file_source(&descriptor.name)
                .cloned()
                .ok_or_else(|| ReactError::Other("missing admitted source".to_string()))?;
            registry.register_descriptor_with_source(descriptor, source);
        }
        // Discovery must not freeze ordinary Skill content or reread a local alias.
        files.put(
            skill_dir.join("SKILL.md"),
            &document("shared-skill", "LATE_TARGET_BODY !`pwd`"),
        );
        let registry = Arc::new(RwLock::new(registry));
        let context = ToolContext {
            resource_guards: vec![InvocationResourceGuard::new(())],
            cancel: Some(Arc::new(tokio_util::sync::CancellationToken::new())),
            ..Default::default()
        };
        let activation_tool =
            ActivateSkillTool::new(registry.clone(), vec!["shared-skill".to_string()]);
        let activated = activation_tool
            .execute_with_context(
                ToolParameters::from([("name".to_string(), json!("shared-skill"))]),
                &context,
            )
            .await?;
        assert!(activated.success);
        assert!(
            activated
                .output
                .contains("LATE_TARGET_BODY TARGET_EXECUTED")
        );
        assert!(!activated.output.contains("LOCAL_WRONG"));
        assert!(activated.output.contains("references/guide.md"));
        registry.read().await.activate("local-skill").await?;
        let resource = ReadSkillResourceTool::new(registry.clone())
            .execute_with_context(
                ToolParameters::from([
                    ("skill_name".to_string(), json!("shared-skill")),
                    ("path".to_string(), json!("references/guide.md")),
                ]),
                &context,
            )
            .await?;
        assert!(resource.output.contains("TARGET_RESOURCE 中文"));
        assert!(!resource.output.contains("LOCAL_WRONG_RESOURCE"));
        let scripts = RunSkillScriptTool::new(registry).with_sandbox_manager(wrong.clone());
        let malformed = scripts
            .execute(ToolParameters::from([
                ("skill_name".to_string(), json!("shared-skill")),
                ("script".to_string(), json!("scripts/run.sh")),
                ("args".to_string(), json!("\"unterminated")),
            ]))
            .await
            .err()
            .ok_or_else(|| {
                ReactError::Other("malformed args started a target process".to_string())
            })?;
        assert!(malformed.to_string().contains("Malformed script arguments"));
        assert_eq!(
            target.calls.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1
        );
        for (name, marker) in [
            ("shared-skill", "TARGET_EXECUTED"),
            ("local-skill", "LOCAL_EXECUTED"),
        ] {
            let result = scripts
                .execute_with_context(
                    ToolParameters::from([
                        ("skill_name".to_string(), json!(name)),
                        ("script".to_string(), json!("scripts/run.sh")),
                    ]),
                    &context,
                )
                .await?;
            assert!(result.success);
            assert!(result.output.contains(marker));
        }
        let target_calls = target.calls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(target_calls.len(), 3); // inline + target probe + actual script
        assert!(
            target_calls
                .iter()
                .all(|command| command.resource_guards.len() == 1 && command.cancel.is_some())
        );
        assert!(target_calls.iter().all(
            |command| !command.env.contains_key("HOME") && !command.env.contains_key("TMPDIR")
        ));
        assert!(
            target_calls
                .iter()
                .all(|command| command.working_dir.as_deref() == Some(skill_dir.as_path()))
        );
        assert!(target_calls.iter().any(|command| matches!(&command.kind, CommandKind::Program { args, .. }
            if args.iter().any(|arg| arg == &skill_dir.join("scripts/run.sh").display().to_string()))));
        assert_eq!(
            local.calls.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1
        );
        assert!(
            wrong
                .calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn disconnected_source_does_not_read_same_name_local_skill_or_resource() -> Result<()> {
        let root = TestRoot::new()?;
        let skill_dir = root.0.join("offline-skill");
        std::fs::create_dir_all(skill_dir.join("references"))?;
        std::fs::write(
            skill_dir.join("SKILL.md"),
            document("offline-skill", "LOCAL_FALLBACK"),
        )?;
        std::fs::write(
            skill_dir.join("references/guide.md"),
            "LOCAL_FALLBACK_RESOURCE",
        )?;
        let files = Arc::new(TargetFiles::new(root.0.clone()));
        files.put(
            skill_dir.join("SKILL.md"),
            &document("offline-skill", "target body"),
        );
        files.put(skill_dir.join("references/guide.md"), "target resource");
        let mut loader =
            SkillLoader::new().with_custom_source(SkillFileSource::new(files.clone(), None));
        let descriptors = loader.discover_directory(root.0.clone()).await?;
        let mut registry = SkillRegistry::new();
        for descriptor in descriptors {
            let source = loader
                .get_file_source(&descriptor.name)
                .cloned()
                .ok_or_else(|| ReactError::Other("missing target source".to_string()))?;
            registry.register_descriptor_with_source(descriptor, source);
        }
        files.fail_reads.store(true, Ordering::SeqCst);
        let error = registry
            .activate("offline-skill")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("activation used local fallback".to_string()))?;
        assert!(error.to_string().contains("target disconnected"));
        // Resource path is tested after explicit restore, independent of late-read failure.
        registry.reset_activation_state();
        assert!(registry.mark_activated("offline-skill"));
        let tool = ReadSkillResourceTool::new(Arc::new(RwLock::new(registry)));
        let error = tool
            .execute(ToolParameters::from([
                ("skill_name".to_string(), json!("offline-skill")),
                ("path".to_string(), json!("references/guide.md")),
            ]))
            .await
            .err()
            .ok_or_else(|| ReactError::Other("resource used local fallback".to_string()))?;
        assert!(error.to_string().contains("target disconnected"));
        Ok(())
    }

    #[tokio::test]
    async fn target_source_without_executor_never_runs_inline_or_script_locally() -> Result<()> {
        let root = TestRoot::new()?;
        let skill_dir = root.0.join("missing-executor");
        let marker = root.0.join("must-not-exist");
        std::fs::create_dir_all(skill_dir.join("scripts"))?;
        std::fs::write(
            skill_dir.join("scripts/run.sh"),
            format!("touch '{}'", marker.display()),
        )?;
        let files = Arc::new(TargetFiles::new(root.0.clone()));
        files.put(
            skill_dir.join("SKILL.md"),
            &document(
                "missing-executor",
                &format!("!`touch '{}'`", marker.display()),
            ),
        );
        files.put(skill_dir.join("scripts/run.sh"), "touch must-not-exist");
        let mut loader = SkillLoader::new().with_custom_source(SkillFileSource::new(files, None));
        let descriptors = loader.discover_directory(root.0.clone()).await?;
        let mut registry = SkillRegistry::new();
        let wrong = Arc::new(TargetExecutor::new("WRONG"));
        registry.set_sandbox_manager(wrong.clone());
        for descriptor in descriptors {
            let source = loader
                .get_file_source(&descriptor.name)
                .cloned()
                .ok_or_else(|| ReactError::Other("missing target source".to_string()))?;
            registry.register_descriptor_with_source(descriptor, source);
        }
        let error = registry
            .activate("missing-executor")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("inline command ran without executor".to_string()))?;
        assert!(error.to_string().contains("requires a matching executor"));
        registry.reset_activation_state();
        assert!(registry.mark_activated("missing-executor"));
        let tool = RunSkillScriptTool::new(Arc::new(RwLock::new(registry)))
            .with_sandbox_manager(wrong.clone());
        let error = tool
            .execute(ToolParameters::from([
                ("skill_name".to_string(), json!("missing-executor")),
                ("script".to_string(), json!("scripts/run.sh")),
            ]))
            .await
            .err()
            .ok_or_else(|| ReactError::Other("script ran without executor".to_string()))?;
        assert!(
            error
                .to_string()
                .contains("requires a matching script executor")
        );
        assert!(!marker.exists());
        assert!(
            wrong
                .calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn inline_command_keeps_target_cwd_when_no_local_directory_exists() -> Result<()> {
        let root = TestRoot::new()?;
        let skill_dir = root.0.join("target-only");
        assert!(!skill_dir.exists());
        let files = Arc::new(TargetFiles::new(root.0.clone()));
        files.put(
            skill_dir.join("SKILL.md"),
            &document("target-only", "Where: !`pwd`"),
        );
        let executor = Arc::new(TargetExecutor::new("TARGET_CWD"));
        let mut loader = SkillLoader::new()
            .with_custom_source(SkillFileSource::new(files, Some(executor.clone())));
        let descriptors = loader.discover_directory(root.0.clone()).await?;
        let mut registry = SkillRegistry::new();
        for descriptor in descriptors {
            let source = loader
                .get_file_source(&descriptor.name)
                .cloned()
                .ok_or_else(|| ReactError::Other("missing target source".to_string()))?;
            registry.register_descriptor_with_source(descriptor, source);
        }
        assert!(
            registry
                .activate("target-only")
                .await?
                .instructions
                .contains("TARGET_CWD")
        );
        let calls = executor
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls
                .first()
                .and_then(|command| command.working_dir.as_deref()),
            Some(skill_dir.as_path())
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_file_waiter_keeps_invocation_guard_until_blocking_operation_ends()
    -> Result<()> {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let files = Arc::new(TargetFiles::new(PathBuf::from("/target")));
        let dropped = Arc::new(AtomicBool::new(false));
        let context = ToolContext {
            resource_guards: vec![InvocationResourceGuard::new(Dropped(dropped.clone()))],
            ..Default::default()
        };
        let source = SkillFileSource::new(files.clone(), None).for_invocation(Some(&context));
        drop(context);
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            source
                .on_file_system(files, move |_| {
                    let _ = entered_tx.send(());
                    release_rx
                        .recv()
                        .map_err(|error| ReactError::Other(error.to_string()))?;
                    Ok(())
                })
                .await
        });
        entered_rx
            .await
            .map_err(|error| ReactError::Other(error.to_string()))?;
        task.abort();
        let _ = task.await;
        let retained = !dropped.load(Ordering::SeqCst);
        release_tx
            .send(())
            .map_err(|error| ReactError::Other(error.to_string()))?;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|error| ReactError::Other(error.to_string()))?;
        assert!(
            retained,
            "observer cancellation released an unsettled physical file operation"
        );
        Ok(())
    }

    #[tokio::test]
    async fn uncertain_target_inline_effect_poison_original_activation_and_cannot_replay()
    -> Result<()> {
        struct LostResult(std::sync::atomic::AtomicUsize);
        impl SandboxExecutor for LostResult {
            fn name(&self) -> &str {
                "lost-result"
            }
            fn isolation_level(&self) -> IsolationLevel {
                IsolationLevel::Process
            }
            fn is_available(&self) -> BoxFuture<'_, bool> {
                Box::pin(async { true })
            }
            fn execute(&self, _: SandboxCommand) -> BoxFuture<'_, Result<ExecutionResult>> {
                Box::pin(async move {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    Err(echo_core::error::ToolError::RemoteFailure {
                        tool: "skill-inline".to_string(),
                        message: "target result lost".to_string(),
                        http_status: None,
                        failure: Box::new(echo_core::tools::ToolFailure {
                            category: echo_core::tools::ToolFailureCategory::PartialSideEffect,
                            recovery: echo_core::tools::ToolRecoveryAction::VerifyThenRetry,
                            side_effect: echo_core::tools::ToolSideEffect::Possible,
                            retry_after_ms: None,
                            idempotency_key: None,
                            postcondition: None,
                        }),
                    }
                    .into())
                })
            }
        }
        let root = TestRoot::new()?;
        let skill_dir = root.0.join("uncertain-skill");
        let files = Arc::new(TargetFiles::new(root.0.clone()));
        files.put(
            skill_dir.join("SKILL.md"),
            &document("uncertain-skill", "!`write-result`"),
        );
        let executor = Arc::new(LostResult(std::sync::atomic::AtomicUsize::new(0)));
        let mut loader = SkillLoader::new()
            .with_custom_source(SkillFileSource::new(files, Some(executor.clone())));
        let descriptors = loader.discover_directory(root.0.clone()).await?;
        let mut registry = SkillRegistry::new();
        for descriptor in descriptors {
            let source = loader
                .get_file_source(&descriptor.name)
                .cloned()
                .ok_or_else(|| ReactError::Other("missing target source".to_string()))?;
            registry.register_descriptor_with_source(descriptor, source);
        }
        let first = registry
            .activate("uncertain-skill")
            .await
            .err()
            .ok_or_else(|| ReactError::Other("unknown effect was published active".to_string()))?;
        assert!(first.to_string().contains("target result lost"));
        assert!(!registry.is_activated("uncertain-skill"));
        let retry = registry
            .activate("uncertain-skill")
            .await
            .err()
            .ok_or_else(|| {
                ReactError::Other("unknown command was automatically replayed".to_string())
            })?;
        assert!(retry.to_string().contains("unknown side-effect settlement"));
        assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        Ok(())
    }
    #[tokio::test]
    async fn discovery_backend_failure_never_admits_later_local_same_name() -> Result<()> {
        for fail_read in [true, false] {
            let root = TestRoot::new()?;
            let project = root.0.join("project");
            let target_file = project.join("skills/shared-skill/SKILL.md");
            let local = root.0.join("local-skills");
            std::fs::create_dir_all(local.join("shared-skill"))?;
            std::fs::write(
                local.join("shared-skill/SKILL.md"),
                document("shared-skill", "LOCAL_MUST_NOT_BE_ADMITTED"),
            )?;
            let files = Arc::new(TargetFiles::new(project.clone()));
            files.put(
                target_file.clone(),
                &document("shared-skill", "target body"),
            );
            assert!(files.metadata(&target_file)?.is_file());
            files.fail_reads.store(fail_read, Ordering::SeqCst);
            files.fail_canonicalize.store(!fail_read, Ordering::SeqCst);
            let mut loader =
                SkillLoader::new().with_project_source(SkillFileSource::new(files.clone(), None));
            let error = loader
                .discover(&[
                    DiscoveryScope::Project(project),
                    DiscoveryScope::Custom(local),
                ])
                .await
                .err()
                .ok_or_else(|| {
                    ReactError::Other(
                        "failed target was silently replaced by local same-name Skill".into(),
                    )
                })?;
            assert!(
                matches!(error, ReactError::Tool(error) if matches!(*error, echo_core::error::ToolError::RemoteFailure { .. }))
            );
            assert!(loader.get_descriptor("shared-skill").is_none());
            assert_eq!(loader.skill_count(), 0);
            assert!(
                loader.discovery_diagnostics().is_empty(),
                "backend errors must not be mislabeled document parse diagnostics"
            );
            assert_eq!(files.body_reads.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn malformed_document_keeps_original_parse_diagnostic_and_skip_behavior() -> Result<()> {
        let root = TestRoot::new()?;
        let project = root.0.join("project");
        let target_file = project.join("skills/shared-skill/SKILL.md");
        let local = root.0.join("local-skills");
        std::fs::create_dir_all(local.join("shared-skill"))?;
        std::fs::write(
            local.join("shared-skill/SKILL.md"),
            document("shared-skill", "VALID_LOCAL_DOCUMENT"),
        )?;
        let files = Arc::new(TargetFiles::new(project.clone()));
        files.put(
            target_file.clone(),
            "---\nname: shared-skill\n---\nmissing description",
        );
        let mut loader = SkillLoader::new().with_project_source(SkillFileSource::new(files, None));
        let descriptors = loader
            .discover(&[
                DiscoveryScope::Project(project),
                DiscoveryScope::Custom(local),
            ])
            .await?;
        assert_eq!(descriptors.len(), 1);
        assert!(
            loader
                .discovery_diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.path == target_file && diagnostic.is_error)
        );
        assert!(
            loader
                .get_file_source("shared-skill")
                .is_some_and(SkillFileSource::is_local)
        );
        assert!(
            loader
                .get_document("shared-skill")
                .is_some_and(|document| document.source().contains("VALID_LOCAL_DOCUMENT"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn original_inline_script_and_probe_inherit_executor_defaults_unless_policy_is_explicit()
    -> Result<()> {
        let root = TestRoot::new()?;
        let files = Arc::new(TargetFiles::new(root.0.clone()));
        for (name, body) in [
            ("no-policy", "Inline !`pwd`"),
            ("empty-policy", "plain body"),
            ("explicit-policy", "plain body"),
        ] {
            let dir = root.0.join(name);
            files.put(dir.join("SKILL.md"), &document(name, body));
            files.put(dir.join("scripts/run.sh"), "printf body");
        }
        let executor = Arc::new(TargetExecutor::new("CONFIGURED_NETWORK_DENIED"));
        let mut loader = SkillLoader::new()
            .with_custom_source(SkillFileSource::new(files, Some(executor.clone())));
        let descriptors = loader.discover_directory(root.0.clone()).await?;
        let mut registry = SkillRegistry::new();
        for mut descriptor in descriptors {
            let source = loader
                .get_file_source(&descriptor.name)
                .cloned()
                .ok_or_else(|| ReactError::Other("missing source".into()))?;
            if descriptor.name == "empty-policy" {
                descriptor.sandbox =
                    Some(crate::skills::external::types::SkillSandboxPolicy::default());
            } else if descriptor.name == "explicit-policy" {
                descriptor.sandbox = Some(crate::skills::external::types::SkillSandboxPolicy {
                    network: Some(false),
                    ..Default::default()
                });
            }
            registry.register_descriptor_with_source(descriptor, source);
        }
        let registry = Arc::new(RwLock::new(registry));
        let activation = ActivateSkillTool::new(
            registry.clone(),
            vec![
                "no-policy".into(),
                "empty-policy".into(),
                "explicit-policy".into(),
            ],
        );
        let scripts = RunSkillScriptTool::new(registry);
        let context = ToolContext {
            resource_guards: vec![InvocationResourceGuard::new(())],
            cancel: Some(Arc::new(tokio_util::sync::CancellationToken::new())),
            ..Default::default()
        };
        for name in ["no-policy", "empty-policy", "explicit-policy"] {
            assert!(
                activation
                    .execute_with_context(
                        ToolParameters::from([("name".into(), json!(name))]),
                        &context
                    )
                    .await?
                    .success
            );
            assert!(
                scripts
                    .execute_with_context(
                        ToolParameters::from([
                            ("skill_name".into(), json!(name)),
                            ("script".into(), json!("scripts/run.sh"))
                        ]),
                        &context
                    )
                    .await?
                    .success
            );
        }
        let calls = executor.calls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(calls.len(), 7); // default inline/probe/script, empty-policy probe/script, explicit probe/script
        assert!(
            calls
                .iter()
                .all(|command| command.resource_guards.len() == 1 && command.cancel.is_some())
        );
        drop(calls);
        let limits = executor.limits.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(limits.len(), 7);
        assert!(
            limits.iter().take(5).all(Option::is_none),
            "no-policy calls must preserve configured executor defaults"
        );
        assert!(
            limits
                .iter()
                .skip(5)
                .all(|limits| limits.as_ref().is_some_and(|limits| !limits.network)),
            "explicit policy must still enforce its declared network limit"
        );
        assert!(
            limits.iter().all(|limits| !limits
                .as_ref()
                .map(|limits| limits.network)
                .unwrap_or(false)),
            "configured network denial must not become unrestricted"
        );
        Ok(())
    }
}
