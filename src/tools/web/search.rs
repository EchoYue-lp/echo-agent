//! Web 搜索工具
//!
//! 提供 [`WebSearchTool`]，通过 [`SearchProvider`] 抽象支持多种搜索引擎。
//! 默认使用 [`DuckDuckGoProvider`]（免费，无需 API Key）。
//!
//! # 用法
//!
//! ```rust,no_run
//! use echo_agent::tools::web::WebSearchTool;
//!
//! // 使用 DuckDuckGo（免费兜底）
//! let tool = WebSearchTool::with_duckduckgo();
//! ```

use super::providers::SearchProvider;
use super::providers::brave::BraveSearchProvider;
use super::providers::duckduckgo::DuckDuckGoProvider;
use super::providers::tavily::TavilyProvider;
use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_ALLOWED_RESULTS: usize = 10;

/// Web 搜索工具
///
/// 支持通过不同搜索引擎 Provider 进行 Web 搜索。
/// 内置 DuckDuckGo 作为免费兜底方案。
pub struct WebSearchTool {
    provider: Box<dyn SearchProvider>,
    default_max_results: usize,
}

impl WebSearchTool {
    /// 使用自定义 Provider 创建
    pub fn new(provider: Box<dyn SearchProvider>) -> Self {
        Self {
            provider,
            default_max_results: DEFAULT_MAX_RESULTS,
        }
    }

    /// 使用 DuckDuckGo（免费兜底）创建
    pub fn with_duckduckgo() -> Self {
        Self::new(Box::new(DuckDuckGoProvider::new()))
    }

    /// 使用 Brave Search（需 API Key）创建
    pub fn with_brave(api_key: impl Into<String>) -> Self {
        Self::new(Box::new(BraveSearchProvider::new(api_key)))
    }

    /// 使用 Tavily（需 API Key，AI 优化搜索）创建
    pub fn with_tavily(api_key: impl Into<String>) -> Self {
        Self::new(Box::new(TavilyProvider::new(api_key)))
    }

    /// 自动选择最佳可用 Provider 创建
    ///
    /// 优先级：Tavily > Brave > DuckDuckGo
    ///
    /// 从环境变量读取 API Key：
    /// - `TAVILY_API_KEY` → Tavily（AI 优化，最高质量）
    /// - `BRAVE_SEARCH_API_KEY` → Brave Search（高质量）
    /// - 无 Key → DuckDuckGo（免费兜底）
    pub fn auto() -> Self {
        if let Some(provider) = TavilyProvider::from_env() {
            tracing::info!("WebSearch: 自动选择 Tavily Provider");
            return Self::new(Box::new(provider));
        }
        if let Some(provider) = BraveSearchProvider::from_env() {
            tracing::info!("WebSearch: 自动选择 Brave Provider");
            return Self::new(Box::new(provider));
        }
        tracing::info!("WebSearch: 无 API Key，使用 DuckDuckGo Provider");
        Self::with_duckduckgo()
    }

    /// 设置默认最大结果数
    pub fn with_max_results(mut self, n: usize) -> Self {
        self.default_max_results = n.clamp(1, MAX_ALLOWED_RESULTS);
        self
    }

    /// 获取当前 Provider 名称
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "在互联网上搜索信息。返回搜索结果的标题、链接和摘要。\
         参数：query - 搜索关键词（必填），max_results - 最大返回结果数（可选，默认5，最大10）"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词"
                },
                "max_results": {
                    "type": "integer",
                    "description": format!("最大返回结果数（默认{}，最大{}）", DEFAULT_MAX_RESULTS, MAX_ALLOWED_RESULTS)
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let query = parameters
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("query".to_string()))?;

            if query.trim().is_empty() {
                return Ok(ToolResult::error("搜索关键词不能为空"));
            }

            let max_results = parameters
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.default_max_results as u64) as usize;

            let max_results = max_results.clamp(1, MAX_ALLOWED_RESULTS);

            tracing::info!(
                "WebSearch: query='{}', max_results={}, provider={}",
                query,
                max_results,
                self.provider.name()
            );

            match self.provider.search(query, max_results).await {
                Ok(results) => Ok(ToolResult::success_json(
                    serde_json::to_value(&results).unwrap_or_default(),
                )),
                Err(e) => Ok(ToolResult::error(format!(
                    "搜索失败 (provider: {}): {}",
                    self.provider.name(),
                    e
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::tools::web::providers::SearchResult;

    #[test]
    fn test_empty_results_json() {
        let results: Vec<SearchResult> = vec![];
        let json = serde_json::to_value(&results).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[test]
    fn test_results_json_structure() {
        let results = vec![
            SearchResult {
                title: "Rust".into(),
                url: "https://rust-lang.org".into(),
                snippet: "A programming language".into(),
            },
            SearchResult {
                title: "Cargo".into(),
                url: "https://doc.rust-lang.org/cargo".into(),
                snippet: String::new(),
            },
        ];

        let json = serde_json::to_value(&results).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["title"], "Rust");
        assert_eq!(arr[0]["url"], "https://rust-lang.org");
        assert_eq!(arr[0]["snippet"], "A programming language");
        assert_eq!(arr[1]["title"], "Cargo");
        assert_eq!(arr[1]["snippet"], "");
    }
}
