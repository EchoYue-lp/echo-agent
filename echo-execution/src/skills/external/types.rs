use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use glob;
use serde::{Deserialize, Serialize};

use echo_core::sandbox::IsolationLevel;

// -- Sandbox policy for skills --

/// Per-skill sandbox isolation policy declared in SKILL.md frontmatter.
///
/// When a skill declares a sandbox policy, the framework enforces the specified
/// isolation level on script execution within that skill's context.
///
/// ```yaml
/// sandbox:
///   isolation: container
///   network: false
///   timeout: 300
///   allowed_paths: [/tmp/analysis]
///   denied_paths: [/etc, ~/.ssh]
/// ```
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

    /// Explicit trigger keywords/phrases for skill routing
    #[serde(default)]
    pub triggers: Option<Vec<String>>,

    /// Hooks for intercepting agent lifecycle events
    #[serde(default)]
    pub hooks: Option<crate::skills::hooks::HooksDefinition>,

    /// Per-skill sandbox isolation policy
    #[serde(default)]
    pub sandbox: Option<SkillSandboxPolicy>,

    /// Skill dependencies (auto-activated before this skill)
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,

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
            triggers: self.triggers.unwrap_or_default(),
            hooks: self.hooks,
            sandbox: self.sandbox,
            depends_on: self.depends_on.unwrap_or_default(),
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

// -- Backward compatibility aliases --

// -- Tests --

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
            metadata: None,
            allowed_tools: Some(AllowedToolsValue::String("Bash(git:*) Read".into())),
            shell: Some("bash".into()),
            paths: Some(vec!["*.py".into()]),
            triggers: None,
            hooks: None,
            sandbox: None,
            depends_on: None,
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
            triggers: None,
            hooks: None,
            sandbox: None,
            depends_on: None,
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

    fn make_desc_with_paths(paths: Vec<&str>) -> SkillDescriptor {
        SkillDescriptor {
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
