//! 工具系统
//!
//! 定义 [`Tool`] trait 和 [`ToolManager`]，支持工具注册、执行、并发限流、超时重试。
//!
//! # 核心类型
//!
//! - [`Tool`]: 工具接口 trait，所有工具必须实现
//! - [`ToolManager`]: 工具管理器，负责注册和执行
//! - [`ToolResult`]: 工具执行结果
//! - [`ToolExecutionConfig`]: 执行配置（超时、重试、并发）
//!
//! # 快速开始
//!
//! ```rust
//! use echo_agent::tools::ToolManager;
//!
//! // 创建工具管理器
//! let manager = ToolManager::new();
//!
//! // 列出已注册工具
//! println!("已注册工具: {:?}", manager.list_tools());
//! ```
//!
//! # 自定义工具
//!
//! ```rust
//! use echo_agent::prelude::*;
//! use async_trait::async_trait;
//!
//! /// 简单的计算器工具
//! struct Calculator;
//!
//! #[async_trait]
//! impl Tool for Calculator {
//!     fn name(&self) -> &str {
//!         "calculator"
//!     }
//!
//!     fn description(&self) -> &str {
//!         "执行简单的数学计算"
//!     }
//!
//!     fn parameters(&self) -> serde_json::Value {
//!         serde_json::json!({
//!             "type": "object",
//!             "properties": {
//!                 "expression": {
//!                     "type": "string",
//!                     "description": "数学表达式，如 '1+2*3'"
//!                 }
//!             },
//!             "required": ["expression"]
//!         })
//!     }
//!
//!     async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
//!         let expr = params.get("expression")
//!             .and_then(|v| v.as_str())
//!             .unwrap_or("");
//!
//!         // 简化示例：只处理加法
//!         let result = if expr.contains('+') {
//!             let parts: Vec<&str> = expr.split('+').collect();
//!             if parts.len() == 2 {
//!                 let a: i64 = parts[0].trim().parse().unwrap_or(0);
//!                 let b: i64 = parts[1].trim().parse().unwrap_or(0);
//!                 Some(a + b)
//!             } else {
//!                 None
//!             }
//!         } else {
//!             None
//!         };
//!
//!         match result {
//!             Some(n) => Ok(ToolResult::success(format!("计算结果: {}", n))),
//!             None => Ok(ToolResult::error("不支持的表达式".into())),
//!         }
//!     }
//! }
//!
//! # fn main() {
//! let tool = Calculator;
//! assert_eq!(tool.name(), "calculator");
//! # }
//! ```

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
///
/// # 示例
///
/// ```
/// use echo_agent::tools::ToolResult;
///
/// let success = ToolResult::success("执行成功".to_string());
/// assert!(success.success);
/// assert_eq!(success.output, "执行成功");
///
/// let error = ToolResult::error("执行失败".to_string());
/// assert!(!error.success);
/// assert_eq!(error.error, Some("执行失败".to_string()));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// 是否执行成功
    pub success: bool,
    /// 输出内容
    pub output: String,
    /// 错误信息（失败时）
    pub error: Option<String>,
}

/// 工具执行配置：超时、重试、并发度
///
/// # 示例
///
/// ```
/// use echo_agent::tools::ToolExecutionConfig;
///
/// let config = ToolExecutionConfig {
///     timeout_ms: 60_000,      // 60秒超时
///     retry_on_fail: true,     // 失败重试
///     max_retries: 3,          // 最多重试3次
///     retry_delay_ms: 500,     // 首次等待500ms
///     max_concurrency: Some(4), // 最多4个并发
/// };
/// ```
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
    /// 创建成功结果
    pub fn success(output: String) -> Self {
        Self {
            success: true,
            output,
            error: None,
        }
    }

    /// 创建失败结果
    pub fn error(error: String) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error),
        }
    }
}

/// 工具参数类型
pub type ToolParameters = HashMap<String, serde_json::Value>;

/// 工具接口
///
/// 所有工具都必须实现此 trait。工具可以是：
/// - 内置工具（shell、文件操作等）
/// - MCP 远程工具
/// - 用户自定义工具
///
/// # 实现示例
///
/// ```rust
/// use echo_agent::prelude::*;
/// use async_trait::async_trait;
///
/// struct EchoTool;
///
/// #[async_trait]
/// impl Tool for EchoTool {
///     fn name(&self) -> &str { "echo" }
///     fn description(&self) -> &str { "返回输入内容" }
///     fn parameters(&self) -> serde_json::Value {
///         serde_json::json!({
///             "type": "object",
///             "properties": {
///                 "message": { "type": "string" }
///             }
///         })
///     }
///     async fn execute(&self, params: ToolParameters) -> Result<ToolResult> {
///         let msg = params.get("message")
///             .and_then(|v| v.as_str())
///             .unwrap_or("");
///         Ok(ToolResult::success(msg.to_string()))
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称（唯一标识）
    fn name(&self) -> &str;

    /// 工具描述（LLM 用于决策是否调用）
    fn description(&self) -> &str;

    /// 工具参数的 JSON Schema 定义
    fn parameters(&self) -> serde_json::Value;

    /// 执行工具
    async fn execute(&self, parameters: ToolParameters) -> Result<ToolResult>;

    /// 验证参数（可选实现，默认不验证）
    fn validate_parameters(&self, _params: &ToolParameters) -> Result<()> {
        Ok(())
    }
}

/// 工具管理器
///
/// 负责工具的注册、执行、并发控制和超时重试。
///
/// # 示例
///
/// ```rust
/// use echo_agent::tools::ToolManager;
///
/// let manager = ToolManager::new();
///
/// // 列出工具
/// let tools = manager.list_tools();
/// assert!(tools.is_empty());
/// ```
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
    pub(crate) fn get_openai_tools(&mut self) -> Vec<ToolDefinition> {
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

    /// 验证工具参数
    pub fn validate_tool_parameters(
        &self,
        tool_name: &str,
        parameters: &ToolParameters,
    ) -> Result<()> {
        let tool = self
            .get_tool(tool_name)
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;
        tool.validate_parameters(parameters)
    }
}

// ── 单元测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockTool;

    #[test]
    fn test_tool_manager_new() {
        let manager = ToolManager::new();
        assert!(manager.list_tools().is_empty());
        assert!(manager.max_concurrency().is_none());
    }

    #[test]
    fn test_tool_manager_with_config() {
        let config = ToolExecutionConfig {
            timeout_ms: 5000,
            retry_on_fail: true,
            max_retries: 3,
            retry_delay_ms: 100,
            max_concurrency: Some(4),
        };
        let manager = ToolManager::new_with_config(config);
        assert_eq!(manager.max_concurrency(), Some(4));
    }

    #[test]
    fn test_register_single_tool() {
        let mut manager = ToolManager::new();
        let tool = Box::new(MockTool::new("test_tool"));

        manager.register(tool);
        let tools = manager.list_tools();
        assert_eq!(tools.len(), 1);
        assert!(tools.contains(&"test_tool"));
    }

    #[test]
    fn test_register_multiple_tools() {
        let mut manager = ToolManager::new();
        let tools = vec![
            Box::new(MockTool::new("tool1")) as Box<dyn Tool>,
            Box::new(MockTool::new("tool2")) as Box<dyn Tool>,
            Box::new(MockTool::new("tool3")) as Box<dyn Tool>,
        ];

        manager.register_tools(tools);
        assert_eq!(manager.list_tools().len(), 3);
    }

    #[test]
    fn test_unregister_tool() {
        let mut manager = ToolManager::new();
        manager.register(Box::new(MockTool::new("test_tool")));

        let removed = manager.unregister("test_tool");
        assert!(removed.is_some());
        assert!(manager.list_tools().is_empty());
    }

    #[test]
    fn test_unregister_nonexistent_tool() {
        let mut manager = ToolManager::new();
        let removed = manager.unregister("nonexistent");
        assert!(removed.is_none());
    }

    #[test]
    fn test_get_tool() {
        let mut manager = ToolManager::new();
        manager.register(Box::new(MockTool::new("test_tool")));

        let tool = manager.get_tool("test_tool");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "test_tool");

        let missing = manager.get_tool("missing");
        assert!(missing.is_none());
    }

    #[test]
    fn test_get_tool_definitions() {
        let mut manager = ToolManager::new();
        manager.register(Box::new(MockTool::new("tool1")));
        manager.register(Box::new(MockTool::new("tool2")));

        let definitions = manager.get_tool_definitions();
        assert_eq!(definitions.len(), 2);
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("output".to_string());
        assert!(result.success);
        assert_eq!(result.output, "output");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("something went wrong".to_string());
        assert!(!result.success);
        assert!(result.output.is_empty());
        assert_eq!(result.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_tool_execution_config_default() {
        let config = ToolExecutionConfig::default();
        assert_eq!(config.timeout_ms, 30_000);
        assert!(!config.retry_on_fail);
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.retry_delay_ms, 200);
        assert!(config.max_concurrency.is_none());
    }

    #[tokio::test]
    async fn test_execute_tool_not_found() {
        let manager = ToolManager::new();
        let result = manager.execute_tool("nonexistent", HashMap::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_tool_success() {
        let mut manager = ToolManager::new();
        manager.register(Box::new(MockTool::new("test")));

        let result = manager.execute_tool("test", HashMap::new()).await;
        assert!(result.is_ok());
        let tool_result = result.unwrap();
        assert!(tool_result.success);
    }
}
