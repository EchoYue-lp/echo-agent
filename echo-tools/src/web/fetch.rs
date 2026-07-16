//! Web page fetching tool
//!
//! Provides [`WebFetchTool`], fetches URL content and converts it to readable text.
//! Supports HTML → plain text conversion, suitable for LLM consumption.

use crate::security::ssrf_safe_redirect_policy;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::StreamExt;
use futures::future::BoxFuture;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_MAX_LENGTH: usize = 50_000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_TEXT_WIDTH: usize = 120;

/// Hard byte limit on the raw HTTP response body (10 MB).
/// Responses larger than this are rejected before reading to prevent
/// memory exhaustion from large or malicious payloads.
const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;

static HTML_TAG_RE: OnceLock<Option<regex::Regex>> = OnceLock::new();
static WHITESPACE_RE: OnceLock<Option<regex::Regex>> = OnceLock::new();

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
    /// Retained for potential direct-client use by future call sites; the SSRF-safe
    /// path goes through `security::ssrf_safe_get`, which builds its own pinned client.
    #[allow(dead_code)]
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
                    "HTML to text conversion failed ({}), falling back to simple tag stripping: {}",
                    self.text_width,
                    e
                );
                // Fallback: strip HTML tags with a simple regex, then collapse whitespace
                let stripped = HTML_TAG_RE
                    .get_or_init(|| regex::Regex::new(r"<[^>]*>").ok())
                    .as_ref()
                    .map_or_else(
                        || html.to_string(),
                        |re| re.replace_all(html, " ").to_string(),
                    );
                WHITESPACE_RE
                    .get_or_init(|| regex::Regex::new(r"[ \t\r\n]+").ok())
                    .as_ref()
                    .map_or_else(
                        || stripped.trim().to_string(),
                        |re| re.replace_all(&stripped, "\n").trim().to_string(),
                    )
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

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
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

            // SSRF protection: resolve + validate + connect on pinned IPs, closing
            // the DNS-rebinding TOCTOU window that validate_url+client.get leaves.
            let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
            let response = match crate::security::ssrf_safe_get(url, timeout, 5).await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult::error(format!("Request failed: {}", e)));
                }
            };

            tracing::info!("WebFetch: url='{}', max_length={}", url, max_length);

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

            // Reject oversized responses early via Content-Length header
            if let Some(content_length) = response.content_length()
                && content_length > MAX_BODY_BYTES
            {
                return Ok(ToolResult::error(format!(
                    "Response body too large: Content-Length {} exceeds limit {} ({} MB)",
                    content_length,
                    MAX_BODY_BYTES,
                    MAX_BODY_BYTES / (1024 * 1024),
                )));
            }

            // Stream the response body with a hard byte cap to prevent memory exhaustion
            let mut body_bytes = Vec::new();
            let mut byte_stream = response.bytes_stream();
            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(ToolResult::error(format!(
                            "Failed to read response body: {}",
                            e
                        )));
                    }
                };
                if body_bytes.len() + chunk.len() > MAX_BODY_BYTES as usize {
                    return Ok(ToolResult::error(format!(
                        "Response body exceeds limit of {} bytes ({} MB)",
                        MAX_BODY_BYTES,
                        MAX_BODY_BYTES / (1024 * 1024),
                    )));
                }
                body_bytes.extend_from_slice(&chunk);
            }

            let body = String::from_utf8(body_bytes).unwrap_or_else(|e| {
                // Non-UTF-8: fall back to lossy conversion
                String::from_utf8_lossy(e.as_bytes()).into_owned()
            });

            // Process based on content type: only convert HTML/XHTML
            let content = if Self::needs_html_conversion(&content_type) {
                self.html_to_text(&body)
            } else {
                // Return raw content directly for text/plain, application/json, etc.
                body
            };

            let was_truncated = content.chars().count() > max_length;
            let content = Self::truncate_content(&content, max_length);

            let output = format!(
                "URL: {}\nStatus: {}\nContent-Type: {}\n\n{}",
                url, status, content_type, content
            );

            Ok(ToolResult::success(output)
                .with_truncated(was_truncated)
                .with_mime_type(content_type)
                .with_meta("url", url))
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
