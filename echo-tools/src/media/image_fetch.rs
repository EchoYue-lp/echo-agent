//! Image fetch tool for downloading images from URLs and converting to base64.

use echo_core::error::{Result, ToolError};
use echo_core::tools::permission::ToolPermission;
use echo_core::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use serde_json::Value;
use std::time::Duration;

/// Image fetch tool
///
/// Downloads images from URLs and converts them to base64 format
/// suitable for LLM vision processing.
pub struct ImageFetchTool {
    timeout_secs: u64,
}

impl ImageFetchTool {
    /// Create a new image fetch tool
    pub fn new() -> Result<Self> {
        Ok(Self { timeout_secs: 30 })
    }

    /// Set custom timeout in seconds
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }
}

impl Default for ImageFetchTool {
    fn default() -> Self {
        Self { timeout_secs: 30 }
    }
}

impl Tool for ImageFetchTool {
    fn name(&self) -> &str {
        "image_fetch"
    }

    fn permissions(&self) -> Vec<ToolPermission> {
        vec![ToolPermission::Network]
    }

    fn description(&self) -> &str {
        "Downloads an image from a URL and converts it to base64 encoding, suitable for LLM multimodal input. \
         Parameters: url - image URL (required), max_size_mb - maximum file size in MB (optional, default 10MB)"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Image URL (supports http:// or https://)"
                },
                "max_size_mb": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum file size limit (MB, default 10)"
                }
            },
            "required": ["url"]
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        parameters: ToolParameters,
        context: &'a echo_core::tools::ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            let url = parameters
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("url".to_string()))?;

            if url.trim().is_empty() {
                return Ok(ToolResult::error("URL cannot be empty"));
            }

            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error("URL must start with http:// or https://"));
            }

            let max_size_mb = parameters
                .get("max_size_mb")
                .and_then(|v| v.as_u64())
                .unwrap_or(10);

            tracing::info!("ImageFetch: url='{}', max_size_mb={}", url, max_size_mb);

            // Convert MB to bytes
            let max_bytes = usize::try_from(max_size_mb)
                .ok()
                .and_then(|value| value.checked_mul(1024 * 1024))
                .ok_or_else(|| ToolError::InvalidParameter {
                    name: "max_size_mb".to_string(),
                    message: "image size limit is not supported on this platform".to_string(),
                })?;

            // Check if URL points to an image
            let is_image = url.to_lowercase().ends_with(".png")
                || url.to_lowercase().ends_with(".jpg")
                || url.to_lowercase().ends_with(".jpeg")
                || url.to_lowercase().ends_with(".gif")
                || url.to_lowercase().ends_with(".webp")
                || url.to_lowercase().ends_with(".bmp")
                || url.to_lowercase().ends_with(".svg");

            if !is_image {
                // Try to check via HEAD request (SSRF-safe: resolves + pins IPs)
                match crate::security::local_http_request(
                    url,
                    Duration::from_secs(self.timeout_secs),
                    5,
                    reqwest::Method::HEAD,
                )
                .await
                {
                    Ok(response) => {
                        if let Some(ct) = response.headers().get("content-type")
                            && let Ok(content_type) = ct.to_str()
                            && !content_type.starts_with("image/")
                        {
                            return Ok(ToolResult::error(format!(
                                "URL does not point to an image file, Content-Type: {}",
                                content_type
                            )));
                        }
                    }
                    Err(_) => {
                        // HEAD request failed (network error, SSRF block, etc.).
                        // Fall through to GET which also validates SSRF.
                    }
                }
            }

            // Download image (SSRF-safe: resolve + validate + connect on pinned IPs,
            // closing the DNS-rebinding TOCTOU window)
            let response =
                crate::security::local_http_get(url, Duration::from_secs(self.timeout_secs), 5)
                    .await
                    .map_err(|e| {
                        echo_core::error::ReactError::Tool(Box::new(ToolError::ExecutionFailed {
                            tool: "image_fetch".into(),
                            message: format!("Failed to download image: {}", e),
                        }))
                    })?;

            if !response.status().is_success() {
                return Ok(ToolResult::error(format!(
                    "HTTP error: {}",
                    response.status()
                )));
            }

            // Get content type before consuming response
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();

            // Extract mime type
            let mime_subtype = content_type
                .split_once('/')
                .map(|(_, subtype)| subtype)
                .unwrap_or("png")
                .split(';')
                .next()
                .unwrap_or("png")
                .trim();

            let bytes = crate::http_body::read_bounded_body(
                response,
                max_bytes,
                "image_fetch",
                Some(context),
            )
            .await?;

            // Encode to base64
            use base64::Engine;
            let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

            let data_uri = format!("data:image/{};base64,{}", mime_subtype, base64_data);

            let output = format!(
                "URL: {}\nContent-Type: {}\nSize: {} bytes\nBase64 length: {} chars\n\nData URI: {}",
                url,
                content_type,
                bytes.len(),
                base64_data.len(),
                data_uri
            );

            Ok(ToolResult::success(output))
        })
    }
}
