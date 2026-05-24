//! Web search tool
//!
//! Provides [`WebSearchTool`] with support for multiple search engines via [`SearchProvider`].
//! Defaults to [`DuckDuckGoProvider`] (free, no API key required).
//!
//! # Usage
//!
//! ```rust,ignore

use super::providers::SearchProvider;
use super::providers::brave::BraveSearchProvider;
use super::providers::duckduckgo::DuckDuckGoProvider;
use super::providers::tavily::TavilyProvider;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult, ToolRiskLevel};
use futures::future::BoxFuture;
use serde_json::Value;

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_ALLOWED_RESULTS: usize = 10;

/// Web search tool
///
/// Supports web search through different search engine Providers.
/// Built-in DuckDuckGo as the free fallback option.
pub struct WebSearchTool {
    provider: Box<dyn SearchProvider>,
    default_max_results: usize,
}

impl WebSearchTool {
    /// Create with a custom Provider
    pub fn new(provider: Box<dyn SearchProvider>) -> Self {
        Self {
            provider,
            default_max_results: DEFAULT_MAX_RESULTS,
        }
    }

    /// Create with DuckDuckGo (free fallback)
    pub fn with_duckduckgo() -> Self {
        Self::new(Box::new(DuckDuckGoProvider::new()))
    }

    /// Create with Brave Search (requires API key)
    pub fn with_brave(api_key: impl Into<String>) -> Self {
        Self::new(Box::new(BraveSearchProvider::new(api_key)))
    }

    /// Create with Tavily (requires API key, AI-optimized search)
    pub fn with_tavily(api_key: impl Into<String>) -> Self {
        Self::new(Box::new(TavilyProvider::new(api_key)))
    }

    /// Auto-select the best available Provider
    ///
    /// Priority: Tavily > Brave > DuckDuckGo
    ///
    /// Reads API keys from environment variables:
    /// - `TAVILY_API_KEY` → Tavily (AI-optimized, highest quality)
    /// - `BRAVE_SEARCH_API_KEY` → Brave Search (high quality)
    /// - No key → DuckDuckGo (free fallback)
    pub fn auto() -> Self {
        if let Some(provider) = TavilyProvider::from_env() {
            tracing::info!("WebSearch: auto-selected Tavily Provider");
            return Self::new(Box::new(provider));
        }
        if let Some(provider) = BraveSearchProvider::from_env() {
            tracing::info!("WebSearch: auto-selected Brave Provider");
            return Self::new(Box::new(provider));
        }
        tracing::info!("WebSearch: no API key, falling back to DuckDuckGo Provider");
        Self::with_duckduckgo()
    }

    /// Set the default maximum number of results
    pub fn with_max_results(mut self, n: usize) -> Self {
        self.default_max_results = n.clamp(1, MAX_ALLOWED_RESULTS);
        self
    }

    /// Get the current Provider name
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Search for information on the internet. Returns search result titles, links, and snippets. \
         Parameters: query - search keywords (required), max_results - max results returned (optional, default 5, max 10)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords"
                },
                "max_results": {
                    "type": "integer",
                    "description": format!("Maximum number of results (default {}, max {})", DEFAULT_MAX_RESULTS, MAX_ALLOWED_RESULTS)
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
                return Ok(ToolResult::error("Search query cannot be empty"));
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
                    "Search failed (provider: {}): {}",
                    self.provider.name(),
                    e
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::web::providers::SearchResult;

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
