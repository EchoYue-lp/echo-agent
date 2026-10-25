//! Brave Search API Provider
//!
//! High-quality search via the Brave Search API, requires an API Key.
//!
//! # Getting an API Key
//!
//! Visit <https://brave.com/search/api/> to register and get a free API Key (2000 free queries/month).

use super::utils::urlencode;
use super::{SearchProvider, SearchResult};
use async_trait::async_trait;
use echo_core::error::Result;
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
        self.search_url(&url, max_results).await
    }
}

impl BraveSearchProvider {
    async fn search_url(&self, url: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get(url)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|error| crate::http_body::request_error("web_search", error, None))?;

        if !response.status().is_success() {
            return Err(crate::http_body::response_error("web_search", &response));
        }

        let brave_resp: BraveResponse = crate::http_body::read_bounded_json(
            response,
            crate::http_body::MAX_API_RESPONSE_BYTES,
            "web_search",
            None,
        )
        .await?;

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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct LocalBraveProvider {
        provider: BraveSearchProvider,
        endpoint: String,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SearchProvider for LocalBraveProvider {
        fn name(&self) -> &str {
            "local-brave"
        }

        async fn search(&self, _query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.provider.search_url(&self.endpoint, max_results).await
        }
    }

    #[tokio::test]
    async fn actual_http_manager_stops_auth_and_waits_for_rate_limit() -> Result<()> {
        use crate::http_body::test_http::{client, response, server};
        use echo_core::tools::{Tool, ToolFailureCategory, ToolParameters};
        use echo_execution::tools::{ToolExecutionConfig, ToolManager};
        for (status, hint) in [(401, None), (429, Some(2)), (429, Some(31))] {
            let success = r#"{"web":{"results":[{"title":"Rust","url":"https://example.test/","description":"结果🙂"}]}}"#;
            let headers = hint
                .map(|seconds| format!("Retry-After: {seconds}\r\n"))
                .unwrap_or_default();
            let mut responses = vec![response(status, &headers, "secret")];
            if hint == Some(2) {
                responses.push(response(200, "", success));
            }
            let (endpoint, serving) = server(responses)?;
            let calls = Arc::new(AtomicUsize::new(0));
            let provider = LocalBraveProvider {
                provider: BraveSearchProvider {
                    client: client()?,
                    api_key: "fixture-key".to_string(),
                },
                endpoint,
                calls: Arc::clone(&calls),
            };
            let tool = crate::web::WebSearchTool::new(Box::new(provider));
            let manager = ToolManager::new_with_config(ToolExecutionConfig {
                timeout_ms: 0,
                retry_on_fail: true,
                max_retries: 2,
                retry_delay_ms: 0,
                max_concurrency: None,
                max_read_concurrency: None,
            });
            let mut params = ToolParameters::new();
            params.insert("query".to_string(), serde_json::json!("test"));
            let tool_name = tool.name().to_string();
            manager.register(Box::new(tool));
            let result = manager.execute_tool(&tool_name, params).await?;
            let requests = serving.join().map_err(|_| {
                echo_core::error::ReactError::Other("HTTP fixture panicked".to_string())
            })??;
            eprintln!(
                "HTTP manager fixture status={status}, hint={hint:?}, logical_attempts={}, http_requests={}",
                calls.load(Ordering::SeqCst),
                requests.len()
            );
            if hint != Some(2) {
                assert!(!result.success);
                let expected = if status == 401 {
                    ToolFailureCategory::Permanent
                } else {
                    ToolFailureCategory::Transient
                };
                assert_eq!(
                    result.failure.as_ref().map(|failure| failure.category),
                    Some(expected)
                );
                assert_eq!(
                    result
                        .failure
                        .as_ref()
                        .and_then(|failure| failure.retry_after_ms),
                    hint.map(|seconds| seconds * 1_000)
                );
                assert_eq!(requests.len(), 1);
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            } else {
                assert!(result.success);
                assert_eq!(requests.len(), 2);
                assert_eq!(calls.load(Ordering::SeqCst), 2);
                let elapsed = requests
                    .first()
                    .zip(requests.get(1))
                    .map(|(first, second)| second.at.saturating_duration_since(first.at))
                    .ok_or_else(|| {
                        echo_core::error::ReactError::Other("retry request missing".to_string())
                    })?;
                assert!(elapsed >= std::time::Duration::from_secs(2));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn successful_pages_keep_fetch_before_paginate() -> Result<()> {
        use crate::http_body::test_http::{client, response, server};
        use echo_core::tools::{Tool, ToolParameters};

        let body = r#"{"web":{"results":[{"title":"First","url":"https://example.test/1","description":"中文🙂"},{"title":"Second","url":"https://example.test/2","description":"next"}]}}"#;
        let (endpoint, serving) = server(vec![response(200, "", body), response(200, "", body)])?;
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = LocalBraveProvider {
            provider: BraveSearchProvider {
                client: client()?,
                api_key: "fixture-key".to_string(),
            },
            endpoint,
            calls: Arc::clone(&calls),
        };
        let tool = crate::web::WebSearchTool::new(Box::new(provider));
        let mut parameters = ToolParameters::from([
            ("query".to_string(), serde_json::json!("unchanged query")),
            ("limit".to_string(), serde_json::json!(1)),
        ]);
        let first = tool.execute(parameters.clone()).await?;
        assert!(first.success);
        assert_eq!(
            first
                .data
                .as_ref()
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("title"))
                .and_then(serde_json::Value::as_str),
            Some("First")
        );
        let cursor = first.metadata.get("page.next_cursor").ok_or_else(|| {
            echo_core::error::ReactError::Other("first page has no cursor".to_string())
        })?;
        parameters.insert("cursor".to_string(), serde_json::json!(cursor));
        let second = tool.execute(parameters).await?;
        assert!(second.success);
        assert_eq!(
            second
                .data
                .as_ref()
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("title"))
                .and_then(serde_json::Value::as_str),
            Some("Second")
        );
        assert!(!second.metadata.contains_key("page.next_cursor"));
        assert_eq!(
            second.metadata.get("page.returned").map(String::as_str),
            Some("1")
        );
        let requests = serving.join().map_err(|_| {
            echo_core::error::ReactError::Other("HTTP fixture panicked".to_string())
        })??;
        assert_eq!(requests.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        eprintln!(
            "successful pagination: logical_attempts=2, http_requests={}",
            requests.len()
        );
        Ok(())
    }

    #[tokio::test]
    async fn actual_http_failures_keep_status_and_do_not_return_error_bodies() -> Result<()> {
        use crate::http_body::test_http::{client, response, server};
        use echo_core::tools::{ToolFailure, ToolFailureCategory};
        for status in [401, 402, 403, 404, 408, 429, 500] {
            let (endpoint, serving) = server(vec![response(
                status,
                "Retry-After: 2\r\n",
                "fixture-secret",
            )])?;
            let provider = BraveSearchProvider {
                client: client()?,
                api_key: "fixture-key".to_string(),
            };
            let error = provider
                .search_url(&endpoint, 5)
                .await
                .err()
                .ok_or_else(|| {
                    echo_core::error::ReactError::Other("HTTP error returned success".to_string())
                })?;
            let failure = ToolFailure::from_error(&error, false);
            let expected = match status {
                408 => ToolFailureCategory::Timeout,
                429 | 500 => ToolFailureCategory::Transient,
                _ => ToolFailureCategory::Permanent,
            };
            assert_eq!(failure.category, expected);
            assert_eq!(failure.retry_after_ms, Some(2_000));
            assert_eq!(
                failure.allows_automatic_retry(),
                matches!(status, 408 | 429 | 500)
            );
            assert!(
                matches!(&error, echo_core::error::ReactError::Tool(inner) if matches!(inner.as_ref(), echo_core::error::ToolError::RemoteFailure { http_status: Some(actual), .. } if *actual == status))
            );
            assert!(!error.to_string().contains("fixture-secret"));
            assert!(!error.to_string().contains("fixture-key"));
            let requests = serving.join().map_err(|_| {
                echo_core::error::ReactError::Other("HTTP fixture panicked".to_string())
            })??;
            assert_eq!(requests.len(), 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn actual_http_missing_malformed_and_conflicting_hints_remain_unknown() -> Result<()> {
        use crate::http_body::test_http::{client, response, server};
        use echo_core::tools::ToolFailure;
        for (headers, expected) in [
            ("", None),
            ("Retry-After: bad\r\n", None),
            ("Retry-After: -1\r\n", None),
            ("Retry-After: 18446744073709551615\r\n", None),
            ("Retry-After: 0\r\n", Some(0)),
            ("Retry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\n", Some(0)),
            ("Retry-After: 2\r\nRetry-After: 3\r\n", None),
        ] {
            let (endpoint, serving) = server(vec![response(429, headers, "secret")])?;
            let provider = BraveSearchProvider {
                client: client()?,
                api_key: "fixture-key".to_string(),
            };
            let error = provider
                .search_url(&endpoint, 5)
                .await
                .err()
                .ok_or_else(|| {
                    echo_core::error::ReactError::Other("HTTP429 returned success".to_string())
                })?;
            assert_eq!(
                ToolFailure::from_error(&error, false).retry_after_ms,
                expected
            );
            assert_eq!(
                serving
                    .join()
                    .map_err(|_| echo_core::error::ReactError::Other(
                        "HTTP fixture panicked".to_string()
                    ))??
                    .len(),
                1
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn invalid_private_endpoint_is_permanent_without_echoing_it() -> Result<()> {
        use crate::http_body::test_http::client;
        use echo_core::tools::{ToolFailure, ToolFailureCategory};
        let provider = BraveSearchProvider {
            client: client()?,
            api_key: "fixture-key".to_string(),
        };
        let error = provider
            .search_url("http://[fixture-secret", 5)
            .await
            .err()
            .ok_or_else(|| {
                echo_core::error::ReactError::Other("invalid URI returned success".to_string())
            })?;
        assert_eq!(
            ToolFailure::from_error(&error, false).category,
            ToolFailureCategory::Permanent
        );
        assert!(!error.to_string().contains("fixture-secret"));
        Ok(())
    }

    #[tokio::test]
    async fn actual_connection_failure_and_timeout_keep_typed_causes() -> Result<()> {
        use crate::http_body::test_http::client;
        use echo_core::error::{ReactError, ToolError};
        use echo_core::tools::{ToolFailure, ToolFailureCategory};

        let closed = std::net::TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}", closed.local_addr()?);
        drop(closed);
        let provider = BraveSearchProvider {
            client: client()?,
            api_key: "fixture-key".to_string(),
        };
        let error = provider
            .search_url(&endpoint, 5)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("closed local port returned success".to_string()))?;
        assert_eq!(
            ToolFailure::from_error(&error, false).category,
            ToolFailureCategory::Transient
        );
        assert!(!error.to_string().contains(&endpoint));

        // A bound listener keeps the TCP connection open without producing an
        // HTTP response. The real client's deadline supplies the typed cause.
        let silent = std::net::TcpListener::bind("127.0.0.1:0")?;
        let endpoint = format!("http://{}", silent.local_addr()?);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(20))
            .build()
            .map_err(|error| crate::http_body::request_error("web_search", error, None))?;
        let provider = BraveSearchProvider {
            client,
            api_key: "fixture-key".to_string(),
        };
        let error = provider
            .search_url(&endpoint, 5)
            .await
            .err()
            .ok_or_else(|| ReactError::Other("silent endpoint returned success".to_string()))?;
        assert!(
            matches!(error, ReactError::Tool(ref inner) if matches!(inner.as_ref(), ToolError::Timeout(_)))
        );
        assert_eq!(
            ToolFailure::from_error(&error, false).category,
            ToolFailureCategory::Timeout
        );
        drop(silent);
        Ok(())
    }

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
