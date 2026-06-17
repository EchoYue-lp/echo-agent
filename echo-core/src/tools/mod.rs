//! Tool system core trait and types

pub mod permission;
pub mod skill;

use crate::error::Result;
use futures::future::BoxFuture;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;

/// Classifies the kind of result a tool produced.
///
/// This enriches [`ToolResult`] so downstream consumers (CLI rendering,
/// trace analysis, eval scoring) can handle results appropriately without
/// parsing the raw output string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind_type", rename_all = "snake_case")]
pub enum ToolResultKind {
    /// Plain text output.
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

impl Default for ToolResultKind {
    fn default() -> Self {
        Self::Text
    }
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

/// Streaming tool output event
///
/// When a tool implements [`Tool::execute_stream`] and [`Tool::supports_streaming`],
/// it produces a sequence of these events during execution, enabling real-time
/// progress reporting and incremental output delivery to the UI / caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum ToolStreamEvent {
    /// Progress notification with optional percentage (0-100).
    Progress {
        /// Human-readable progress description.
        message: String,
        /// Optional completion percentage (0-100).
        percent: Option<u8>,
    },
    /// Incremental partial output chunk (e.g. streaming text generation).
    PartialOutput {
        /// Output fragment produced so far.
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
            retry_on_fail: false,
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
            serde_json::Value::Array(arr) => {
                Self::Array(arr.iter().map(|v| Self::from_json(v)).collect())
            }
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

    /// Stream tool execution, producing incremental [`ToolStreamEvent`]s.
    ///
    /// The default implementation wraps [`Self::execute`] into a single
    /// [`ToolStreamEvent::Complete`] event, so existing tools work without
    /// any changes. Override this and [`Self::supports_streaming`] when
    /// the tool can produce real-time progress or partial output.
    ///
    /// # Returns
    ///
    /// A future that resolves to a boxed [`Stream`] of [`ToolStreamEvent`].
    /// The stream MUST end with a [`ToolStreamEvent::Complete`] event that
    /// carries the final [`ToolResult`].
    fn execute_stream<'a>(
        &'a self,
        params: ToolParameters,
    ) -> BoxFuture<'a, Result<Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>>>> {
        Box::pin(async move {
            let result = self.execute(params).await?;
            let stream: Pin<Box<dyn Stream<Item = ToolStreamEvent> + Send + 'a>> = Box::pin(
                stream::once(async move { ToolStreamEvent::Complete(result) }),
            );
            Ok(stream)
        })
    }

    /// Whether this tool supports streaming execution.
    ///
    /// Return `true` only if [`Self::execute_stream`] emits meaningful
    /// intermediate events (Progress / PartialOutput). The default is
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

/// 运行时上下文，工具执行时由 ExecuteStage 注入。
///
/// 所有字段均为 `Option`，`None` = 回退默认行为（向后兼容老工具）。
/// 工具 override `Tool::execute_with_context` 时可读取这些字段。
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    /// 会话绑定的默认工作目录（通常是 worktree 路径）。
    /// None = 回退进程 `current_dir`。
    pub working_dir: Option<std::path::PathBuf>,
    /// 会话标识（透传给 trace/audit）。
    pub conversation_id: Option<String>,
    /// 当前 run 标识（透传给 trace/audit）。
    pub run_id: Option<String>,
}

impl ToolContext {
    /// 解析路径：有绑定且为相对路径则 join；绝对路径或无绑定则原样返回。
    pub fn resolve_path<'a>(&self, path: &'a std::path::Path) -> std::borrow::Cow<'a, std::path::Path> {
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
    }

    #[test]
    fn test_new_sets_fields() {
        let ctx = ToolContext {
            working_dir: Some(PathBuf::from("/tmp/wt")),
            conversation_id: Some("conv-1".into()),
            run_id: Some("run-1".into()),
        };
        assert_eq!(ctx.working_dir.as_deref(), Some(std::path::Path::new("/tmp/wt")));
        assert_eq!(ctx.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(ctx.run_id.as_deref(), Some("run-1"));
    }

    #[test]
    fn test_resolve_path_relative_joins() {
        let ctx = ToolContext { working_dir: Some(PathBuf::from("/repo/wt")), ..Default::default() };
        let resolved = ctx.resolve_path(std::path::Path::new("src/main.rs"));
        assert_eq!(resolved.as_ref(), std::path::Path::new("/repo/wt/src/main.rs"));
    }

    #[test]
    fn test_resolve_path_absolute_not_joined() {
        let ctx = ToolContext { working_dir: Some(PathBuf::from("/repo/wt")), ..Default::default() };
        let resolved = ctx.resolve_path(std::path::Path::new("/etc/hosts"));
        assert_eq!(resolved.as_ref(), std::path::Path::new("/etc/hosts"));
    }

    #[test]
    fn test_resolve_path_no_working_dir_passthrough() {
        let ctx = ToolContext::default();
        let resolved = ctx.resolve_path(std::path::Path::new("a/b"));
        assert_eq!(resolved.as_ref(), std::path::Path::new("a/b"));
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
        fn execute<'a>(
            &'a self,
            params: ToolParameters,
        ) -> BoxFuture<'a, Result<ToolResult>> {
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
        };
        let mut params = ToolParameters::new();
        params.insert("x".into(), serde_json::json!("hello"));
        let result = tool.execute_with_context(params, &ctx).await.unwrap();
        assert!(result.success, "expected success");
        assert_eq!(result.output, "echo: hello");
    }
}
