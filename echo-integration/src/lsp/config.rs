//! LSP configuration loading from `.lsp.yaml` files.

use echo_core::lsp::LspServerConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level LSP configuration.
#[derive(Debug, Clone, Default)]
pub struct LspConfig {
    /// Map from language name to server configuration.
    pub servers: HashMap<String, LspServerConfig>,
}

impl LspConfig {
    /// Discover language servers already installed on the local machine.
    ///
    /// Discovery is deliberately conservative: a server is configured only
    /// when the project contains the corresponding language and its executable
    /// is present in `PATH`. Nothing is downloaded or installed.
    pub fn discover(project_root: &Path) -> Self {
        let search_paths = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .unwrap_or_default();
        Self::discover_with_search_paths(project_root, &search_paths)
    }

    pub fn discover_with_search_paths(project_root: &Path, search_paths: &[PathBuf]) -> Self {
        let mut config = Self::default();
        for definition in language_definitions() {
            if !project_uses_language(project_root, definition) {
                continue;
            }
            let executable = definition
                .executables
                .iter()
                .find(|candidate| command_available(candidate.command, search_paths));
            let Some(executable) = executable else {
                continue;
            };
            config.servers.insert(
                definition.language.to_string(),
                LspServerConfig {
                    language: definition.language.to_string(),
                    command: executable.command.to_string(),
                    args: executable
                        .args
                        .iter()
                        .map(|value| value.to_string())
                        .collect(),
                    extensions: definition
                        .extensions
                        .iter()
                        .map(|value| value.to_string())
                        .collect(),
                    env: HashMap::new(),
                    initialization_options: None,
                    max_restarts: 3,
                },
            );
        }
        config
    }

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

struct ExecutableDefinition {
    command: &'static str,
    args: &'static [&'static str],
}

struct LanguageDefinition {
    language: &'static str,
    markers: &'static [&'static str],
    extensions: &'static [&'static str],
    executables: &'static [ExecutableDefinition],
}

fn language_definitions() -> &'static [LanguageDefinition] {
    &[
        LanguageDefinition {
            language: "rust",
            markers: &["Cargo.toml"],
            extensions: &[".rs"],
            executables: &[ExecutableDefinition {
                command: "rust-analyzer",
                args: &[],
            }],
        },
        LanguageDefinition {
            language: "python",
            markers: &["pyproject.toml", "requirements.txt", "setup.py", "Pipfile"],
            extensions: &[".py", ".pyi"],
            executables: &[
                ExecutableDefinition {
                    command: "basedpyright-langserver",
                    args: &["--stdio"],
                },
                ExecutableDefinition {
                    command: "pyright-langserver",
                    args: &["--stdio"],
                },
                ExecutableDefinition {
                    command: "pylsp",
                    args: &[],
                },
            ],
        },
        LanguageDefinition {
            language: "typescript",
            markers: &["package.json", "tsconfig.json", "jsconfig.json"],
            extensions: &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"],
            executables: &[ExecutableDefinition {
                command: "typescript-language-server",
                args: &["--stdio"],
            }],
        },
        LanguageDefinition {
            language: "go",
            markers: &["go.mod", "go.work"],
            extensions: &[".go"],
            executables: &[ExecutableDefinition {
                command: "gopls",
                args: &[],
            }],
        },
        LanguageDefinition {
            language: "java",
            markers: &["pom.xml", "build.gradle", "build.gradle.kts"],
            extensions: &[".java"],
            executables: &[ExecutableDefinition {
                command: "jdtls",
                args: &[],
            }],
        },
        LanguageDefinition {
            language: "c_cpp",
            markers: &["CMakeLists.txt", "compile_commands.json", "meson.build"],
            extensions: &[".c", ".h", ".cc", ".cpp", ".cxx", ".hpp"],
            executables: &[ExecutableDefinition {
                command: "clangd",
                args: &[],
            }],
        },
    ]
}

fn project_uses_language(root: &Path, definition: &LanguageDefinition) -> bool {
    if definition
        .markers
        .iter()
        .any(|marker| root.join(marker).is_file())
    {
        return true;
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        if depth > 4 || visited >= 10_000 {
            continue;
        }
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            visited = visited.saturating_add(1);
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !matches!(
                    name.as_ref(),
                    ".git"
                        | "target"
                        | "node_modules"
                        | "vendor"
                        | ".venv"
                        | "venv"
                        | "dist"
                        | "build"
                ) {
                    stack.push((path, depth.saturating_add(1)));
                }
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}"));
            if extension.as_ref().is_some_and(|extension| {
                definition.extensions.iter().any(|value| value == extension)
            }) {
                return true;
            }
        }
    }
    false
}

fn command_available(command: &str, search_paths: &[PathBuf]) -> bool {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 {
        return command_path.is_file();
    }
    search_paths.iter().any(|directory| {
        let candidate = directory.join(command);
        if candidate.is_file() {
            return true;
        }
        if cfg!(windows) {
            return ["exe", "cmd", "bat"]
                .iter()
                .any(|extension| directory.join(format!("{command}.{extension}")).is_file());
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_config() -> Result<(), String> {
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
        let config = LspConfig::from_yaml(yaml)?;
        assert_eq!(config.servers.len(), 3);
        assert_eq!(
            config
                .get("python")
                .ok_or_else(|| "python config missing".to_string())?
                .command,
            "pyright-langserver"
        );
        assert_eq!(
            config
                .get_for_extension(".rs")
                .ok_or_else(|| "Rust extension missing".to_string())?
                .0,
            "rust"
        );
        assert_eq!(
            config
                .get_for_extension("py")
                .ok_or_else(|| "Python extension missing".to_string())?
                .0,
            "python"
        );
        Ok(())
    }

    #[test]
    fn discovers_installed_server_for_project_language() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let bin = tempfile::tempdir()?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname='demo'\n",
        )?;
        fs::write(bin.path().join("rust-analyzer"), "")?;
        let config =
            LspConfig::discover_with_search_paths(project.path(), &[bin.path().to_path_buf()]);
        let rust = config.get("rust").ok_or("rust server was not discovered")?;
        assert_eq!(rust.command, "rust-analyzer");
        Ok(())
    }
}
