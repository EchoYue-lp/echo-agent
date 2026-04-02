use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Tier 1: SkillDescriptor (catalog metadata, ~50-100 tokens per skill) ─────

/// Lightweight skill metadata loaded at discovery time (Tier 1).
///
/// Aligned with the [agentskills.io specification](https://agentskills.io/specification).
/// Only `name` and `description` are required; all other fields are optional.
///
/// At session startup the agent builds a **catalog** from all discovered descriptors
/// and injects it into the system prompt. Each descriptor costs ~50-100 tokens,
/// so even dozens of skills keep the base context small.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDescriptor {
    /// Unique skill name (kebab-case: lowercase, hyphens, 1-64 chars).
    /// Must match the parent directory name per spec.
    pub name: String,

    /// Human-readable description (max 1024 chars).
    /// Should describe what the skill does **and** when to use it.
    pub description: String,

    /// Absolute path to the `SKILL.md` file.
    #[serde(skip)]
    pub location: PathBuf,

    /// SPDX license identifier or reference to a bundled license file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Environment requirements (intended product, system packages, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,

    /// Arbitrary key-value metadata (author, version, tags, etc.).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,

    /// Pre-approved tools the skill may use (space-delimited in SKILL.md).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        alias = "allowed-tools"
    )]
    pub allowed_tools: Vec<String>,

    /// Preferred shell for inline commands: `"bash"` (default) or `"powershell"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,

    /// File path patterns for conditional activation.
    /// When set, the skill is only surfaced/activated when the user touches
    /// a file matching one of these glob patterns (e.g., `["*.py", "tests/**"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,

    /// Hooks for intercepting tool calls.
    #[serde(skip)]
    pub hooks: Option<crate::skills::hooks::HooksDefinition>,
}

impl SkillDescriptor {
    /// Generate a single catalog line for system-prompt injection.
    ///
    /// Format: `- <name>: <description>` (with optional path constraints)
    pub fn catalog_line(&self) -> String {
        if self.paths.is_empty() {
            format!("- {}: {}", self.name, self.description)
        } else {
            format!(
                "- {}: {} [activates for: {}]",
                self.name,
                self.description,
                self.paths.join(", ")
            )
        }
    }

    /// Validate the name according to the agentskills.io spec.
    /// Returns a list of warnings (empty = valid).
    pub fn validate_name(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let name = &self.name;

        if name.is_empty() || name.len() > 64 {
            warnings.push(format!(
                "name '{}' length {} outside 1-64 range",
                name,
                name.len()
            ));
        }
        if name.starts_with('-') || name.ends_with('-') {
            warnings.push(format!("name '{}' must not start or end with hyphen", name));
        }
        if name.contains("--") {
            warnings.push(format!(
                "name '{}' must not contain consecutive hyphens",
                name
            ));
        }
        for ch in name.chars() {
            if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
                warnings.push(format!(
                    "name '{}' contains invalid character '{}' (only lowercase, digits, hyphens allowed)",
                    name, ch
                ));
                break;
            }
        }
        warnings
    }
}

// ── Tier 2: SkillContent (full instructions, loaded on activation) ───────────

/// Full skill content returned when a skill is activated (Tier 2).
///
/// Contains the `SKILL.md` Markdown body (frontmatter stripped) plus
/// a listing of bundled resource files discovered in `scripts/`, `references/`,
/// and `assets/` directories.
#[derive(Debug, Clone)]
pub struct SkillContent {
    /// The skill's catalog descriptor.
    pub descriptor: SkillDescriptor,

    /// The Markdown body of `SKILL.md` (everything after the frontmatter).
    /// This is the skill's primary instructions text.
    pub instructions: String,

    /// Bundled resource files discovered in the skill directory.
    pub resources: Vec<SkillResourceEntry>,
}

impl SkillContent {
    /// Format the skill content as a structured block for LLM injection.
    ///
    /// Uses XML-style tags so the agent harness can identify skill content
    /// during context compaction.
    pub fn to_prompt_block(&self) -> String {
        let skill_dir = self
            .descriptor
            .location
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        let mut block = format!(
            "<skill_content name=\"{}\">\n{}\n\nSkill directory: {}\n\
             Relative paths in this skill are relative to the skill directory.",
            self.descriptor.name,
            self.instructions.trim(),
            skill_dir,
        );

        if !self.descriptor.allowed_tools.is_empty() {
            block.push_str(&format!(
                "\n\n<allowed_tools>\nThis skill only permits the following tools: {}\n\
                 Do NOT use other tools while working within this skill's scope.\n</allowed_tools>",
                self.descriptor.allowed_tools.join(", ")
            ));
        }

        if !self.resources.is_empty() {
            block.push_str("\n\n<skill_resources>");
            for res in &self.resources {
                block.push_str(&format!(
                    "\n  <file kind=\"{}\">{}</file>",
                    res.kind, res.relative_path
                ));
            }
            block.push_str("\n</skill_resources>");
        }

        block.push_str("\n</skill_content>");
        block
    }
}

// ── Tier 3: Resource entries ─────────────────────────────────────────────────

/// A bundled resource file discovered in the skill directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResourceEntry {
    /// Path relative to the skill directory (e.g. `references/guide.md`).
    pub relative_path: String,

    /// The kind of resource, inferred from the parent directory name.
    pub kind: SkillResourceKind,
}

/// Classification of a bundled resource file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillResourceKind {
    Script,
    Reference,
    Asset,
    Other,
}

impl std::fmt::Display for SkillResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Script => write!(f, "script"),
            Self::Reference => write!(f, "reference"),
            Self::Asset => write!(f, "asset"),
            Self::Other => write!(f, "other"),
        }
    }
}

// ── SKILL.md Frontmatter (raw deserialization target) ────────────────────────

/// Raw YAML frontmatter as deserialized from a `SKILL.md` file.
///
/// Supports both the agentskills.io standard fields and legacy echo-agent
/// extensions (`version`, `author`, `tags`, `instructions`, `resources`).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct RawFrontmatter {
    pub name: String,
    pub description: String,

    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: Option<AllowedToolsValue>,

    /// Preferred shell: "bash" (default) or "powershell"
    #[serde(default)]
    pub shell: Option<String>,

    /// Conditional activation path patterns (glob syntax)
    #[serde(default)]
    pub paths: Option<Vec<String>>,

    /// Hooks for intercepting tool calls (PreToolUse/PostToolUse)
    #[serde(default)]
    pub hooks: Option<crate::skills::hooks::HooksDefinition>,

    // Legacy echo-agent extensions (auto-detected, emit deprecation warning)
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub resources: Option<Vec<LegacyResourceRef>>,
}

/// `allowed-tools` can be either a space-delimited string or a list.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub(crate) enum AllowedToolsValue {
    String(String),
    List(Vec<String>),
}

impl AllowedToolsValue {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::String(s) => s.split_whitespace().map(|s| s.to_string()).collect(),
            Self::List(v) => v,
        }
    }
}

impl RawFrontmatter {
    /// Convert to a `SkillDescriptor`, folding legacy fields into `metadata`.
    pub fn into_descriptor(self, location: PathBuf) -> SkillDescriptor {
        let mut metadata = self.metadata.unwrap_or_default();

        if let Some(version) = &self.version {
            metadata
                .entry("version".to_string())
                .or_insert_with(|| version.clone());
        }
        if let Some(author) = &self.author {
            metadata
                .entry("author".to_string())
                .or_insert_with(|| author.clone());
        }
        if let Some(tags) = &self.tags {
            metadata
                .entry("tags".to_string())
                .or_insert_with(|| tags.join(", "));
        }

        SkillDescriptor {
            name: self.name,
            description: self.description,
            location,
            license: self.license,
            compatibility: self.compatibility,
            metadata,
            allowed_tools: self.allowed_tools.map(|v| v.into_vec()).unwrap_or_default(),
            shell: self.shell,
            paths: self.paths.unwrap_or_default(),
            hooks: self.hooks,
        }
    }

    /// Whether this frontmatter uses legacy echo-agent extensions.
    pub fn is_legacy_format(&self) -> bool {
        self.instructions.is_some() || self.resources.is_some()
    }
}

/// Legacy resource reference from the old echo-agent SKILL.md format.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LegacyResourceRef {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub load_on_startup: Option<bool>,
}

// ── Backward compatibility aliases ───────────────────────────────────────────

#[deprecated(note = "Use SkillDescriptor instead")]
pub type SkillMeta = SkillDescriptor;

#[deprecated(note = "Use LegacyResourceRef instead")]
pub type ResourceRef = LegacyResourceRef;

/// Legacy loaded skill struct (kept for API compatibility).
#[deprecated(note = "Use SkillContent instead")]
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub meta: SkillDescriptor,
    pub skill_dir: PathBuf,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptor_validate_name_valid() {
        let d = SkillDescriptor {
            name: "code-review".into(),
            description: "Review code".into(),
            location: PathBuf::new(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            hooks: None,
        };
        assert!(d.validate_name().is_empty());
    }

    #[test]
    fn test_descriptor_validate_name_invalid() {
        let cases = vec![
            ("Code-Review", "uppercase"),
            ("-code", "starts with hyphen"),
            ("code-", "ends with hyphen"),
            ("code--review", "consecutive hyphens"),
            ("code_review", "underscore"),
        ];
        for (name, reason) in cases {
            let d = SkillDescriptor {
                name: name.into(),
                description: "test".into(),
                location: PathBuf::new(),
                license: None,
                compatibility: None,
                metadata: HashMap::new(),
                allowed_tools: vec![],
                shell: None,
                paths: vec![],
                hooks: None,
            };
            assert!(
                !d.validate_name().is_empty(),
                "expected warnings for {}: {}",
                name,
                reason
            );
        }
    }

    #[test]
    fn test_descriptor_catalog_line() {
        let d = SkillDescriptor {
            name: "pdf-processing".into(),
            description: "Extract PDF text, fill forms.".into(),
            location: PathBuf::new(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            hooks: None,
        };
        assert_eq!(
            d.catalog_line(),
            "- pdf-processing: Extract PDF text, fill forms."
        );
    }

    #[test]
    fn test_skill_content_prompt_block() {
        let content = SkillContent {
            descriptor: SkillDescriptor {
                name: "test-skill".into(),
                description: "A test".into(),
                location: PathBuf::from("/home/user/skills/test-skill/SKILL.md"),
                license: None,
                compatibility: None,
                metadata: HashMap::new(),
                allowed_tools: vec![],
                shell: None,
                paths: vec![],
                hooks: None,
            },
            instructions: "# Instructions\n\nDo the thing.".into(),
            resources: vec![
                SkillResourceEntry {
                    relative_path: "scripts/run.py".into(),
                    kind: SkillResourceKind::Script,
                },
                SkillResourceEntry {
                    relative_path: "references/guide.md".into(),
                    kind: SkillResourceKind::Reference,
                },
            ],
        };
        let block = content.to_prompt_block();
        assert!(block.contains("<skill_content name=\"test-skill\">"));
        assert!(block.contains("# Instructions"));
        assert!(block.contains("<skill_resources>"));
        assert!(block.contains("scripts/run.py"));
        assert!(block.contains("</skill_content>"));
    }

    #[test]
    fn test_resource_kind_display() {
        assert_eq!(SkillResourceKind::Script.to_string(), "script");
        assert_eq!(SkillResourceKind::Reference.to_string(), "reference");
        assert_eq!(SkillResourceKind::Asset.to_string(), "asset");
        assert_eq!(SkillResourceKind::Other.to_string(), "other");
    }

    #[test]
    fn test_raw_frontmatter_into_descriptor() {
        let raw = RawFrontmatter {
            name: "my-skill".into(),
            description: "Does things".into(),
            license: Some("MIT".into()),
            compatibility: None,
            metadata: None,
            allowed_tools: Some(AllowedToolsValue::String("Bash(git:*) Read".into())),
            shell: Some("bash".into()),
            paths: Some(vec!["*.py".into()]),
            hooks: None,
            version: Some("1.0.0".into()),
            author: Some("team".into()),
            tags: Some(vec!["code".into(), "review".into()]),
            instructions: None,
            resources: None,
        };

        let desc = raw.into_descriptor(PathBuf::from("/skills/my-skill/SKILL.md"));
        assert_eq!(desc.name, "my-skill");
        assert_eq!(desc.license, Some("MIT".into()));
        assert_eq!(desc.allowed_tools, vec!["Bash(git:*)", "Read"]);
        assert_eq!(desc.shell, Some("bash".into()));
        assert_eq!(desc.paths, vec!["*.py"]);
        assert_eq!(desc.metadata.get("version").unwrap(), "1.0.0");
        assert_eq!(desc.metadata.get("author").unwrap(), "team");
        assert_eq!(desc.metadata.get("tags").unwrap(), "code, review");
    }

    #[test]
    fn test_raw_frontmatter_legacy_detection() {
        let standard = RawFrontmatter {
            name: "s".into(),
            description: "d".into(),
            license: None,
            compatibility: None,
            metadata: None,
            allowed_tools: None,
            shell: None,
            paths: None,
            hooks: None,
            version: None,
            author: None,
            tags: None,
            instructions: None,
            resources: None,
        };
        assert!(!standard.is_legacy_format());

        let legacy = RawFrontmatter {
            instructions: Some("do stuff".into()),
            ..standard
        };
        assert!(legacy.is_legacy_format());
    }

    #[test]
    fn test_allowed_tools_value_string() {
        let v = AllowedToolsValue::String("Bash(git:*) Read Write".into());
        assert_eq!(v.into_vec(), vec!["Bash(git:*)", "Read", "Write"]);
    }

    #[test]
    fn test_allowed_tools_value_list() {
        let v = AllowedToolsValue::List(vec!["Read".into(), "Write".into()]);
        assert_eq!(v.into_vec(), vec!["Read", "Write"]);
    }
}
