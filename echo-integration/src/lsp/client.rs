//! Stdio-based LSP client — spawns a language server process and
//! communicates via JSON-RPC over stdin/stdout.

use echo_core::lsp::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticCode, DiagnosticSeverity, HoverInfo,
    Location, LspClient, LspError, LspResult, LspServerConfig, LspServerStatus, Position, Range,
    TextChange,
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

fn invalid_result(method: &str, message: impl std::fmt::Display) -> LspError {
    // Report schema failures without copying arbitrary server content to logs/errors.
    LspError::ServerError(format!("Invalid {method} result: {message}"))
}

fn decode_result<T: serde::de::DeserializeOwned>(
    method: &str,
    value: serde_json::Value,
) -> LspResult<T> {
    serde_json::from_value(value)
        .map_err(|_| invalid_result(method, "response does not match the expected value shape"))
}

fn range_object(method: &str, value: &serde_json::Value) -> LspResult<Range> {
    let range = value
        .as_object()
        .ok_or_else(|| invalid_result(method, "range must be an object"))?;
    for name in ["start", "end"] {
        let position = range
            .get(name)
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| invalid_result(method, "range positions must be objects"))?;
        for coordinate in ["line", "character"] {
            let value = position
                .get(coordinate)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| invalid_result(method, "position coordinates must be integers"))?;
            if value > 2_147_483_647 {
                return Err(invalid_result(
                    method,
                    "coordinate exceeds LSP uinteger range",
                ));
            }
        }
    }
    let range = decode_result::<Range>(method, value.clone())?;
    validate_range(method, &range)?;
    Ok(range)
}

fn location_object(method: &str, value: &serde_json::Value) -> LspResult<Location> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_result(method, "location must be an object"))?;
    let uri = object
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid_result(method, "location URI must be a string"))?;
    let range = object
        .get("range")
        .ok_or_else(|| invalid_result(method, "missing location range"))?;
    Ok(Location {
        uri: uri.to_string(),
        range: range_object(method, range)?,
    })
}

fn validate_range(method: &str, range: &Range) -> LspResult<()> {
    if [
        range.start.line,
        range.start.character,
        range.end.line,
        range.end.character,
    ]
    .into_iter()
    .any(|coordinate| coordinate > 2_147_483_647)
    {
        return Err(invalid_result(
            method,
            "coordinate exceeds LSP uinteger range",
        ));
    }
    let start = (range.start.line, range.start.character);
    let end = (range.end.line, range.end.character);
    if start > end {
        return Err(invalid_result(method, "range end precedes start"));
    }
    Ok(())
}

fn decode_locations(
    method: &str,
    result: serde_json::Value,
    single: bool,
) -> LspResult<Vec<Location>> {
    if result.is_null() {
        return Ok(Vec::new());
    }
    if single && result.is_object() {
        return location_object(method, &result).map(|location| vec![location]);
    }
    let items = result
        .as_array()
        .ok_or_else(|| invalid_result(method, "locations must be an array"))?;
    items
        .iter()
        .map(|value| location_object(method, value))
        .collect()
}

fn marked_string(method: &str, value: &serde_json::Value) -> LspResult<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid_result(method, "expected a marked string"))?;
    let language = object
        .get("language")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid_result(method, "marked string language must be a string"))?;
    let text = object
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid_result(method, "marked string value must be a string"))?;
    // Retain the language and code text through the existing Markdown projection.
    // Choose a fence longer than any run of backticks in the server text.
    let longest = text
        .chars()
        .fold((0usize, 0usize), |(run, longest), ch| {
            let run = if ch == '`' { run.saturating_add(1) } else { 0 };
            (run, longest.max(run))
        })
        .1;
    let fence: String = std::iter::repeat_n('`', longest.saturating_add(1).max(3)).collect();
    Ok(format!("{fence}{language}\n{text}\n{fence}"))
}

fn markup_content(method: &str, value: &serde_json::Value) -> LspResult<String> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_result(method, "expected markup content"))?;
    match object.get("kind").and_then(serde_json::Value::as_str) {
        Some("markdown" | "plaintext") => {}
        _ => {
            return Err(invalid_result(
                method,
                "markup kind must be markdown or plaintext",
            ));
        }
    }
    object
        .get("value")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| invalid_result(method, "markup value must be a string"))
}

fn hover_contents(method: &str, value: &serde_json::Value) -> LspResult<String> {
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|item| marked_string(method, item))
            .collect::<LspResult<Vec<_>>>()
            .map(|items| items.join("\n\n"));
    }
    if value.get("kind").is_some() {
        markup_content(method, value)
    } else {
        marked_string(method, value)
    }
}

fn decode_hover(result: serde_json::Value) -> LspResult<Option<HoverInfo>> {
    let method = "textDocument/hover";
    if result.is_null() {
        return Ok(None);
    }
    let object = result
        .as_object()
        .ok_or_else(|| invalid_result(method, "expected a Hover object"))?;
    let contents = object
        .get("contents")
        .ok_or_else(|| invalid_result(method, "missing contents"))?;
    let range = object
        .get("range")
        .map(|value| range_object(method, value))
        .transpose()?;
    if let Some(range) = &range {
        validate_range(method, range)?;
    }
    Ok(Some(HoverInfo {
        contents: hover_contents(method, contents)?,
        range,
    }))
}

fn optional_string(
    method: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> LspResult<Option<String>> {
    match object.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_result(method, format!("{name} must be a string"))),
    }
}

fn completion_kind(value: &serde_json::Value) -> LspResult<CompletionItemKind> {
    use CompletionItemKind::*;
    match value.as_u64() {
        Some(1) => Ok(Text),
        Some(2) => Ok(Method),
        Some(3) => Ok(Function),
        Some(4) => Ok(Constructor),
        Some(5) => Ok(Field),
        Some(6) => Ok(Variable),
        Some(7) => Ok(Class),
        Some(8) => Ok(Interface),
        Some(9) => Ok(Module),
        Some(10) => Ok(Property),
        Some(11) => Ok(Unit),
        Some(12) => Ok(Value),
        Some(13) => Ok(Enum),
        Some(14) => Ok(Keyword),
        Some(15) => Ok(Snippet),
        Some(16) => Ok(Color),
        Some(17) => Ok(File),
        Some(18) => Ok(Reference),
        Some(19) => Ok(Folder),
        Some(20) => Ok(EnumMember),
        Some(21) => Ok(Constant),
        Some(22) => Ok(Struct),
        Some(23) => Ok(Event),
        Some(24) => Ok(Operator),
        Some(25) => Ok(TypeParameter),
        _ => Err(invalid_result(
            "textDocument/completion",
            "kind must be an integer from 1 through 25",
        )),
    }
}

fn decode_completions(result: serde_json::Value) -> LspResult<Vec<CompletionItem>> {
    let method = "textDocument/completion";
    if result.is_null() {
        return Ok(Vec::new());
    }
    let items = if let Some(object) = result.as_object() {
        if object
            .get("isIncomplete")
            .and_then(serde_json::Value::as_bool)
            .is_none()
        {
            return Err(invalid_result(
                method,
                "CompletionList isIncomplete must be a boolean",
            ));
        }
        object.get("items").and_then(serde_json::Value::as_array)
    } else {
        result.as_array()
    }
    .ok_or_else(|| invalid_result(method, "expected completion items array"))?;
    items
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| invalid_result(method, "expected a completion item object"))?;
            let label = object
                .get("label")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid_result(method, "completion label must be a string"))?
                .to_string();
            let kind = match object.get("kind") {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => Some(completion_kind(value)?),
            };
            let documentation = match object.get("documentation") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(text)) => Some(text.clone()),
                Some(value) => Some(markup_content(method, value)?),
            };
            Ok(CompletionItem {
                label,
                kind,
                detail: optional_string(method, object, "detail")?,
                insert_text: optional_string(method, object, "insertText")?,
                documentation,
            })
        })
        .collect()
}

// One URI entry owns either the latest complete diagnostics or its schema
// failure. A rejected update cannot masquerade as empty or retain old success.
type DiagnosticsCache = Arc<Mutex<HashMap<String, Result<Vec<Diagnostic>, String>>>>;

fn decode_diagnostics(params: &serde_json::Value) -> LspResult<Vec<Diagnostic>> {
    let method = "textDocument/publishDiagnostics";
    let items = params
        .get("diagnostics")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid_result(method, "diagnostics must be an array"))?;
    items
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| invalid_result(method, "diagnostic must be an object"))?;
            let range = object
                .get("range")
                .ok_or_else(|| invalid_result(method, "missing diagnostic range"))?;
            let message = object
                .get("message")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid_result(method, "diagnostic message must be a string"))?
                .to_string();
            let severity = match object.get("severity") {
                None | Some(serde_json::Value::Null) => None,
                Some(value) => Some(match value.as_u64() {
                    Some(1) => DiagnosticSeverity::Error,
                    Some(2) => DiagnosticSeverity::Warning,
                    Some(3) => DiagnosticSeverity::Information,
                    Some(4) => DiagnosticSeverity::Hint,
                    _ => {
                        return Err(invalid_result(
                            method,
                            "severity must be an integer from 1 through 4",
                        ));
                    }
                }),
            };
            let code = match object.get("code") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(value)) => {
                    Some(DiagnosticCode::String(value.clone()))
                }
                Some(value) => {
                    let value = value
                        .as_i64()
                        .and_then(|number| i32::try_from(number).ok())
                        .ok_or_else(|| {
                            invalid_result(
                                method,
                                "diagnostic code must be an i32 integer or string",
                            )
                        })?;
                    Some(DiagnosticCode::Integer(value))
                }
            };
            Ok(Diagnostic {
                range: range_object(method, range)?,
                severity,
                message,
                source: optional_string(method, object, "source")?,
                code,
            })
        })
        .collect()
}

struct LspRuntime {
    status: LspServerStatus,
    pending: HashMap<u64, oneshot::Sender<LspResult<JsonRpcResponse>>>,
    accepting: bool,
}

impl LspRuntime {
    fn register(
        &mut self,
        id: u64,
        tx: oneshot::Sender<LspResult<JsonRpcResponse>>,
    ) -> LspResult<()> {
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
    diagnostics_cache: DiagnosticsCache,
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
        diagnostics_cache: DiagnosticsCache,
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
                // Matching malformed responses settle that request explicitly. Do not
                // drop their frame and make the caller wait for a transport timeout.
                let response = serde_json::from_value::<JsonRpcResponse>(value).map_err(|_| {
                    LspError::ServerError("Invalid JSON-RPC response shape".to_string())
                });
                let mut runtime = runtime.lock().unwrap_or_else(|poison| poison.into_inner());
                if let Some(tx) = runtime.pending.remove(&id) {
                    let _ = tx.send(response);
                }
            } else if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
                // Server notification
                if method == "textDocument/publishDiagnostics" {
                    let Some(params) = value.get("params").filter(|params| params.is_object())
                    else {
                        return "Invalid textDocument/publishDiagnostics: params must be an object"
                            .to_string();
                    };
                    let Some(uri) = params
                        .get("uri")
                        .and_then(serde_json::Value::as_str)
                        .filter(|uri| !uri.is_empty())
                    else {
                        // No document identity exists to own this error. Use the
                        // established connection/process settlement boundary.
                        return "Invalid textDocument/publishDiagnostics: URI must be a nonempty string".to_string();
                    };
                    let diagnostics = if value.get("jsonrpc").and_then(serde_json::Value::as_str)
                        == Some("2.0")
                    {
                        decode_diagnostics(params).map_err(|error| match error {
                            LspError::ServerError(message) => message,
                            other => other.to_string(),
                        })
                    } else {
                        Err(
                            "Invalid textDocument/publishDiagnostics: JSON-RPC version must be 2.0"
                                .to_string(),
                        )
                    };
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
            Ok(Ok(response)) => response.map_err(|error| invalid_result(method, error))?,
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

        if response.jsonrpc != "2.0" || (response.error.is_some() && response.result.is_some()) {
            return Err(invalid_result(
                method,
                "expected one JSON-RPC 2.0 result or error",
            ));
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
                            "relatedInformation": false
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
            let cache = self.diagnostics_cache.lock().await;
            self.ensure_live()?;
            // The existing admission boundary is the read's linearization
            // point, after cache acquisition. Fatal cleanup closes admission
            // before clearing cache or awaiting the real process exit.
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !runtime.accepting || !runtime.status.initialized {
                return Err(match &runtime.status.last_error {
                    Some(error) => LspError::ServerError(error.clone()),
                    None => LspError::NotInitialized,
                });
            }
            match cache.get(uri) {
                Some(Ok(diagnostics)) => Ok(diagnostics.clone()),
                Some(Err(error)) => Err(LspError::ServerError(error.clone())),
                None => Ok(Vec::new()),
            }
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

            decode_locations("textDocument/definition", result, true)
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

            decode_locations("textDocument/references", result, false)
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

            decode_hover(result)
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

            decode_completions(result)
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

    const DIAGNOSTIC_FATAL_SERVER: &str = r#"
import json, sys
while True:
    line = sys.stdin.buffer.readline()
    if not line: break
    size = int(line.split(b':', 1)[1].strip())
    sys.stdin.buffer.readline()
    request = json.loads(sys.stdin.buffer.read(size))
    if request.get('method') == 'initialize':
        reply = {'jsonrpc':'2.0','id':request['id'],'result':{'capabilities':{}}}
    elif request.get('method') == 'textDocument/hover':
        reply = {'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'diagnostics':[]}}
    else: continue
    body = json.dumps(reply).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
"#;

    #[tokio::test]
    async fn diagnostic_fidelity_terminal_admission_rejects_reads_while_child_teardown_is_blocked()
    -> Result<(), String> {
        let mut client = StdioLspClient::new(LspServerConfig {
            language: "test".to_string(),
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                "-c".to_string(),
                DIAGNOSTIC_FATAL_SERVER.to_string(),
            ],
            extensions: Vec::new(),
            env: HashMap::new(),
            initialization_options: None,
            max_restarts: 0,
        });
        client
            .initialize("file:///fixture")
            .await
            .map_err(|error| error.to_string())?;
        let child = client.child.as_ref().ok_or("missing process")?.clone();
        // Block the real existing cleanup owner at its Child mutex, making the
        // admission-closed / process-still-live interval deterministic.
        let child_guard = child.lock().await;
        let read = async {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    let closed = !client
                        .runtime
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .accepting;
                    if closed {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| {
                LspError::CommunicationError("reader did not close admission".to_string())
            })?;
            let status = client.status();
            assert!(
                status.running && status.initialized && status.pid.is_some(),
                "must not invent process exit"
            );
            client.diagnostics("file:///unseen.test").await.map(|_| ())
        };
        let request = client.hover(
            "file:///fatal",
            Position {
                line: 0,
                character: 3,
            },
        );
        let (pending, diagnostics) = tokio::join!(request, read);
        drop(child_guard);
        client.shutdown().await.map_err(|error| error.to_string())?;
        assert!(pending.is_err());
        assert!(
            matches!(diagnostics, Err(LspError::ServerError(_))),
            "terminal admission fabricated clean diagnostics: {diagnostics:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn diagnostic_fidelity_terminal_admission_rechecks_after_cache_wait() -> Result<(), String>
    {
        let mut client = StdioLspClient::new(LspServerConfig {
            language: "test".to_string(),
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                "-c".to_string(),
                DIAGNOSTIC_FATAL_SERVER.to_string(),
            ],
            extensions: Vec::new(),
            env: HashMap::new(),
            initialization_options: None,
            max_restarts: 0,
        });
        client
            .initialize("file:///fixture")
            .await
            .map_err(|error| error.to_string())?;
        let child = client.child.as_ref().ok_or("missing process")?.clone();
        let child_guard = child.lock().await;
        let cache_guard = client.diagnostics_cache.lock().await;
        let mut retrieval = client.diagnostics("file:///unseen.test");
        assert!(
            futures::poll!(&mut retrieval).is_pending(),
            "retrieve must wait on held cache"
        );
        let release = async {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if !client
                        .runtime
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .accepting
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .map_err(|_| "fatal did not close admission".to_string())?;
            drop(cache_guard);
            Ok::<_, String>(())
        };
        let (pending, released) = tokio::join!(
            client.hover(
                "file:///fatal",
                Position {
                    line: 0,
                    character: 3
                }
            ),
            release
        );
        released?;
        let diagnostics = retrieval.await;
        drop(child_guard);
        client.shutdown().await.map_err(|error| error.to_string())?;
        assert!(pending.is_err());
        assert!(
            matches!(diagnostics, Err(LspError::ServerError(_))),
            "cache wait crossed closed admission and returned success: {diagnostics:?}"
        );
        Ok(())
    }

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
