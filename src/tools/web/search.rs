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

use super::providers::brave::BraveSearchProvider;
use super::providers::duckduckgo::DuckDuckGoProvider;
use super::providers::tavily::TavilyProvider;
use super::providers::{SearchProvider, SearchResult};
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

    /// 格式化搜索结果
    fn format_results(query: &str, results: &[SearchResult]) -> String {
        if results.is_empty() {
            return format!("未找到与 '{}' 相关的搜索结果。", query);
        }

        let mut output = format!("搜索结果（共 {} 条）:\n\n", results.len());
        for (i, r) in results.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", i + 1, r.title));
            output.push_str(&format!("   {}\n", r.url));
            if !r.snippet.is_empty() {
                output.push_str(&format!("   {}\n", r.snippet));
            }
            output.push('\n');
        }
        output
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
                return Ok(ToolResult::error("搜索关键词不能为空".into()));
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
                Ok(results) => {
                    let output = Self::format_results(query, &results);
                    Ok(ToolResult::success(output))
                }
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
    use super::*;

    #[test]
    fn test_format_results_empty() {
        let output = WebSearchTool::format_results("test", &[]);
        assert!(output.contains("未找到"));
    }

    #[test]
    fn test_format_results_with_data() {
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

        let output = WebSearchTool::format_results("rust", &results);
        assert!(output.contains("共 2 条"));
        assert!(output.contains("1. Rust"));
        assert!(output.contains("https://rust-lang.org"));
        assert!(output.contains("A programming language"));
        assert!(output.contains("2. Cargo"));
    }
}
