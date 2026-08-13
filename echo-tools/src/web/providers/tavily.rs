//! Tavily Search API Provider
//!
//! AI-optimized search via the Tavily AI Search API, requires an API Key.
//!
//! # Features
//!
//! - AI-optimized search results with higher quality summaries
//! - Supports automatic web content extraction
//! - Designed specifically for AI Agents
//!
//! # Getting an API Key
//!
//! Visit <https://tavily.com/> to register and get an API Key.

use super::utils::truncate_chars;
use super::{SearchProvider, SearchResult};
use async_trait::async_trait;
use echo_core::error::{Result, ToolError};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Tavily Search API Provider
///
/// Searches via the Tavily AI Search API, requires an API Key.
pub struct TavilyProvider {
    client: Client,
    api_key: String,
}

impl TavilyProvider {
    /// Create a new Tavily Provider
    ///
    /// - `api_key`: Tavily API Key
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            api_key: api_key.into(),
        }
    }

    /// Create from environment variable
    ///
    /// Reads the `TAVILY_API_KEY` environment variable.
    pub fn from_env() -> Option<Self> {
        std::env::var("TAVILY_API_KEY").ok().map(Self::new)
    }
}

/// Tavily search request body
#[derive(Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    max_results: usize,
    #[serde(rename = "include_answer")]
    include_answer: bool,
}

impl std::fmt::Debug for TavilyRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TavilyRequest")
            .field("api_key", &"[REDACTED]")
            .field("query", &self.query)
            .field("max_results", &self.max_results)
            .field("include_answer", &self.include_answer)
            .finish()
    }
}

/// Tavily search response
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
    #[allow(dead_code)]
    answer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let request = TavilyRequest {
            api_key: self.api_key.clone(),
            query: query.to_string(),
            max_results,
            include_answer: false,
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("Tavily request failed: {}", e),
            })?;

        let status = response.status();
        if status.as_u16() == 401 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Tavily API Key is invalid or expired".into(),
            }
            .into());
        }
        if status.as_u16() == 429 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Tavily API rate limit exceeded".into(),
            }
            .into());
        }
        if !status.is_success() {
            let body = crate::http_body::read_bounded_text(
                response,
                crate::http_body::MAX_API_RESPONSE_BYTES,
                "web_search",
                None,
            )
            .await?;
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!(
                    "Tavily returned error ({}): {}",
                    status,
                    truncate_chars(&body, 200)
                ),
            }
            .into());
        }

        let tavily_resp: TavilyResponse = crate::http_body::read_bounded_json(
            response,
            crate::http_body::MAX_API_RESPONSE_BYTES,
            "web_search",
            None,
        )
        .await?;

        Ok(tavily_resp
            .results
            .into_iter()
            .take(max_results)
            .filter_map(|r| {
                let title = r.title?.trim().to_string();
                let url = r.url?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                Some(SearchResult {
                    title,
                    url,
                    snippet: r.content.unwrap_or_default().trim().to_string(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tavily_request_serialization() {
        let req = TavilyRequest {
            api_key: "test-key".into(),
            query: "rust programming".into(),
            max_results: 5,
            include_answer: false,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["api_key"], "test-key");
        assert_eq!(json["query"], "rust programming");
        assert_eq!(json["max_results"], 5);
        assert_eq!(json["include_answer"], false);
    }

    #[test]
    fn test_parse_tavily_response() {
        let json = r#"{
            "results": [
                {
                    "title": "Rust Programming Language",
                    "url": "https://www.rust-lang.org/",
                    "content": "A language empowering everyone to build reliable and efficient software."
                },
                {
                    "title": "Learn Rust",
                    "url": "https://doc.rust-lang.org/book/",
                    "content": "The Rust Programming Language book."
                }
            ],
            "answer": "Rust is a systems programming language."
        }"#;

        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.results.len(), 2);
        assert!(resp.answer.is_some());

        let results: Vec<SearchResult> = resp
            .results
            .into_iter()
            .filter_map(|r| {
                let title = r.title?.trim().to_string();
                let url = r.url?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                Some(SearchResult {
                    title,
                    url,
                    snippet: r.content.unwrap_or_default().trim().to_string(),
                })
            })
            .collect();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(
            results[0].snippet,
            "A language empowering everyone to build reliable and efficient software."
        );
    }

    #[test]
    fn test_parse_tavily_empty_response() {
        let json = r#"{"results": [], "answer": null}"#;
        let resp: TavilyResponse = serde_json::from_str(json).unwrap();
        assert!(resp.results.is_empty());
        assert!(resp.answer.is_none());
    }
}
