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
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
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
import json, os, sys, time
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
    close_stdin_after_write = False
    if method == 'initialize':
        related_information = message.get('params', {}).get('capabilities', {}).get('textDocument', {}).get('publishDiagnostics', {}).get('relatedInformation')
        body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'capabilities':{}}}).encode()
    elif method == 'shutdown':
        body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{}}).encode()
    elif mode == 'diagnostic_fidelity' and method == 'textDocument/hover':
        case = message['params']['textDocument']['uri'].rsplit('/', 1)[-1]
        if case == 'advertisement':
            reply = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'contents':str(related_information).lower()}}).encode()
            sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(reply) + reply)
            sys.stdout.buffer.flush()
            continue
        span = {'start': {'line': 0, 'character': 1}, 'end': {'line': 0, 'character': 3}}
        base = {'range': span, 'message': '中文😀 diagnostic', 'source': 'fixture'}
        values = {
            'valid': [{**base, 'severity': n, 'code': n} for n in [1,2,3,4]] + [{**base, 'code': '7'}, {**base, 'severity': None, 'code': None, 'source': None}],
            'integer-bounds': [{**base, 'code': -2147483648}, {**base, 'code': 2147483647}],
            'clear': [],
            'bad-severity': [{**base, 'severity': 0}],
            'bad-code-bool': [{**base, 'code': True}],
            'bad-code-float': [{**base, 'code': 1.5}],
            'bad-code-overflow': [{**base, 'code': 2147483648}],
            'bad-range-tuple': [{**base, 'range': [[0,1],[0,3]]}],
            'bad-position-tuple': [{**base, 'range': {'start': [0,1], 'end': span['end']}}],
            'bad-message': [{**base, 'message': 42}],
            'mixed': [base, {**base, 'code': False}],
            'other': [base],
        }
        uri = 'file:///other.test' if case == 'other' else 'file:///中文😀.test'
        if case == 'missing-uri':
            params = {'diagnostics': []}
        elif case == 'wrong-uri':
            params = {'uri': 42, 'diagnostics': []}
        elif case == 'missing-params':
            params = None
        elif case == 'missing-list':
            params = {'uri': uri}
        else:
            params = {'uri': uri, 'diagnostics': values[case]}
        notice = {'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':params}
        payload = json.dumps(notice, ensure_ascii=False).encode()
        sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(payload) + payload)
        sys.stdout.buffer.flush()
        # This response is an ordering barrier after the notification. The Rust
        # client reader must commit diagnostics state before settling the request.
        body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'contents':'barrier'}}).encode()
    elif mode == 'response_fidelity' and method.startswith('textDocument/'):
        case = message['params']['textDocument']['uri'].rsplit('/', 1)[-1]
        if message['params'].get('position') != {'line': 0, 'character': 3}:
            response = {'error': {'code': -32602, 'message': 'UTF-16 position changed'}}
        else:
            span = {'start': {'line': 0, 'character': 1}, 'end': {'line': 0, 'character': 3}}
            location = {'uri': 'file:///中文😀.test', 'range': span}
            responses = {
                'null': None,
                'empty': [],
                'hover-markup': {'contents': {'kind': 'markdown', 'value': '中文😀'}, 'range': span},
                'hover-array': {'contents': ['中文😀', {'language': 'rust', 'value': 'let 中文 = "😀";'}], 'range': span},
                'hover-string': {'contents': '中文😀'},
                'hover-missing': {'range': span},
                'hover-bad-range': {'contents': '中文😀', 'range': {'start': {'line': 0, 'character': -1}, 'end': span['end']}},
                'hover-null-range': {'contents': '中文😀', 'range': None},
                'hover-tuple-range': {'contents': '中文😀', 'range': [[0,1],[0,3]]},
                'hover-tuple-position': {'contents': '中文😀', 'range': {'start': [0,1], 'end': span['end']}},
                'tuple-location': ['file:///中文😀.test', span],
                'tuple-locations': [['file:///中文😀.test', span]],
                'mixed-tuple-locations': [location, ['file:///中文😀.test', span]],
                'hover-overflow-range': {'contents': '中文😀', 'range': {'start': {'line': 0, 'character': 2147483648}, 'end': {'line': 0, 'character': 2147483648}}},
                'hover-reversed-range': {'contents': '中文😀', 'range': {'start': span['end'], 'end': span['start']}},
                'hover-bad-content': {'contents': {'kind': 'markdown', 'value': 42}},
                'location': location,
                'locations': [location],
                'bad-location': {'uri': 'file:///bad.test'},
                'bad-locations': [location, {'uri': 'file:///bad.test'}],
                'completion': [{'label': '中文😀', 'kind': 3, 'insertText': '中文😀()', 'detail': 'fn 中文()', 'documentation': {'kind': 'markdown', 'value': '说明😀'}}],
                'completion-no-kind': [{'label': '中文😀'}],
                'completion-null-kind': [{'label': '中文😀', 'kind': None}],
                'completion-list': {'isIncomplete': False, 'items': [{'label': '中文😀', 'kind': 3, 'insertText': '中文😀()'}]},
                'bad-completion-mixed': [{'label': 'valid', 'kind': 1}, {'label': 'invalid', 'kind': 26}],
                'bad-completion-detail': [{'label': 'valid', 'kind': 1, 'detail': 42}],
                'bad-completion-doc': [{'label': 'valid', 'kind': 1, 'documentation': {'kind': 'html', 'value': '<p>bad</p>'}}],
                'bad-completion-iscomplete': {'isIncomplete': 'false', 'items': []},
                'bad-completion': [{'label': '中文😀', 'kind': 0}],
                'bad-completion-list': {'isIncomplete': False, 'items': {}},
            }
            if case == 'invalid-version':
                response = {'result': {'contents': 'wrong version'}}
            elif case == 'null-error':
                response = {'result': {'contents': 'ambiguous'}, 'error': None}
            elif case == 'tuple-error':
                response = {'error': [-32603, 'tuple error', None]}
            elif case == 'malformed-error-code':
                response = {'error': {'code': 'bad', 'message': 'invalid code'}}
            elif case == 'malformed-error-message':
                response = {'error': {'code': -32603}}
            elif case == 'result-and-error':
                response = {'result': {'contents': 'ambiguous'}, 'error': {'code': -32603, 'message': 'ambiguous'}}
            else:
                response = {} if case == 'missing-result' else {'result': responses[case]}
        body = json.dumps({'jsonrpc': '1.0' if case == 'invalid-version' else '2.0','id':message['id'], **response}, ensure_ascii=False).encode()
    elif mode == 'eof' and method == 'initialized':
        break
    elif mode == 'request_eof' and method == 'textDocument/definition':
        break
    elif mode == 'writer_fail' and method == 'textDocument/hover':
        body = json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'contents':'ready'}}).encode()
        close_stdin_after_write = True
    elif mode == 'malformed_header' and method == 'initialized':
        sys.stdout.buffer.write(b'Broken-Header\r\n\r\nbody')
        sys.stdout.buffer.flush()
        time.sleep(60)
        break
    elif mode == 'missing_length' and method == 'initialized':
        sys.stdout.buffer.write(b'X-Test: value\r\n\r\nbody')
        sys.stdout.buffer.flush()
        time.sleep(60)
        break
    else:
        continue
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
    if close_stdin_after_write:
        os.close(0)
        time.sleep(60)
        break
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

    async fn response_fidelity_client() -> Result<StdioLspClient, String> {
        let mut client = StdioLspClient::new(mock_config("test", "response_fidelity", 0));
        client
            .initialize("file:///response-fixture")
            .await
            .map_err(|e| e.to_string())?;
        Ok(client)
    }

    fn unicode_position() -> echo_core::lsp::Position {
        // In "中😀x", x starts at UTF-16 offset 3, not a UTF-8 byte offset.
        echo_core::lsp::Position {
            line: 0,
            character: 3,
        }
    }

    #[tokio::test]
    async fn response_fidelity_hover_retains_range_and_unicode() -> Result<(), String> {
        let mut client = response_fidelity_client().await?;
        for case in ["hover-markup", "hover-array"] {
            let hover = client
                .hover(&format!("file:///{case}"), unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .ok_or("missing hover")?;
            assert!(hover.contents.contains("中文😀"));
            let range = hover.range.ok_or("server hover range was discarded")?;
            assert_eq!(range.start.character, 1);
            assert_eq!(range.end.character, 3);
        }
        let hover = client
            .hover("file:///hover-string", unicode_position())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("missing hover")?;
        assert_eq!(hover.contents, "中文😀");
        assert!(hover.range.is_none());
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn response_fidelity_nullable_results_are_valid_empty_results() -> Result<(), String> {
        let mut client = response_fidelity_client().await?;
        assert!(
            client
                .hover("file:///null", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_none()
        );
        assert!(
            client
                .goto_definition("file:///null", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        assert!(
            client
                .find_references("file:///null", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        assert!(
            client
                .completion("file:///null", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        assert!(
            client
                .goto_definition("file:///empty", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        assert!(
            client
                .find_references("file:///empty", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        assert!(
            client
                .completion("file:///empty", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn response_fidelity_malformed_results_are_explicit_server_errors() -> Result<(), String>
    {
        let mut client = response_fidelity_client().await?;
        for case in [
            "hover-missing",
            "hover-bad-range",
            "hover-null-range",
            "hover-reversed-range",
            "hover-overflow-range",
            "invalid-version",
            "result-and-error",
            "hover-bad-content",
            "missing-result",
        ] {
            assert!(matches!(
                client
                    .hover(&format!("file:///{case}"), unicode_position())
                    .await,
                Err(echo_core::lsp::LspError::ServerError(_))
            ));
        }
        for case in ["bad-location", "bad-locations"] {
            assert!(matches!(
                client
                    .goto_definition(&format!("file:///{case}"), unicode_position())
                    .await,
                Err(echo_core::lsp::LspError::ServerError(_))
            ));
            assert!(matches!(
                client
                    .find_references(&format!("file:///{case}"), unicode_position())
                    .await,
                Err(echo_core::lsp::LspError::ServerError(_))
            ));
        }
        for case in [
            "bad-completion",
            "bad-completion-list",
            "bad-completion-mixed",
            "bad-completion-detail",
            "bad-completion-doc",
            "bad-completion-iscomplete",
        ] {
            assert!(matches!(
                client
                    .completion(&format!("file:///{case}"), unicode_position())
                    .await,
                Err(echo_core::lsp::LspError::ServerError(_))
            ));
        }
        // A malformed result is request-local; it must not invent process death.
        assert!(client.is_initialized());
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn response_fidelity_locations_preserve_server_utf16_coordinates() -> Result<(), String> {
        let mut client = response_fidelity_client().await?;
        for case in ["location", "locations"] {
            let locations = client
                .goto_definition(&format!("file:///{case}"), unicode_position())
                .await
                .map_err(|e| e.to_string())?;
            assert_eq!(locations.len(), 1);
            let location = locations.first().ok_or("missing definition")?;
            assert_eq!(location.uri, "file:///中文😀.test");
            assert_eq!(location.range.start.character, 1);
            assert_eq!(location.range.end.character, 3);
        }
        let locations = client
            .find_references("file:///locations", unicode_position())
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(locations.len(), 1);
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn response_fidelity_completion_decodes_lsp_fields() -> Result<(), String> {
        let mut client = response_fidelity_client().await?;
        for case in ["completion", "completion-list"] {
            let items = client
                .completion(&format!("file:///{case}"), unicode_position())
                .await
                .map_err(|e| e.to_string())?;
            let item = items.first().ok_or("valid LSP completion was discarded")?;
            assert_eq!(items.len(), 1);
            assert_eq!(
                serde_json::to_value(item.kind).map_err(|e| e.to_string())?,
                serde_json::json!("Function")
            );
            assert_eq!(item.label, "中文😀");
            assert_eq!(item.insert_text.as_deref(), Some("中文😀()"));
            if case == "completion" {
                assert_eq!(item.documentation.as_deref(), Some("说明😀"));
            }
        }
        for case in ["completion-no-kind", "completion-null-kind"] {
            let items = client
                .completion(&format!("file:///{case}"), unicode_position())
                .await
                .map_err(|e| e.to_string())?;
            let item = items
                .first()
                .ok_or("valid optional-kind completion was discarded")?;
            assert!(
                serde_json::to_value(item.kind)
                    .map_err(|e| e.to_string())?
                    .is_null()
            );
        }
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn response_fidelity_rejects_tuple_shapes_at_each_native_boundary() -> Result<(), String>
    {
        let mut client = response_fidelity_client().await?;
        for case in ["hover-tuple-range", "hover-tuple-position"] {
            let result = client
                .hover(&format!("file:///{case}"), unicode_position())
                .await;
            assert!(
                matches!(result, Err(echo_core::lsp::LspError::ServerError(_))),
                "{case}: {result:?}"
            );
        }
        for case in ["tuple-location", "tuple-locations", "mixed-tuple-locations"] {
            let result = client
                .goto_definition(&format!("file:///{case}"), unicode_position())
                .await;
            assert!(
                matches!(result, Err(echo_core::lsp::LspError::ServerError(_))),
                "{case}: {result:?}"
            );
        }
        for case in ["tuple-locations", "mixed-tuple-locations"] {
            let result = client
                .find_references(&format!("file:///{case}"), unicode_position())
                .await;
            assert!(
                matches!(result, Err(echo_core::lsp::LspError::ServerError(_))),
                "{case}: {result:?}"
            );
        }
        assert!(client.is_initialized());
        assert!(
            client
                .hover("file:///hover-string", unicode_position())
                .await
                .map_err(|e| e.to_string())?
                .is_some()
        );
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn response_fidelity_malformed_error_settles_request_without_killing_connection()
    -> Result<(), String> {
        let mut client = response_fidelity_client().await?;
        let mut failures = Vec::new();
        for case in [
            "null-error",
            "malformed-error-code",
            "malformed-error-message",
            "tuple-error",
        ] {
            let uri = format!("file:///{case}");
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                client.hover(&uri, unicode_position()),
            )
            .await;
            if !matches!(result, Ok(Err(echo_core::lsp::LspError::ServerError(_)))) {
                failures.push(format!(
                    "{case}: expected explicit ServerError, got {result:?}"
                ));
            }
            assert!(client.is_initialized());
            assert!(
                client
                    .hover("file:///hover-string", unicode_position())
                    .await
                    .map_err(|e| e.to_string())?
                    .is_some()
            );
        }
        client.shutdown().await.map_err(|e| e.to_string())?;
        assert!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    async fn diagnostic_fidelity_client() -> Result<StdioLspClient, String> {
        let mut client = StdioLspClient::new(mock_config("test", "diagnostic_fidelity", 0));
        client
            .initialize("file:///diagnostic-fixture")
            .await
            .map_err(|e| e.to_string())?;
        Ok(client)
    }

    async fn publish_diagnostics_barrier(
        client: &StdioLspClient,
        case: &str,
    ) -> Result<(), String> {
        client
            .hover(&format!("file:///{case}"), unicode_position())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn diagnostic_fidelity_optional_severity_and_typed_codes_are_lossless()
    -> Result<(), String> {
        let mut client = diagnostic_fidelity_client().await?;
        publish_diagnostics_barrier(&client, "valid").await?;
        let values = client
            .diagnostics("file:///中文😀.test")
            .await
            .map_err(|e| e.to_string())?;
        let values = serde_json::to_value(values).map_err(|e| e.to_string())?;
        let items = values.as_array().ok_or("diagnostic list shape")?;
        assert_eq!(items.len(), 6);
        for (index, severity) in ["Error", "Warning", "Information", "Hint"]
            .into_iter()
            .enumerate()
        {
            let item = items.get(index).ok_or("missing diagnostic")?;
            assert_eq!(item.get("severity"), Some(&serde_json::json!(severity)));
            assert_eq!(
                item.get("code"),
                Some(&serde_json::json!({"kind":"integer", "value": index.saturating_add(1)}))
            );
            assert_eq!(
                item.get("message"),
                Some(&serde_json::json!("中文😀 diagnostic"))
            );
        }
        let absent = items.get(4).ok_or("missing absent severity")?;
        assert_eq!(absent.get("severity"), Some(&serde_json::Value::Null));
        assert_eq!(
            absent.get("code"),
            Some(&serde_json::json!({"kind":"string", "value":"7"}))
        );
        let null = items.get(5).ok_or("missing null severity")?;
        assert_eq!(null.get("severity"), Some(&serde_json::Value::Null));
        assert_eq!(null.get("code"), Some(&serde_json::Value::Null));
        assert_eq!(null.get("source"), Some(&serde_json::Value::Null));
        publish_diagnostics_barrier(&client, "integer-bounds").await?;
        let bounds = serde_json::to_value(
            client
                .diagnostics("file:///中文😀.test")
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let bounds = bounds.as_array().ok_or("missing bound array")?;
        assert_eq!(
            bounds.first().and_then(|v| v.get("code")),
            Some(&serde_json::json!({"kind":"integer", "value":-2147483648_i64}))
        );
        assert_eq!(
            bounds.get(1).and_then(|v| v.get("code")),
            Some(&serde_json::json!({"kind":"integer", "value":2147483647_i64}))
        );
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn diagnostic_fidelity_errors_replace_old_good_then_recover_or_clear()
    -> Result<(), String> {
        let mut client = diagnostic_fidelity_client().await?;
        publish_diagnostics_barrier(&client, "other").await?;
        for case in [
            "bad-severity",
            "bad-code-bool",
            "bad-code-float",
            "bad-code-overflow",
            "bad-range-tuple",
            "bad-position-tuple",
            "bad-message",
            "mixed",
            "missing-list",
        ] {
            publish_diagnostics_barrier(&client, "valid").await?;
            publish_diagnostics_barrier(&client, case).await?;
            let result = client.diagnostics("file:///中文😀.test").await;
            assert!(
                matches!(result, Err(echo_core::lsp::LspError::ServerError(_))),
                "{case} fabricated success: {result:?}"
            );
            assert_eq!(
                client
                    .diagnostics("file:///other.test")
                    .await
                    .map_err(|e| e.to_string())?
                    .len(),
                1
            );
            publish_diagnostics_barrier(&client, "valid").await?;
            assert_eq!(
                client
                    .diagnostics("file:///中文😀.test")
                    .await
                    .map_err(|e| e.to_string())?
                    .len(),
                6
            );
            publish_diagnostics_barrier(&client, "clear").await?;
            assert!(
                client
                    .diagnostics("file:///中文😀.test")
                    .await
                    .map_err(|e| e.to_string())?
                    .is_empty()
            );
        }
        assert!(client.is_initialized());
        client.shutdown().await.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn diagnostic_fidelity_unaddressable_notification_settles_existing_runtime()
    -> Result<(), String> {
        for case in ["missing-uri", "wrong-uri", "missing-params"] {
            let mut client = diagnostic_fidelity_client().await?;
            let uri = format!("file:///{case}");
            let pending = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                client.hover(&uri, unicode_position()),
            )
            .await
            .map_err(|_| "bad notification left pending request".to_string())?;
            assert!(
                pending.is_err(),
                "unaddressable malformed notification silently succeeded"
            );
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while client.is_running() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| "runtime did not settle malformed notification".to_string())?;
            let status = client.status();
            assert!(!status.initialized && status.pid.is_none());
            assert!(
                status
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains("publishDiagnostics"))
            );
            assert!(client.process_has_exited().await);
            assert!(matches!(
                client.diagnostics("file:///中文😀.test").await,
                Err(echo_core::lsp::LspError::ServerError(_))
            ));
            client.shutdown().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn diagnostic_fidelity_does_not_advertise_unrepresented_related_information()
    -> Result<(), String> {
        let mut client = diagnostic_fidelity_client().await?;
        let hover = client
            .hover("file:///advertisement", unicode_position())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("missing capability observation")?;
        assert_eq!(hover.contents, "false");
        client.shutdown().await.map_err(|e| e.to_string())
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
        if !old.read().await.process_has_exited().await {
            return Err("EOF published terminal status before child exit".to_string());
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
        let eof_error = exited.last_error.clone();
        manager.stop_server(language).await?;
        let stopped = manager
            .status_all()
            .await
            .into_iter()
            .find(|status| status.language == language)
            .ok_or("missing stopped EOF status")?;
        if stopped.last_error != eof_error {
            return Err("explicit stop erased the prior EOF failure".to_string());
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
    async fn writer_failure_settles_status_and_pending_without_stdout_eof() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        manager.load_config(&LspConfig {
            servers: [(
                language.to_string(),
                mock_config(language, "writer_fail", 1),
            )]
            .into_iter()
            .collect(),
        })?;
        manager.start_server(language).await?;
        let client = manager.get_client(language).ok_or("missing client")?;
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            client.read().await.hover(
                "file:///writer-ready.test",
                echo_core::lsp::Position {
                    line: 0,
                    character: 0,
                },
            ),
        )
        .await
        .map_err(|_| "writer failure fixture did not close stdin".to_string())?
        .map_err(|error| error.to_string())?;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while client.read().await.is_running() {
                let _ = client
                    .read()
                    .await
                    .did_save("file:///writer-ready.test")
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "writer task did not observe the closed stdin".to_string())?;
        let request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
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
        .map_err(|_| "writer failure left a pending request".to_string())?;
        let status = wait_for_exit(&manager, language).await?;
        if request.is_ok()
            || !status
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("stdin write failed"))
        {
            return Err(format!("writer failure was not authoritative: {status:?}"));
        }
        if !client.read().await.process_has_exited().await {
            return Err("writer failure published terminal status before child exit".to_string());
        }
        manager.shutdown_all().await;
        Ok(())
    }

    #[tokio::test]
    async fn malformed_or_missing_content_length_settles_runtime() -> Result<(), String> {
        for (mode, expected) in [
            ("malformed_header", "Malformed LSP header"),
            ("missing_length", "Missing LSP Content-Length header"),
        ] {
            let mut manager = LspManager::new();
            let language = "test";
            manager.load_config(&LspConfig {
                servers: [(language.to_string(), mock_config(language, mode, 1))]
                    .into_iter()
                    .collect(),
            })?;
            manager.start_server(language).await?;
            let client = manager
                .get_client(language)
                .ok_or("missing malformed client")?;
            let status = wait_for_exit(&manager, language).await?;
            if !status
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains(expected))
            {
                return Err(format!("{mode} did not settle with {expected}: {status:?}"));
            }
            if !client.read().await.process_has_exited().await {
                return Err(format!(
                    "{mode} published terminal status before child exit"
                ));
            }
            manager.shutdown_all().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn clean_stop_does_not_invent_a_transport_error() -> Result<(), String> {
        let mut manager = LspManager::new();
        let language = "test";
        manager.load_config(&LspConfig {
            servers: [(language.to_string(), mock_config(language, "stable", 1))]
                .into_iter()
                .collect(),
        })?;
        manager.start_server(language).await?;
        manager.stop_server(language).await?;
        let status = manager
            .status_all()
            .await
            .into_iter()
            .find(|status| status.language == language)
            .ok_or("missing clean stop status")?;
        if status.running
            || status.initialized
            || status.pid.is_some()
            || status.last_error.is_some()
        {
            return Err(format!("clean stop invented a failure: {status:?}"));
        }
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
