//! Search engine Provider abstraction
//!
//! Defines the [`SearchProvider`] trait and [`SearchResult`] type,
//! supporting multiple search engine backends (DuckDuckGo, Brave, Tavily, etc.).

pub mod brave;
pub mod duckduckgo;
pub mod tavily;
pub mod utils;

use crate::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Search result entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Title
    pub title: String,
    /// URL
    pub url: String,
    /// Snippet / summary
    pub snippet: String,
}

/// Search engine Provider trait
///
/// All search engine backends must implement this trait.
/// The framework provides [`duckduckgo::DuckDuckGoProvider`] as a free fallback.
///
/// # Extension
///
/// To implement `BraveSearchProvider` or `TavilyProvider`, simply implement this trait
/// and pass it via `WebSearchTool::new(provider)`.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Provider name
    fn name(&self) -> &str;

    /// Execute a search query
    ///
    /// - `query`: search keywords
    /// - `max_results`: maximum number of results to return
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>>;
}
