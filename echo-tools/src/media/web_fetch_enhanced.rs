//! Enhanced web fetch tool with image support.

use crate::security::ssrf_safe_redirect_policy;
use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

static CLIENT: OnceLock<Client> = OnceLock::new();
static HTML_TAG_RE: OnceLock<Option<regex::Regex>> = OnceLock::new();

fn build_client() -> &'static Client {
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
            )
            .timeout(Duration::from_secs(30))
            .redirect(ssrf_safe_redirect_policy())
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("Failed to build HTTP client: {}, using default", e);
                Client::new()
            })
    })
}

/// Enhanced web fetch tool with image download support
///
/// Can fetch web pages and download images as base64.
// TODO(v0.3): replace WebFetchTool with this enhanced version after image pipeline validation
#[allow(dead_code)]
pub struct WebFetchToolEnhanced {
    client: Client,
    max_content_length: usize,
    text_width: usize,
    timeout_secs: u64,
}

impl WebFetchToolEnhanced {
    /// Create a new enhanced web fetch tool
    pub fn new() -> Self {
        Self {
            client: build_client().clone(),
            max_content_length: 50_000,
            text_width: 120,
            timeout_secs: 30,
        }
    }

    /// Set maximum content length for text responses
    pub fn with_max_content_length(mut self, n: usize) -> Self {
        self.max_content_length = n;
        self
    }

    /// Set HTML to text conversion line width
    pub fn with_text_width(mut self, width: usize) -> Self {
        self.text_width = width;
        self
    }

    /// Set timeout in seconds
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Check if content type is an image
    fn is_image_content_type(content_type: &str) -> bool {
        content_type.starts_with("image/")
    }

    /// Check if URL likely points to an image (by extension)
    fn is_image_url(url: &str) -> bool {
        let lower = url.to_lowercase();
        lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".gif")
            || lower.ends_with(".webp")
            || lower.ends_with(".bmp")
            || lower.ends_with(".svg")
            || lower.ends_with(".ico")
    }

    /// Convert HTML to readable text
    fn html_to_text(&self, html: &str) -> String {
        match html2text::from_read(html.as_bytes(), self.text_width) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(
                    "HTML to text conversion failed: {}, falling back to tag removal",
                    e
                );
                // Fallback: simple tag removal
                HTML_TAG_RE
                    .get_or_init(|| regex::Regex::new(r"<[^>]+>").ok())
                    .as_ref()
                    .map_or_else(
                        || html.to_string(),
                        |re| re.replace_all(html, "").to_string(),
                    )
            }
        }
    }

    /// Check if content type needs HTML conversion
    fn needs_html_conversion(content_type: &str) -> bool {
        content_type.contains("text/html") || content_type.contains("application/xhtml")
    }

    /// Truncate content by character count
    fn truncate_content(content: &str, max_len: usize) -> String {
        if content.chars().count() <= max_len {
            content.to_string()
        } else {
            let truncated: String = content.chars().take(max_len).collect();
            format!("{}\n\n[... content truncated ...]", truncated)
        }
    }

    /// Download image as base64 data URI
    #[allow(dead_code)]
    async fn download_image_as_base64(&self, url: &str) -> Result<(String, String, usize)> {
        let response = self.client.get(url).send().await.map_err(|e| {
            echo_core::error::ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                tool: "web_fetch_enhanced".into(),
                message: format!("Failed to download image: {}", e),
            }))
        })?;

        if !response.status().is_success() {
            return Err(echo_core::error::ReactError::Tool(Box::new(
                ToolError::ExecutionFailed {
                    tool: "web_fetch_enhanced".into(),
                    message: format!("HTTP error: {}", response.status()),
                },
            )));
        }

        // Get content type (before consuming response)
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let mime_subtype = content_type.split('/').nth(1).unwrap_or("png");

        // Download binary data
        let bytes = response.bytes().await.map_err(|e| {
            echo_core::error::ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                tool: "web_fetch_enhanced".into(),
                message: format!("Failed to read image data: {}", e),
            }))
        })?;

        let size = bytes.len();
        use base64::Engine;
        let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let data_uri = format!("data:image/{};base64,{}", mime_subtype, base64_data);

        Ok((data_uri, content_type.to_string(), size))
    }
}

impl Default for WebFetchToolEnhanced {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WebFetchToolEnhanced {
    fn name(&self) -> &str {
        "web_fetch_enhanced"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Enhanced web fetch tool, supports: HTML-to-text, JSON formatting, image download to base64. \
         Parameters: url - web address (required), mode - processing mode (text/json/image, default text), \
         max_length - maximum content length (optional, default 50000 chars)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from"
                },
                "mode": {
                    "type": "string",
                    "enum": ["text", "json", "image"],
                    "description": "Processing mode: text - extract text, image - download image as base64, json - return raw JSON"
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
                .ok_or_else(|| {
                    echo_core::error::ReactError::Tool(Box::new(ToolError::MissingParameter(
                        "url".to_string(),
                    )))
                })?;

            if url.trim().is_empty() {
                return Ok(ToolResult::error("URL cannot be empty"));
            }

            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error("URL must start with http:// or https://"));
            }

            let mode = parameters
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("text");
            let max_length = parameters
                .get("max_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(50_000) as usize;

            tracing::info!("WebFetchEnhanced: url='{}', mode='{}'", url, mode);

            // Check if URL points to an image (regardless of mode)
            let is_image_url = Self::is_image_url(url);

            // If mode is explicitly "image" or URL looks like an image
            if mode == "image" || is_image_url {
                // Verify content type via SSRF-safe HEAD request
                match crate::security::ssrf_safe_request(
                    url,
                    std::time::Duration::from_secs(30),
                    5,
                    reqwest::Method::HEAD,
                )
                .await
                {
                    Ok(response) if response.status().is_success() => {
                        let content_type = response
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("application/octet-stream");
                        if !Self::is_image_content_type(content_type) {
                            return Ok(ToolResult::error(format!(
                                "URL is not an image, Content-Type: {}",
                                content_type
                            )));
                        }
                    }
                    Ok(response) => {
                        return Ok(ToolResult::error(format!(
                            "HTTP error: {}",
                            response.status()
                        )));
                    }
                    Err(_) => {
                        // HEAD failed — proceed to download (GET also validates SSRF)
                    }
                }

                // Download image (SSRF-safe: resolve + validate + connect on pinned IPs)
                let response =
                    crate::security::ssrf_safe_get(url, std::time::Duration::from_secs(30), 5)
                        .await
                        .map_err(|e| {
                            echo_core::error::ReactError::Tool(Box::new(
                                ToolError::ExecutionFailed {
                                    tool: "web_fetch_enhanced".into(),
                                    message: format!("Failed to download image: {}", e),
                                },
                            ))
                        })?;

                if !response.status().is_success() {
                    return Ok(ToolResult::error(format!(
                        "HTTP error: {}",
                        response.status()
                    )));
                }

                let ct = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();

                let mime_subtype = ct.split('/').nth(1).unwrap_or("png");

                let bytes = response.bytes().await.map_err(|e| {
                    echo_core::error::ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                        tool: "web_fetch_enhanced".into(),
                        message: format!("Failed to read image data: {}", e),
                    }))
                })?;

                let size = bytes.len();
                use base64::Engine;
                let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let data_uri = format!("data:image/{};base64,{}", mime_subtype, base64_data);

                // Truncate base64 if too long
                let data_uri_display = if data_uri.len() > 1000 {
                    format!(
                        "{}... ({} chars total)",
                        &data_uri[..data_uri.floor_char_boundary(1000)],
                        data_uri.len()
                    )
                } else {
                    data_uri.clone()
                };

                let output = format!(
                    "URL: {}\nContent-Type: {}\nSize: {} bytes\n\nBase64 Data URI:\n{}",
                    url, ct, size, data_uri_display
                );

                return Ok(ToolResult::success(output));
            }

            // Text mode (default) or JSON mode (SSRF-safe)
            let response =
                crate::security::ssrf_safe_get(url, std::time::Duration::from_secs(30), 5)
                    .await
                    .map_err(|e| {
                        echo_core::error::ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                            tool: "web_fetch_enhanced".into(),
                            message: format!("Request failed: {}", e),
                        }))
                    })?;

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::error(format!("HTTP error: {}", status)));
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();

            let body = response.text().await.map_err(|e| {
                echo_core::error::ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                    tool: "web_fetch_enhanced".into(),
                    message: format!("Failed to read response body: {}", e),
                }))
            })?;

            let content = if mode == "json" {
                // Return JSON as-is (validated)
                if serde_json::from_str::<Value>(&body).is_err() {
                    format!("{}\n\n[Warning: content is not valid JSON]", body)
                } else {
                    body
                }
            } else {
                // Text mode: convert HTML to plain text
                if Self::needs_html_conversion(&content_type) {
                    self.html_to_text(&body)
                } else {
                    body
                }
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
