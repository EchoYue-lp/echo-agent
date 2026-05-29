//! Tool system core trait and types

pub mod permission;
pub mod skill;

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    Diff {
        unified_diff: String,
    },
    /// A reference to a file (path in metadata).
    FileReference {
        path: String,
    },
    /// Command execution output with exit code.
    CommandOutput {
        exit_code: Option<i32>,
    },
    /// A structured error with an error code.
    StructuredError {
        error_code: String,
    },
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
            kind: ToolResultKind::StructuredError { error_code: "tool_error".into() },
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
            serde_json::Value::Number(n) => {
                Self::Number(n.as_f64().unwrap_or(0.0))
            }
            serde_json::Value::Bool(b) => Self::Bool(*b),
            serde_json::Value::Array(arr) => {
                Self::Array(arr.iter().map(|v| Self::from_json(v)).collect())
            }
            serde_json::Value::Object(obj) => {
                Self::Object(obj.iter().map(|(k, v)| (k.clone(), Self::from_json(v))).collect())
            }
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
            map.iter().map(|(k, v)| (k.clone(), ParamValue::from_json(v))).collect()
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
    pub fn validate_required(&self, key: &str, expected_type: &str) -> std::result::Result<(), String> {
        match self.parsed.get(key) {
            None => Err(format!("Missing required parameter: {key}")),
            Some(v) if v.type_name() != expected_type => {
                Err(format!(
                    "Parameter '{key}': expected {expected_type}, got {}",
                    v.type_name()
                ))
            }
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
    fn execute<'a>(&'a self, parameters: ToolParameters) -> BoxFuture<'a, Result<ToolResult>>;

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
