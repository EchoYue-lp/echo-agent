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
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(output: String) -> Self {
        Self {
            success: true,
            output,
            error: None,
        }
    }

    pub fn error(error: String) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error),
        }
    }
}

/// 工具执行配置：超时、重试、并发度
#[derive(Debug, Clone)]
pub struct ToolExecutionConfig {
    pub timeout_ms: u64,
    pub retry_on_fail: bool,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
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
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;
    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>>;

    fn validate_parameters(&self, _params: &ToolParameters) -> Result<()> {
        Ok(())
    }

    fn permissions(&self) -> Vec<permission::ToolPermission> {
        vec![]
    }
}

/// 强类型工具接口
pub trait TypedTool: Send + Sync {
    type Params: DeserializeOwned + JsonSchema;

    fn name(&self) -> &str;
    fn description(&self) -> &str;
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
            let value =
                serde_json::to_value(&parameters).map_err(|e| ToolError::InvalidParameter {
                    name: "(serialization)".to_string(),
                    message: e.to_string(),
                })?;
            let params: T::Params =
                serde_json::from_value(value).map_err(|e| ToolError::InvalidParameter {
                    name: "(deserialization)".to_string(),
                    message: e.to_string(),
                })?;
            TypedTool::execute_typed(self, params).await
        })
    }
}
