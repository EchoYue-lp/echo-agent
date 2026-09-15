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

use super::client::{LspLifecycle, StdioLspClient};
use super::config::LspConfig;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_MANAGER_GENERATION: AtomicU64 = AtomicU64::new(1);

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
    /// Shared close fence for all clients derived from this manager.
    lifecycle: Arc<LspLifecycle>,
}

impl LspManager {
    /// Create a new empty manager.
    pub fn new() -> Self {
        let generation = NEXT_MANAGER_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .unwrap_or(1);
        Self {
            clients: HashMap::new(),
            configs: HashMap::new(),
            extension_map: HashMap::new(),
            project_root_uri: None,
            lifecycle: Arc::new(LspLifecycle::new(generation)),
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
        if !self.lifecycle.is_live(self.lifecycle.generation) {
            return Err("LSP manager is closed".to_string());
        }
        let config = self
            .configs
            .get(language)
            .cloned()
            .ok_or_else(|| format!("No configuration for language: {language}"))?;

        // Replacing a language entry must first close the previous client.
        // Otherwise a retained derived handle would remain live after the
        // manager map points at the replacement and could spawn an unowned
        // child.
        if self.clients.contains_key(language) {
            self.stop_server(language).await?;
        }

        let mut client = StdioLspClient::new_bound(
            config,
            Arc::clone(&self.lifecycle),
            self.lifecycle.generation,
        );

        // Initialize with project root
        let root_uri = self.project_root_uri.as_deref().unwrap_or("file:///");

        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client.initialize(root_uri),
        )
        .await
        .map_err(|_| format!("Timed out initializing {language} server"))?
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
        if !self.lifecycle.is_live(self.lifecycle.generation) {
            return None;
        }
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
        if !self.lifecycle.is_live(self.lifecycle.generation) {
            return None;
        }
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
        for client in self.clients.values() {
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
        // Close the fence first. Handles retained by SDK callers then become
        // stale immediately, even while child teardown awaits I/O.
        self.lifecycle.close();
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
    use std::fs;

    fn test_config(language: &str) -> LspServerConfig {
        LspServerConfig {
            language: language.to_string(),
            command: "nonexistent-lsp-test-command".to_string(),
            args: Vec::new(),
            extensions: vec![".test".to_string()],
            env: HashMap::new(),
            initialization_options: None,
            max_restarts: 0,
        }
    }

    #[tokio::test]
    async fn retained_client_is_stale_after_manager_shutdown() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        let client = StdioLspClient::new_bound(
            test_config(language),
            Arc::clone(&manager.lifecycle),
            manager.lifecycle.generation,
        );
        manager
            .clients
            .insert(language.to_string(), Arc::new(RwLock::new(client)));
        let retained = manager
            .get_client(language)
            .ok_or_else(|| "test client was not registered".to_string())?;

        manager.shutdown_all().await;

        let mut retained = retained.write().await;
        let error = retained
            .initialize("file:///tmp/lsp-test")
            .await
            .err()
            .ok_or_else(|| "stale client unexpectedly initialized".to_string())?;
        if !matches!(error, echo_core::lsp::LspError::NotInitialized) {
            return Err(format!("unexpected stale client error: {error}"));
        }
        if retained.is_running() {
            return Err("stale client reports a running child".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn replacing_language_client_closes_retained_handle() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        let client = StdioLspClient::new_bound(
            test_config(language),
            Arc::clone(&manager.lifecycle),
            manager.lifecycle.generation,
        );
        manager
            .clients
            .insert(language.to_string(), Arc::new(RwLock::new(client)));
        let retained = manager
            .get_client(language)
            .ok_or_else(|| "test client was not registered".to_string())?;

        let config = LspConfig {
            servers: [(language.to_string(), test_config(language))]
                .into_iter()
                .collect(),
        };
        manager.load_config(&config);
        let error = manager
            .start_server(language)
            .await
            .err()
            .ok_or_else(|| "test server unexpectedly started".to_string())?;
        if !error.contains("Failed to initialize") {
            return Err(format!("unexpected replacement error: {error}"));
        }

        let mut retained = retained.write().await;
        let stale_error = retained
            .initialize("file:///tmp/lsp-test")
            .await
            .err()
            .ok_or_else(|| "replaced client unexpectedly initialized".to_string())?;
        if !matches!(stale_error, echo_core::lsp::LspError::NotInitialized) {
            return Err(format!("unexpected stale replacement error: {stale_error}"));
        }
        Ok(())
    }

    #[test]
    fn test_load_config() -> Result<(), String> {
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
        )?;

        manager.load_config(&config);
        assert_eq!(manager.configured_languages().len(), 2);
        assert!(manager.configured_languages().contains(&"python"));
        assert!(manager.configured_languages().contains(&"rust"));
        Ok(())
    }

    #[test]
    fn test_extension_mapping() -> Result<(), String> {
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
        )?;

        manager.load_config(&config);
        assert_eq!(
            manager.extension_map.get(".py"),
            Some(&"python".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "opt-in live LSP smoke test; set ECHO_AGENT_LSP_SMOKE=1"]
    async fn live_installed_servers_initialize_on_real_project_fixture() -> Result<(), String> {
        if std::env::var("ECHO_AGENT_LSP_SMOKE").as_deref() != Ok("1") {
            return Err(
                "set ECHO_AGENT_LSP_SMOKE=1 before running ignored LSP smoke tests".to_string(),
            );
        }
        let project = tempfile::tempdir().map_err(|error| error.to_string())?;
        fs::create_dir_all(project.path().join("src")).map_err(|error| error.to_string())?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"eko-lsp-smoke\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .map_err(|error| error.to_string())?;
        fs::write(
            project.path().join("src/lib.rs"),
            "pub fn answer() -> u32 { 42 }\n",
        )
        .map_err(|error| error.to_string())?;
        fs::write(
            project.path().join("package.json"),
            "{\"name\":\"eko-lsp-smoke\",\"private\":true}",
        )
        .map_err(|error| error.to_string())?;
        fs::write(
            project.path().join("tsconfig.json"),
            "{\"compilerOptions\":{\"strict\":true}}",
        )
        .map_err(|error| error.to_string())?;
        fs::write(
            project.path().join("index.ts"),
            "export const answer: number = 42;\n",
        )
        .map_err(|error| error.to_string())?;
        fs::write(
            project.path().join("pyproject.toml"),
            "[project]\nname = \"eko-lsp-smoke\"\nversion = \"0.1.0\"\n",
        )
        .map_err(|error| error.to_string())?;
        fs::write(project.path().join("main.py"), "answer: int = 42\n")
            .map_err(|error| error.to_string())?;

        let config = LspConfig::discover(project.path());
        if config.servers.is_empty() {
            return Err("no supported language server was found on PATH".to_string());
        }
        let mut manager = LspManager::new();
        manager.load_config(&config);
        manager.set_project_root(project.path());
        let languages = manager
            .configured_languages()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        for language in &languages {
            manager.start_server(language).await?;
        }
        let statuses = manager.status_all().await;
        if statuses.iter().any(|status| !status.initialized) {
            manager.shutdown_all().await;
            return Err("at least one discovered language server did not initialize".to_string());
        }
        manager.shutdown_all().await;
        Ok(())
    }
}
