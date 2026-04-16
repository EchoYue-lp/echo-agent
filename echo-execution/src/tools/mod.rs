//! 工具系统核心框架
//!
//! 提供 [`ToolManager`] 和工具 trait 的 re-export。

use echo_core::error::{Result, ToolError};
use echo_core::llm::types::ToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub use echo_core::tools::{Tool, ToolExecutionConfig, ToolParameters, ToolResult, TypedTool};

/// 工具管理器
///
/// 负责工具的注册、执行、并发控制和超时重试。
pub struct ToolManager {
    tools: HashMap<String, Box<dyn Tool>>,
    config: ToolExecutionConfig,
    /// 并发限流器
    semaphore: Option<Arc<Semaphore>>,
    /// 缓存的工具定义
    cached_definitions: Option<Vec<ToolDefinition>>,
}

impl ToolManager {
    /// 获取 OpenAI 格式的工具定义列表（带缓存）
    ///
    /// 首次调用时构建并缓存，后续直接返回缓存值。
    /// 注册新工具后缓存会自动失效。
    pub fn get_openai_tools(&mut self) -> Vec<ToolDefinition> {
        if let Some(ref cached) = self.cached_definitions {
            return cached.clone();
        }
        let definitions: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|tool| ToolDefinition::from_tool(&**tool))
            .collect();
        self.cached_definitions = Some(definitions.clone());
        definitions
    }

    /// 使缓存失效（注册/注销工具时调用）
    fn invalidate_cache(&mut self) {
        self.cached_definitions = None;
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
            cached_definitions: None,
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
            cached_definitions: None,
        }
    }

    /// 返回并发度限制（`None` = 不限制）
    pub fn max_concurrency(&self) -> Option<usize> {
        self.config.max_concurrency
    }

    /// 注册单个工具
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
        self.invalidate_cache();
    }

    /// 批量注册工具
    pub fn register_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.tools.insert(tool.name().to_string(), tool);
        }
        self.invalidate_cache();
    }

    /// 注销工具
    pub fn unregister(&mut self, tool_name: &str) -> Option<Box<dyn Tool>> {
        let tool = self.tools.remove(tool_name);
        if tool.is_some() {
            self.invalidate_cache();
        }
        tool
    }

    /// 列出所有已注册的工具名称
    pub fn list_tools(&self) -> Vec<&str> {
        self.tools.keys().map(|name| name.as_str()).collect()
    }

    /// 获取工具引用
    pub fn get_tool(&self, tool_name: &str) -> Option<&dyn Tool> {
        self.tools.get(tool_name).map(|tool| &**tool)
    }

    /// 获取工具定义列表（用于展示或调试）
    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition::from_tool(&**tool))
            .collect()
    }

    /// 执行工具
    ///
    /// 支持并发控制、超时和重试。
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        parameters: ToolParameters,
    ) -> Result<ToolResult> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;

        // 并发控制：获取信号量许可
        let _permit = if let Some(sem) = &self.semaphore {
            match sem.acquire().await {
                Ok(permit) => Some(permit),
                Err(e) => {
                    tracing::warn!("Failed to acquire semaphore permit: {}", e);
                    return Err(ToolError::ExecutionFailed {
                        tool: tool_name.to_string(),
                        message: format!("Concurrency limit error: {}", e),
                    }
                    .into());
                }
            }
        } else {
            None
        };

        let max_retries = if self.config.retry_on_fail {
            self.config.max_retries
        } else {
            0
        };

        let mut last_err: Option<echo_core::error::ReactError> = None;

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

    /// 验证工具参数
    pub fn validate_tool_parameters(
        &self,
        tool_name: &str,
        parameters: &ToolParameters,
    ) -> Result<()> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        futures::executor::block_on(tool.validate_parameters(parameters))
    }
}
