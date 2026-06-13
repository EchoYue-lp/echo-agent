//! LSP configuration loading from `.lsp.yaml` files.

use echo_core::lsp::LspServerConfig;
use std::collections::HashMap;
use std::path::Path;

/// Top-level LSP configuration.
#[derive(Debug, Clone, Default)]
pub struct LspConfig {
    /// Map from language name to server configuration.
    pub servers: HashMap<String, LspServerConfig>,
}

impl LspConfig {
    /// Load configuration from a `.lsp.yaml` file.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read LSP config file {}: {e}", path.display()))?;
        Self::from_yaml(&content)
    }

    /// Parse configuration from a YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self, String> {
        let file: echo_core::lsp::LspConfigFile = serde_yaml_ng::from_str(yaml)
            .map_err(|e| format!("Failed to parse LSP config YAML: {e}"))?;
        Ok(Self {
            servers: file.languages,
        })
    }

    /// Get the configuration for a specific language.
    pub fn get(&self, language: &str) -> Option<&LspServerConfig> {
        self.servers.get(language)
    }

    /// Get the configuration for a file extension.
    pub fn get_for_extension(&self, ext: &str) -> Option<(&str, &LspServerConfig)> {
        let ext = if ext.starts_with('.') {
            ext
        } else {
            &format!(".{ext}")
        };
        for (lang, config) in &self.servers {
            if config.extensions.iter().any(|e| e == ext) {
                return Some((lang.as_str(), config));
            }
        }
        None
    }

    /// Merge another config into this one (other takes precedence).
    pub fn merge(&mut self, other: LspConfig) {
        self.servers.extend(other.servers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let yaml = r#"
languages:
  python:
    language: python
    command: pyright-langserver
    args: ["--stdio"]
    extensions: [".py", ".pyi"]
  typescript:
    language: typescript
    command: typescript-language-server
    args: ["--stdio"]
    extensions: [".ts", ".tsx", ".js", ".jsx"]
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
"#;
        let config = LspConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.servers.len(), 3);
        assert_eq!(config.get("python").unwrap().command, "pyright-langserver");
        assert_eq!(config.get_for_extension(".rs").unwrap().0, "rust");
        assert_eq!(config.get_for_extension("py").unwrap().0, "python");
    }
}
