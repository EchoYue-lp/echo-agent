//! Image fetch tool for downloading images from URLs and converting to base64.

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

/// Image fetch tool
///
/// Downloads images from URLs and converts them to base64 format
/// suitable for LLM vision processing.
#[allow(dead_code)]
pub struct ImageFetchTool {
    client: Client,
    timeout_secs: u64,
}

impl ImageFetchTool {
    /// Create a new image fetch tool
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            timeout_secs: 30,
        }
    }

    /// Set custom timeout in seconds
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Detect if a URL points to an image based on extension or content-type
    #[allow(dead_code)]
    async fn is_image_url(&self, url: &str) -> bool {
        // First check extension
        let lower = url.to_lowercase();
        for ext in &[
            ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".svg", ".ico",
        ] {
            if lower.ends_with(ext) {
                return true;
            }
        }

        // If extension is unclear, try HEAD request to check content-type
        if let Ok(response) = self.client.head(url).send().await
            && let Some(content_type) = response.headers().get("content-type")
            && let Ok(ct) = content_type.to_str()
        {
            return ct.starts_with("image/");
        }

        false
    }

    /// Download image from URL and return base64 encoded data
    #[allow(dead_code)]
    async fn download_image_as_base64(&self, url: &str) -> Result<(String, String)> {
        let response = self.client.get(url).send().await.map_err(|e| {
            crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                tool: "image_fetch".into(),
                message: format!("下载图片失败: {}", e),
            })
        })?;

        if !response.status().is_success() {
            return Err(crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                tool: "image_fetch".into(),
                message: format!("HTTP 错误: {}", response.status()),
            }));
        }

        // Get content type to determine image format (before consuming response)
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        // Extract mime type (e.g., "image/png" -> "png")
        let mime_type = content_type.split('/').nth(1).unwrap_or("png");

        // Download binary data
        let bytes = response.bytes().await.map_err(|e| {
            crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                tool: "image_fetch".into(),
                message: format!("读取图片数据失败: {}", e),
            })
        })?;

        // Encode to base64
        use base64::Engine;
        let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        Ok((
            format!("data:image/{};base64,{}", mime_type, base64_data),
            mime_type.to_string(),
        ))
    }
}

impl Default for ImageFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ImageFetchTool {
    fn name(&self) -> &str {
        "image_fetch"
    }

    fn description(&self) -> &str {
        "从 URL 下载图片并转换为 base64 编码，支持用于 LLM 多模态输入。\
         参数：url - 图片 URL（必填），max_size_mb - 最大文件大小 MB（可选，默认 10MB）"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "图片 URL（支持 http:// 或 https://）"
                },
                "max_size_mb": {
                    "type": "integer",
                    "description": "最大文件大小限制（MB，默认 10）"
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
                return Ok(ToolResult::error("URL 不能为空"));
            }

            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error("URL 必须以 http:// 或 https:// 开头"));
            }

            let max_size_mb = parameters
                .get("max_size_mb")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            tracing::info!("ImageFetch: url='{}', max_size_mb={}", url, max_size_mb);

            // Convert MB to bytes
            let max_bytes = max_size_mb * 1024 * 1024;

            // Create client with size limit
            let client = Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client");

            // Check if URL points to an image
            let is_image = url.to_lowercase().ends_with(".png")
                || url.to_lowercase().ends_with(".jpg")
                || url.to_lowercase().ends_with(".jpeg")
                || url.to_lowercase().ends_with(".gif")
                || url.to_lowercase().ends_with(".webp")
                || url.to_lowercase().ends_with(".bmp")
                || url.to_lowercase().ends_with(".svg");

            if !is_image {
                // Try to check via HEAD request
                if let Ok(response) = client.head(url).send().await
                    && let Some(ct) = response.headers().get("content-type")
                    && let Ok(content_type) = ct.to_str()
                    && !content_type.starts_with("image/")
                {
                    return Ok(ToolResult::error(format!(
                        "URL 不指向图片文件，Content-Type: {}",
                        content_type
                    )));
                }
            }

            // Download image
            let response = client.get(url).send().await.map_err(|e| {
                crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                    tool: "image_fetch".into(),
                    message: format!("下载图片失败: {}", e),
                })
            })?;

            if !response.status().is_success() {
                return Ok(ToolResult::error(format!(
                    "HTTP 错误: {}",
                    response.status()
                )));
            }

            // Check content length if available
            if let Some(len) = response.content_length()
                && len > max_bytes as u64
            {
                return Ok(ToolResult::error(format!(
                    "图片过大: {} bytes，超过限制 {} MB",
                    len, max_size_mb
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
            let mime_subtype = content_type.split('/').nth(1).unwrap_or("png");

            // Download binary data
            let bytes = response.bytes().await.map_err(|e| {
                crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                    tool: "image_fetch".into(),
                    message: format!("读取图片数据失败: {}", e),
                })
            })?;

            // Verify size again after download
            if bytes.len() > max_bytes {
                return Ok(ToolResult::error(format!(
                    "图片过大: {} bytes，超过限制 {} MB",
                    bytes.len(),
                    max_size_mb
                )));
            }

            // Encode to base64
            use base64::Engine;
            let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);

            let data_uri = format!("data:image/{};base64,{}", mime_subtype, base64_data);

            let mut output = format!(
                "URL: {}\nContent-Type: {}\n大小: {} bytes\nBase64 长度: {} 字符\n\n数据 URI: {}",
                url,
                content_type,
                bytes.len(),
                base64_data.len(),
                &data_uri[..data_uri.len().min(200)]
            );
            output.push_str("...\n\n提示: 使用 data_uri 作为 ContentPart::ImageUrl 的 url 字段。");

            Ok(ToolResult::success(output))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_is_image_url_by_extension() {
        let tool = ImageFetchTool::new();
        assert!(tool.is_image_url("https://example.com/image.png").await);
        assert!(tool.is_image_url("https://example.com/photo.JPG").await);
        assert!(tool.is_image_url("https://example.com/pic.webp").await);
        assert!(!tool.is_image_url("https://example.com/page.html").await);
    }
}
