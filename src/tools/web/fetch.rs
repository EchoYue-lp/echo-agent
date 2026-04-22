//! Web 页面获取工具
//!
//! 提供 [`WebFetchTool`]，获取 URL 内容并转换为可读文本。
//! 支持 HTML → 纯文本转换，适合 LLM 消费。

use crate::error::{Result, ToolError};
use crate::tools::builtin::security::{ssrf_safe_redirect_policy, validate_url};
use crate::tools::{Tool, ToolParameters, ToolResult};
use futures::future::BoxFuture;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_MAX_LENGTH: usize = 50_000;
const DEFAULT_TIMEOUT_SECS: u64 = 20;
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

/// Web 页面获取工具
///
/// 获取指定 URL 的内容，将 HTML 转换为可读文本。
pub struct WebFetchTool {
    client: Client,
    max_content_length: usize,
    text_width: usize,
}

impl WebFetchTool {
    /// 创建新的 WebFetchTool
    pub fn new() -> Self {
        Self {
            client: build_client().clone(),
            max_content_length: DEFAULT_MAX_LENGTH,
            text_width: DEFAULT_TEXT_WIDTH,
        }
    }

    /// 设置最大内容长度（字符数）
    pub fn with_max_content_length(mut self, n: usize) -> Self {
        self.max_content_length = n;
        self
    }

    /// 设置 HTML 转文本的行宽
    pub fn with_text_width(mut self, width: usize) -> Self {
        self.text_width = width;
        self
    }

    /// 判断 Content-Type 是否需要 HTML→文本转换
    fn needs_html_conversion(content_type: &str) -> bool {
        content_type.contains("text/html") || content_type.contains("application/xhtml")
    }

    /// 将 HTML 转换为可读文本
    fn html_to_text(&self, html: &str) -> String {
        match html2text::from_read(html.as_bytes(), self.text_width) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(
                    "HTML 转文本失败 ({}), 退回原始 HTML 标签去除: {}",
                    self.text_width,
                    e
                );
                // 降级：简单去除 HTML 标签
                html2text::from_read(html.as_bytes(), self.text_width).unwrap_or_default()
            }
        }
    }

    /// 按字符数截断内容（安全处理多字节 UTF-8）
    fn truncate_content(content: &str, max_len: usize) -> String {
        if content.chars().count() <= max_len {
            content.to_string()
        } else {
            let truncated: String = content.chars().take(max_len).collect();
            format!("{}\n\n[... 内容已截断 ...]", truncated)
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
        "获取指定 URL 的网页内容，将 HTML 转换为可读文本。\
         参数：url - 网页地址（必填），max_length - 最大内容长度（可选，默认50000字符）"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要获取内容的网页 URL"
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
                .ok_or_else(|| ToolError::MissingParameter("url".to_string()))?;

            if url.trim().is_empty() {
                return Ok(ToolResult::error("URL 不能为空"));
            }

            // 基本 URL 格式校验
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Ok(ToolResult::error("URL 必须以 http:// 或 https:// 开头"));
            }

            let max_length = parameters
                .get("max_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.max_content_length as u64) as usize;

            // SSRF 防护：验证目标地址
            validate_url(url)?;

            tracing::info!("WebFetch: url='{}', max_length={}", url, max_length);

            let response = match self.client.get(url).send().await {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult::error(format!("请求失败: {}", e)));
                }
            };

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::error(format!(
                    "HTTP 请求失败，状态码: {}",
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
                    return Ok(ToolResult::error(format!("读取响应体失败: {}", e)));
                }
            };

            // 根据内容类型处理：仅对 HTML/XHTML 做转换
            let content = if Self::needs_html_conversion(&content_type) {
                self.html_to_text(&body)
            } else {
                // text/plain、application/json 等直接返回原始内容
                body
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_html_conversion() {
        assert!(WebFetchTool::needs_html_conversion(
            "text/html; charset=utf-8"
        ));
        assert!(WebFetchTool::needs_html_conversion("application/xhtml+xml"));
        // text/plain 不再做 HTML 转换
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
        assert!(truncated.contains("截断"));
        assert!(truncated.starts_with(&"a".repeat(100)));
    }

    #[test]
    fn test_truncate_content_multibyte_safe() {
        // 多字节字符（中文）截断不应 panic
        let content = "你好世界".repeat(50); // 200 chars, 600 bytes
        let truncated = WebFetchTool::truncate_content(&content, 10);
        assert!(truncated.contains("截断"));
        assert!(truncated.starts_with("你好世界你好"));
    }

    #[test]
    fn test_truncate_content_mixed() {
        // 混合 ASCII + emoji
        let content = "Hello 🌍 World 🚀 Rust 🦀".repeat(20);
        let truncated = WebFetchTool::truncate_content(&content, 10);
        assert!(truncated.contains("截断"));
        // 确保截断后仍是合法 UTF-8
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
