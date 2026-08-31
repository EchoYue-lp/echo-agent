use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::path::PathBuf;

use glob;
use serde::{Deserialize, Serialize};

use echo_core::sandbox::IsolationLevel;

// -- Sandbox policy for programmatic skill descriptors --

/// Per-skill sandbox isolation policy for programmatically registered
/// descriptors. The official `SKILL.md` file format has no sandbox field;
/// hosts that need this policy attach it through the runtime API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillSandboxPolicy {
    /// Required isolation level. `None` means no enforcement (default behavior).
    ///
    /// Values: `none`, `process`, `os-sandbox`, `container`, `orchestrated`
    pub isolation: Option<IsolationLevel>,

    /// Whether network access is permitted within the sandbox.
    /// Defaults to `false` (deny) when isolation is specified.
    pub network: Option<bool>,

    /// Execution timeout in seconds. `None` means no additional timeout.
    #[serde(alias = "timeout")]
    pub timeout_secs: Option<u64>,

    /// Paths the sandbox permits for file access.
    #[serde(default)]
    pub allowed_paths: Vec<PathBuf>,

    /// Paths the sandbox denies for file access.
    #[serde(default)]
    pub denied_paths: Vec<PathBuf>,
}

impl SkillSandboxPolicy {
    /// Whether this policy actually constrains execution.
    pub fn is_constraining(&self) -> bool {
        self.isolation.is_some()
            || self.network.is_some()
            || self.timeout_secs.is_some()
            || !self.allowed_paths.is_empty()
            || !self.denied_paths.is_empty()
    }

    /// Whether network access is permitted. Defaults to `false` when isolation is set.
    pub fn network_allowed(&self) -> bool {
        self.network.unwrap_or(false)
    }
}

// -- Tier 1: SkillDescriptor (catalog metadata, ~50-100 tokens per skill) --

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

    /// Origin source tag for grouped unload (e.g. `"plugin:my-plugin"`).
    ///
    /// Set by the plugin integrator when loading skills from a plugin
    /// directory so `SkillRegistry::unregister_by_source` can remove exactly
    /// that plugin's skills on disable/uninstall. `None` for user-loaded /
    /// built-in skills (never group-unloaded). Skipped in serialization to
    /// avoid a breaking change for frontend consumers.
    #[serde(skip, default)]
    pub source: Option<String>,

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

    /// Explicit trigger keywords/phrases for skill routing.
    /// When the user query matches any trigger, the skill is activated.
    /// Complements the `description` field for precise routing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,

    /// Hooks for intercepting agent lifecycle events.
    #[serde(skip)]
    pub hooks: Option<crate::skills::hooks::HooksDefinition>,

    /// Per-skill sandbox isolation policy.
    /// When set, tool execution within this skill's context is constrained
    /// according to the policy (isolation level, network, paths, timeout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SkillSandboxPolicy>,

    /// Other skills that must be activated before this one.
    /// The framework auto-activates dependencies during `activate_skill`.
    /// Circular dependencies are detected at discovery time and logged as warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

impl SkillDescriptor {
    /// Generate a single catalog line for system-prompt injection.
    ///
    /// Format: `- <name>: <description>` (with optional path constraints and triggers)
    pub fn catalog_line(&self) -> String {
        let mut line = format!("- {}: {}", self.name, self.description);

        let mut annotations = Vec::new();
        if !self.paths.is_empty() {
            annotations.push(format!("activates for: {}", self.paths.join(", ")));
        }
        if !self.triggers.is_empty() {
            annotations.push(format!("triggers: {}", self.triggers.join(", ")));
        }
        if !self.depends_on.is_empty() {
            annotations.push(format!("depends: {}", self.depends_on.join(", ")));
        }

        if !annotations.is_empty() {
            line.push_str(&format!(" [{}]", annotations.join("; ")));
        }

        line
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

    /// Validate glob patterns in `paths` field.
    /// Returns a list of warnings for invalid or suspicious patterns.
    pub fn validate_paths(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for pattern in &self.paths {
            // Reject path traversal in patterns
            if pattern.contains("..") {
                warnings.push(format!(
                    "path pattern '{}' contains '..' which is not allowed",
                    pattern
                ));
                continue;
            }

            // Check for unbalanced glob characters
            let open_brace = pattern.matches('{').count();
            let close_brace = pattern.matches('}').count();
            if open_brace != close_brace {
                warnings.push(format!("path pattern '{}' has unbalanced braces", pattern));
            }

            let open_bracket = pattern.matches('[').count();
            let close_bracket = pattern.matches(']').count();
            if open_bracket != close_bracket {
                warnings.push(format!(
                    "path pattern '{}' has unbalanced brackets",
                    pattern
                ));
            }

            // Warn on overly broad patterns
            if pattern == "**" || pattern == "*" {
                warnings.push(format!(
                    "path pattern '{}' is overly broad and may match unintended files",
                    pattern
                ));
            }

            // Verify pattern is compilable via glob crate
            if let Err(e) = glob::Pattern::new(pattern) {
                warnings.push(format!(
                    "path pattern '{}' is not valid glob syntax: {}",
                    pattern, e
                ));
            }
        }
        warnings
    }

    /// Check whether a touched file path satisfies this skill's conditional activation rules.
    ///
    /// Skills without `paths` are always considered a match.
    pub fn matches_context_path(&self, context_path: &str) -> bool {
        if self.paths.is_empty() {
            return true;
        }

        let normalized = context_path.replace('\\', "/");
        let trimmed = normalized.trim_start_matches("./");
        let file_name = Path::new(trimmed)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(trimmed);

        self.paths.iter().any(|pattern| {
            glob::Pattern::new(pattern).ok().is_some_and(|glob| {
                glob.matches(trimmed) || glob.matches(&normalized) || glob.matches(file_name)
            })
        })
    }

    /// Check whether the descriptor permits use of a given tool name.
    ///
    /// Empty `allowed_tools` means unrestricted. Match semantics mirror hook/tool
    /// matching so exact names, globs, and `Bash` -> `Bash(git:*)` prefix variants
    /// behave consistently.
    pub fn permits_tool(&self, tool_name: &str) -> bool {
        if self.allowed_tools.is_empty() {
            return true;
        }

        skill_allows_tool(&self.allowed_tools, tool_name)
    }
}

/// Framework control tools remain available while a skill narrows its domain
/// tool surface. Without these, a restricted skill could hide finalization,
/// further discovery, resource loading, or an explicit user question.
pub fn is_skill_control_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "final_answer"
            | "tool_search"
            | "activate_skill"
            | "read_skill_resource"
            | "run_skill_script"
            | "human_in_loop"
    )
}

pub fn skill_allows_tool<'a, I>(allowed_tools: I, tool_name: &str) -> bool
where
    I: IntoIterator<Item = &'a String>,
{
    is_skill_control_tool(tool_name)
        || allowed_tools
            .into_iter()
            .any(|matcher| tool_matcher(matcher, tool_name))
}

/// Match a tool name against a matcher pattern.
///
/// Supports:
/// - Exact match: `"Read"` matches `"Read"`
/// - Wildcard: `"*"` matches everything
/// - Glob patterns: `"Bash(*)"` matches `"Bash(git:status)"`
/// - Prefix: `"Bash"` matches `"Bash(git:status)"`
pub fn tool_matcher(matcher: &str, tool_name: &str) -> bool {
    if matcher == "*" || matcher == tool_name {
        return true;
    }
    if let Ok(pattern) = glob::Pattern::new(matcher)
        && pattern.matches(tool_name)
    {
        return true;
    }
    tool_name.starts_with(matcher) && tool_name.as_bytes().get(matcher.len()).copied() == Some(b'(')
}

/// One parsed `SKILL.md` document before runtime resource discovery.
///
/// The descriptor is the catalog projection of frontmatter, while
/// `instructions` is the Markdown body after the closing delimiter.
#[derive(Debug, Clone)]
pub struct SkillDocument {
    descriptor: SkillDescriptor,
    instructions: String,
    source: String,
}

impl SkillDocument {
    pub(crate) fn new(descriptor: SkillDescriptor, instructions: String, source: String) -> Self {
        Self {
            descriptor,
            instructions,
            source,
        }
    }

    pub fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn into_descriptor(self) -> SkillDescriptor {
        self.descriptor
    }

    /// Attach runtime provenance without changing parsed frontmatter facts.
    pub fn set_registration_source(&mut self, source: impl Into<String>) {
        self.descriptor.source = Some(source.into());
    }

    /// Render this parsed document with a new standard `allowed-tools` value.
    /// Evolution and host tooling use this canonical writer instead of a
    /// second YAML parser or legacy-field normalizer.
    pub fn render_with_allowed_tools(
        &self,
        allowed_tools: &[String],
    ) -> std::result::Result<String, String> {
        #[derive(Serialize)]
        struct OfficialFrontmatter {
            name: String,
            description: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            license: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            compatibility: Option<String>,
            #[serde(skip_serializing_if = "BTreeMap::is_empty")]
            metadata: BTreeMap<String, String>,
            #[serde(rename = "allowed-tools", skip_serializing_if = "Option::is_none")]
            allowed_tools: Option<String>,
        }

        let frontmatter = OfficialFrontmatter {
            name: self.descriptor.name.clone(),
            description: self.descriptor.description.clone(),
            license: self.descriptor.license.clone(),
            compatibility: self.descriptor.compatibility.clone(),
            metadata: self.descriptor.metadata.clone().into_iter().collect(),
            allowed_tools: (!allowed_tools.is_empty()).then(|| allowed_tools.join(" ")),
        };
        let yaml = serde_yaml_ng::to_string(&frontmatter).map_err(|error| error.to_string())?;
        Ok(format!("---\n{yaml}---\n{}", self.instructions))
    }
}

// -- Tier 2: SkillContent (full instructions, loaded on activation) --

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
                "\n\n<allowed_tools>\nThis skill declares the following preferred/allowed tools: {}\n\
                 Runtime enforcement currently applies to the built-in skill tools such as \
                 read_skill_resource and run_skill_script.\n</allowed_tools>",
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

// -- Tier 3: Resource entries --

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

// -- SKILL.md Frontmatter (raw deserialization target) --

/// Raw YAML frontmatter as deserialized from a `SKILL.md` file.
///
/// Only the official agentskills.io fields are accepted (`deny_unknown_fields`)
/// and `metadata` maps string keys to string values, matching the spec's
/// string-to-string contract. echo-agent uses no private frontmatter
/// extensions: routing is description-driven and per-skill hooks are not part
/// of the file format.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFrontmatter {
    pub name: String,
    pub description: String,

    #[serde(default = "default_optional_string")]
    pub license: Option<String>,
    #[serde(default = "default_optional_string")]
    pub compatibility: Option<String>,
    #[serde(default = "default_optional_metadata")]
    pub metadata: Option<HashMap<String, String>>,
    /// Official format: one space-separated string. YAML sequences and the
    /// legacy underscore spelling are rejected by serde instead of being
    /// silently normalized.
    #[serde(default = "default_optional_string", rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
}

fn default_optional_string() -> Option<String> {
    None
}

fn default_optional_metadata() -> Option<HashMap<String, String>> {
    None
}

impl RawFrontmatter {
    /// Convert validated frontmatter into catalog metadata.
    ///
    /// The runtime extension fields on [`SkillDescriptor`] (`triggers`,
    /// `paths`, …) have no file-based source in the standard format; they
    /// remain available only to programmatic descriptors.
    pub fn into_descriptor(self, location: PathBuf) -> SkillDescriptor {
        SkillDescriptor {
            source: None,
            name: self.name,
            description: self.description,
            location,
            license: self.license,
            compatibility: self.compatibility,
            metadata: self.metadata.unwrap_or_default(),
            allowed_tools: self
                .allowed_tools
                .map(|value| value.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            shell: None,
            paths: Vec::new(),
            triggers: Vec::new(),
            // Per-skill hooks have no file-based source in the official format.
            hooks: None,
            sandbox: None,
            depends_on: Vec::new(),
        }
    }
}

// -- Tests --

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptor_validate_name_valid() {
        let d = SkillDescriptor {
            source: None,
            name: "code-review".into(),
            description: "Review code".into(),
            location: PathBuf::new(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
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
                source: None,
                name: name.into(),
                description: "test".into(),
                location: PathBuf::new(),
                license: None,
                compatibility: None,
                metadata: HashMap::new(),
                allowed_tools: vec![],
                shell: None,
                paths: vec![],
                triggers: vec![],
                hooks: None,
                sandbox: None,
                depends_on: vec![],
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
            source: None,
            name: "pdf-processing".into(),
            description: "Extract PDF text, fill forms.".into(),
            location: PathBuf::new(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
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
                source: None,
                name: "test-skill".into(),
                description: "A test".into(),
                location: PathBuf::from("/home/user/skills/test-skill/SKILL.md"),
                license: None,
                compatibility: None,
                metadata: HashMap::new(),
                allowed_tools: vec![],
                shell: None,
                paths: vec![],
                triggers: vec![],
                hooks: None,
                sandbox: None,
                depends_on: vec![],
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
            metadata: Some(HashMap::from([
                ("version".to_string(), "1.0.0".to_string()),
                ("author".to_string(), "team".to_string()),
                ("tags".to_string(), "code, review".to_string()),
            ])),
            allowed_tools: Some("Bash(git:*) Read".into()),
        };

        let desc = raw.into_descriptor(PathBuf::from("/skills/my-skill/SKILL.md"));
        assert_eq!(desc.name, "my-skill");
        assert_eq!(desc.license, Some("MIT".into()));
        assert_eq!(desc.allowed_tools, vec!["Bash(git:*)", "Read"]);
        assert_eq!(
            desc.metadata.get("version").map(String::as_str),
            Some("1.0.0")
        );
        assert_eq!(
            desc.metadata.get("author").map(String::as_str),
            Some("team")
        );
        assert_eq!(
            desc.metadata.get("tags").map(String::as_str),
            Some("code, review")
        );
    }

    #[test]
    fn test_standard_metadata_keeps_extension_fields_empty() -> std::result::Result<(), String> {
        let content = r#"---
name: routed-skill
description: Standard layout; routing is description-driven.
metadata:
  category: automation
  sample-count: "7"
allowed-tools: shell read_file apply_patch
---
Body"#;
        let document = SkillDocument::parse(content).map_err(|error| error.to_string())?;
        let desc = document.descriptor();
        assert_eq!(
            desc.metadata.get("category").map(String::as_str),
            Some("automation")
        );
        assert_eq!(
            desc.metadata.get("sample-count").map(String::as_str),
            Some("7")
        );
        assert_eq!(
            desc.allowed_tools,
            vec!["shell", "read_file", "apply_patch"]
        );
        assert!(desc.triggers.is_empty());
        assert!(desc.paths.is_empty());
        assert!(desc.depends_on.is_empty());
        assert!(desc.shell.is_none());
        assert!(desc.sandbox.is_none());
        assert!(desc.hooks.is_none());
        Ok::<(), String>(())
    }

    #[test]
    fn test_legacy_top_level_extension_fields_are_rejected() {
        // Legacy pre-standardization files declared echo-agent semantics at
        // the frontmatter top level; parsing must fail closed so catalog
        // tooling surfaces the unmigrated file instead of silently dropping
        // routing/sandbox intent.
        for legacy_field in [
            "triggers:\n  - 周报\n",
            "shell: bash\n",
            "paths:\n  - \"*.pdf\"\n",
            "sandbox:\n  isolation: process\n",
            "depends_on:\n  - web-search\n",
        ] {
            let content =
                format!("---\nname: legacy\ndescription: Legacy layout.\n{legacy_field}---\nBody");
            assert!(
                SkillDocument::parse(&content).is_err(),
                "legacy top-level field must be rejected: {legacy_field}"
            );
        }
    }

    #[test]
    fn test_frontmatter_hooks_are_rejected() {
        // Per-skill hooks are not part of the official file format; a
        // frontmatter hooks block is an unknown official field and must fail.
        let content = "---\nname: hooked\ndescription: Hooked.\nhooks:\n  Stop:\n    - hooks:\n        - type: command\n          command: echo done\n---\nBody";
        assert!(SkillDocument::parse(content).is_err());
    }

    #[test]
    fn test_non_scalar_metadata_is_rejected() {
        // The spec maps metadata string keys to string values; nested maps
        // (including any vendor namespace) fail parsing instead of being
        // silently reshaped.
        for nested in [
            "metadata:\n  echo-agent:\n    triggers: [周报]\n",
            "metadata:\n  vendor:\n    nested: true\n",
        ] {
            let content = format!("---\nname: nested\ndescription: Nested.\n{nested}---\nBody");
            assert!(
                SkillDocument::parse(&content).is_err(),
                "non-scalar metadata must be rejected: {nested}"
            );
        }
    }

    #[test]
    fn test_allowed_tools_must_be_string() {
        let content = "---\nname: list-tools\ndescription: Invalid list form.\nallowed-tools: [Read, Write]\n---\nBody";
        assert!(SkillDocument::parse(content).is_err());
        let content = "---\nname: underscore-tools\ndescription: Invalid alias form.\nallowed_tools: Read Write\n---\nBody";
        assert!(SkillDocument::parse(content).is_err());
    }

    fn make_desc_with_paths(paths: Vec<&str>) -> SkillDescriptor {
        SkillDescriptor {
            source: None,
            name: "test".into(),
            description: "test".into(),
            location: PathBuf::new(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            shell: None,
            paths: paths.into_iter().map(String::from).collect(),
            triggers: vec![],
            hooks: None,
            sandbox: None,
            depends_on: vec![],
        }
    }

    #[test]
    fn test_validate_paths_good() {
        let d = make_desc_with_paths(vec!["*.py", "src/**/*.rs", "tests/**"]);
        let warnings = d.validate_paths();
        // No traversal or syntax errors
        assert!(!warnings.iter().any(|w| w.contains("'..'")));
        assert!(!warnings.iter().any(|w| w.contains("not valid glob")));
    }

    #[test]
    fn test_validate_paths_traversal() {
        let d = make_desc_with_paths(vec!["../secret.txt"]);
        let warnings = d.validate_paths();
        assert!(warnings.iter().any(|w| w.contains("'..'")));
    }

    #[test]
    fn test_validate_paths_unbalanced_braces() {
        let d = make_desc_with_paths(vec!["*.py", "{foo,bar"]);
        let warnings = d.validate_paths();
        assert!(warnings.iter().any(|w| w.contains("unbalanced")));
    }

    #[test]
    fn test_validate_paths_overly_broad() {
        let d = make_desc_with_paths(vec!["**"]);
        let warnings = d.validate_paths();
        assert!(warnings.iter().any(|w| w.contains("overly broad")));
    }

    #[test]
    fn test_matches_context_path() {
        let d = make_desc_with_paths(vec!["*.py", "tests/**"]);
        assert!(d.matches_context_path("main.py"));
        assert!(d.matches_context_path("./tests/unit/test_demo.rs"));
        assert!(!d.matches_context_path("src/main.rs"));
    }

    #[test]
    fn test_permits_tool() {
        let mut d = make_desc_with_paths(vec![]);
        d.allowed_tools = vec!["read_skill_resource".into(), "Bash(*)".into()];
        assert!(d.permits_tool("read_skill_resource"));
        assert!(d.permits_tool("Bash(git:status)"));
        assert!(d.permits_tool("run_skill_script"));
        assert!(d.permits_tool("final_answer"));
        assert!(!d.permits_tool("write_file"));
    }
}
