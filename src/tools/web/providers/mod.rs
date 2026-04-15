//! 搜索引擎 Provider 抽象
//!
//! 定义 [`SearchProvider`] trait 和 [`SearchResult`] 类型，
//! 支持多种搜索引擎后端（DuckDuckGo、Brave、Tavily 等）。

pub mod brave;
pub mod duckduckgo;
pub mod tavily;
pub mod utils;

use crate::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 搜索结果条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 标题
    pub title: String,
    /// 链接
    pub url: String,
    /// 摘要
    pub snippet: String,
}

/// 搜索引擎 Provider trait
///
/// 所有搜索引擎后端必须实现此 trait。
/// 框架内置 [`duckduckgo::DuckDuckGoProvider`] 作为免费兜底方案。
///
/// # 扩展
///
/// 实现 `BraveSearchProvider` 或 `TavilyProvider` 只需实现此 trait，
/// 然后通过 `WebSearchTool::new(provider)` 传入即可。
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Provider 名称
    fn name(&self) -> &str;

    /// 执行搜索查询
    ///
    /// - `query`: 搜索关键词
    /// - `max_results`: 最大返回结果数
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>>;
}
