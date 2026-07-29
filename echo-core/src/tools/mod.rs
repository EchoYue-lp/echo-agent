//! Tool system core trait and types

pub mod artifact;
pub mod permission;
pub mod skill;

use crate::error::{ReactError, Result, SandboxError, ToolError};
use crate::sandbox::SandboxExecutor;
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

/// Stable failure category shared by tool execution, traces, checkpoints, and UIs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureCategory {
    InvalidArguments,
    Unavailable,
    Timeout,
    Cancelled,
    Transient,
    Permanent,
    PartialSideEffect,
}

impl ToolFailureCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Transient => "transient",
            Self::Permanent => "permanent",
            Self::PartialSideEffect => "partial_side_effect",
        }
    }
}

/// What must happen before the same logical tool operation can continue.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolRecoveryAction {
    CorrectArguments,
    Retry,
    RestoreThenRetry,
    VerifyThenRetry,
    Stop,
}

impl ToolRecoveryAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CorrectArguments => "correct_arguments",
            Self::Retry => "retry",
            Self::RestoreThenRetry => "restore_then_retry",
            Self::VerifyThenRetry => "verify_then_retry",
            Self::Stop => "stop",
        }
    }
}

/// Whether an unsuccessful attempt may already have changed external state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ToolSideEffect {
    None,
    Possible,
    Confirmed,
}

/// Structured recovery facts for an unsuccessful tool result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFailure {
    pub category: ToolFailureCategory,
    pub recovery: ToolRecoveryAction,
    pub side_effect: ToolSideEffect,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub postcondition: Option<String>,
}

impl ToolFailure {
    pub fn new(category: ToolFailureCategory) -> Self {
        let recovery = match category {
            ToolFailureCategory::InvalidArguments => ToolRecoveryAction::CorrectArguments,
            ToolFailureCategory::Unavailable => ToolRecoveryAction::RestoreThenRetry,
            ToolFailureCategory::Timeout | ToolFailureCategory::PartialSideEffect => {
                ToolRecoveryAction::VerifyThenRetry
            }
            ToolFailureCategory::Transient => ToolRecoveryAction::Retry,
            ToolFailureCategory::Cancelled | ToolFailureCategory::Permanent => {
                ToolRecoveryAction::Stop
            }
        };
        let side_effect = if category == ToolFailureCategory::PartialSideEffect {
            ToolSideEffect::Possible
        } else {
            ToolSideEffect::None
        };
        Self {
            category,
            recovery,
            side_effect,
            retry_after_ms: None,
            idempotency_key: None,
            postcondition: None,
        }
    }

    pub fn retryable(mut self) -> Self {
        if matches!(
            self.category,
            ToolFailureCategory::Unavailable
                | ToolFailureCategory::Timeout
                | ToolFailureCategory::Transient
        ) {
            self.recovery = ToolRecoveryAction::Retry;
        }
        self
    }

    pub fn with_retry_after(mut self, retry_after_ms: u64) -> Self {
        self.retry_after_ms = Some(retry_after_ms);
        self
    }

    pub fn with_side_effect(mut self, side_effect: ToolSideEffect) -> Self {
        self.side_effect = side_effect;
        if side_effect != ToolSideEffect::None && self.idempotency_key.is_none() {
            self.recovery = ToolRecoveryAction::VerifyThenRetry;
        }
        self
    }

    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn with_postcondition(mut self, postcondition: impl Into<String>) -> Self {
        self.postcondition = Some(postcondition.into());
        self
    }

    /// Automatic replay is deliberately narrower than "might recover".
    pub fn allows_automatic_retry(&self) -> bool {
        matches!(
            self.category,
            ToolFailureCategory::Unavailable
                | ToolFailureCategory::Timeout
                | ToolFailureCategory::Transient
        ) && self.recovery == ToolRecoveryAction::Retry
            && (self.side_effect == ToolSideEffect::None || self.idempotency_key.is_some())
    }

    /// Conservatively classify framework execution errors.
    pub fn from_error(error: &ReactError, may_have_side_effects: bool) -> Self {
        let possible_side_effect = |mut failure: Self| {
            if may_have_side_effects {
                if matches!(
                    failure.category,
                    ToolFailureCategory::Unavailable
                        | ToolFailureCategory::Transient
                        | ToolFailureCategory::Permanent
                ) {
                    failure.category = ToolFailureCategory::PartialSideEffect;
                }
                return failure.with_side_effect(ToolSideEffect::Possible);
            }
            failure
        };
        match error {
            ReactError::Tool(tool_error) => match tool_error.as_ref() {
                ToolError::MissingParameter(_)
                | ToolError::InvalidParameter { .. }
                | ToolError::InvalidPath { .. }
                | ToolError::AccessDenied { .. }
                | ToolError::FileTooLarge { .. } => {
                    Self::new(ToolFailureCategory::InvalidArguments)
                }
                ToolError::Timeout(_) => {
                    possible_side_effect(Self::new(ToolFailureCategory::Timeout).retryable())
                }
                ToolError::NotFound(_) | ToolError::ExecutionFailed { .. } => {
                    possible_side_effect(Self::new(ToolFailureCategory::Permanent))
                }
            },
            ReactError::Sandbox(sandbox_error) => match sandbox_error.as_ref() {
                SandboxError::Unavailable(_) | SandboxError::StartFailed(_) => {
                    Self::new(ToolFailureCategory::Unavailable)
                }
                SandboxError::Timeout(_) => {
                    possible_side_effect(Self::new(ToolFailureCategory::Timeout).retryable())
                }
                SandboxError::Cancelled(_) => Self::new(ToolFailureCategory::Cancelled),
                SandboxError::ResourceExceeded(_)
                | SandboxError::PermissionDenied(_)
                | SandboxError::IoError(_) => {
                    possible_side_effect(Self::new(ToolFailureCategory::Permanent))
                }
            },
            ReactError::Io(io_error) => match io_error.kind() {
                std::io::ErrorKind::TimedOut => {
                    possible_side_effect(Self::new(ToolFailureCategory::Timeout).retryable())
                }
                std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::NotConnected => {
                    possible_side_effect(Self::new(ToolFailureCategory::Transient).retryable())
                }
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                    Self::new(ToolFailureCategory::InvalidArguments)
                }
                _ => possible_side_effect(Self::new(ToolFailureCategory::Permanent)),
            },
            #[cfg(feature = "mcp")]
            ReactError::Mcp(mcp_error) => match mcp_error.as_ref() {
                crate::error::McpError::ConnectionFailed(_)
                | crate::error::McpError::InitializationFailed(_)
                | crate::error::McpError::TransportClosed => {
                    let failure = Self::new(ToolFailureCategory::Unavailable);
                    if may_have_side_effects {
                        possible_side_effect(failure)
                    } else {
                        failure.retryable()
                    }
                }
                crate::error::McpError::ToolCallFailed { code: -32602, .. } => {
                    Self::new(ToolFailureCategory::InvalidArguments)
                }
                crate::error::McpError::ToolCallFailed {
                    code: -32603 | -32099..=-32000,
                    ..
                } => possible_side_effect(Self::new(ToolFailureCategory::Transient).retryable()),
                crate::error::McpError::ProtocolError(_)
                | crate::error::McpError::ToolCallFailed { .. } => {
                    possible_side_effect(Self::new(ToolFailureCategory::Permanent))
                }
            },
            _ => possible_side_effect(Self::new(ToolFailureCategory::Permanent)),
        }
    }
}

/// Classifies the kind of result a tool produced.
///
/// This enriches [`ToolResult`] so downstream consumers (CLI rendering,
/// trace analysis, eval scoring) can handle results appropriately without
/// parsing the raw output string.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind_type", rename_all = "snake_case")]
pub enum ToolResultKind {
    /// Plain text output.
    #[default]
    Text,
    /// Structured JSON data (also available in `ToolResult.data`).
    Json,
    /// An image (MIME type in `ToolResult.mime_type`).
    Image { mime_type: String },
    /// Tabular data with column headers and row data.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// A unified diff (e.g., from `edit_file`).
    Diff { unified_diff: String },
    /// A reference to a file (path in metadata).
    FileReference { path: String },
    /// Command execution output with exit code.
    CommandOutput { exit_code: Option<i32> },
    /// A structured error with an error code.
    StructuredError { error_code: String },
}

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The kind of result (enables type-aware downstream handling).
    #[serde(default)]
    pub kind: ToolResultKind,
    /// Whether the tool completed successfully.
    pub success: bool,
    /// Text output returned to the caller.
    pub output: String,
    /// Error message when `success` is false.
    pub error: Option<String>,
    /// Structured failure and recovery facts when `success` is false.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub failure: Option<ToolFailure>,
    /// Optional binary output (mutually exclusive with `output` in practice).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bytes: Option<Vec<u8>>,
    /// Optional structured data (JSON). When present, callers can render it
    /// directly instead of parsing the `output` string.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<serde_json::Value>,
    /// Whether the output was truncated to fit within a length limit.
    #[serde(default)]
    pub truncated: bool,
    /// MIME type of the output content, when known (e.g. `text/html`, `image/png`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mime_type: Option<String>,
    /// Arbitrary key-value metadata (e.g. source URL, file path, duration, token count).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl ToolResult {
    /// Construct a successful text result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            kind: ToolResultKind::Text,
            success: true,
            output: output.into(),
            error: None,
            failure: None,
            bytes: None,
            data: None,
            truncated: false,
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Construct a successful result with structured JSON data.
    ///
    /// The `output` field is set to a compact JSON representation so
    /// plain-text consumers still see something useful.
    pub fn success_json(data: serde_json::Value) -> Self {
        let output = serde_json::to_string(&data).unwrap_or_default();
        Self {
            kind: ToolResultKind::Json,
            success: true,
            output,
            error: None,
            failure: None,
            bytes: None,
            data: Some(data),
            truncated: false,
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Construct a successful result with a specific [`ToolResultKind`].
    pub fn success_with_kind(kind: ToolResultKind, output: impl Into<String>) -> Self {
        Self {
            kind,
            success: true,
            output: output.into(),
            error: None,
            failure: None,
            bytes: None,
            data: None,
            truncated: false,
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Construct a failed result with an error message.
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            kind: ToolResultKind::StructuredError {
                error_code: "tool_error".into(),
            },
            success: false,
            output: String::new(),
            error: Some(error.into()),
            failure: Some(ToolFailure::new(ToolFailureCategory::Permanent)),
            bytes: None,
            data: None,
            truncated: false,
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Construct a successful result that carries binary payload.
    pub fn binary(bytes: Vec<u8>) -> Self {
        Self {
            kind: ToolResultKind::Text,
            success: true,
            output: String::new(),
            error: None,
            failure: None,
            bytes: Some(bytes),
            data: None,
            truncated: false,
            mime_type: None,
            metadata: HashMap::new(),
        }
    }

    /// Attach binary payload without forcing call sites to rewrite struct literals.
    pub fn with_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.bytes = Some(bytes);
        self
    }

    /// Override the text output on an existing result.
    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = output.into();
        self
    }

    /// Override the error text on an existing result.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.success = false;
        self.error = Some(error.into());
        if self.failure.is_none() {
            self.failure = Some(ToolFailure::new(ToolFailureCategory::Permanent));
        }
        self
    }

    /// Construct a failed result with an explicit category.
    pub fn failure(category: ToolFailureCategory, error: impl Into<String>) -> Self {
        let mut result = Self::error(error);
        result.failure = Some(ToolFailure::new(category));
        result
    }

    /// Construct a failed result that requires corrected arguments.
    pub fn invalid_arguments(error: impl Into<String>) -> Self {
        Self::failure(ToolFailureCategory::InvalidArguments, error)
    }

    /// Replace structured recovery facts on an unsuccessful result.
    pub fn with_failure(mut self, failure: ToolFailure) -> Self {
        self.success = false;
        self.failure = Some(failure);
        self
    }

    /// Attach structured data to an existing result.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Mark the output as truncated.
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// Set the MIME type for the output content.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Insert a key-value metadata entry.
    pub fn with_meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Bulk-insert metadata entries.
    pub fn with_metadata(mut self, meta: HashMap<String, String>) -> Self {
        self.metadata = meta;
        self
    }
}

/// Logical channel for incremental tool output.
///
/// Tools such as shell map process pipes onto these channels; other tools may
/// use [`ToolOutputChannel::Log`] for unstructured progress text.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputChannel {
    /// Primary process / command stdout.
    Stdout,
    /// Process stderr or equivalent error stream.
    Stderr,
    /// Tool-defined log / diagnostic stream.
    Log,
}

/// Streaming tool output event
///
/// When a tool implements [`Tool::execute_stream_with_context`] and
/// [`Tool::supports_streaming`], it produces a sequence of these events during
/// execution, enabling real-time progress reporting and incremental output
/// delivery to the UI / caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
// Complete stays inline to preserve the public stream event API.
#[allow(clippy::large_enum_variant)]
pub enum ToolStreamEvent {
    /// Progress notification with optional percentage (0-100).
    Progress {
        /// Human-readable progress description.
        message: String,
        /// Optional completion percentage (0-100).
        percent: Option<u8>,
    },
    /// Incremental output chunk on a named channel.
    Output {
        /// Which logical stream the chunk belongs to.
        channel: ToolOutputChannel,
        /// Output fragment (UTF-8 text; may be a partial line).
        chunk: String,
    },
    /// Terminal event carrying the final [`ToolResult`].
    /// The stream ends after this event is emitted.
    Complete(ToolResult),
}

/// Tool execution config: timeout, retry, concurrency
#[derive(Debug, Clone)]
pub struct ToolExecutionConfig {
    /// Maximum execution time for a single attempt.
    pub timeout_ms: u64,
    /// Whether failed executions should be retried automatically.
    pub retry_on_fail: bool,
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Delay between retry attempts.
    pub retry_delay_ms: u64,
    /// Optional concurrency cap shared by the tool manager (write/execute tools).
    pub max_concurrency: Option<usize>,
    /// Optional concurrency cap for read-only tools (higher limit, default 32).
    pub max_read_concurrency: Option<usize>,
}

impl Default for ToolExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            retry_on_fail: true,
            max_retries: 2,
            retry_delay_ms: 200,
            max_concurrency: None,
            max_read_concurrency: Some(32),
        }
    }
}

/// Tool parameter type (simple key-value from LLM).
pub type ToolParameters = HashMap<String, serde_json::Value>;

/// A type-safe parameter value extracted from raw JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    String(String),
    Number(f64),
    Bool(bool),
    Array(Vec<ParamValue>),
    Object(HashMap<String, ParamValue>),
    Null,
}

impl ParamValue {
    /// Extract from a JSON value.
    pub fn from_json(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::String(s) => Self::String(s.clone()),
            serde_json::Value::Number(n) => Self::Number(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::Bool(b) => Self::Bool(*b),
            serde_json::Value::Array(arr) => Self::Array(arr.iter().map(Self::from_json).collect()),
            serde_json::Value::Object(obj) => Self::Object(
                obj.iter()
                    .map(|(k, v)| (k.clone(), Self::from_json(v)))
                    .collect(),
            ),
            serde_json::Value::Null => Self::Null,
        }
    }

    /// Get as string, if this is a String variant.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Get as number, if this is a Number variant.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Get as bool, if this is a Bool variant.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Type name for error messages.
    pub fn type_name(&self) -> &str {
        match self {
            Self::String(_) => "string",
            Self::Number(_) => "number",
            Self::Bool(_) => "bool",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
            Self::Null => "null",
        }
    }
}

/// Type-safe tool call parameters with validation support.
#[derive(Debug, Clone)]
pub struct ToolCallParams {
    /// Original raw JSON from the LLM.
    pub raw: serde_json::Value,
    /// Parsed type-safe values.
    parsed: HashMap<String, ParamValue>,
}

impl ToolCallParams {
    /// Create from a raw JSON value.
    pub fn from_value(value: &serde_json::Value) -> Self {
        let parsed = if let serde_json::Value::Object(map) = value {
            map.iter()
                .map(|(k, v)| (k.clone(), ParamValue::from_json(v)))
                .collect()
        } else {
            HashMap::new()
        };
        Self {
            raw: value.clone(),
            parsed,
        }
    }

    /// Create from a ToolParameters map.
    pub fn from_params(params: &ToolParameters) -> Self {
        let mut map = serde_json::Map::new();
        for (k, v) in params {
            map.insert(k.clone(), v.clone());
        }
        Self::from_value(&serde_json::Value::Object(map))
    }

    /// Get a string parameter.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.parsed.get(key).and_then(|v| v.as_str())
    }

    /// Get a numeric parameter.
    pub fn get_number(&self, key: &str) -> Option<f64> {
        self.parsed.get(key).and_then(|v| v.as_f64())
    }

    /// Get a boolean parameter.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.parsed.get(key).and_then(|v| v.as_bool())
    }

    /// Get a parameter value by key.
    pub fn get(&self, key: &str) -> Option<&ParamValue> {
        self.parsed.get(key)
    }

    /// Validate that a required parameter exists and has the expected type.
    /// Returns `Ok(())` or `Err(error_message)`.
    pub fn validate_required(
        &self,
        key: &str,
        expected_type: &str,
    ) -> std::result::Result<(), String> {
        match self.parsed.get(key) {
            None => Err(format!("Missing required parameter: {key}")),
            Some(v) if v.type_name() != expected_type => Err(format!(
                "Parameter '{key}': expected {expected_type}, got {}",
                v.type_name()
            )),
            _ => Ok(()),
        }
    }

    /// Check if a parameter exists.
    pub fn has(&self, key: &str) -> bool {
        self.parsed.contains_key(key)
    }

    /// Number of parameters.
    pub fn len(&self) -> usize {
        self.parsed.len()
    }

    /// Whether there are no parameters.
    pub fn is_empty(&self) -> bool {
        self.parsed.is_empty()
    }
}

/// Tool risk level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ToolRiskLevel {
    /// Read-only operation, no side effects (e.g. search, read file)
    ReadOnly,
    /// Standard operation, limited side effects (e.g. write file, call API)
    #[default]
    Standard,
    /// Dangerous operation, irreversible side effects (e.g. execute shell command, delete data, SQL write)
    Dangerous,
}

/// Trait for types that can register tools.
///
/// Decouples tool registration from any concrete tool-manager implementation.
pub trait ToolRegistrar {
    fn register(&mut self, tool: Box<dyn Tool>);
}

/// Helper trait for `#[derive(Tool)]` — provides a typed `run` method that
/// the derive macro's generated `execute()` delegates to.
///
/// Users override `run()` with their tool's business logic; the derive macro
/// handles JSON Schema generation, parameter deserialization, and `Tool` trait
/// boilerplate automatically.
pub trait ToolRunner<P = ToolParameters>: Tool + Sized {
    /// Execute the tool with typed, deserialized parameters.
    fn run(&self, params: P) -> impl std::future::Future<Output = Result<ToolResult>> + Send;
}

/// Tool interface trait
pub trait Tool: Send + Sync {
    /// Stable tool identifier exposed to the model.
    fn name(&self) -> &str;
    /// Human-readable tool description.
    fn description(&self) -> &str;
    /// JSON Schema describing accepted parameters.
    fn parameters(&self) -> serde_json::Value;
    /// Execute the tool with untyped JSON parameters.
    ///
    /// **Default implementation** delegates to [`Self::execute_with_context`]
    /// with an empty [`ToolContext`] — so tools that override
    /// `execute_with_context` need NOT also implement `execute`. Legacy tools
    /// that only implement `execute` keep working because the default
    /// `execute_with_context` delegates back to `execute` (no infinite
    /// recursion: a concrete `impl Tool` must override at least one of the two).
    fn execute<'a>(&'a self, parameters: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
        // Cannot delegate to execute_with_context directly because its ctx
        // borrow is tied to 'a (self's lifetime) and a default() temporary
        // would not live that long. Inline a default ctx owned by the future.
        Box::pin(async move {
            let ctx = ToolContext::default();
            self.execute_with_context(parameters, &ctx).await
        })
    }

    /// Execute the tool with a runtime [`ToolContext`].
    ///
    /// **Default implementation** ignores `ctx` and delegates to
    /// [`Self::execute`] — therefore existing `impl Tool` blocks need no
    /// changes to keep working. Tools that care about `working_dir`
    /// (ShellTool, file tools, git, worktree_tool) override this method to
    /// honor `ctx.working_dir` / `ctx.resolve_path` when building
    /// `SandboxCommand`s or resolving file paths.
    ///
    /// The framework's [`ToolManager::execute_tool_with_context`] always
    /// routes through this method, so a per-agent `working_dir` is naturally
    /// delivered to tools without the (shared, stateless) ToolManager holding
    /// any session state — avoiding cross-session cwd contamination.
    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        self.execute(parameters)
    }

    /// Optional sandbox injection (P2: run_code 真沙箱).
    ///
    /// Tools that hold a sandbox (`ShellTool`/`RunCodeTool`) override this to
    /// receive the executor at agent-setup time (via `set_sandbox_manager` →
    /// `ToolManager::apply_sandbox`). Returns `true` if the tool accepted the
    /// sandbox, `false` (default) for tools that don't use one.
    fn set_sandbox(&self, _sandbox: Arc<dyn SandboxExecutor>) -> bool {
        false
    }

    /// Whether this tool is exempt from the parallel batch timeout.
    ///
    /// Long-running tools (subagent dispatch such as `agent_tool` /
    /// `delegate_readonly`, web research) that internally run their own
    /// multi-step ReAct should override this to return `true`. Such tools are
    /// separated out of the concurrent batch's total timeout (see
    /// `react_loop.rs`) and instead run with their own per-execution timeout
    /// (e.g. the subagent 600s default in `SubagentExecutor`), because their
    /// latency is inherently far higher than typical file/shell tools and would
    /// otherwise dominate the batch budget, prematurely cancelling peers.
    ///
    /// Default: `false` (subject to the batch timeout like all other tools).
    fn exempt_from_batch_timeout(&self) -> bool {
        false
    }

    /// Whether this tool enforces its own execution deadline.
    ///
    /// Tools that opt out of the outer batch timeout are long-running by
    /// design and normally own a more appropriate deadline (for example, a
    /// Subagent dispatch timeout). Keep that established behavior as the
    /// default while exposing the decision explicitly to [`ToolManager`],
    /// which otherwise applies the ordinary per-tool timeout as well.
    fn manages_own_timeout(&self) -> bool {
        self.exempt_from_batch_timeout()
    }

    /// Stream tool execution with full [`ToolContext`].
    ///
    /// Prefer this over [`Self::execute_stream`]. The default implementation
    /// calls [`Self::execute_with_context`] and yields a single
    /// [`ToolStreamEvent::Complete`]. Override together with
    /// [`Self::supports_streaming`] when the tool can emit real-time
    /// [`ToolStreamEvent::Progress`] / [`ToolStreamEvent::Output`].
    ///
    /// The returned stream MUST end with [`ToolStreamEvent::Complete`].
    fn execute_stream_with_context<'a>(
        &'a self,
        params: ToolParameters,
        ctx: &ToolContext,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let result = self.execute_with_context(params, &ctx).await?;
            let stream: Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>> = Box::pin(
                stream::once(async move { ToolStreamEvent::Complete(result) }),
            );
            Ok(stream)
        })
    }

    /// Stream tool execution with an empty [`ToolContext`].
    ///
    /// Compatibility entry point — delegates to
    /// [`Self::execute_stream_with_context`]. New call sites should pass a
    /// real context instead.
    fn execute_stream<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            let ctx = ToolContext::default();
            self.execute_stream_with_context(params, &ctx).await
        })
    }

    /// Whether this tool supports streaming execution.
    ///
    /// Return `true` only if [`Self::execute_stream_with_context`] emits
    /// meaningful intermediate events (`Progress` / `Output`). The default is
    /// `false` so that non-streaming tools are not inadvertently routed
    /// through the stream path.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Validate parameters before execution.
    fn validate_parameters<'a>(&'a self, _params: &'a ToolParameters) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Permissions required to invoke this tool.
    fn permissions(&self) -> Vec<permission::ToolPermission> {
        vec![]
    }

    /// Risk level of this tool. Dangerous tools require explicit approval.
    fn risk_level(&self) -> ToolRiskLevel {
        ToolRiskLevel::Standard
    }

    /// Human-readable capability declaration (e.g. "Reads files", "Executes shell commands").
    fn capability_description(&self) -> &str {
        match self.risk_level() {
            ToolRiskLevel::ReadOnly => "Read-only: no side effects",
            ToolRiskLevel::Standard => "Standard: limited side effects",
            ToolRiskLevel::Dangerous => "Dangerous: irreversible side effects",
        }
    }
}

/// 应用层注入的 run 级上下文（跨 spawn 安全，值传递）。
///
/// 由应用层在每次 run 启动时构造，经 `AgentInvocationContext` 随单次调用
/// 传入，最终由 pipeline 填入 [`ToolContext`]。
///
/// 为什么需要这个：`tokio::task_local!` 不会跨 `tokio::spawn` 继承。subagent 在框架
/// 层的 `tokio::spawn`（subagent_executor.rs）里执行，task_local 全部丢失。而
/// `ExternalRunContext` 是值传递（Clone），天然跨 spawn 安全，是传递 run_id /
/// cancel / trace_sink 的正确通路。
///
/// `trace_sink` 用 `serde_json::Value`（而非应用层的具体事件类型）以避免框架依赖
/// 应用层类型——应用层在填入时把自己的事件序列化成 Value。
pub type TraceSinkFn = std::sync::Arc<dyn Fn(serde_json::Value) + Send + Sync>;

/// Nested delegation policy for agents and subagents.
///
/// Lives in `echo_core` because [`ToolContext`] is the cross-spawn channel used
/// by `agent_tool`; higher-level task/runtime crates may re-export this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NestedDelegationPolicy {
    /// Whether this subagent role may spawn child subagents.
    pub can_spawn_subagents: bool,
    /// Current delegation depth for this subagent.
    pub delegate_depth: u8,
    /// Maximum permitted delegation depth.
    pub max_delegate_depth: u8,
}

impl Default for NestedDelegationPolicy {
    fn default() -> Self {
        Self {
            can_spawn_subagents: false,
            delegate_depth: 0,
            max_delegate_depth: 2,
        }
    }
}

impl NestedDelegationPolicy {
    /// Whether a child subagent can be created under this policy.
    pub fn can_delegate(&self) -> bool {
        self.can_spawn_subagents && self.delegate_depth < self.max_delegate_depth
    }

    /// Policy to pass to a child subagent, if delegation is allowed.
    pub fn child_policy(&self) -> Option<Self> {
        if !self.can_delegate() {
            return None;
        }

        Some(Self {
            can_spawn_subagents: self.can_spawn_subagents,
            delegate_depth: self.delegate_depth.saturating_add(1),
            max_delegate_depth: self.max_delegate_depth,
        })
    }
}

#[derive(Clone)]
pub struct ExternalRunContext {
    /// 当前会话标识，跨主 agent/subagent 保持稳定。
    pub conversation_id: Option<String>,
    /// 当前应用层正式 run 标识。普通对话 turn 尚未创建任务 run 时为 None。
    pub run_id: Option<String>,
    /// 当前用户输入/agent turn 的稳定标识。
    pub turn_id: Option<String>,
    /// 当前 run 内的一次具体执行标识。
    ///
    /// `None` 表示只有 run 级上下文。设置后，subagent / tool trace 应使用它
    /// 作为前端可见执行记录的稳定 id，而不是再临时分配一套 dispatch id。
    pub execution_id: Option<String>,
    /// 隔离资源的稳定标识，可跨同一逻辑任务的多次执行尝试复用。
    ///
    /// 与 `execution_id` 不同，此字段不表示一次具体执行，也不用于事件关联。
    /// Worktree / workspace 等隔离资源可优先使用它来避免重试时重复创建。
    pub isolation_id: Option<String>,
    /// 触发本次 run 的消息 id（chat 场景下 = message_key，用于把 subagent
    /// 执行流钉到聊天流里对应的消息区块上）。`None` = 非 chat 路径（cron 等）。
    pub message_id: Option<String>,
    /// 当前 run 的取消令牌。
    pub cancel: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
    /// Subagent trace 事件回传通道。
    pub trace_sink: Option<TraceSinkFn>,
    /// Nested delegation policy to propagate into tools such as `agent_tool`.
    pub delegation_policy: Option<NestedDelegationPolicy>,
}

/// 运行时上下文，工具执行时由 ExecuteStage 注入。
///
/// 所有字段均为 `Option`，`None` = 回退默认行为（向后兼容老工具）。
/// 工具 override `Tool::execute_with_context` 时可读取这些字段。
///
/// `cancel` / `trace_sink` 由应用层经 [`ExternalRunContext`] 注入 subagent，
/// 再由 pipeline 填入此处——这是一条跨 `tokio::spawn` 安全的值传递通路（替代会跨
/// spawn 断裂的 task_local）。
#[derive(Clone, Default)]
pub struct ToolContext {
    /// 会话绑定的默认工作目录（通常是 worktree 路径）。
    /// None = 回退进程 `current_dir`。
    pub working_dir: Option<std::path::PathBuf>,
    /// 会话标识（透传给 trace/audit）。
    pub conversation_id: Option<String>,
    /// 当前 run 标识（透传给 trace/audit）。
    pub run_id: Option<String>,
    /// 当前用户输入/agent turn 标识。
    pub turn_id: Option<String>,
    /// 触发当前工具调用的消息标识。Chat 场景用于把工具派生的执行流绑定到
    /// 正确的消息区块；非 Chat 路径为 `None`。
    pub message_id: Option<String>,
    /// 当前 run 内的一次具体执行标识。
    pub execution_id: Option<String>,
    /// Stable identity for this logical tool call and all of its retry attempts.
    pub call_id: Option<String>,
    /// Optional application-selected root and retention policy for complete
    /// tool-output artifacts. Streaming tools should use this instead of
    /// retaining unbounded output in memory.
    pub output_artifacts: Option<artifact::ToolOutputArtifactConfig>,
    /// Invocation-scoped schema visibility shared with `tool_search`.
    pub tool_visibility: Option<std::sync::Arc<ToolVisibilityState>>,
    /// 跨 spawn 安全的取消令牌（值传递，非 task_local）。
    pub cancel: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
    /// 跨 spawn 安全的 trace 回传（值传递）。
    pub trace_sink: Option<TraceSinkFn>,
    /// Nested delegation policy for LLM-callable dispatch tools.
    pub delegation_policy: Option<NestedDelegationPolicy>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("working_dir", &self.working_dir)
            .field("conversation_id", &self.conversation_id)
            .field("run_id", &self.run_id)
            .field("turn_id", &self.turn_id)
            .field("message_id", &self.message_id)
            .field("execution_id", &self.execution_id)
            .field("call_id", &self.call_id)
            .field("output_artifacts", &self.output_artifacts)
            .field(
                "visible_tool_count",
                &self
                    .tool_visibility
                    .as_ref()
                    .map(|visibility| visibility.visible_names().len()),
            )
            .field(
                "cancel",
                &self.cancel.as_ref().map(|_| "<CancellationToken>"),
            )
            .field("trace_sink", &self.trace_sink.as_ref().map(|_| "<sink>"))
            .field("delegation_policy", &self.delegation_policy)
            .finish()
    }
}

/// Mutable per-invocation tool visibility without duplicating the tool registry.
#[derive(Debug)]
pub struct ToolVisibilityState {
    available: std::collections::HashSet<String>,
    eligible: std::sync::RwLock<std::collections::HashSet<String>>,
    visible: std::sync::RwLock<std::collections::HashSet<String>>,
}

impl ToolVisibilityState {
    pub fn new(
        eligible: std::collections::HashSet<String>,
        initial: std::collections::HashSet<String>,
    ) -> Self {
        Self::with_available(eligible.clone(), eligible, initial)
    }

    /// Build visibility with a wider activation catalog than the currently
    /// eligible policy surface. Product policy and skills may later promote
    /// names from `available` through [`Self::extend_eligibility_and_activate`].
    pub fn with_available(
        available: std::collections::HashSet<String>,
        eligible: std::collections::HashSet<String>,
        initial: std::collections::HashSet<String>,
    ) -> Self {
        let eligible = eligible
            .into_iter()
            .filter(|name| available.contains(name))
            .collect();
        let state = Self {
            available,
            eligible: std::sync::RwLock::new(eligible),
            visible: std::sync::RwLock::new(std::collections::HashSet::new()),
        };
        state.activate(initial);
        state
    }

    pub fn is_eligible(&self, name: &str) -> bool {
        self.eligible
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(name)
    }

    pub fn is_visible(&self, name: &str) -> bool {
        self.visible
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(name)
    }

    pub fn activate<I>(&self, names: I) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        let eligible = self
            .eligible
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut visible = self
            .visible
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut activated = Vec::new();
        for name in names {
            if eligible.contains(&name) && visible.insert(name.clone()) {
                activated.push(name);
            }
        }
        activated.sort();
        activated
    }

    /// Extend the invocation policy and expose the promoted schemas together.
    pub fn extend_eligibility_and_activate<I>(&self, names: I) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        let promoted = {
            let mut eligible = self
                .eligible
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            names
                .into_iter()
                .filter(|name| self.available.contains(name))
                .inspect(|name| {
                    eligible.insert(name.clone());
                })
                .collect::<Vec<_>>()
        };
        self.activate(promoted)
    }

    pub fn available_names(&self) -> Vec<String> {
        let mut names = self.available.iter().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn visible_names(&self) -> Vec<String> {
        let mut names = self
            .visible
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

impl ToolContext {
    /// 解析路径：有绑定且为相对路径则 join；绝对路径或无绑定则原样返回。
    pub fn resolve_path<'a>(
        &self,
        path: &'a std::path::Path,
    ) -> std::borrow::Cow<'a, std::path::Path> {
        match &self.working_dir {
            Some(base) if !path.is_absolute() => std::borrow::Cow::Owned(base.join(path)),
            _ => std::borrow::Cow::Borrowed(path),
        }
    }
}

#[cfg(test)]
mod tool_context_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_default_is_all_none() {
        let ctx = ToolContext::default();
        assert!(ctx.working_dir.is_none());
        assert!(ctx.conversation_id.is_none());
        assert!(ctx.run_id.is_none());
        assert!(ctx.message_id.is_none());
        assert!(ctx.delegation_policy.is_none());
    }

    #[test]
    fn permanent_and_partial_failures_cannot_be_made_automatically_retryable() {
        let permanent = ToolFailure::new(ToolFailureCategory::Permanent).retryable();
        let partial = ToolFailure::new(ToolFailureCategory::PartialSideEffect)
            .with_idempotency_key("call-1")
            .retryable();

        assert!(!permanent.allows_automatic_retry());
        assert!(!partial.allows_automatic_retry());
    }

    // ── stage4 P4.1: cache_user_id single-source ────────────────────────────
    // The external cache_user_id channel (ExternalRunContext.cache_user_id +
    // ToolContext.cache_user_id) was dead — no tool ever read ToolContext's
    // field. Removed in favor of the single config source. These compile-time
    // assertions guard the removal: both structs must construct without it.

    #[test]
    fn external_run_context_constructs_without_cache_user_id() {
        let _ctx = ExternalRunContext {
            conversation_id: Some("conv-1".to_string()),
            run_id: Some("run-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            execution_id: None,
            isolation_id: None,
            message_id: None,
            cancel: None,
            trace_sink: None,
            delegation_policy: None,
            // No cache_user_id field — if it still exists, this fails to compile.
        };
    }

    #[test]
    fn tool_context_constructs_without_cache_user_id() {
        let _ctx = ToolContext {
            working_dir: None,
            conversation_id: None,
            run_id: None,
            turn_id: None,
            message_id: None,
            execution_id: None,
            call_id: None,
            output_artifacts: None,
            tool_visibility: None,
            cancel: None,
            trace_sink: None,
            delegation_policy: None,
            // No cache_user_id field — if it still exists, this fails to compile.
        };
    }

    #[test]
    fn test_new_sets_fields() {
        let ctx = ToolContext {
            working_dir: Some(PathBuf::from("/tmp/wt")),
            conversation_id: Some("conv-1".into()),
            run_id: Some("run-1".into()),
            ..Default::default()
        };
        assert_eq!(
            ctx.working_dir.as_deref(),
            Some(std::path::Path::new("/tmp/wt"))
        );
        assert_eq!(ctx.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(ctx.run_id.as_deref(), Some("run-1"));
    }

    #[test]
    fn test_resolve_path_relative_joins() {
        let ctx = ToolContext {
            working_dir: Some(PathBuf::from("/repo/wt")),
            ..Default::default()
        };
        let resolved = ctx.resolve_path(std::path::Path::new("src/main.rs"));
        assert_eq!(
            resolved.as_ref(),
            std::path::Path::new("/repo/wt/src/main.rs")
        );
    }

    #[test]
    fn test_resolve_path_absolute_not_joined() {
        let ctx = ToolContext {
            working_dir: Some(PathBuf::from("/repo/wt")),
            ..Default::default()
        };
        let resolved = ctx.resolve_path(std::path::Path::new("/etc/hosts"));
        assert_eq!(resolved.as_ref(), std::path::Path::new("/etc/hosts"));
    }

    #[test]
    fn test_resolve_path_no_working_dir_passthrough() {
        let ctx = ToolContext::default();
        let resolved = ctx.resolve_path(std::path::Path::new("a/b"));
        assert_eq!(resolved.as_ref(), std::path::Path::new("a/b"));
    }

    #[test]
    fn visibility_can_promote_available_policy_names() {
        let state = ToolVisibilityState::with_available(
            ["tool_search", "git_status", "git_commit"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ["tool_search", "git_status"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ["tool_search", "git_commit"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );

        assert!(state.is_visible("tool_search"));
        assert!(!state.is_visible("git_commit"));
        assert_eq!(
            state.extend_eligibility_and_activate(["git_commit".to_string()]),
            vec!["git_commit"]
        );
        assert!(state.is_eligible("git_commit"));
        assert!(state.is_visible("git_commit"));
    }
}

#[cfg(test)]
mod exempt_from_batch_timeout_tests {
    use super::*;

    /// A tool that does NOT override `exempt_from_batch_timeout`. Verifies the
    /// trait default is `false` (subject to the batch timeout like all ordinary
    /// tools). Guards against accidental default flip.
    struct OrdinaryTool;

    impl Tool for OrdinaryTool {
        fn name(&self) -> &str {
            "ordinary"
        }
        fn description(&self) -> &str {
            "test tool"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({})
        }
    }

    /// A tool that DOES override `exempt_from_batch_timeout -> true`, modeling
    /// long-running dispatch tools (agent_tool / delegate_readonly).
    struct LongRunningTool;

    impl Tool for LongRunningTool {
        fn name(&self) -> &str {
            "long_running"
        }
        fn description(&self) -> &str {
            "long-running dispatch tool"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn exempt_from_batch_timeout(&self) -> bool {
            true
        }
    }

    #[test]
    fn ordinary_tool_is_not_exempt_by_default() {
        let t = OrdinaryTool;
        assert!(
            !t.exempt_from_batch_timeout(),
            "default must be false — ordinary tools are subject to the batch timeout"
        );
    }

    #[test]
    fn long_running_tool_can_opt_into_exempt() {
        let t = LongRunningTool;
        assert!(
            t.exempt_from_batch_timeout(),
            "override -> true must be honored so long-running tools bypass the batch timeout"
        );
    }

    /// Trait-object dispatch: the exemption must be observable through
    /// `dyn Tool` (this is how `react_loop` reads it via `ToolManager::get_tool`).
    #[test]
    fn exemption_visible_through_dyn_trait_object() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(OrdinaryTool), Box::new(LongRunningTool)];
        let ordinary = tools
            .iter()
            .find(|t| t.name() == "ordinary")
            .expect("ordinary tool present");
        let long_running = tools
            .iter()
            .find(|t| t.name() == "long_running")
            .expect("long_running tool present");
        assert!(!ordinary.exempt_from_batch_timeout());
        assert!(long_running.exempt_from_batch_timeout());
        assert!(long_running.manages_own_timeout());
    }
}

#[cfg(test)]
mod execute_with_context_tests {
    use super::*;

    /// A "legacy-style" tool that only implements `execute`. Verifies the
    /// trait's default `execute_with_context` delegates to `execute`
    /// regardless of the supplied context.
    struct LegacyTool;

    impl Tool for LegacyTool {
        fn name(&self) -> &str {
            "legacy"
        }
        fn description(&self) -> &str {
            "old-style tool, no context awareness"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn execute<'a>(&'a self, params: ToolParameters) -> BoxFuture<'a, Result<ToolResult>> {
            Box::pin(async move {
                let msg = format!(
                    "echo: {}",
                    params.get("x").and_then(|v| v.as_str()).unwrap_or("")
                );
                Ok(ToolResult::success(msg))
            })
        }
    }

    #[tokio::test]
    async fn test_default_delegates_to_execute_ignoring_ctx() {
        let tool = LegacyTool;
        // A non-default ctx that the default impl must ignore.
        let ctx = ToolContext {
            working_dir: Some(std::path::PathBuf::from("/wt")),
            conversation_id: Some("c".into()),
            run_id: Some("r".into()),
            ..Default::default()
        };
        let mut params = ToolParameters::new();
        params.insert("x".into(), serde_json::json!("hello"));
        let result = tool.execute_with_context(params, &ctx).await.unwrap();
        assert!(result.success, "expected success");
        assert_eq!(result.output, "echo: hello");
    }
}

#[cfg(test)]
mod tool_stream_event_contract_tests {
    use super::*;

    #[test]
    fn tool_stream_event_serde_round_trip_output_channels() {
        let events = vec![
            ToolStreamEvent::Progress {
                message: "running".into(),
                percent: Some(40),
            },
            ToolStreamEvent::Output {
                channel: ToolOutputChannel::Stdout,
                chunk: "hello 中文 🦀".into(),
            },
            ToolStreamEvent::Output {
                channel: ToolOutputChannel::Stderr,
                chunk: "warn\n".into(),
            },
            ToolStreamEvent::Output {
                channel: ToolOutputChannel::Log,
                chunk: "note".into(),
            },
            ToolStreamEvent::Complete(ToolResult::success("done").with_meta("exit_code", "0")),
        ];

        for event in events {
            let json = serde_json::to_string(&event).expect("serialize");
            let back: ToolStreamEvent = serde_json::from_str(&json).expect("deserialize");
            let again = serde_json::to_string(&back).expect("re-serialize");
            assert_eq!(json, again, "serde round-trip must be stable");
        }
    }

    #[test]
    fn tool_stream_event_json_uses_output_not_partial_output() {
        let event = ToolStreamEvent::Output {
            channel: ToolOutputChannel::Stdout,
            chunk: "x".into(),
        };
        let json = serde_json::to_value(&event).expect("to_value");
        assert_eq!(json["event_type"], "output");
        assert!(json.get("chunk").is_some() || json.get("channel").is_some());
        assert!(
            json.get("event_type").and_then(|v| v.as_str()) != Some("partial_output"),
            "PartialOutput variant must not exist"
        );
    }
}
