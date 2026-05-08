//! DuckDuckGo search Provider
//!
//! Dual-strategy implementation:
//! 1. **HTML search** (`html.duckduckgo.com`): full search results, but may be blocked by anti-scraping CAPTCHA
//! 2. **Instant Answer API** (`api.duckduckgo.com`): returns abstracts and related topics, stable and free
//!
//! Automatically falls back to the API when HTML search is blocked.

use super::utils::{percent_decode, truncate_chars, urlencode};
use super::{SearchProvider, SearchResult};
use async_trait::async_trait;
use echo_core::error::{Result, ToolError};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::time::Duration;
use tracing::warn;

/// DuckDuckGo search Provider (free, no API Key required)
///
/// Prefers HTML search for full results; automatically falls back to Instant Answer API if blocked by anti-scraping.
pub struct DuckDuckGoProvider {
    client: Client,
}

impl DuckDuckGoProvider {
    /// Create a new DuckDuckGo provider instance.
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
            )
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }

    /// Extract the actual URL from a DuckDuckGo redirect URL
    fn extract_url(href: &str) -> String {
        // Exact match query parameter uddg= (preceded by ? or &)
        let search = "?uddg=";
        if let Some(pos) = href.find(search) {
            let encoded = &href[pos + search.len()..];
            let encoded = encoded.split('&').next().unwrap_or(encoded);
            return percent_decode(encoded);
        }
        let search = "&uddg=";
        if let Some(pos) = href.find(search) {
            let encoded = &href[pos + search.len()..];
            let encoded = encoded.split('&').next().unwrap_or(encoded);
            return percent_decode(encoded);
        }

        if href.starts_with("//") {
            return format!("https:{}", href);
        }

        href.to_string()
    }
}

impl Default for DuckDuckGoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &str {
        "duckduckgo"
    }

    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let encoded_query = urlencode(query);

        // Strategy 1: HTML search
        let url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
        let response = self
            .client
            .get(&url)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("DuckDuckGo request failed: {}", e),
            })?;

        if !response.status().is_success() {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("DuckDuckGo returned error status: {}", response.status()),
            }
            .into());
        }

        let html = response
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("Failed to read response body: {}", e),
            })?;

        // Detect CAPTCHA / anti-scraping page
        if html.contains("anomaly-modal") || html.contains("bots use DuckDuckGo") {
            warn!(
                "DuckDuckGo HTML search blocked by anti-scraping, falling back to Instant Answer API"
            );
            return self.search_via_api(query, max_results).await;
        }

        // Attempt to parse HTML search results
        let results = parse_ddg_html(&html, max_results)?;
        if !results.is_empty() {
            return Ok(results);
        }

        // HTML parsing returned no results, falling back to API
        warn!("DuckDuckGo HTML parsing returned no results, falling back to Instant Answer API");
        self.search_via_api(query, max_results).await
    }
}

// ── Instant Answer API fallback ───────────────────────────────────────────────────

impl DuckDuckGoProvider {
    /// Search using the DuckDuckGo Instant Answer API
    ///
    /// Returns the abstract (Abstract) and related topics (RelatedTopics).
    /// Less comprehensive than full search, but stable and reliable, unaffected by anti-scraping.
    async fn search_via_api(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://api.duckduckgo.com/?q={}&format=json&no_html=1",
            urlencode(query)
        );

        let response =
            self.client
                .get(&url)
                .send()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_search".into(),
                    message: format!("DuckDuckGo API request failed: {}", e),
                })?;

        if !response.status().is_success() {
            return Err(ToolError::ExecutionFailed {
                tool: "web_search".into(),
                message: format!("DuckDuckGo API returned error: {}", response.status()),
            }
            .into());
        }

        let api_resp: DdgApiResponse =
            response
                .json()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_search".into(),
                    message: format!("DuckDuckGo API response parsing failed: {}", e),
                })?;

        let mut results = Vec::new();

        // Abstract as the first result
        if !api_resp.abstract_text.is_empty() && !api_resp.abstract_url.is_empty() {
            results.push(SearchResult {
                title: api_resp
                    .heading
                    .or_else(|| Some(query.to_string()))
                    .unwrap(),
                url: api_resp.abstract_url,
                snippet: truncate_chars(&api_resp.abstract_text, 300),
            });
        }

        // Supplement with RelatedTopics
        for topic in &api_resp.related_topics {
            if results.len() >= max_results {
                break;
            }
            if let Some(rt) = topic.as_object() {
                let text = rt.get("Text").and_then(|v| v.as_str()).unwrap_or("");
                let url = rt.get("FirstURL").and_then(|v| v.as_str()).unwrap_or("");
                if !text.is_empty() && !url.is_empty() {
                    // Extract the title from the text (the part before the first ' - ')
                    let title = text.split(" - ").next().unwrap_or(text).to_string();
                    results.push(SearchResult {
                        title,
                        url: url.to_string(),
                        snippet: truncate_chars(text, 300),
                    });
                }
            }
        }

        Ok(results)
    }
}

/// DuckDuckGo Instant Answer API response
#[derive(Debug, Deserialize)]
struct DdgApiResponse {
    #[serde(rename = "AbstractText")]
    abstract_text: String,
    #[serde(rename = "AbstractURL")]
    abstract_url: String,
    #[serde(rename = "Heading")]
    heading: Option<String>,
    #[serde(rename = "RelatedTopics")]
    related_topics: Vec<serde_json::Value>,
}

// ── HTML search result parsing ─────────────────────────────────────────────────────────

fn parse_ddg_html(html: &str, max_results: usize) -> Result<Vec<SearchResult>> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();

    let result_selectors = [".result", ".web-result", ".results_links"];
    let title_selectors = ["a.result__a", "a.result__title", "h2 a"];
    let snippet_selectors = [".result__snippet", "td.result__snippet", ".snippet"];

    for result_sel in &result_selectors {
        let Ok(selector) = Selector::parse(result_sel) else {
            continue;
        };

        for element in document.select(&selector) {
            if results.len() >= max_results {
                break;
            }

            let (title_text, url) = extract_title_and_url(&element, &title_selectors);
            if title_text.is_empty() || url.is_empty() {
                continue;
            }
            if url.contains("duckduckgo.com") && !url.contains("uddg=") {
                continue;
            }

            let snippet_text = extract_snippet(&element, &snippet_selectors);

            results.push(SearchResult {
                title: title_text,
                url,
                snippet: snippet_text,
            });
        }

        if !results.is_empty() {
            break;
        }
    }

    Ok(results)
}

fn extract_title_and_url(
    element: &scraper::ElementRef,
    title_selectors: &[&str],
) -> (String, String) {
    for sel_str in title_selectors {
        let Ok(selector) = Selector::parse(sel_str) else {
            continue;
        };
        if let Some(a) = element.select(&selector).next() {
            let text = a.text().collect::<String>().trim().to_string();
            let href = a.value().attr("href").unwrap_or("");
            return (text, DuckDuckGoProvider::extract_url(href));
        }
    }
    (String::new(), String::new())
}

fn extract_snippet(element: &scraper::ElementRef, snippet_selectors: &[&str]) -> String {
    for sel_str in snippet_selectors {
        let Ok(selector) = Selector::parse(sel_str) else {
            continue;
        };
        if let Some(s) = element.select(&selector).next() {
            let text = s.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_url_redirect() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com&rut=abc";
        assert_eq!(DuckDuckGoProvider::extract_url(href), "https://example.com");
    }

    #[test]
    fn test_extract_url_no_false_positive() {
        let href = "https://example.com/page?foo=bar";
        assert_eq!(
            DuckDuckGoProvider::extract_url(href),
            "https://example.com/page?foo=bar"
        );
    }

    #[test]
    fn test_extract_url_amp_uddg() {
        let href = "//duckduckgo.com/l/?foo=1&uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc";
        assert_eq!(
            DuckDuckGoProvider::extract_url(href),
            "https://example.com/page"
        );
    }

    #[test]
    fn test_extract_url_protocol_relative() {
        assert_eq!(
            DuckDuckGoProvider::extract_url("//example.com/page"),
            "https://example.com/page"
        );
    }

    #[test]
    fn test_detect_captcha() {
        let html =
            r#"<html><body><div class="anomaly-modal">bots use DuckDuckGo too</div></body></html>"#;
        assert!(html.contains("anomaly-modal"));
    }

    #[test]
    fn test_parse_ddg_html_with_results() {
        let html = r#"
        <html><body>
        <div class="result">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=abc">Rust Programming Language</a>
            <a class="result__snippet">A language empowering everyone to build reliable and efficient software.</a>
        </div>
        </body></html>"#;
        let results = parse_ddg_html(html, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
    }

    #[test]
    fn test_parse_ddg_html_empty() {
        let html = "<html><body><p>No results</p></body></html>";
        let results = parse_ddg_html(html, 10).unwrap();
        assert!(results.is_empty());
    }
}
