//! 工具系统核心 trait 和类型

pub mod permission;

use crate::error::{Result, ToolError};
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
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
}

impl ToolResult {
    /// Construct a successful text result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
            bytes: None,
        }
    }

    /// Construct a failed result with an error message.
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
            bytes: None,
        }
    }

    /// Construct a successful result that carries binary payload.
    pub fn binary(bytes: Vec<u8>) -> Self {
        Self {
            success: true,
            output: String::new(),
            error: None,
            bytes: Some(bytes),
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
}

/// 工具执行配置：超时、重试、并发度
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

/// 工具参数类型
pub type ToolParameters = HashMap<String, serde_json::Value>;

/// 工具接口 trait
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
}

/// 强类型工具接口
pub trait TypedTool: Send + Sync {
    /// Strongly typed parameter payload accepted by the tool.
    type Params: DeserializeOwned + JsonSchema;

    /// Stable tool identifier exposed to the model.
    fn name(&self) -> &str;
    /// Human-readable tool description.
    fn description(&self) -> &str;
    /// Execute the tool after parameters are deserialized.
    fn execute_typed(&self, params: Self::Params) -> BoxFuture<'_, Result<ToolResult>>;
}

/// TypedTool 自动实现 Tool
impl<T: TypedTool> Tool for T {
    fn name(&self) -> &str {
        TypedTool::name(self)
    }

    fn description(&self) -> &str {
        TypedTool::description(self)
    }

    fn parameters(&self) -> serde_json::Value {
        let schema = schemars::schema_for!(T::Params);
        serde_json::to_value(schema).unwrap_or_default()
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            // Avoid redundant serialization: HashMap → Value::Object directly
            let value = serde_json::Value::Object(parameters.into_iter().collect());
            let params: T::Params =
                serde_json::from_value(value).map_err(|e| ToolError::InvalidParameter {
                    name: "(deserialization)".to_string(),
                    message: e.to_string(),
                })?;
            TypedTool::execute_typed(self, params).await
        })
    }
}
