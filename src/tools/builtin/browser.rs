//! 浏览器/Web 自动化工具
//!
//! 提供 Web 内容获取和处理能力：
//! - web_fetch: 获取并解析网页内容
//! - web_extract: 从 HTML 中提取结构化内容（标题/正文/链接）
//! - web_search: 搜索并返回结果页摘要

use futures::future::BoxFuture;
use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use crate::error::{Result, ToolError};
use crate::tools::{Tool, ToolParameters, ToolResult};

// ── Web Fetch ──────────────────────────────────────────────────────────────

pub struct WebFetchTool;

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "获取网页内容并提取为可读文本。支持 CSS 选择器提取指定区域"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "要获取的网页 URL"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS 选择器，只提取匹配区域的内容（可选）"
                },
                "max_length": {
                    "type": "integer",
                    "description": "返回内容的最大字符数（默认 10000）"
                }
            },
            "required": ["url"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let url_str = parameters
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("url".to_string()))?;

            // 验证 URL
            let parsed = Url::parse(url_str).map_err(|_| ToolError::InvalidParameter {
                name: "url".to_string(),
                message: format!("无效的 URL: {}", url_str),
            })?;

            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(ToolError::InvalidParameter {
                    name: "url".to_string(),
                    message: "仅支持 http/https 协议".to_string(),
                }
                .into());
            }

            let max_length = parameters
                .get("max_length")
                .and_then(|v| v.as_u64())
                .unwrap_or(10000) as usize;

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("Mozilla/5.0 (compatible; EchoAgent/1.5)")
                .build()
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_fetch".to_string(),
                    message: format!("创建 HTTP 客户端失败: {}", e),
                })?;

            let response =
                client
                    .get(url_str)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "web_fetch".to_string(),
                        message: format!("请求失败: {}", e),
                    })?;

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::success(format!("HTTP {}: 请求失败", status)));
            }

            let html = response
                .text()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_fetch".to_string(),
                    message: format!("读取响应失败: {}", e),
                })?;

            let document = Html::parse_document(&html);
            let text = if let Some(sel_str) = parameters.get("selector").and_then(|v| v.as_str()) {
                // CSS 选择器模式
                let selector =
                    Selector::parse(sel_str).map_err(|_| ToolError::InvalidParameter {
                        name: "selector".to_string(),
                        message: format!("无效的 CSS 选择器: {}", sel_str),
                    })?;
                document
                    .select(&selector)
                    .map(|el| el.text().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                // 提取正文：去掉 script/style 标签，获取 body 文本
                remove_noise(&document)
            };

            let text = clean_text(&text);
            let truncated = if text.len() > max_length {
                format!("{}...(截断，原长度 {})", &text[..max_length], text.len())
            } else {
                text
            };

            Ok(ToolResult::success(truncated))
        })
    }
}

// ── Web Extract ────────────────────────────────────────────────────────────

pub struct WebExtractTool;

impl Tool for WebExtractTool {
    fn name(&self) -> &str {
        "web_extract"
    }

    fn description(&self) -> &str {
        "从 HTML 内容中提取结构化信息：标题、所有链接、段落、表格等"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "html": {
                    "type": "string",
                    "description": "要解析的 HTML 内容"
                },
                "extract_type": {
                    "type": "string",
                    "enum": ["links", "headings", "paragraphs", "tables", "all"],
                    "description": "提取类型（默认 'all'）"
                }
            },
            "required": ["html"]
        })
    }

    fn execute(&self, parameters: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let html = parameters
                .get("html")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::MissingParameter("html".to_string()))?;

            let extract_type = parameters
                .get("extract_type")
                .and_then(|v| v.as_str())
                .unwrap_or("all");

            let document = Html::parse_document(html);
            let mut result = serde_json::json!({});

            if (extract_type == "all" || extract_type == "headings")
                && let Ok(sel) = Selector::parse("h1, h2, h3, h4, h5, h6")
            {
                let headings: Vec<Value> = document
                    .select(&sel)
                    .map(|el| {
                        let tag = el.value().name().to_string();
                        let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        serde_json::json!({"tag": tag, "text": text})
                    })
                    .take(50)
                    .collect();
                if !headings.is_empty() {
                    result["headings"] = serde_json::json!(headings);
                }
            }

            if (extract_type == "all" || extract_type == "links")
                && let Ok(sel) = Selector::parse("a[href]")
            {
                let links: Vec<Value> = document
                    .select(&sel)
                    .filter_map(|el| {
                        let href = el.value().attr("href")?;
                        let text = el.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        if href.starts_with('#') || href.is_empty() {
                            None
                        } else {
                            Some(serde_json::json!({"text": text, "href": href}))
                        }
                    })
                    .take(100)
                    .collect();
                if !links.is_empty() {
                    result["links"] = serde_json::json!(links);
                }
            }

            if (extract_type == "all" || extract_type == "paragraphs")
                && let Ok(sel) = Selector::parse("p")
            {
                let paragraphs: Vec<String> = document
                    .select(&sel)
                    .map(|el| el.text().collect::<Vec<_>>().join(" ").trim().to_string())
                    .filter(|t| !t.is_empty())
                    .take(30)
                    .collect();
                if !paragraphs.is_empty() {
                    result["paragraphs"] = serde_json::json!(paragraphs);
                }
            }

            if (extract_type == "all" || extract_type == "tables")
                && let Ok(sel) = Selector::parse("table")
            {
                let table_count = document.select(&sel).count();
                if table_count > 0 {
                    result["table_count"] = serde_json::json!(table_count);
                }
            }

            if result.as_object().is_none_or(|o| o.is_empty()) {
                Ok(ToolResult::success_json(serde_json::json!({
                    "error": "未能从 HTML 中提取到有意义的结构化内容"
                })))
            } else {
                Ok(ToolResult::success_json(result))
            }
        })
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn remove_noise(document: &Html) -> String {
    let html_str = document.root_element().html();
    html2text::from_read(html_str.as_bytes(), 80).unwrap_or_else(|_| {
        // 回退：简单提取所有文本
        let text: String = document.root_element().text().collect::<Vec<_>>().join(" ");
        let re = regex::Regex::new(r"\s+").unwrap();
        re.replace_all(&text, " ").to_string()
    })
}

fn clean_text(text: &str) -> String {
    // 合并多行空白
    let re = regex::Regex::new(r"\n{3,}").unwrap();
    let text = re.replace_all(text, "\n\n");
    // 合并空格
    let re2 = regex::Regex::new(r" {2,}").unwrap();
    let text = re2.replace_all(&text, " ");
    // 合并只有空白的行
    let re3 = regex::Regex::new(r"\n\s*\n\s*\n").unwrap();
    re3.replace_all(&text, "\n\n").to_string()
}
