//! Brave Search API Provider
//!
//! High-quality search via the Brave Search API, requires an API Key.
//!
//! # Getting an API Key
//!
//! Visit <https://brave.com/search/api/> to register and get a free API Key (2000 free queries/month).

use super::utils::{truncate_chars, urlencode};
use super::{SearchProvider, SearchResult};
use async_trait::async_trait;
use echo_core::error::{Result, ToolError};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// Brave Search API Provider
///
/// Searches via the Brave Search API, requires an API Key.
pub struct BraveSearchProvider {
    client: Client,
    api_key: String,
}

impl BraveSearchProvider {
    /// Create a new Brave Search Provider
    ///
    /// - `api_key`: Brave Search API Key
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
    /// Reads the `BRAVE_SEARCH_API_KEY` environment variable.
    pub fn from_env() -> Option<Self> {
        std::env::var("BRAVE_SEARCH_API_KEY").ok().map(Self::new)
    }
}

/// Brave Search API response structure
#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Option<Vec<BraveResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    fn name(&self) -> &str {
        "brave"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencode(query),
            max_results
        );

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("Brave Search request failed: {}", e),
            })?;

        let status = response.status();
        if status.as_u16() == 401 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Brave Search API Key is invalid or expired".into(),
            }
            .into());
        }
        if status.as_u16() == 429 {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: "Brave Search API rate limit exceeded".into(),
            }
            .into());
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!(
                    "Brave Search returned error ({}): {}",
                    status,
                    truncate_chars(&body, 200)
                ),
            }
            .into());
        }

        let brave_resp: BraveResponse =
            response
                .json()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_search".into(),
                    message: format!("Brave Search response parsing failed: {}", e),
                })?;

        let results = brave_resp.web.and_then(|w| w.results).unwrap_or_default();

        Ok(results
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
                    snippet: r.description.unwrap_or_default().trim().to_string(),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("rust-lang"), "rust-lang");
        assert_eq!(urlencode("café"), "caf%C3%A9");
    }

    #[test]
    fn test_parse_brave_response() {
        let json = r#"{
            "web": {
                "results": [
                    {
                        "title": "Rust Programming Language",
                        "url": "https://www.rust-lang.org/",
                        "description": "A language empowering everyone"
                    },
                    {
                        "title": "Rust Documentation",
                        "url": "https://doc.rust-lang.org/",
                        "description": null
                    }
                ]
            }
        }"#;

        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results: Vec<SearchResult> = resp
            .web
            .and_then(|w| w.results)
            .unwrap_or_default()
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
                    snippet: r.description.unwrap_or_default().trim().to_string(),
                })
            })
            .collect();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone");
        assert!(results[1].snippet.is_empty());
    }

    #[test]
    fn test_parse_brave_empty_response() {
        let json = r#"{"web": {"results": []}}"#;
        let resp: BraveResponse = serde_json::from_str(json).unwrap();
        let results = resp.web.and_then(|w| w.results).unwrap_or_default();
        assert!(results.is_empty());
    }
}
