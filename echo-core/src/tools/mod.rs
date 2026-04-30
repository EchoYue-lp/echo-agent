//! Tool system core trait and types

pub mod permission;

use crate::error::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
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
}

impl ToolResult {
    /// Construct a successful text result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
            bytes: None,
            data: None,
        }
    }

    /// Construct a successful result with structured JSON data.
    ///
    /// The `output` field is set to a compact JSON representation so
    /// plain-text consumers still see something useful.
    pub fn success_json(data: serde_json::Value) -> Self {
        let output = serde_json::to_string(&data).unwrap_or_default();
        Self {
            success: true,
            output,
            error: None,
            bytes: None,
            data: Some(data),
        }
    }

    /// Construct a failed result with an error message.
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
            bytes: None,
            data: None,
        }
    }

    /// Construct a successful result that carries binary payload.
    pub fn binary(bytes: Vec<u8>) -> Self {
        Self {
            success: true,
            output: String::new(),
            error: None,
            bytes: Some(bytes),
            data: None,
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
    /// Optional concurrency cap shared by the tool manager.
    pub max_concurrency: Option<usize>,
}

impl Default for ToolExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            retry_on_fail: false,
            max_retries: 2,
            retry_delay_ms: 200,
            max_concurrency: None,
        }
    }
}

/// Tool parameter type
pub type ToolParameters = HashMap<String, serde_json::Value>;

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

/// Tool interface trait
pub trait Tool: Send + Sync {
    /// Stable tool identifier exposed to the model.
    fn name(&self) -> &str;
    /// Human-readable tool description.
    fn description(&self) -> &str;
    /// JSON Schema describing accepted parameters.
    fn parameters(&self) -> serde_json::Value;
    /// Execute the tool with untyped JSON parameters.
    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>>;

    /// Validate parameters before execution.
    fn validate_parameters(&self, _params: &ToolParameters) -> BoxFuture<'_, Result<()>> {
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
}
