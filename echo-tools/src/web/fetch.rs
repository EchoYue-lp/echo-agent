//! Web page fetching tool
//!
//! Provides [`WebFetchTool`], fetches URL content and converts it to readable text.
//! Supports HTML → plain text conversion, suitable for LLM consumption.

use crate::security::{ssrf_safe_redirect_policy, validate_url};
use echo_core::error::{Result, ToolError};
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_MAX_LENGTH: usize = 50_000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_TEXT_WIDTH: usize = 120;

static CLIENT: OnceLock<Client> = OnceLock::new();

fn build_client() -> &'static Client {
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
            )
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .redirect(ssrf_safe_redirect_policy())
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("Failed to build HTTP client: {}, using default", e);
                Client::new()
            })
    })
}

/// Web page fetching tool
///
/// Fetches content from the specified URL, converting HTML to readable text.
pub struct WebFetchTool {
    client: Client,
    max_content_length: usize,
    text_width: usize,
}

impl WebFetchTool {
    /// Create a new WebFetchTool
    pub fn new() -> Self {
        Self {
            client: build_client().clone(),
            max_content_length: DEFAULT_MAX_LENGTH,
            text_width: DEFAULT_TEXT_WIDTH,
        }
    }

    /// Set the maximum content length (in characters)
    pub fn with_max_content_length(mut self, n: usize) -> Self {
        self.max_content_length = n;
        self
    }

    /// Set the HTML-to-text line width
    pub fn with_text_width(mut self, width: usize) -> Self {
        self.text_width = width;
        self
    }

    /// Check whether the Content-Type requires HTML→text conversion
    fn needs_html_conversion(content_type: &str) -> bool {
        content_type.contains("text/html") || content_type.contains("application/xhtml")
    }

    /// Convert HTML to readable text
    fn html_to_text(&self, html: &str) -> String {
        match html2text::from_read(html.as_bytes(), self.text_width) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(
                    "HTML to text conversion failed ({}), falling back to raw HTML tag stripping: {}",
                    self.text_width,
                    e
                );
                // Fallback: simple HTML tag removal
                html2text::from_read(html.as_bytes(), self.text_width).unwrap_or_default()
            }
        }
    }

    /// Truncate content by character count (safely handles multi-byte UTF-8)
    fn truncate_content(content: &str, max_len: usize) -> String {
        if content.chars().count() <= max_len {
            content.to_string()
        } else {
            let truncated: String = content.chars().take(max_len).collect();
            format!("{}\n\n[... content truncated ...]", truncated)
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetches web page content from a specified URL and converts HTML to readable text. \
         Parameters: url - web page address (required), max_length - maximum content length (optional, default 50000 chars)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The web page URL to fetch content from"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length to return (characters, default 50000)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let url = parameters
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("url".to_string()))?;

            if url.trim().is_empty() {
                return Ok(ToolResult::error("URL cannot be empty"));
            }

            // Basic URL format validation
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error("URL must start with http:// or https://"));
            }

            let max_length = parameters
                .get("max_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.max_content_length as u64) as usize;

            // SSRF protection: validate target address
            validate_url(url)?;

            tracing::info!("WebFetch: url='{}', max_length={}", url, max_length);

            let response = match self.client.get(url).send().await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult::error(format!("Request failed: {}", e)));
                }
            };

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::error(format!(
                    "HTTP request failed, status code: {}",
                    status
                )));
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();

            let body = match response.text().await {
                Ok(t) => t,
                Err(e) => {
                    return Ok(ToolResult::error(format!(
                        "Failed to read response body: {}",
                        e
                    )));
                }
            };

            // Process based on content type: only convert HTML/XHTML
            let content = if Self::needs_html_conversion(&content_type) {
                self.html_to_text(&body)
            } else {
                // Return raw content directly for text/plain, application/json, etc.
                body
            };

            let content = Self::truncate_content(&content, max_length);

            let output = format!(
                "URL: {}\nStatus: {}\nContent-Type: {}\n\n{}",
                url, status, content_type, content
            );

            Ok(ToolResult::success(output))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_html_conversion() {
        assert!(WebFetchTool::needs_html_conversion(
            "text/html; charset=utf-8"
        ));
        assert!(WebFetchTool::needs_html_conversion("application/xhtml+xml"));
        // text/plain should not be HTML-converted
        assert!(!WebFetchTool::needs_html_conversion("text/plain"));
        assert!(!WebFetchTool::needs_html_conversion("application/json"));
        assert!(!WebFetchTool::needs_html_conversion("image/png"));
    }

    #[test]
    fn test_truncate_content_short() {
        let content = "Hello world";
        let truncated = WebFetchTool::truncate_content(content, 100);
        assert_eq!(truncated, content);
    }

    #[test]
    fn test_truncate_content_long_ascii() {
        let content = "a".repeat(200);
        let truncated = WebFetchTool::truncate_content(&content, 100);
        assert!(truncated.contains("truncated"));
        assert!(truncated.starts_with(&"a".repeat(100)));
    }

    #[test]
    fn test_truncate_content_multibyte_safe() {
        // Multibyte character truncation should not panic
        let content = "HelloWorld".repeat(50); // 200 chars, 600 bytes?
        let truncated = WebFetchTool::truncate_content(&content, 10);
        assert!(truncated.contains("truncated"));
        assert!(truncated.starts_with("HelloWorld"));
    }

    #[test]
    fn test_truncate_content_mixed() {
        // Mixed ASCII + emoji
        let content = "Hello 🌍 World 🚀 Rust 🦀".repeat(20);
        let truncated = WebFetchTool::truncate_content(&content, 10);
        assert!(truncated.contains("truncated"));
        // Ensure truncated result is still valid UTF-8
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }

    #[test]
    fn test_html_to_text() {
        let tool = WebFetchTool::new();
        let html = "<html><body><h1>Title</h1><p>Hello world</p></body></html>";
        let text = tool.html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Hello"));
    }
}
