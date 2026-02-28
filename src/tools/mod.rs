//! 工具系统
//!
//! 定义 [`Tool`] trait 和 [`ToolManager`]（注册、执行、并发限流、超时重试）。
//! 内置工具位于 [`builtin`]，扩展工具位于 [`files`]、[`others`]、[`shell`]。

pub mod builtin;
pub mod files;
pub mod others;
pub mod shell;

use crate::error::{Result, ToolError};
use crate::llm::types::ToolDefinition;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// 工具执行配置：超时、重试、并发度
#[derive(Debug, Clone)]
pub struct ToolExecutionConfig {
    /// 单次工具执行超时（毫秒）。0 = 不限制。默认 30_000（30 秒）
    pub timeout_ms: u64,
    /// 工具执行失败时是否自动重试。默认 false
    pub retry_on_fail: bool,
    /// `retry_on_fail=true` 时的最大重试次数。默认 2
    pub max_retries: u32,
    /// 重试前首次等待（毫秒），每次翻倍指数退避。默认 200
    pub retry_delay_ms: u64,
    /// 并行工具调用时的最大并发数。`None` = 不限制（全并发）。默认 `None`
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

pub type ToolParameters = HashMap<String, serde_json::Value>;

/// 工具接口，所有内置和外部工具均实现此 trait
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// 工具参数的 JSON Schema 定义
    fn parameters(&self) -> serde_json::Value;
    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult>;
}

pub struct ToolManager {
    tools: HashMap<String, Box<dyn Tool>>,
    config: ToolExecutionConfig,
    /// 并发限流器：`Some(sem)` 表示最多同时执行 N 个工具；`None` = 不限制
    semaphore: Option<Arc<Semaphore>>,
}

impl ToolManager {
    pub(crate) fn to_openai_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition::from_tool(&**tool))
            .collect()
    }
}

impl Default for ToolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolManager {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            semaphore: None,
            config: ToolExecutionConfig::default(),
        }
    }

    pub fn new_with_config(config: ToolExecutionConfig) -> Self {
        let semaphore = config
            .max_concurrency
            .map(|n| Arc::new(Semaphore::new(n.max(1))));
        Self {
            tools: HashMap::new(),
            semaphore,
            config,
        }
    }

    /// 返回并发度限制（`None` = 不限制）
    pub fn max_concurrency(&self) -> Option<usize> {
        self.config.max_concurrency
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn register_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.tools.insert(tool.name().to_string(), tool);
        }
    }

    pub fn list_tools(&self) -> Vec<&str> {
        self.tools.keys().map(|name| name.as_str()).collect()
    }

    pub fn get_tool(&self, tool_name: &str) -> Option<&dyn Tool> {
        self.tools.get(tool_name).map(|tool| &**tool)
    }

    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition::from_tool(&**tool))
            .collect()
    }

    pub async fn execute_tool(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
    ) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;

        let _permit = if let Some(sem) = &self.semaphore {
            Some(sem.acquire().await.unwrap())
        } else {
            None
        };

        let max_retries = if self.config.retry_on_fail {
            self.config.max_retries
        } else {
            0
        };

        let mut last_err: Option<crate::error::ReactError> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                let delay_ms = self.config.retry_delay_ms * (1u64 << (attempt as u64 - 1).min(5));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let result = if self.config.timeout_ms > 0 {
                match tokio::time::timeout(
                    Duration::from_millis(self.config.timeout_ms),
                    tool.execute(parameters.clone()),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => Err(ToolError::Timeout(tool_name.to_string()).into()),
                }
            } else {
                tool.execute(parameters.clone()).await
            };

            match result {
                Ok(r) => return Ok(r),
                Err(e) if attempt < max_retries => {
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_err.unwrap_or_else(|| ToolError::NotFound(tool_name.to_string()).into()))
    }
}
