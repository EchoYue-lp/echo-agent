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
    /// Last known status survives removal of an owned client.
    settled: HashMap<String, LspServerStatus>,
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
            settled: HashMap::new(),
            configs: HashMap::new(),
            extension_map: HashMap::new(),
            project_root_uri: None,
            lifecycle: Arc::new(LspLifecycle::new(generation)),
        }
    }

    /// Load configuration from an `LspConfig`.
    pub fn load_config(&mut self, config: &LspConfig) -> Result<(), String> {
        if !self.lifecycle.is_live(self.lifecycle.generation) {
            return Err("LSP manager is closed".to_string());
        }
        if !self.clients.is_empty() {
            return Err(
                "Stop servers or use reload_config before changing LSP configuration".to_string(),
            );
        }
        for (lang, server_config) in &config.servers {
            self.configs.insert(lang.clone(), server_config.clone());
        }
        self.rebuild_extensions();
        Ok(())
    }

    /// Replace the complete configuration after awaiting teardown of old children.
    pub async fn reload_config(&mut self, config: &LspConfig) -> Result<(), String> {
        if !self.lifecycle.is_live(self.lifecycle.generation) {
            return Err("LSP manager is closed".to_string());
        }
        let languages: Vec<String> = self.clients.keys().cloned().collect();
        for language in languages {
            self.stop_server(&language).await?;
        }
        self.configs = config.servers.clone();
        self.settled.clear();
        self.rebuild_extensions();
        Ok(())
    }

    fn rebuild_extensions(&mut self) {
        self.extension_map.clear();
        for (lang, server_config) in &self.configs {
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
        if let Some(status) = self.settled.get(language) {
            client.set_restart_count(status.restart_count);
        }

        // Initialize with project root
        let root_uri = self.project_root_uri.as_deref().unwrap_or("file:///");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client.initialize(root_uri),
        )
        .await
        .map_err(|_| format!("Timed out initializing {language} server"))
        .and_then(|result| {
            result.map_err(|e| format!("Failed to initialize {language} server: {e}"))
        });
        if let Err(error) = result {
            // A timed-out initialize future is cancelled, so settle its child
            // and I/O tasks before publishing the failed status.
            let _ = client.shutdown().await;
            let mut status = client.status();
            status.running = false;
            status.initialized = false;
            status.pid = None;
            status.last_error = Some(error.clone());
            self.settled.insert(language.to_string(), status);
            return Err(error);
        }

        self.settled.remove(language);

        self.clients
            .insert(language.to_string(), Arc::new(RwLock::new(client)));

        tracing::info!("LSP server started for language: {language}");
        Ok(())
    }

    /// Stop a language server.
    pub async fn stop_server(&mut self, language: &str) -> Result<(), String> {
        if let Some(client) = self.clients.remove(language) {
            let mut client = client.write().await;
            let result = client
                .shutdown()
                .await
                .map_err(|e| format!("Failed to shutdown {language} server: {e}"));
            let mut status = client.status();
            status.running = false;
            status.initialized = false;
            status.pid = None;
            if let Err(error) = &result {
                status.last_error = Some(error.clone());
            }
            self.settled.insert(language.to_string(), status);
            tracing::info!("LSP server stopped for language: {language}");
            result?;
        }
        Ok(())
    }

    /// Restart a language server.
    pub async fn restart_server(&mut self, language: &str) -> Result<(), String> {
        if !self.lifecycle.is_live(self.lifecycle.generation) {
            return Err("LSP manager is closed".to_string());
        }
        let limit = self
            .configs
            .get(language)
            .ok_or_else(|| format!("No configuration for language: {language}"))?
            .max_restarts;
        let status = if let Some(client) = self.clients.get(language) {
            client.read().await.status()
        } else {
            self.settled
                .get(language)
                .cloned()
                .unwrap_or_else(|| self.empty_status(language))
        };
        if status.restart_count >= limit {
            return Err(format!("Restart limit reached for {language}: {limit}"));
        }
        let next = status.restart_count.saturating_add(1);
        self.stop_server(language).await?;
        let mut settled = self.settled.remove(language).unwrap_or(status);
        settled.restart_count = next;
        self.settled.insert(language.to_string(), settled);
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
        if !client.read().await.is_initialized() {
            return None;
        }
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
        self.clients
            .iter()
            .filter_map(|(language, client)| {
                client
                    .try_read()
                    .ok()
                    .filter(|client| client.is_running())
                    .map(|_| language.as_str())
            })
            .collect()
    }

    fn empty_status(&self, language: &str) -> LspServerStatus {
        LspServerStatus {
            language: language.to_string(),
            running: false,
            initialized: false,
            restart_count: 0,
            last_error: None,
            pid: None,
        }
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
                statuses.push(
                    self.settled
                        .get(lang)
                        .cloned()
                        .unwrap_or_else(|| self.empty_status(lang)),
                );
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

    const MOCK_LSP: &str = r#"
import json, sys
mode = sys.argv[1]
while True:
    line = sys.stdin.buffer.readline()
    if not line:
        break
    if not line.lower().startswith(b'content-length:'):
        continue
    size = int(line.split(b':', 1)[1].strip())
    sys.stdin.buffer.readline()
    message = json.loads(sys.stdin.buffer.read(size))
    method = message.get('method')
    if method == 'initialize':
        body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'capabilities':{}}}).encode()
    elif method == 'shutdown':
        body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{}}).encode()
    elif mode == 'eof' and method == 'initialized':
        break
    elif mode == 'request_eof' and method == 'textDocument/definition':
        break
    else:
        continue
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
"#;

    fn mock_config(language: &str, mode: &str, max_restarts: u32) -> LspServerConfig {
        LspServerConfig {
            language: language.to_string(),
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                "-c".to_string(),
                MOCK_LSP.to_string(),
                mode.to_string(),
            ],
            extensions: vec![".test".to_string()],
            env: HashMap::new(),
            initialization_options: None,
            max_restarts,
        }
    }

    async fn wait_for_exit(
        manager: &LspManager,
        language: &str,
    ) -> Result<LspServerStatus, String> {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(status) = manager
                    .status_all()
                    .await
                    .into_iter()
                    .find(|status| status.language == language)
                    && !status.running
                {
                    return status;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "language server did not reach EOF".to_string())
    }

    #[tokio::test]
    async fn eof_clears_runtime_and_restart_exhaustion_preserves_error() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        manager.load_config(&LspConfig {
            servers: [(language.to_string(), mock_config(language, "eof", 1))]
                .into_iter()
                .collect(),
        })?;
        manager.start_server(language).await?;
        let old = manager.get_client(language).ok_or("missing first client")?;
        let exited = wait_for_exit(&manager, language).await?;
        if exited.initialized || exited.pid.is_some() || exited.last_error.is_none() {
            return Err(format!("EOF did not settle status: {exited:?}"));
        }
        if old
            .read()
            .await
            .diagnostics("file:///example.test")
            .await
            .is_ok()
        {
            return Err("EOF retained diagnostics access".to_string());
        }
        manager.restart_server(language).await?;
        let restarted = wait_for_exit(&manager, language).await?;
        if restarted.restart_count != 1 || restarted.last_error.is_none() {
            return Err(format!("restart status lost attempt/error: {restarted:?}"));
        }
        if manager.restart_server(language).await.is_ok() {
            return Err("restart exceeded configured limit".to_string());
        }
        manager.shutdown_all().await;
        Ok(())
    }

    #[tokio::test]
    async fn pending_request_settles_when_server_closes_stdout() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        manager.load_config(&LspConfig {
            servers: [(
                language.to_string(),
                mock_config(language, "request_eof", 1),
            )]
            .into_iter()
            .collect(),
        })?;
        manager.start_server(language).await?;
        let client = manager.get_client(language).ok_or("missing client")?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            client
                .read()
                .await
                .goto_definition(
                    "file:///example.test",
                    echo_core::lsp::Position {
                        line: 0,
                        character: 0,
                    },
                )
                .await
        })
        .await
        .map_err(|_| "pending request waited for the full request timeout".to_string())?;
        if result.is_ok()
            || wait_for_exit(&manager, language)
                .await?
                .last_error
                .is_none()
        {
            return Err("EOF did not fail pending request and status".to_string());
        }
        manager.shutdown_all().await;
        Ok(())
    }

    #[tokio::test]
    async fn reload_waits_for_old_owner_and_replaces_routes() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        manager.load_config(&LspConfig {
            servers: [(language.to_string(), mock_config(language, "stable", 2))]
                .into_iter()
                .collect(),
        })?;
        manager.start_server(language).await?;
        let old = manager.get_client(language).ok_or("missing old client")?;
        let mut next = mock_config("next", "stable", 2);
        next.extensions = vec![".next".to_string()];
        let replacement = LspConfig {
            servers: [("next".to_string(), next)].into_iter().collect(),
        };
        if manager.load_config(&replacement).is_ok() {
            return Err("synchronous load changed a running configuration".to_string());
        }
        if manager.get_client_for_file("example.test").await.is_none() {
            return Err("rejected load changed extension routing".to_string());
        }
        manager.reload_config(&replacement).await?;
        if manager.get_client_for_file("example.test").await.is_some()
            || manager.configured_languages() != ["next"]
            || old.read().await.is_running()
        {
            return Err("reload retained old route or process".to_string());
        }
        let error = old
            .write()
            .await
            .initialize("file:///")
            .await
            .err()
            .ok_or("old handle revived")?;
        if !matches!(error, echo_core::lsp::LspError::NotInitialized) {
            return Err(format!("old handle returned wrong error: {error}"));
        }
        manager.start_server("next").await?;
        if manager.get_client_for_file("example.next").await.is_none() {
            return Err("new extension was not routed".to_string());
        }
        manager.shutdown_all().await;
        if manager.load_config(&replacement).is_ok()
            || manager.reload_config(&replacement).await.is_ok()
            || manager.restart_server("next").await.is_ok()
        {
            return Err("closed manager accepted a new lifecycle".to_string());
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_start_and_repeated_start_leave_honest_status() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        let mut broken = test_config(language);
        broken.max_restarts = 1;
        manager.load_config(&LspConfig {
            servers: [(language.to_string(), broken)].into_iter().collect(),
        })?;
        let error = manager
            .start_server(language)
            .await
            .err()
            .ok_or("invalid command started")?;
        let status = manager
            .status_all()
            .await
            .into_iter()
            .find(|status| status.language == language)
            .ok_or("missing failed status")?;
        if status.running
            || status.pid.is_some()
            || status.last_error.as_deref() != Some(error.as_str())
        {
            return Err(format!("failed spawn status is inconsistent: {status:?}"));
        }
        let retry_error = manager
            .restart_server(language)
            .await
            .err()
            .ok_or("broken restart unexpectedly succeeded")?;
        let retry_status = manager
            .status_all()
            .await
            .into_iter()
            .find(|status| status.language == language)
            .ok_or("missing failed retry status")?;
        if retry_status.restart_count != 1
            || retry_status.last_error.as_deref() != Some(retry_error.as_str())
            || manager.restart_server(language).await.is_ok()
        {
            return Err(format!(
                "failed retry budget/status drifted: {retry_status:?}"
            ));
        }
        manager
            .reload_config(&LspConfig {
                servers: [(language.to_string(), mock_config(language, "stable", 2))]
                    .into_iter()
                    .collect(),
            })
            .await?;
        manager.start_server(language).await?;
        let old = manager.get_client(language).ok_or("missing old client")?;
        manager.start_server(language).await?;
        if old.read().await.is_running() || manager.running_servers() != [language] {
            return Err("repeated start retained the prior owner".to_string());
        }
        manager.shutdown_all().await;
        Ok(())
    }

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
        let config = LspConfig {
            servers: [(language.to_string(), test_config(language))]
                .into_iter()
                .collect(),
        };
        manager.load_config(&config)?;
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

        manager.load_config(&config)?;
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

        manager.load_config(&config)?;
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
        manager.load_config(&config)?;
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
