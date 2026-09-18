//! Stdio-based LSP client — spawns a language server process and
//! communicates via JSON-RPC over stdin/stdout.

use echo_core::lsp::{
    CompletionItem, Diagnostic, HoverInfo, Location, LspClient, LspError, LspResult,
    LspServerConfig, LspServerStatus, Position, TextChange,
};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use super::jsonrpc::{self, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Shared manager lifecycle fence captured by every derived client handle.
///
/// The fence is intentionally internal to the framework. SDK handles retain
/// the existing client shape, while the manager remains the only authority
/// that can keep a child process live.
pub(crate) struct LspLifecycle {
    pub(crate) generation: u64,
    closed: AtomicBool,
}

impl LspLifecycle {
    pub(crate) fn new(generation: u64) -> Self {
        Self {
            generation,
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_live(&self, generation: u64) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.generation == generation
    }
}

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_LSP_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

fn accepts_lsp_message_size(bytes: usize) -> bool {
    bytes <= MAX_LSP_MESSAGE_BYTES
}

struct LspRuntime {
    status: LspServerStatus,
    pending: HashMap<u64, oneshot::Sender<JsonRpcResponse>>,
    accepting: bool,
}

impl LspRuntime {
    fn register(&mut self, id: u64, tx: oneshot::Sender<JsonRpcResponse>) -> LspResult<()> {
        if !self.status.running || !self.accepting {
            return Err(LspError::NotInitialized);
        }
        self.pending.insert(id, tx);
        Ok(())
    }

    fn settle(&mut self, error: Option<String>) {
        self.accepting = false;
        self.status.running = false;
        self.status.initialized = false;
        self.status.pid = None;
        if self.status.last_error.is_none() {
            self.status.last_error = error;
        }
        self.pending.clear();
    }

    fn begin_terminal(&mut self, error: Option<String>) {
        self.accepting = false;
        if self.status.last_error.is_none() {
            self.status.last_error = error;
        }
        self.pending.clear();
    }
}

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
    child: Option<Arc<Mutex<Child>>>,
    /// Channel to send JSON-RPC messages to the writer task.
    writer_tx: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    writer_task: Option<tokio::task::JoinHandle<()>>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
    /// Next request ID.
    next_id: AtomicU64,
    /// Status and request admission share one atomic failure boundary.
    runtime: Arc<StdMutex<LspRuntime>>,
    /// Cached diagnostics per file URI.
    diagnostics_cache: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    /// Manager lifecycle captured when this derived handle was created.
    lifecycle: Arc<LspLifecycle>,
    /// Generation associated with this client handle.
    lifecycle_generation: u64,
    /// Client-level close fence, set by explicit stop/shutdown.
    closed: Arc<AtomicBool>,
    /// Suppress intentional EOF/error during graceful shutdown.
    stopping: Arc<AtomicBool>,
}

impl StdioLspClient {
    /// Create a new client (does not start the server yet).
    pub fn new(config: LspServerConfig) -> Self {
        let lifecycle = Arc::new(LspLifecycle::new(0));
        Self::new_bound(config, lifecycle, 0)
    }

    /// Create a client bound to a manager lifecycle.
    pub(crate) fn new_bound(
        config: LspServerConfig,
        lifecycle: Arc<LspLifecycle>,
        lifecycle_generation: u64,
    ) -> Self {
        let language = config.language.clone();
        Self {
            language: language.clone(),
            config,
            child: None,
            writer_tx: None,
            writer_task: None,
            reader_task: None,
            next_id: AtomicU64::new(1),
            runtime: Arc::new(StdMutex::new(LspRuntime {
                status: LspServerStatus {
                    language: language.clone(),
                    running: false,
                    initialized: false,
                    restart_count: 0,
                    last_error: None,
                    pid: None,
                },
                pending: HashMap::new(),
                accepting: false,
            })),
            diagnostics_cache: Arc::new(Mutex::new(HashMap::new())),
            lifecycle,
            lifecycle_generation,
            closed: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    fn ensure_live(&self) -> LspResult<()> {
        if self.closed.load(Ordering::SeqCst) || !self.lifecycle.is_live(self.lifecycle_generation)
        {
            return Err(LspError::NotInitialized);
        }
        Ok(())
    }

    fn update_runtime(&self, update: impl FnOnce(&mut LspServerStatus)) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        update(&mut runtime.status);
    }

    pub(crate) fn set_restart_count(&self, count: u32) {
        self.update_runtime(|status| status.restart_count = count);
    }

    pub(crate) fn set_last_error(&self, error: String) {
        self.update_runtime(|status| status.last_error = Some(error));
    }

    fn mark_initialized_if_running(&self) -> bool {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !runtime.status.running || !runtime.accepting {
            return false;
        }
        runtime.status.initialized = true;
        true
    }

    /// Kill a process without sending more protocol messages.
    async fn abort_process(&mut self) {
        self.writer_tx = None;
        if let Some(child) = self.child.take() {
            Self::terminate_child(&child).await;
        }
        for task in [self.writer_task.take(), self.reader_task.take()]
            .into_iter()
            .flatten()
        {
            let mut task = task;
            if tokio::time::timeout(std::time::Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        self.diagnostics_cache.lock().await.clear();
        self.runtime
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .settle(None);
    }

    async fn terminate_child(child: &Arc<Mutex<Child>>) {
        let mut child = child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    #[cfg(test)]
    pub(crate) async fn process_has_exited(&self) -> bool {
        let Some(child) = self.child.as_ref() else {
            return true;
        };
        child.lock().await.try_wait().ok().flatten().is_some()
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

        let child_id = child.id();
        let child = Arc::new(Mutex::new(child));
        {
            let mut runtime = self
                .runtime
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            runtime.status.running = true;
            runtime.status.initialized = false;
            runtime.status.last_error = None;
            runtime.status.pid = child_id;
            runtime.accepting = true;
        }

        // Writer failure must settle pending calls even if the child keeps
        // stdout open indefinitely after its stdin has closed.
        let writer_runtime = Arc::clone(&self.runtime);
        let writer_cache = Arc::clone(&self.diagnostics_cache);
        let writer_child = Arc::clone(&child);
        let writer_stopping = Arc::clone(&self.stopping);
        let writer_task = tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(data) = writer_rx.recv().await {
                let result = match stdin.write_all(&data).await {
                    Ok(()) => stdin.flush().await,
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    let detail = format!("Language server stdin write failed: {error}");
                    writer_runtime
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .begin_terminal(
                            (!writer_stopping.load(Ordering::SeqCst)).then_some(detail),
                        );
                    writer_cache.lock().await.clear();
                    Self::terminate_child(&writer_child).await;
                    writer_runtime
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .settle(None);
                    break;
                }
            }
        });

        // Spawn reader task
        let diagnostics_cache = self.diagnostics_cache.clone();
        let runtime = Arc::clone(&self.runtime);
        let closed = Arc::clone(&self.closed);
        let stopping = Arc::clone(&self.stopping);
        let reader_child = Arc::clone(&child);
        let reader_task = tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let error =
                Self::read_loop(reader, Arc::clone(&runtime), diagnostics_cache.clone()).await;
            runtime
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .begin_terminal(
                    (!closed.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst))
                        .then_some(error),
                );
            diagnostics_cache.lock().await.clear();
            Self::terminate_child(&reader_child).await;
            runtime
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .settle(None);
        });

        self.child = Some(child);
        self.writer_tx = Some(writer_tx);
        self.writer_task = Some(writer_task);
        self.reader_task = Some(reader_task);
        Ok(())
    }

    /// Read loop — parses LSP framed messages from stdout.
    async fn read_loop(
        mut reader: BufReader<tokio::process::ChildStdout>,
        runtime: Arc<StdMutex<LspRuntime>>,
        diagnostics_cache: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    ) -> String {
        let mut header_line = String::new();

        loop {
            // Read headers until empty line
            let mut content_length: Option<usize> = None;
            loop {
                header_line.clear();
                match reader.read_line(&mut header_line).await {
                    Ok(0) => return "Language server stdout closed".to_string(),
                    Ok(_) => {}
                    Err(error) => return format!("Language server stdout read failed: {error}"),
                }

                let trimmed = header_line.trim();
                if trimmed.is_empty() {
                    break; // End of headers
                }

                if let Some((name, _)) = trimmed.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        let Some(length) = jsonrpc::parse_content_length(trimmed) else {
                            return "Invalid LSP Content-Length header".to_string();
                        };
                        content_length = Some(length);
                    }
                } else {
                    return "Malformed LSP header".to_string();
                }
            }

            let Some(len) = content_length else {
                return "Missing LSP Content-Length header".to_string();
            };
            if !accepts_lsp_message_size(len) {
                return format!("Language server message exceeds {MAX_LSP_MESSAGE_BYTES} bytes");
            }

            // Read body
            let mut body = vec![0u8; len];
            if let Err(error) = reader.read_exact(&mut body).await {
                return format!("Language server message body truncated: {error}");
            }

            // Parse JSON
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
                continue;
            };

            // Check if it's a notification (no id) or a response (has id)
            if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                // Response to a request
                if let Ok(resp) = serde_json::from_value::<JsonRpcResponse>(value) {
                    let mut runtime = runtime.lock().unwrap_or_else(|poison| poison.into_inner());
                    if let Some(tx) = runtime.pending.remove(&id) {
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
                    if runtime
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .status
                        .running
                    {
                        diagnostics_cache
                            .lock()
                            .await
                            .insert(uri.to_string(), diagnostics);
                    }
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
        self.ensure_live()?;
        if !self.is_running() {
            return Err(LspError::NotInitialized);
        }
        let writer_tx = self.writer_tx.as_ref().ok_or(LspError::NotInitialized)?;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);
        let data = jsonrpc::encode_message(&request)
            .map_err(|e| LspError::CommunicationError(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            self.runtime
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .register(id, tx)?;
        }

        if writer_tx.send(data).await.is_err() {
            self.runtime
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .pending
                .remove(&id);
            return Err(LspError::CommunicationError("Writer channel closed".into()));
        }

        let response = match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                self.runtime
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .pending
                    .remove(&id);
                return Err(LspError::CommunicationError(
                    "Response channel closed".into(),
                ));
            }
            Err(_) => {
                self.runtime
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .pending
                    .remove(&id);
                let cancellation = JsonRpcNotification::new(
                    "$/cancelRequest",
                    Some(serde_json::json!({ "id": id })),
                );
                if let Ok(data) = jsonrpc::encode_message(&cancellation) {
                    let _ = writer_tx.send(data).await;
                }
                return Err(LspError::CommunicationError(format!(
                    "LSP request '{method}' timed out after {}s",
                    REQUEST_TIMEOUT.as_secs()
                )));
            }
        };

        self.ensure_live()?;
        if !self.is_running() {
            return Err(LspError::NotInitialized);
        }

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
        self.ensure_live()?;
        if !self.is_running() {
            return Err(LspError::NotInitialized);
        }
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
        self.status().running
    }

    fn is_initialized(&self) -> bool {
        self.status().initialized
    }

    fn initialize<'a>(&'a mut self, root_uri: &'a str) -> BoxFuture<'a, LspResult<()>> {
        Box::pin(async move {
            self.ensure_live()?;
            if self.child.is_some() {
                return Err(LspError::CommunicationError(
                    "Language server is already started".to_string(),
                ));
            }
            // Spawn the process
            if let Err(error) = self.spawn_process() {
                self.set_last_error(error.to_string());
                return Err(error);
            }

            // Shutdown may win while spawning or while the child is starting.
            // Abort the just-created child instead of allowing an unowned
            // process to survive the manager that created it.
            if let Err(error) = self.ensure_live() {
                self.abort_process().await;
                return Err(error);
            }

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

            if let Err(error) = self.send_request("initialize", Some(params)).await {
                self.set_last_error(error.to_string());
                self.abort_process().await;
                return Err(error);
            }

            // Send initialized notification
            if let Err(error) = self
                .send_notification("initialized", Some(serde_json::json!({})))
                .await
            {
                self.set_last_error(error.to_string());
                self.abort_process().await;
                return Err(error);
            }

            if let Err(error) = self.ensure_live() {
                self.abort_process().await;
                return Err(error);
            }

            if !self.mark_initialized_if_running() {
                self.abort_process().await;
                return Err(LspError::NotInitialized);
            }
            Ok(())
        })
    }

    fn shutdown(&mut self) -> BoxFuture<'_, LspResult<()>> {
        Box::pin(async move {
            self.stopping.store(true, Ordering::SeqCst);
            if self.is_running() {
                // Send shutdown request
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    self.send_request("shutdown", None),
                )
                .await;
                // Send exit notification
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    self.send_notification("exit", None),
                )
                .await;
            }
            self.closed.store(true, Ordering::SeqCst);
            self.abort_process().await;
            Ok(())
        })
    }

    fn diagnostics<'a>(&'a self, uri: &'a str) -> BoxFuture<'a, LspResult<Vec<Diagnostic>>> {
        Box::pin(async move {
            self.ensure_live()?;
            if !self.is_initialized() {
                return Err(LspError::NotInitialized);
            }
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
        self.runtime
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .status
            .clone()
    }
}

#[cfg(test)]
mod message_size_tests {
    use super::*;

    #[test]
    fn rejects_unbounded_content_length() {
        assert!(accepts_lsp_message_size(MAX_LSP_MESSAGE_BYTES));
        assert!(!accepts_lsp_message_size(
            MAX_LSP_MESSAGE_BYTES.saturating_add(1)
        ));
    }

    #[test]
    fn eof_settlement_and_request_admission_share_one_boundary() -> Result<(), String> {
        let mut runtime = LspRuntime {
            status: LspServerStatus {
                language: "test".to_string(),
                running: true,
                initialized: true,
                restart_count: 0,
                last_error: None,
                pid: Some(42),
            },
            pending: HashMap::new(),
            accepting: true,
        };
        let (before_tx, before_rx) = oneshot::channel();
        runtime
            .register(1, before_tx)
            .map_err(|error| error.to_string())?;
        runtime.settle(Some("EOF".to_string()));
        if before_rx.blocking_recv().is_ok() {
            return Err("settlement did not close the admitted request".to_string());
        }
        let (after_tx, _after_rx) = oneshot::channel();
        if runtime.register(2, after_tx).is_ok() || !runtime.pending.is_empty() {
            return Err("request was admitted after EOF settlement".to_string());
        }
        Ok(())
    }
}
