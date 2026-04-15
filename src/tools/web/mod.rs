//! Web 工具模块
//!
//! 提供 Web 搜索和页面获取能力：
//!
//! - [`WebSearchTool`][]: 在互联网上搜索信息
//! - [`WebFetchTool`][]: 获取网页内容并转换为可读文本
//!
//! # 快速开始
//!
//! ```rust,no_run
//! use echo_agent::tools::web::{WebSearchTool, WebFetchTool};
//!
//! let search_tool = WebSearchTool::with_duckduckgo();
//! let fetch_tool = WebFetchTool::new();
//! ```

pub mod fetch;
pub mod providers;
pub mod search;

pub use fetch::WebFetchTool;
pub use search::WebSearchTool;
