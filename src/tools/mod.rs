//! Tools façade
//!
//! 此模块由两部分组成：
//! - `echo_execution::tools` 提供共享的 `ToolManager` 与核心工具抽象
//! - 根 crate 下的 `builtin` / `files` / `shell` / `web` / `media` 提供产品层工具
//!
//! 如需直接依赖拆分后的 crate，可使用 [`crate::workspace::execution::tools`]。
//!
//! # 核心类型
//!
//! - [`Tool`]: 工具接口 trait，所有工具必须实现
//! - [`ToolManager`][]: 工具管理器，负责注册和执行
//! - [`ToolResult`][]: 工具执行结果
//! - [`ToolExecutionConfig`][]: 执行配置（超时、重试、并发）
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
//! use futures::future::BoxFuture;
//!
//! /// 简单的计算器工具
//! struct Calculator;
//!
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
//!     fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
//!         Box::pin(async move {
//!             let expr = params.get("expression")
//!                 .and_then(|v| v.as_str())
//!                 .unwrap_or("");
//!
//!             // 简化示例：只处理加法
//!             let result = if expr.contains('+') {
//!                 let parts: Vec<&str> = expr.split('+').collect();
//!                 if parts.len() == 2 {
//!                     let a: i64 = parts[0].trim().parse().unwrap_or(0);
//!                     let b: i64 = parts[1].trim().parse().unwrap_or(0);
//!                     Some(a + b)
//!                 } else {
//!                     None
//!                 }
//!             } else {
//!                 None
//!             };
//!
//!             match result {
//!                 Some(n) => Ok(ToolResult::success(format!("计算结果: {}", n))),
//!                 None => Ok(ToolResult::error("不支持的表达式")),
//!             }
//!         })
//!     }
//! }
//!
//! # fn main() {
//! let tool = Calculator;
//! assert_eq!(tool.name(), "calculator");
//! # }
//! ```

/// Built-in tools (security, think, etc.)
pub mod builtin;
/// File manipulation tools
pub mod files;
pub mod permission;
pub mod shell;

#[cfg(feature = "web")]
pub mod web;

#[cfg(feature = "media")]
pub mod media;

/// Direct re-exports from `echo_execution::tools`.
pub mod execution {
    pub use echo_execution::tools::*;
}

pub use echo_execution::tools::{
    Tool, ToolExecutionConfig, ToolManager, ToolParameters, ToolResult, TypedTool,
};
