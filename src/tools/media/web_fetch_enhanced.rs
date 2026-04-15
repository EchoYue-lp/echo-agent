//! Enhanced web fetch tool with image support.

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

static CLIENT: OnceLock<Client> = OnceLock::new();

fn build_client() -> &'static Client {
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/131.0.0.0 Safari/537.36",
            )
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
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
            timeout_secs: 20,
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
                tracing::warn!("HTML 转文本失败: {}, 退回去除标签", e);
                // Fallback: simple tag removal
                let re = regex::Regex::new(r"<[^>]+>").unwrap();
                re.replace_all(html, "").to_string()
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
            format!("{}\n\n[... 内容已截断 ...]", truncated)
        }
    }

    /// Download image as base64 data URI
    #[allow(dead_code)]
    async fn download_image_as_base64(&self, url: &str) -> Result<(String, String, usize)> {
        let response = self.client.get(url).send().await.map_err(|e| {
            crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                tool: "web_fetch_enhanced".into(),
                message: format!("下载图片失败: {}", e),
            })
        })?;

        if !response.status().is_success() {
            return Err(crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                tool: "web_fetch_enhanced".into(),
                message: format!("HTTP 错误: {}", response.status()),
            }));
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
            crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                tool: "web_fetch_enhanced".into(),
                message: format!("读取图片数据失败: {}", e),
            })
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

    fn description(&self) -> &str {
        "增强的网页获取工具，支持：HTML 转文本、JSON 格式化、图片下载转 base64。\
         参数：url - 网址（必填），mode - 模式选择（text/json/image，默认 text），\
         max_length - 最大内容长度（可选，默认50000字符）"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要获取内容的 URL"
                },
                "mode": {
                    "type": "string",
                    "enum": ["text", "json", "image"],
                    "description": "处理模式：text-提取文本，image-下载图片转base64，json-返回原始JSON"
                },
                "max_length": {
                    "type": "integer",
                    "description": "最大返回内容长度（字符数，默认50000）"
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
                    crate::error::ReactError::Tool(ToolError::MissingParameter("url".to_string()))
                })?;

            if url.trim().is_empty() {
                return Ok(ToolResult::error("URL 不能为空".into()));
            }

            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error(
                    "URL 必须以 http:// 或 https:// 开头".into(),
                ));
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

            let client = Client::builder()
                .user_agent("Mozilla/5.0 (compatible; EchoAgent/1.0)")
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to build HTTP client: {}, using default", e);
                    Client::new()
                });

            // Check if URL points to an image (regardless of mode)
            let is_image_url = Self::is_image_url(url);

            // If mode is explicitly "image" or URL looks like an image
            if mode == "image" || is_image_url {
                // Verify it's actually an image
                let response = client.head(url).send().await.map_err(|e| {
                    crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                        tool: "web_fetch_enhanced".into(),
                        message: format!("HEAD 请求失败: {}", e),
                    })
                })?;

                if !response.status().is_success() {
                    return Ok(ToolResult::error(format!(
                        "HTTP 错误: {}",
                        response.status()
                    )));
                }

                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("application/octet-stream");

                if !Self::is_image_content_type(content_type) {
                    return Ok(ToolResult::error(format!(
                        "URL 不是图片，Content-Type: {}",
                        content_type
                    )));
                }

                // Download image
                let result: std::result::Result<(String, String, usize), ToolError> =
                    async {
                        let response = client.get(url).send().await.map_err(|e| {
                            ToolError::ExecutionFailed {
                                tool: "web_fetch_enhanced".into(),
                                message: format!("下载图片失败: {}", e),
                            }
                        })?;

                        if !response.status().is_success() {
                            return Err(ToolError::ExecutionFailed {
                                tool: "web_fetch_enhanced".into(),
                                message: format!("HTTP 错误: {}", response.status()),
                            });
                        }

                        let content_type = response
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("image/jpeg")
                            .to_string();

                        let mime_subtype = content_type.split('/').nth(1).unwrap_or("png");

                        let bytes =
                            response
                                .bytes()
                                .await
                                .map_err(|e| ToolError::ExecutionFailed {
                                    tool: "web_fetch_enhanced".into(),
                                    message: format!("读取图片数据失败: {}", e),
                                })?;

                        let size = bytes.len();
                        use base64::Engine;
                        let base64_data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        Ok((
                            format!("data:image/{};base64,{}", mime_subtype, base64_data),
                            content_type,
                            size,
                        ))
                    }
                    .await;

                let (data_uri, ct, size) = result.map_err(crate::error::ReactError::Tool)?;

                // Truncate base64 if too long
                let data_uri_display = if data_uri.len() > 1000 {
                    format!("{}... (共 {} 字符)", &data_uri[..1000], data_uri.len())
                } else {
                    data_uri.clone()
                };

                let output = format!(
                    "URL: {}\nContent-Type: {}\n大小: {} bytes\n\nBase64 数据 URI:\n{}",
                    url, ct, size, data_uri_display
                );

                return Ok(ToolResult::success(output));
            }

            // Text mode (default) or JSON mode
            let response = client.get(url).send().await.map_err(|e| {
                crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                    tool: "web_fetch_enhanced".into(),
                    message: format!("请求失败: {}", e),
                })
            })?;

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::error(format!("HTTP 错误: {}", status)));
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .to_string();

            let body = response.text().await.map_err(|e| {
                crate::error::ReactError::Tool(ToolError::ExecutionFailed {
                    tool: "web_fetch_enhanced".into(),
                    message: format!("读取响应体失败: {}", e),
                })
            })?;

            let content = if mode == "json" {
                // Return JSON as-is (validated)
                if serde_json::from_str::<Value>(&body).is_err() {
                    format!("{}\n\n[警告: 内容不是有效 JSON]", body)
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
                "URL: {}\n状态码: {}\n内容类型: {}\n\n{}",
                url, status, content_type, content
            );

            Ok(ToolResult::success(output))
        })
    }
}
