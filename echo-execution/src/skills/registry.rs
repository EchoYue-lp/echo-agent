//! Skill Registry -- central lifecycle manager for both code-based and file-based skills.
//!
//! Replaces the old `SkillManager` with full progressive-disclosure support:
//!
//! | Phase | What happens | Token cost |
//! |-------|-------------|------------|
//! | Discovery | `SKILL.md` frontmatter parsed -> `SkillDescriptor` | ~50-100 per skill |
//! | Catalog | Compact list injected into system prompt | sum of above |
//! | Activation | Full `SKILL.md` body loaded via `activate_skill` tool | <5000 (recommended) |
//! | Resources | Individual files loaded via `read_skill_resource` tool | varies |

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::warn;

use crate::sandbox::SandboxManager;
use crate::skills::SkillInfo;
use crate::skills::external::prompt_exec::{PromptContext, SkillSource, process_skill_content};
use crate::skills::external::types::{
    SkillContent, SkillDescriptor, SkillResourceEntry, SkillResourceKind,
};

// -- SkillRegistry --

/// Central skill lifecycle manager.
///
/// Tracks both code-based skills (registered via [`Skill`](crate::skills::Skill) trait)
/// and file-based skills (discovered from `SKILL.md` files). Provides:
///
/// - **Catalog generation** for system-prompt injection (Tier 1)
/// - **Activation tracking** with deduplication (Tier 2)
/// - **Resource access** from activated skill directories (Tier 3)
pub struct SkillRegistry {
    /// File-based skills: name -> descriptor (Tier 1 metadata)
    descriptors: HashMap<String, SkillDescriptor>,

    /// Legacy instructions parsed from frontmatter, keyed by skill name.
    /// Used during activation when the SKILL.md body is empty.
    legacy_instructions: HashMap<String, String>,

    /// Skills activated in the current session (dedup set)
    activated: HashSet<String>,

    /// Code-based skills: name -> info (registered via `add_skill`)
    code_skills: HashMap<String, SkillInfo>,

    /// Session identifier for variable substitution in skill content.
    session_id: String,

    /// Optional sandbox manager used when activating local skills with inline commands.
    sandbox: Option<Arc<SandboxManager>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        let session_id = format!("session-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        Self {
            session_id,
            descriptors: HashMap::new(),
            legacy_instructions: HashMap::new(),
            activated: HashSet::new(),
            code_skills: HashMap::new(),
            sandbox: None,
        }
    }

    // -- File-based skills (progressive disclosure) --

    /// Register a discovered file-based skill descriptor.
    pub fn register_descriptor(&mut self, descriptor: SkillDescriptor) {
        // Validate paths during registration
        for warning in descriptor.validate_paths() {
            warn!("Skill '{}': {}", descriptor.name, warning);
        }
        self.descriptors.insert(descriptor.name.clone(), descriptor);
    }

    /// Register a discovered file-based skill descriptor and its legacy instructions.
    pub fn register_descriptor_with_legacy(
        &mut self,
        descriptor: SkillDescriptor,
        legacy_instructions: Option<String>,
    ) {
        if let Some(legacy) = legacy_instructions
            && !legacy.trim().is_empty()
        {
            self.legacy_instructions
                .insert(descriptor.name.clone(), legacy);
        }
        self.register_descriptor(descriptor);
    }

    /// Attach a sandbox manager used for inline command execution during activation.
    pub fn set_sandbox_manager(&mut self, manager: Arc<SandboxManager>) {
        self.sandbox = Some(manager);
    }

    /// Get a descriptor by name.
    pub fn get_descriptor(&self, name: &str) -> Option<&SkillDescriptor> {
        self.descriptors.get(name)
    }

    /// List all discovered file-based skill descriptors.
    pub fn list_descriptors(&self) -> Vec<&SkillDescriptor> {
        let mut descs: Vec<&SkillDescriptor> = self.descriptors.values().collect();
        descs.sort_by_key(|d| &d.name);
        descs
    }

    /// Number of discovered file-based skills.
    pub fn descriptor_count(&self) -> usize {
        self.descriptors.len()
    }

    /// Generate the skill catalog text for system-prompt injection.
    ///
    /// Returns `None` if no file-based skills are available (caller should
    /// omit the catalog section entirely per spec).
    pub fn catalog_prompt(&self) -> Option<String> {
        if self.descriptors.is_empty() {
            return None;
        }

        let mut lines = Vec::with_capacity(self.descriptors.len() + 4);
        lines.push(
            "The following skills provide specialized instructions for specific tasks.\n\
             When a task matches a skill's description, call the `activate_skill` tool \
             with the skill's name to load its full instructions."
                .to_string(),
        );
        lines.push(String::new());

        let mut names: Vec<&String> = self.descriptors.keys().collect();
        names.sort();
        for name in names {
            if let Some(desc) = self.descriptors.get(name) {
                lines.push(desc.catalog_line());
            }
        }

        Some(lines.join("\n"))
    }

    /// List all available skill names (for `activate_skill` tool enum constraint).
    pub fn available_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.descriptors.keys().cloned().collect();
        names.sort();
        names
    }

    // -- Activation tracking --

    /// Mark a skill as activated. Returns `false` if already activated (dedup).
    pub fn mark_activated(&mut self, name: &str) -> bool {
        self.activated.insert(name.to_string())
    }

    /// Check whether a skill has been activated in this session.
    pub fn is_activated(&self, name: &str) -> bool {
        self.activated.contains(name)
    }

    /// Number of activated skills.
    pub fn activated_count(&self) -> usize {
        self.activated.len()
    }

    /// Activate a skill: read its full content from disk, execute inline
    /// commands, and substitute variables.
    ///
    /// Returns the structured `SkillContent` or an error.
    /// Automatically marks the skill as activated.
    pub async fn activate(&mut self, name: &str) -> echo_core::error::Result<SkillContent> {
        self.activate_with_args(name, &[], SkillSource::Local).await
    }

    /// Activate a skill with user-provided arguments and source context.
    ///
    /// This is the full activation path that:
    /// 1. Reads the `SKILL.md` body
    /// 2. Falls back to legacy frontmatter `instructions` if body is empty
    /// 3. Substitutes variables (`${SKILL_DIR}`, `${SESSION_ID}`, `${ARGUMENTS}`, etc.)
    /// 4. Executes inline commands (`` !`cmd` `` and `` ```! cmd ``` ``),
    ///    using the configured sandbox path when available, or the direct fallback
    ///    with minimal env + best-effort timeout termination otherwise
    /// 5. Enumerates bundled resources
    pub async fn activate_with_args(
        &mut self,
        name: &str,
        args: &[String],
        source: SkillSource,
    ) -> echo_core::error::Result<SkillContent> {
        let descriptor = self.descriptors.get(name).ok_or_else(|| {
            echo_core::error::ReactError::Other(format!("Skill '{}' not found in catalog", name))
        })?;

        let location = &descriptor.location;
        let skill_dir = location.parent().ok_or_else(|| {
            echo_core::error::ReactError::Other(format!(
                "Cannot determine skill directory from '{}'",
                location.display()
            ))
        })?;

        let raw_content = tokio::fs::read_to_string(location).await.map_err(|e| {
            echo_core::error::ReactError::Other(format!(
                "Failed to read SKILL.md at '{}': {}",
                location.display(),
                e
            ))
        })?;

        let mut raw_instructions = extract_body(&raw_content);

        // Fall back to legacy instructions from frontmatter if body is empty
        if raw_instructions.trim().is_empty()
            && let Some(legacy) = self.legacy_instructions.get(name)
            && !legacy.trim().is_empty()
        {
            warn!(
                "Skill '{}': using legacy frontmatter instructions (body is empty)",
                name
            );
            raw_instructions = legacy.clone();
        }

        // Process inline commands and variable substitution
        let ctx = PromptContext {
            skill_dir: skill_dir.display().to_string(),
            session_id: self.session_id.clone(),
            arguments: args.to_vec(),
            shell: descriptor.shell.clone(),
            source,
            sandbox: self.sandbox.clone(),
            ..Default::default()
        };
        let instructions = process_skill_content(&raw_instructions, &ctx).await;

        let resources = enumerate_resources(skill_dir).await;

        self.activated.insert(name.to_string());

        Ok(SkillContent {
            descriptor: descriptor.clone(),
            instructions,
            resources,
        })
    }

    // -- Code-based skills --

    /// Record a code-based skill that was installed via `add_skill`.
    pub fn record_code_skill(&mut self, info: SkillInfo) {
        self.code_skills.insert(info.name.clone(), info);
    }

    /// Check if a code-based skill is installed.
    pub fn has_code_skill(&self, name: &str) -> bool {
        self.code_skills.contains_key(name)
    }

    /// Get a code-based skill's info.
    pub fn get_code_skill(&self, name: &str) -> Option<&SkillInfo> {
        self.code_skills.get(name)
    }

    /// List all installed code-based skills.
    pub fn list_code_skills(&self) -> Vec<&SkillInfo> {
        let mut infos: Vec<&SkillInfo> = self.code_skills.values().collect();
        infos.sort_by_key(|i| &i.name);
        infos
    }

    // -- Unified queries --

    /// Check if a skill (code-based or file-based) is installed/discovered.
    pub fn is_installed(&self, name: &str) -> bool {
        self.code_skills.contains_key(name) || self.descriptors.contains_key(name)
    }

    /// Total number of skills (code + file-based).
    pub fn count(&self) -> usize {
        self.code_skills.len() + self.descriptors.len()
    }

    /// List all installed skills as `SkillInfo` (unified view).
    pub fn list(&self) -> Vec<&SkillInfo> {
        self.list_code_skills()
    }

    /// Get a skill's info by name (code-based only, for backward compat).
    pub fn get(&self, name: &str) -> Option<&SkillInfo> {
        self.code_skills.get(name)
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// -- Shared registry handle (for tools that need concurrent access) --

/// Thread-safe handle to a `SkillRegistry`, shared between the agent and tools.
pub type SharedRegistry = Arc<RwLock<SkillRegistry>>;

/// Create a new shared registry from an existing one.
pub fn shared_registry(registry: SkillRegistry) -> SharedRegistry {
    Arc::new(RwLock::new(registry))
}

// -- Helpers --

/// Extract the Markdown body from a SKILL.md file (strip YAML frontmatter).
fn extract_body(content: &str) -> String {
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
        String::new()
    }
}

/// Enumerate resource files in `scripts/`, `references/`, `assets/` under a skill dir.
async fn enumerate_resources(skill_dir: &std::path::Path) -> Vec<SkillResourceEntry> {
    let mut resources = Vec::new();

    let dirs = [
        ("scripts", SkillResourceKind::Script),
        ("references", SkillResourceKind::Reference),
        ("assets", SkillResourceKind::Asset),
    ];

    for (dir_name, kind) in &dirs {
        let dir_path = skill_dir.join(dir_name);
        if !dir_path.is_dir() {
            continue;
        }
        if let Ok(mut entries) = tokio::fs::read_dir(&dir_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_file()
                    && let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                {
                    resources.push(SkillResourceEntry {
                        relative_path: format!("{}/{}", dir_name, file_name),
                        kind: *kind,
                    });
                }
            }
        }
    }

    // Also enumerate top-level .md files that aren't SKILL.md (legacy resource files)
    if let Ok(mut entries) = tokio::fs::read_dir(skill_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name != "SKILL.md"
                && (name.ends_with(".md")
                    || name.ends_with(".txt")
                    || name.ends_with(".yaml")
                    || name.ends_with(".yml")
                    || name.ends_with(".json"))
            {
                resources.push(SkillResourceEntry {
                    relative_path: name.to_string(),
                    kind: SkillResourceKind::Other,
                });
            }
        }
    }

    resources.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    resources
}

// -- Backward compatibility --

#[deprecated(note = "Use SkillRegistry instead")]
pub type SkillManager = SkillRegistry;

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn make_descriptor(name: &str, desc: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.into(),
            description: desc.into(),
            location: PathBuf::from(format!("/skills/{}/SKILL.md", name)),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            hooks: None,
        }
    }

    #[test]
    fn test_registry_new() {
        let reg = SkillRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(reg.catalog_prompt().is_none());
    }

    #[test]
    fn test_register_descriptor() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("code-review", "Review code quality"));

        assert_eq!(reg.descriptor_count(), 1);
        assert!(reg.get_descriptor("code-review").is_some());
        assert!(reg.is_installed("code-review"));
    }

    #[test]
    fn test_register_descriptor_with_legacy() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor_with_legacy(
            make_descriptor("legacy-skill", "Legacy"),
            Some("Use legacy instructions.".to_string()),
        );

        assert_eq!(
            reg.legacy_instructions
                .get("legacy-skill")
                .map(String::as_str),
            Some("Use legacy instructions.")
        );
    }

    #[tokio::test]
    async fn test_activate_falls_back_to_legacy_instructions() {
        let root = std::env::temp_dir().join(format!(
            "echo-skill-registry-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let skill_dir = root.join("legacy-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: legacy-skill\ndescription: Legacy skill\n---\n",
        )
        .unwrap();

        let mut reg = SkillRegistry::new();
        reg.register_descriptor_with_legacy(
            SkillDescriptor {
                name: "legacy-skill".into(),
                description: "Legacy skill".into(),
                location: skill_dir.join("SKILL.md"),
                license: None,
                compatibility: None,
                metadata: HashMap::new(),
                allowed_tools: vec![],
                shell: None,
                paths: vec![],
                hooks: None,
            },
            Some("Use the legacy body".to_string()),
        );

        let content = reg
            .activate_with_args("legacy-skill", &[], SkillSource::Local)
            .await
            .unwrap();
        assert!(content.instructions.contains("Use the legacy body"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_catalog_prompt() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("code-review", "Review code"));
        reg.register_descriptor(make_descriptor("data-analysis", "Analyze data"));

        let catalog = reg.catalog_prompt().unwrap();
        assert!(catalog.contains("activate_skill"));
        assert!(catalog.contains("- code-review: Review code"));
        assert!(catalog.contains("- data-analysis: Analyze data"));
    }

    #[test]
    fn test_activation_tracking() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("test", "Test skill"));

        assert!(!reg.is_activated("test"));
        assert!(reg.mark_activated("test"));
        assert!(reg.is_activated("test"));
        assert!(!reg.mark_activated("test")); // dedup
        assert_eq!(reg.activated_count(), 1);
    }

    #[test]
    fn test_code_skills() {
        let mut reg = SkillRegistry::new();
        reg.record_code_skill(SkillInfo {
            name: "calculator".into(),
            description: "Math operations".into(),
            tool_names: vec!["add".into(), "subtract".into()],
            has_prompt_injection: true,
        });

        assert!(reg.has_code_skill("calculator"));
        assert!(reg.is_installed("calculator"));
        assert_eq!(reg.count(), 1);
    }

    #[test]
    fn test_available_names() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("b-skill", "B"));
        reg.register_descriptor(make_descriptor("a-skill", "A"));

        let names = reg.available_names();
        assert_eq!(names, vec!["a-skill", "b-skill"]);
    }

    #[test]
    fn test_extract_body() {
        let content = "---\nname: test\ndescription: Test\n---\n\n# Instructions\n\nDo stuff.";
        let body = extract_body(content);
        assert_eq!(body, "# Instructions\n\nDo stuff.");
    }

    #[test]
    fn test_extract_body_no_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here.";
        let body = extract_body(content);
        assert_eq!(body, content);
    }

    #[test]
    fn test_extract_body_malformed_frontmatter_returns_empty() {
        let content = "---\nname: test\ndescription: missing terminator\n# Instructions";
        let body = extract_body(content);
        assert!(body.is_empty());
    }

    #[test]
    fn test_mixed_skills() {
        let mut reg = SkillRegistry::new();
        reg.register_descriptor(make_descriptor("file-skill", "File-based"));
        reg.record_code_skill(SkillInfo {
            name: "code-skill".into(),
            description: "Code-based".into(),
            tool_names: vec![],
            has_prompt_injection: false,
        });

        assert_eq!(reg.count(), 2);
        assert!(reg.is_installed("file-skill"));
        assert!(reg.is_installed("code-skill"));
        assert!(!reg.is_installed("missing"));
    }
}
