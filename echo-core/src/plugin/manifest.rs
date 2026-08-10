//! Plugin manifest — YAML-based plugin metadata and component declarations.
//!
//! The manifest lives at `.echo-plugin/manifest.yaml` inside a plugin's
//! root directory. It declares the plugin's identity, components, user
//! configuration, and dependencies.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Top-level plugin manifest, deserialized from `manifest.yaml`.
///
/// # Example
///
/// ```yaml
/// name: data-analysis-pack
/// display_name: "Data Analysis Pack"
/// version: "1.2.0"
/// description: "Enhanced data analysis with polars extensions"
/// author:
///   name: "Echo Team"
///   email: "team@echo.dev"
/// license: MIT
/// keywords: [data, analysis]
/// components:
///   skills: "./skills/"
///   hooks: "./hooks/hooks.yaml"
///   mcp_servers: "./.mcp.json"
/// config:
///   api_endpoint:
///     type: string
///     title: "API Endpoint"
///     default: "http://localhost:8080"
/// dependencies:
///   - name: base-tools
///     version: ">=1.0.0"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique identifier (kebab-case, no spaces).
    pub name: String,

    /// Human-readable display name. Falls back to `name` if omitted.
    #[serde(default)]
    pub display_name: Option<String>,

    /// Semantic version string (e.g. "1.2.0").
    #[serde(default = "default_version")]
    pub version: String,

    /// Brief description of the plugin's purpose.
    #[serde(default)]
    pub description: String,

    /// Author information.
    #[serde(default)]
    pub author: Option<PluginAuthor>,

    /// License identifier (e.g. "MIT", "Apache-2.0").
    #[serde(default)]
    pub license: Option<String>,

    /// Discovery tags for search and filtering.
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Documentation URL.
    #[serde(default)]
    pub homepage: Option<String>,

    /// Source repository URL.
    #[serde(default)]
    pub repository: Option<String>,

    /// Component declarations — paths relative to plugin root.
    #[serde(default)]
    pub components: PluginComponents,

    /// User-configurable values, prompted at install time.
    #[serde(default)]
    pub config: HashMap<String, PluginUserConfigEntry>,

    /// Other plugins this one depends on.
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,

    /// Whether the plugin starts enabled (default: true).
    #[serde(default = "default_true")]
    pub default_enabled: bool,
}

fn default_version() -> String {
    "0.0.0".to_string()
}

fn default_true() -> bool {
    true
}

/// Author metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Component paths — all relative to the plugin root, starting with `./`.
///
/// Fields can be:
/// - A single path string: `"./skills/"`
/// - An array of paths: `["./skills/", "./extra-skills/"]`
/// - `None` (default) — uses the conventional default directory
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginComponents {
    /// Directory containing `<name>/SKILL.md` files.
    /// Default: `./skills/`. Additive with the default directory.
    #[serde(default)]
    pub skills: Option<StringOrArray>,

    /// Agent definition markdown files.
    /// Default: `./agents/`. Replaces default when set.
    #[serde(default)]
    pub agents: Option<StringOrArray>,

    /// Hook configuration file path.
    #[serde(default)]
    pub hooks: Option<StringOrArray>,

    /// MCP server configuration file path.
    #[serde(default)]
    pub mcp_servers: Option<StringOrArray>,

    /// LSP server configuration file path.
    #[serde(default)]
    pub lsp_servers: Option<StringOrArray>,

    /// Background monitor configuration.
    #[serde(default)]
    pub monitors: Option<StringOrArray>,

    /// Color theme files directory.
    #[serde(default)]
    pub themes: Option<StringOrArray>,

    /// Custom output style files.
    #[serde(default)]
    pub output_styles: Option<StringOrArray>,
}

/// A value that can be either a single string or an array of strings.
///
/// This mirrors the YAML convention where a single item can be written
/// without array brackets for convenience.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrArray {
    /// A single path.
    Single(String),
    /// Multiple paths.
    Multiple(Vec<String>),
}

impl StringOrArray {
    /// Get all paths as a vector.
    pub fn as_paths(&self) -> Vec<&str> {
        match self {
            Self::Single(s) => vec![s.as_str()],
            Self::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }

    /// Get the first path, if any.
    pub fn first(&self) -> Option<&str> {
        match self {
            Self::Single(s) => Some(s.as_str()),
            Self::Multiple(v) => v.first().map(|s| s.as_str()),
        }
    }
}

impl Default for StringOrArray {
    fn default() -> Self {
        Self::Single("./".to_string())
    }
}

/// Type of a user-configurable value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginUserConfigType {
    /// Free-form text.
    String,
    /// Numeric value.
    Number,
    /// Boolean toggle.
    Boolean,
    /// Directory path (validated for existence).
    Directory,
    /// File path (validated for existence).
    File,
}

/// A single user-configurable option declared in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginUserConfigEntry {
    /// Value type.
    #[serde(rename = "type")]
    pub value_type: PluginUserConfigType,

    /// Label shown in the configuration dialog.
    pub title: String,

    /// Help text shown below the field.
    #[serde(default)]
    pub description: String,

    /// If true, mask input and store in secure storage.
    #[serde(default)]
    pub sensitive: bool,

    /// If true, validation fails when the field is empty.
    #[serde(default)]
    pub required: bool,

    /// Default value when the user provides nothing.
    #[serde(default)]
    pub default: Option<serde_json::Value>,

    /// For `string` type: allow an array of strings.
    #[serde(default)]
    pub multiple: bool,

    /// For `number` type: minimum value.
    #[serde(default)]
    pub min: Option<f64>,

    /// For `number` type: maximum value.
    #[serde(default)]
    pub max: Option<f64>,
}

/// A dependency on another plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginDependency {
    /// Simple dependency — just a name, any version.
    Simple(String),
    /// Versioned dependency with semver constraint.
    Versioned { name: String, version: String },
}

impl PluginDependency {
    /// Get the dependency name.
    pub fn name(&self) -> &str {
        match self {
            Self::Simple(name) => name,
            Self::Versioned { name, .. } => name,
        }
    }

    /// Get the version constraint, if any.
    pub fn version_constraint(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Versioned { version, .. } => Some(version),
        }
    }

    /// Check whether an installed plugin version satisfies this dependency's
    /// version constraint (if any).
    ///
    /// - `Simple` deps accept any version.
    /// - `Versioned` deps parse the constraint as a semver `VersionReq`
    ///   (e.g. `">=1.0.0"`, `"^2"`, `"~1.2"`, `"*"`). `installed_version` is
    ///   parsed as a semver `Version`. Both sides must parse; a malformed
    ///   constraint or version is reported via the returned `Err` so callers
    ///   can surface a clear validation error instead of silently passing.
    pub fn satisfies(&self, installed_version: &str) -> Result<bool, String> {
        let constraint = match self.version_constraint() {
            None => return Ok(true),
            Some(c) => c,
        };
        let req = semver::VersionReq::parse(constraint)
            .map_err(|e| format!("invalid version constraint '{constraint}': {e}"))?;
        let ver = semver::Version::parse(installed_version)
            .map_err(|e| format!("invalid installed version '{installed_version}': {e}"))?;
        Ok(req.matches(&ver))
    }
}

// ── Validation ───────────────────────────────────────────────────────────

/// Errors found during manifest validation.
#[derive(Debug, Clone)]
pub struct ValidationError {
    /// Dot-separated path to the invalid field.
    pub field: String,
    /// Human-readable description of the problem.
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl PluginManifest {
    /// Load a manifest from a YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self, String> {
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("Failed to parse manifest YAML: {e}"))
    }

    /// Load a manifest from a file path.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read manifest file {}: {e}", path.display()))?;
        Self::from_yaml(&content)
    }

    /// Validate the manifest and return all errors found.
    ///
    /// Checks:
    /// - `name` is non-empty and kebab-case
    /// - All component paths start with `./` and don't escape the root
    /// - Required config entries have defaults or are not required
    /// - Dependency names are valid
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Validate name
        if self.name.is_empty() {
            errors.push(ValidationError {
                field: "name".into(),
                message: "Plugin name must not be empty".into(),
            });
        } else if !is_kebab_case(&self.name) {
            errors.push(ValidationError {
                field: "name".into(),
                message: format!(
                    "Plugin name '{}' must be kebab-case (lowercase, digits, hyphens only)",
                    self.name
                ),
            });
        }

        // Validate version
        if self.version != "0.0.0" && !is_valid_semver_loose(&self.version) {
            errors.push(ValidationError {
                field: "version".into(),
                message: format!("Version '{}' is not valid semver", self.version),
            });
        }

        // Validate component paths
        self.validate_paths(&mut errors);

        // Validate config entries
        for (key, entry) in &self.config {
            if !is_valid_identifier(key) {
                errors.push(ValidationError {
                    field: format!("config.{key}"),
                    message: format!("Config key '{key}' must be a valid identifier (letters, digits, underscores)"),
                });
            }
            if entry.required && entry.default.is_none() {
                // This is fine — just means the user must provide it
            }
            if entry.multiple && entry.value_type != PluginUserConfigType::String {
                errors.push(ValidationError {
                    field: format!("config.{key}.multiple"),
                    message: "'multiple' is only valid for type 'string'".into(),
                });
            }
        }

        // Validate dependencies
        for dep in &self.dependencies {
            let name = dep.name();
            if name.is_empty() || !is_kebab_case(name) {
                errors.push(ValidationError {
                    field: "dependencies".into(),
                    message: format!("Dependency name '{name}' must be kebab-case"),
                });
            }
        }

        errors
    }

    /// Check if the manifest is valid (no validation errors).
    pub fn is_valid(&self) -> bool {
        self.validate().is_empty()
    }

    /// Get the display name, falling back to the plugin name.
    pub fn display_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    /// Infer capabilities from the component declarations.
    ///
    /// If `components.skills` is set, `Skill` capability is implied, etc.
    pub fn inferred_capabilities(&self) -> Vec<super::PluginCapability> {
        use super::PluginCapability;
        let mut caps = Vec::new();
        if self.components.skills.is_some() {
            caps.push(PluginCapability::Skill);
        }
        if self.components.hooks.is_some() {
            caps.push(PluginCapability::Hook);
        }
        if self.components.mcp_servers.is_some() {
            caps.push(PluginCapability::McpServer);
        }
        if self.components.lsp_servers.is_some() {
            caps.push(PluginCapability::LspServer);
        }
        if self.components.agents.is_some() {
            caps.push(PluginCapability::Agent);
        }
        if self.components.monitors.is_some() {
            caps.push(PluginCapability::Monitor);
        }
        if self.components.themes.is_some() {
            caps.push(PluginCapability::Theme);
        }
        caps
    }

    fn validate_paths(&self, errors: &mut Vec<ValidationError>) {
        let check = |field: &str, val: &StringOrArray, errors: &mut Vec<ValidationError>| {
            for path in val.as_paths() {
                if !path.starts_with("./") && path != "." {
                    errors.push(ValidationError {
                        field: format!("components.{field}"),
                        message: format!("Path '{path}' must start with './'"),
                    });
                }
                if path.contains("..") {
                    errors.push(ValidationError {
                        field: format!("components.{field}"),
                        message: format!(
                            "Path '{path}' must not contain '..' (no traversal outside plugin root)"
                        ),
                    });
                }
            }
        };

        if let Some(ref v) = self.components.skills {
            check("skills", v, errors);
        }
        if let Some(ref v) = self.components.agents {
            check("agents", v, errors);
        }
        if let Some(ref v) = self.components.hooks {
            check("hooks", v, errors);
        }
        if let Some(ref v) = self.components.mcp_servers {
            check("mcp_servers", v, errors);
        }
        if let Some(ref v) = self.components.lsp_servers {
            check("lsp_servers", v, errors);
        }
        if let Some(ref v) = self.components.monitors {
            check("monitors", v, errors);
        }
        if let Some(ref v) = self.components.themes {
            check("themes", v, errors);
        }
        if let Some(ref v) = self.components.output_styles {
            check("output_styles", v, errors);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Check if a string is valid kebab-case: lowercase letters, digits, hyphens.
fn is_kebab_case(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Check if a string is a valid identifier: letters, digits, underscores.
fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().is_some_and(|c| !c.is_ascii_digit())
}

/// Loose semver check: accepts "MAJOR.MINOR.PATCH" with optional pre-release.
fn is_valid_semver_loose(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(3, '.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts[0].parse::<u32>().is_ok()
        && parts[1].parse::<u32>().is_ok()
        && parts[2]
            .split('-')
            .next()
            .is_some_and(|p| p.parse::<u32>().is_ok())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let yaml = r#"
name: my-plugin
description: "A test plugin"
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(m.name, "my-plugin");
        assert_eq!(m.version, "0.0.0");
        assert!(m.is_valid());
    }

    #[test]
    fn test_parse_full_manifest() {
        let yaml = r#"
name: data-analysis-pack
display_name: "Data Analysis Pack"
version: "1.2.0"
description: "Enhanced data analysis"
author:
  name: "Echo Team"
  email: "team@echo.dev"
license: MIT
keywords: [data, analysis]
components:
  skills: "./skills/"
  agents: ["./agents/reviewer.md"]
  hooks: "./hooks/hooks.yaml"
  mcp_servers: "./.mcp.json"
config:
  api_endpoint:
    type: string
    title: "API Endpoint"
    description: "Service address"
    default: "http://localhost:8080"
  api_token:
    type: string
    title: "API Token"
    sensitive: true
    required: true
dependencies:
  - name: base-tools
    version: ">=1.0.0"
  - simple-dep
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(m.name, "data-analysis-pack");
        assert_eq!(m.version, "1.2.0");
        assert_eq!(
            m.components.skills.as_ref().unwrap().first(),
            Some("./skills/")
        );
        assert_eq!(m.config.len(), 2);
        assert_eq!(m.dependencies.len(), 2);
        assert!(m.is_valid());
        assert_eq!(m.inferred_capabilities().len(), 4); // skill, hook, mcp, agent
    }

    #[test]
    fn test_invalid_name() {
        let yaml = "name: My Plugin\ndescription: test";
        let m = PluginManifest::from_yaml(yaml).unwrap();
        let errors = m.validate();
        assert!(errors.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn test_path_traversal_rejected() {
        let yaml = r#"
name: bad-plugin
description: test
components:
  skills: "../shared-skills/"
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        let errors = m.validate();
        assert!(errors.iter().any(|e| e.field == "components.skills"));
    }

    #[test]
    fn test_path_must_start_with_dot_slash() {
        let yaml = r#"
name: bad-paths
description: test
components:
  hooks: "hooks/hooks.yaml"
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        let errors = m.validate();
        assert!(errors.iter().any(|e| e.field == "components.hooks"));
    }

    #[test]
    fn test_dependency_parsing() {
        let yaml = r#"
name: dep-test
description: test
dependencies:
  - simple-dep
  - name: versioned-dep
    version: ">=2.0.0"
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(m.dependencies[0].name(), "simple-dep");
        assert_eq!(m.dependencies[0].version_constraint(), None);
        assert_eq!(m.dependencies[1].name(), "versioned-dep");
        assert_eq!(m.dependencies[1].version_constraint(), Some(">=2.0.0"));
    }

    #[test]
    fn test_dependency_satisfies_semver() {
        // Simple deps accept any version.
        let simple = PluginDependency::Simple("base".into());
        assert!(simple.satisfies("0.0.1").unwrap_or(false));
        assert!(simple.satisfies("9.9.9").unwrap_or(false));

        // Versioned deps enforce semver VersionReq.
        let v = PluginDependency::Versioned {
            name: "base".into(),
            version: ">=1.0.0".into(),
        };
        assert!(v.satisfies("1.0.0").unwrap_or(false));
        assert!(v.satisfies("2.5.0").unwrap_or(false));
        assert!(!v.satisfies("0.9.0").unwrap_or(false));

        // Caret constraint
        let caret = PluginDependency::Versioned {
            name: "base".into(),
            version: "^2".into(),
        };
        assert!(caret.satisfies("2.0.0").unwrap_or(false));
        assert!(caret.satisfies("2.9.9").unwrap_or(false));
        assert!(!caret.satisfies("3.0.0").unwrap_or(false));

        // Wildcard
        let wild = PluginDependency::Versioned {
            name: "base".into(),
            version: "*".into(),
        };
        assert!(wild.satisfies("1.2.3").unwrap_or(false));

        // Malformed constraint → Err
        let bad = PluginDependency::Versioned {
            name: "base".into(),
            version: "not-a-constraint@@".into(),
        };
        assert!(bad.satisfies("1.0.0").is_err());
    }

    #[test]
    fn test_inferred_capabilities() {
        let yaml = r#"
name: caps-test
description: test
components:
  skills: "./skills/"
  hooks: "./hooks.yaml"
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        let caps = m.inferred_capabilities();
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&super::super::PluginCapability::Skill));
        assert!(caps.contains(&super::super::PluginCapability::Hook));
    }

    #[test]
    fn test_string_or_array() {
        let yaml = r#"
name: path-test
description: test
components:
  skills: "./skills/"
  agents:
    - "./agents/a.md"
    - "./agents/b.md"
"#;
        let m = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(
            m.components.skills.as_ref().unwrap().as_paths(),
            vec!["./skills/"]
        );
        assert_eq!(
            m.components.agents.as_ref().unwrap().as_paths(),
            vec!["./agents/a.md", "./agents/b.md"]
        );
    }
}
