//! LSP Manager — manages multiple language server processes.
//!
//! The `LspManager` owns a collection of `StdioLspClient` instances,
//! one per configured language. It handles starting, stopping, and
//! routing requests to the appropriate server based on file extension.

use echo_core::lsp::{LspClient, LspServerConfig, LspServerStatus};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::client::StdioLspClient;
use super::config::LspConfig;

/// Manages multiple language server processes.
///
/// Each language gets its own `StdioLspClient` instance. The manager
/// routes requests to the appropriate server based on file extension.
pub struct LspManager {
    /// Active clients, keyed by language name.
    clients: HashMap<String, Arc<RwLock<StdioLspClient>>>,
    /// Configuration for each language.
    configs: HashMap<String, LspServerConfig>,
    /// Extension → language mapping.
    extension_map: HashMap<String, String>,
    /// Project root URI (e.g., `file:///path/to/project`).
    project_root_uri: Option<String>,
}

impl LspManager {
    /// Create a new empty manager.
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            configs: HashMap::new(),
            extension_map: HashMap::new(),
            project_root_uri: None,
        }
    }

    /// Load configuration from an `LspConfig`.
    pub fn load_config(&mut self, config: &LspConfig) {
        for (lang, server_config) in &config.servers {
            self.configs.insert(lang.clone(), server_config.clone());
            for ext in &server_config.extensions {
                let ext = if ext.starts_with('.') {
                    ext.clone()
                } else {
                    format!(".{ext}")
                };
                self.extension_map.insert(ext, lang.clone());
            }
        }
    }

    /// Set the project root directory.
    pub fn set_project_root(&mut self, root: &Path) {
        let uri = format!("file://{}", root.display());
        self.project_root_uri = Some(uri);
    }

    /// Start a language server for the given language.
    pub async fn start_server(&mut self, language: &str) -> Result<(), String> {
        let config = self
            .configs
            .get(language)
            .cloned()
            .ok_or_else(|| format!("No configuration for language: {language}"))?;

        let mut client = StdioLspClient::new(config);

        // Initialize with project root
        let root_uri = self.project_root_uri.as_deref().unwrap_or("file:///");

        client
            .initialize(root_uri)
            .await
            .map_err(|e| format!("Failed to initialize {language} server: {e}"))?;

        self.clients
            .insert(language.to_string(), Arc::new(RwLock::new(client)));

        tracing::info!("LSP server started for language: {language}");
        Ok(())
    }

    /// Stop a language server.
    pub async fn stop_server(&mut self, language: &str) -> Result<(), String> {
        if let Some(client) = self.clients.remove(language) {
            let mut client = client.write().await;
            client
                .shutdown()
                .await
                .map_err(|e| format!("Failed to shutdown {language} server: {e}"))?;
            tracing::info!("LSP server stopped for language: {language}");
        }
        Ok(())
    }

    /// Restart a language server.
    pub async fn restart_server(&mut self, language: &str) -> Result<(), String> {
        self.stop_server(language).await.ok();
        self.start_server(language).await
    }

    /// Get a client for the given file path (based on extension).
    pub async fn get_client_for_file(
        &self,
        file_path: &str,
    ) -> Option<(String, Arc<RwLock<StdioLspClient>>)> {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))?;

        let language = self.extension_map.get(&ext)?;
        let client = self.clients.get(language)?;
        Some((language.clone(), client.clone()))
    }

    /// Get a client for a specific language.
    pub fn get_client(&self, language: &str) -> Option<Arc<RwLock<StdioLspClient>>> {
        self.clients.get(language).cloned()
    }

    /// List all configured languages.
    pub fn configured_languages(&self) -> Vec<&str> {
        self.configs.keys().map(|s| s.as_str()).collect()
    }

    /// List all running servers.
    pub fn running_servers(&self) -> Vec<&str> {
        self.clients.keys().map(|s| s.as_str()).collect()
    }

    /// Get status of all servers.
    pub async fn status_all(&self) -> Vec<LspServerStatus> {
        let mut statuses = Vec::new();

        // Running servers
        for (_lang, client) in &self.clients {
            let client = client.read().await;
            statuses.push(client.status());
        }

        // Configured but not running
        for lang in self.configs.keys() {
            if !self.clients.contains_key(lang) {
                statuses.push(LspServerStatus {
                    language: lang.clone(),
                    running: false,
                    initialized: false,
                    restart_count: 0,
                    last_error: None,
                    pid: None,
                });
            }
        }

        statuses
    }

    /// Shutdown all servers.
    pub async fn shutdown_all(&mut self) {
        let languages: Vec<String> = self.clients.keys().cloned().collect();
        for lang in languages {
            let _ = self.stop_server(&lang).await;
        }
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config() {
        let mut manager = LspManager::new();
        let config = LspConfig::from_yaml(
            r#"
languages:
  python:
    language: python
    command: pyright-langserver
    args: ["--stdio"]
    extensions: [".py", ".pyi"]
  rust:
    language: rust
    command: rust-analyzer
    args: []
    extensions: [".rs"]
"#,
        )
        .unwrap();

        manager.load_config(&config);
        assert_eq!(manager.configured_languages().len(), 2);
        assert!(manager.configured_languages().contains(&"python"));
        assert!(manager.configured_languages().contains(&"rust"));
    }

    #[test]
    fn test_extension_mapping() {
        let mut manager = LspManager::new();
        let config = LspConfig::from_yaml(
            r#"
languages:
  python:
    language: python
    command: pyright-langserver
    args: []
    extensions: [".py"]
"#,
        )
        .unwrap();

        manager.load_config(&config);
        assert_eq!(
            manager.extension_map.get(".py"),
            Some(&"python".to_string())
        );
    }
}
