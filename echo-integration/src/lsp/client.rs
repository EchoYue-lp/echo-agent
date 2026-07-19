//! Stdio-based LSP client — spawns a language server process and
//! communicates via JSON-RPC over stdin/stdout.

use echo_core::lsp::{
    CompletionItem, Diagnostic, HoverInfo, Location, LspClient, LspError, LspResult,
    LspServerConfig, LspServerStatus, Position, TextChange,
};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use super::jsonrpc::{self, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Stdio-based LSP client for a single language server.
///
/// Spawns the server as a child process and communicates via JSON-RPC
/// over stdin (requests) and stdout (responses).
pub struct StdioLspClient {
    /// Language identifier.
    language: String,
    /// Server configuration.
    config: LspServerConfig,
    /// Child process handle.
    child: Option<Child>,
    /// Channel to send JSON-RPC messages to the writer task.
    writer_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    /// Pending request callbacks.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Next request ID.
    next_id: AtomicU64,
    /// Whether the server is running.
    running: AtomicBool,
    /// Whether initialization is complete.
    initialized: AtomicBool,
    /// Restart count.
    restart_count: u32,
    /// Last error message.
    last_error: Option<String>,
    /// Cached diagnostics per file URI.
    diagnostics_cache: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
}

impl StdioLspClient {
    /// Create a new client (does not start the server yet).
    pub fn new(config: LspServerConfig) -> Self {
        let language = config.language.clone();
        Self {
            language,
            config,
            child: None,
            writer_tx: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            running: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            restart_count: 0,
            last_error: None,
            diagnostics_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Spawn the server process and set up communication channels.
    fn spawn_process(&mut self) -> Result<(), LspError> {
        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args);

        // Set environment variables
        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| LspError::SpawnError(format!("{}: {e}", self.config.command)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::SpawnError("Failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::SpawnError("Failed to capture stdout".into()))?;

        // Create writer channel
        let (writer_tx, mut writer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

        // Spawn writer task
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(data) = writer_rx.recv().await {
                if stdin.write_all(&data).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Spawn reader task
        let pending = self.pending.clone();
        let diagnostics_cache = self.diagnostics_cache.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            Self::read_loop(reader, pending, diagnostics_cache).await;
        });

        self.child = Some(child);
        self.writer_tx = Some(writer_tx);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Read loop — parses LSP framed messages from stdout.
    async fn read_loop(
        mut reader: BufReader<tokio::process::ChildStdout>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
        diagnostics_cache: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    ) {
        let mut header_line = String::new();

        loop {
            // Read headers until empty line
            let mut content_length: Option<usize> = None;
            loop {
                header_line.clear();
                match reader.read_line(&mut header_line).await {
                    Ok(0) => {
                        pending.lock().await.clear();
                        return;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        pending.lock().await.clear();
                        return;
                    }
                }

                let trimmed = header_line.trim();
                if trimmed.is_empty() {
                    break; // End of headers
                }

                if let Some(len) = jsonrpc::parse_content_length(trimmed) {
                    content_length = Some(len);
                }
            }

            let Some(len) = content_length else {
                continue;
            };

            // Read body
            let mut body = vec![0u8; len];
            if reader.read_exact(&mut body).await.is_err() {
                pending.lock().await.clear();
                return;
            }

            // Parse JSON
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
                continue;
            };

            // Check if it's a notification (no id) or a response (has id)
            if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                // Response to a request
                if let Ok(resp) = serde_json::from_value::<JsonRpcResponse>(value) {
                    let mut pending = pending.lock().await;
                    if let Some(tx) = pending.remove(&id) {
                        let _ = tx.send(resp);
                    }
                }
            } else if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
                // Server notification
                if method == "textDocument/publishDiagnostics"
                    && let Some(params) = value.get("params")
                    && let Some(uri) = params.get("uri").and_then(|v| v.as_str())
                {
                    let diagnostics: Vec<Diagnostic> = params
                        .get("diagnostics")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let mut cache = diagnostics_cache.lock().await;
                    cache.insert(uri.to_string(), diagnostics);
                }
            }
        }
    }

    /// Send a JSON-RPC request and wait for the response.
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> LspResult<serde_json::Value> {
        let writer_tx = self.writer_tx.as_ref().ok_or(LspError::NotInitialized)?;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);
        let data = jsonrpc::encode_message(&request)
            .map_err(|e| LspError::CommunicationError(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        writer_tx
            .send(data)
            .await
            .map_err(|_| LspError::CommunicationError("Writer channel closed".into()))?;

        let response = rx
            .await
            .map_err(|_| LspError::CommunicationError("Response channel closed".into()))?;

        if let Some(err) = response.error {
            return Err(LspError::ServerError(err.to_string()));
        }

        response
            .result
            .ok_or_else(|| LspError::ServerError("Empty response".into()))
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> LspResult<()> {
        let writer_tx = self.writer_tx.as_ref().ok_or(LspError::NotInitialized)?;

        let notification = JsonRpcNotification::new(method, params);
        let data = jsonrpc::encode_message(&notification)
            .map_err(|e| LspError::CommunicationError(e.to_string()))?;

        writer_tx
            .send(data)
            .await
            .map_err(|_| LspError::CommunicationError("Writer channel closed".into()))?;

        Ok(())
    }
}

impl LspClient for StdioLspClient {
    fn language(&self) -> &str {
        &self.language
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    fn initialize<'a>(&'a mut self, root_uri: &'a str) -> BoxFuture<'a, LspResult<()>> {
        Box::pin(async move {
            // Spawn the process
            self.spawn_process()?;

            // Send initialize request
            let params = serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "completion": {
                            "completionItem": {
                                "snippetSupport": false
                            }
                        },
                        "hover": {
                            "contentFormat": ["markdown", "plaintext"]
                        },
                        "definition": {},
                        "references": {},
                        "publishDiagnostics": {
                            "relatedInformation": true
                        }
                    }
                }
            });

            let _result = self.send_request("initialize", Some(params)).await?;

            // Send initialized notification
            self.send_notification("initialized", Some(serde_json::json!({})))
                .await?;

            self.initialized.store(true, Ordering::SeqCst);
            Ok(())
        })
    }

    fn shutdown(&mut self) -> BoxFuture<'_, LspResult<()>> {
        Box::pin(async move {
            if self.is_running() {
                // Send shutdown request
                let _ = self.send_request("shutdown", None).await;
                // Send exit notification
                let _ = self.send_notification("exit", None).await;

                // Kill the process
                if let Some(ref mut child) = self.child {
                    let _ = child.kill().await;
                }

                self.running.store(false, Ordering::SeqCst);
                self.initialized.store(false, Ordering::SeqCst);
            }
            Ok(())
        })
    }

    fn diagnostics<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<Vec<Diagnostic>>> {
        Box::pin(async move {
            let cache = self.diagnostics_cache.lock().await;
            Ok(cache.get(uri).cloned().unwrap_or_default())
        })
    }

    fn goto_definition<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Vec<Location>>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": position.line, "character": position.character }
            });

            let result = self
                .send_request("textDocument/definition", Some(params))
                .await?;

            // Result can be a single Location or an array
            let locations: Vec<Location> =
                if let Ok(loc) = serde_json::from_value::<Location>(result.clone()) {
                    vec![loc]
                } else {
                    serde_json::from_value(result).unwrap_or_default()
                };

            Ok(locations)
        })
    }

    fn find_references<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Vec<Location>>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": position.line, "character": position.character },
                "context": { "includeDeclaration": true }
            });

            let result = self
                .send_request("textDocument/references", Some(params))
                .await?;

            let locations: Vec<Location> = serde_json::from_value(result).unwrap_or_default();
            Ok(locations)
        })
    }

    fn hover<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Option<HoverInfo>>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": position.line, "character": position.character }
            });

            let result = self
                .send_request("textDocument/hover", Some(params))
                .await?;

            if result.is_null() {
                return Ok(None);
            }

            // Extract contents from hover result
            let contents = result
                .get("contents")
                .map(|v| {
                    if let Some(s) = v.as_str() {
                        s.to_string()
                    } else if let Some(obj) = v.as_object() {
                        obj.get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_default();

            Ok(Some(HoverInfo {
                contents,
                range: None,
            }))
        })
    }

    fn completion<'a>(
        &'a self,
        uri: &'a str,
        position: Position,
    ) -> BoxFuture<'a, LspResult<Vec<CompletionItem>>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": position.line, "character": position.character }
            });

            let result = self
                .send_request("textDocument/completion", Some(params))
                .await?;

            // Result can be CompletionItem[] or CompletionList
            let items: Vec<CompletionItem> = if let Some(items) = result.get("items") {
                serde_json::from_value(items.clone()).unwrap_or_default()
            } else {
                serde_json::from_value(result).unwrap_or_default()
            };

            Ok(items)
        })
    }

    fn did_open<'a>(
        &'a self,
        uri: &'a str,
        language_id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, LspResult<()>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            });
            self.send_notification("textDocument/didOpen", Some(params))
                .await
        })
    }

    fn did_change<'a>(
        &'a self,
        uri: &'a str,
        changes: Vec<TextChange>,
    ) -> BoxFuture<'a, LspResult<()>> {
        Box::pin(async move {
            let content_changes: Vec<serde_json::Value> = changes
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "range": {
                            "start": { "line": c.range.start.line, "character": c.range.start.character },
                            "end": { "line": c.range.end.line, "character": c.range.end.character }
                        },
                        "text": c.text
                    })
                })
                .collect();

            let params = serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": content_changes
            });
            self.send_notification("textDocument/didChange", Some(params))
                .await
        })
    }

    fn did_save<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<()>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": { "uri": uri }
            });
            self.send_notification("textDocument/didSave", Some(params))
                .await
        })
    }

    fn did_close<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<()>> {
        Box::pin(async move {
            let params = serde_json::json!({
                "textDocument": { "uri": uri }
            });
            self.send_notification("textDocument/didClose", Some(params))
                .await
        })
    }

    fn status(&self) -> LspServerStatus {
        LspServerStatus {
            language: self.language.clone(),
            running: self.is_running(),
            initialized: self.is_initialized(),
            restart_count: self.restart_count,
            last_error: self.last_error.clone(),
            pid: self.child.as_ref().and_then(|c| c.id()),
        }
    }
}
