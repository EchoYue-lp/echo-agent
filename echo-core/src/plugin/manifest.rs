//! Flat `plugin.json` manifest used by EchoAgent plugins.
//!
//! Portable Agent Plugins metadata and EchoAgent's local configuration share
//! one root document. Component locations are fixed by the package layout and
//! are therefore not repeated in the manifest.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Canonical Agent Plugins 1.0 manifest schema identifier.
pub const AGENT_PLUGIN_SCHEMA_V1: &str =
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// Root `plugin.json` document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    /// Selects the Agent Plugins validation and interpretation contract.
    #[serde(rename = "$schema")]
    pub schema: String,

    /// Portable plugin identifier.
    pub name: String,

    /// Optional UI label. Falls back to `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    /// Plugin version. Agent Plugins recommends SemVer but does not require it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Short human-readable description.
    #[serde(default)]
    pub description: String,

    /// Optional author metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,

    /// Documentation or homepage location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// Source repository location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// License string; an SPDX identifier is recommended by the standard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Search and discovery terms.
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Start enabled when configuration is complete.
    #[serde(default = "default_true")]
    pub default_enabled: bool,

    /// User-configurable values managed by the embedding application.
    #[serde(default)]
    pub config: HashMap<String, PluginUserConfigEntry>,

    /// Optional dependency ordering between installed plugins.
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,

    /// Agent Plugins 1.0 treats unknown top-level fields as non-fatal. Keep
    /// them so callers can report diagnostics without losing source data.
    #[serde(flatten)]
    unknown_fields: HashMap<String, serde_json::Value>,
}

/// Portable author metadata. Every member is optional in Agent Plugins 1.0.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginAuthor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Type of an EchoAgent user-configurable value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginUserConfigType {
    String,
    Number,
    Boolean,
    Directory,
    File,
}

/// One application-managed configuration value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginUserConfigEntry {
    #[serde(rename = "type")]
    pub value_type: PluginUserConfigType,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

/// Optional dependency on another installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginDependency {
    Simple(String),
    Versioned { name: String, version: String },
}

impl PluginDependency {
    pub fn name(&self) -> &str {
        match self {
            Self::Simple(name) => name,
            Self::Versioned { name, .. } => name,
        }
    }

    pub fn version_constraint(&self) -> Option<&str> {
        match self {
            Self::Simple(_) => None,
            Self::Versioned { version, .. } => Some(version),
        }
    }

    pub fn satisfies(&self, installed_version: Option<&str>) -> Result<bool, String> {
        let Some(constraint) = self.version_constraint() else {
            return Ok(true);
        };
        let version = installed_version
            .ok_or_else(|| format!("dependency '{}' has no installed version", self.name()))?;
        let requirement = semver::VersionReq::parse(constraint)
            .map_err(|error| format!("invalid version constraint '{constraint}': {error}"))?;
        let parsed = semver::Version::parse(version)
            .map_err(|error| format!("invalid installed version '{version}': {error}"))?;
        Ok(requirement.matches(&parsed))
    }
}

/// One fatal manifest validation error.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

impl PluginManifest {
    /// Parse a root Agent Plugins manifest.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|error| format!("Failed to parse plugin.json: {error}"))
    }

    /// Load a root Agent Plugins manifest.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|error| format!("Failed to read manifest file {}: {error}", path.display()))?;
        Self::from_json(&content)
    }

    /// Non-fatal unknown fields that a conforming client reports and ignores.
    pub fn unknown_top_level_fields(&self) -> Vec<&str> {
        let mut fields = self
            .unknown_fields
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        fields.sort_unstable();
        fields
    }

    /// Validate portable metadata plus embedding application's root configuration fields.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.schema != AGENT_PLUGIN_SCHEMA_V1 {
            errors.push(ValidationError {
                field: "$schema".to_string(),
                message: format!(
                    "Unsupported Agent Plugins schema '{}'; expected '{AGENT_PLUGIN_SCHEMA_V1}'",
                    self.schema
                ),
            });
        }
        if !is_agent_plugin_name(&self.name) {
            errors.push(ValidationError {
                field: "name".to_string(),
                message: "Plugin name must be 1-64 lowercase ASCII letters, digits, hyphens, or periods; begin and end alphanumeric; and contain neither '--' nor '..'"
                    .to_string(),
            });
        }

        self.validate_plugin_fields(&mut errors);
        errors
    }

    pub fn is_valid(&self) -> bool {
        self.validate().is_empty()
    }

    pub fn version_label(&self) -> &str {
        self.version.as_deref().unwrap_or("unspecified")
    }

    pub fn display_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    pub fn resolve_user_config(
        &self,
        provided: &HashMap<String, serde_json::Value>,
    ) -> Result<HashMap<String, serde_json::Value>, Vec<ValidationError>> {
        let mut resolved = self.user_config_defaults();
        let mut errors = provided
            .keys()
            .filter(|key| !self.config.contains_key(*key))
            .map(|key| ValidationError {
                field: format!("config.{key}"),
                message: "Unknown plugin configuration key".to_string(),
            })
            .collect::<Vec<_>>();
        resolved.extend(
            provided
                .iter()
                .filter(|(key, value)| self.config.contains_key(*key) && !value.is_null())
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        errors.extend(self.validate_user_config(&resolved));
        if errors.is_empty() {
            Ok(resolved)
        } else {
            Err(errors)
        }
    }

    pub fn user_config_defaults(&self) -> HashMap<String, serde_json::Value> {
        self.config
            .iter()
            .filter_map(|(key, entry)| {
                entry
                    .default
                    .as_ref()
                    .map(|value| (key.clone(), value.clone()))
            })
            .collect()
    }

    pub fn validate_user_config(
        &self,
        values: &HashMap<String, serde_json::Value>,
    ) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        for key in values.keys() {
            if !self.config.contains_key(key) {
                errors.push(ValidationError {
                    field: format!("config.{key}"),
                    message: "Unknown plugin configuration key".to_string(),
                });
            }
        }
        for (key, entry) in &self.config {
            let value = values.get(key);
            if value.is_none_or(serde_json::Value::is_null) {
                if entry.required {
                    errors.push(ValidationError {
                        field: format!("config.{key}"),
                        message: "Required plugin configuration value is missing".to_string(),
                    });
                }
                continue;
            }
            if let Some(value) = value
                && let Some(message) = validate_config_value(entry, value)
            {
                errors.push(ValidationError {
                    field: format!("config.{key}"),
                    message,
                });
            }
        }
        errors
    }

    fn validate_plugin_fields(&self, errors: &mut Vec<ValidationError>) {
        for (key, entry) in &self.config {
            if !is_valid_identifier(key) {
                errors.push(ValidationError {
                    field: format!("config.{key}"),
                    message: "Config keys must start with a letter or underscore and contain only ASCII letters, digits, or underscores"
                        .to_string(),
                });
            }
            if entry.multiple && entry.value_type != PluginUserConfigType::String {
                errors.push(ValidationError {
                    field: format!("config.{key}.multiple"),
                    message: "'multiple' is only valid for type 'string'".to_string(),
                });
            }
            if (entry.min.is_some() || entry.max.is_some())
                && entry.value_type != PluginUserConfigType::Number
            {
                errors.push(ValidationError {
                    field: format!("config.{key}"),
                    message: "'min' and 'max' are only valid for type 'number'".to_string(),
                });
            }
            if let (Some(minimum), Some(maximum)) = (entry.min, entry.max)
                && minimum > maximum
            {
                errors.push(ValidationError {
                    field: format!("config.{key}"),
                    message: "'min' must not be greater than 'max'".to_string(),
                });
            }
            if let Some(default) = entry.default.as_ref()
                && let Some(message) = validate_config_value(entry, default)
            {
                errors.push(ValidationError {
                    field: format!("config.{key}.default"),
                    message,
                });
            }
        }
        for dependency in &self.dependencies {
            if !is_agent_plugin_name(dependency.name()) {
                errors.push(ValidationError {
                    field: "dependencies".to_string(),
                    message: format!(
                        "Dependency name '{}' is not a valid Agent Plugin name",
                        dependency.name()
                    ),
                });
            }
        }
    }
}

fn validate_config_value(
    entry: &PluginUserConfigEntry,
    value: &serde_json::Value,
) -> Option<String> {
    match entry.value_type {
        PluginUserConfigType::String if entry.multiple => {
            let Some(values) = value.as_array() else {
                return Some("Expected an array of strings".to_string());
            };
            if values.iter().any(|item| !item.is_string()) {
                return Some("Expected an array containing only strings".to_string());
            }
            if entry.required && values.is_empty() {
                return Some("Required value must not be empty".to_string());
            }
        }
        PluginUserConfigType::String => {
            let Some(text) = value.as_str() else {
                return Some("Expected a string".to_string());
            };
            if entry.required && text.is_empty() {
                return Some("Required value must not be empty".to_string());
            }
        }
        PluginUserConfigType::Number => {
            let Some(number) = value.as_f64() else {
                return Some("Expected a number".to_string());
            };
            if let Some(minimum) = entry.min
                && number < minimum
            {
                return Some(format!("Value must be at least {minimum}"));
            }
            if let Some(maximum) = entry.max
                && number > maximum
            {
                return Some(format!("Value must be at most {maximum}"));
            }
        }
        PluginUserConfigType::Boolean => {
            if !value.is_boolean() {
                return Some("Expected a boolean".to_string());
            }
        }
        PluginUserConfigType::Directory | PluginUserConfigType::File => {
            let Some(path) = value.as_str() else {
                return Some("Expected a filesystem path string".to_string());
            };
            let exists = match entry.value_type {
                PluginUserConfigType::Directory => Path::new(path).is_dir(),
                PluginUserConfigType::File => Path::new(path).is_file(),
                _ => false,
            };
            if !exists {
                return Some(match entry.value_type {
                    PluginUserConfigType::Directory => format!("Directory does not exist: {path}"),
                    PluginUserConfigType::File => format!("File does not exist: {path}"),
                    _ => String::new(),
                });
            }
        }
    }
    None
}

fn is_agent_plugin_name(name: &str) -> bool {
    let len = name.chars().count();
    (1..=64).contains(&len)
        && name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && name
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && name.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '.'
        })
        && !name.contains("--")
        && !name.contains("..")
}

fn is_valid_identifier(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: serde_json::Value) -> Result<PluginManifest, String> {
        let mut document = serde_json::json!({
            "$schema": AGENT_PLUGIN_SCHEMA_V1,
            "name": "example.plugin",
            "version": "1.2.0"
        });
        let fields = extra
            .as_object()
            .ok_or_else(|| "test manifest fields must be an object".to_string())?;
        let object = document
            .as_object_mut()
            .ok_or_else(|| "test manifest must be an object".to_string())?;
        object.extend(fields.clone());
        PluginManifest::from_json(
            &serde_json::to_string(&document).map_err(|error| error.to_string())?,
        )
    }

    #[test]
    fn parses_flat_plugin_manifest() -> Result<(), String> {
        let parsed = manifest(serde_json::json!({
            "displayName": "Example Plugin"
        }))?;
        assert!(parsed.is_valid());
        assert_eq!(parsed.display_name(), "Example Plugin");
        assert_eq!(parsed.version_label(), "1.2.0");
        Ok(())
    }

    #[test]
    fn rejects_missing_or_unsupported_schema() -> Result<(), String> {
        let parsed = PluginManifest::from_json(r#"{"name":"example"}"#)
            .err()
            .unwrap_or_default();
        assert!(parsed.contains("$schema"));

        let parsed = PluginManifest::from_json(
            r#"{"$schema":"https://example.invalid/plugin.json","name":"example"}"#,
        )?;
        assert!(
            parsed
                .validate()
                .iter()
                .any(|error| error.field == "$schema")
        );
        Ok(())
    }

    #[test]
    fn supports_standard_name_periods_and_rejects_double_separators() {
        assert!(is_agent_plugin_name("acme.tools"));
        assert!(!is_agent_plugin_name("acme..tools"));
        assert!(!is_agent_plugin_name("acme--tools"));
        assert!(!is_agent_plugin_name("Uppercase"));
    }

    #[test]
    fn unknown_top_level_fields_are_non_fatal() -> Result<(), String> {
        let parsed = PluginManifest::from_json(&format!(
            r#"{{"$schema":"{AGENT_PLUGIN_SCHEMA_V1}","name":"example","future":true}}"#
        ))?;
        assert!(parsed.is_valid());
        assert_eq!(parsed.unknown_top_level_fields(), vec!["future"]);
        Ok(())
    }

    #[test]
    fn validates_root_configuration() -> Result<(), String> {
        let parsed = manifest(serde_json::json!({
            "config": {
                "retries": {
                    "type": "number",
                    "title": "Retries",
                    "min": 5,
                    "max": 2
                }
            }
        }))?;
        let errors = parsed.validate();
        assert!(errors.iter().any(|error| error.field.contains("retries")));
        Ok(())
    }

    #[test]
    fn dependency_constraints_handle_missing_versions() {
        let dependency = PluginDependency::Versioned {
            name: "base.tools".to_string(),
            version: ">=1.0.0".to_string(),
        };
        assert!(dependency.satisfies(Some("1.2.0")).unwrap_or(false));
        assert!(dependency.satisfies(None).is_err());
    }

    #[test]
    fn resolves_and_validates_user_configuration() -> Result<(), String> {
        let parsed = manifest(serde_json::json!({
            "config": {
                "endpoint": {
                    "type": "string",
                    "title": "Endpoint",
                    "required": true,
                    "default": "https://example.com"
                }
            }
        }))?;
        let resolved = parsed
            .resolve_user_config(&HashMap::new())
            .map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
        assert_eq!(
            resolved.get("endpoint").and_then(serde_json::Value::as_str),
            Some("https://example.com")
        );
        let invalid = HashMap::from([(
            "endpoint".to_string(),
            serde_json::json!(["https://example.com"]),
        )]);
        assert!(!parsed.validate_user_config(&invalid).is_empty());
        Ok(())
    }
}
