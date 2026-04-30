//! Browser/Web automation tools
//!
//! Provides web content fetching and processing capabilities:
//! - web_fetch: fetch and parse web page content
//! - web_extract: extract structured content from HTML (title/body/links)
//! - web_search: search and return result page summaries

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
        "Fetch web page content and extract as readable text. Supports CSS selector for targeted extraction"
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Web page URL to fetch"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector, only extract content from matching regions (optional)"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum character count for returned content (default 10000)"
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

            // Validate URL
            let parsed = Url::parse(url_str).map_err(|_| ToolError::InvalidParameter {
                name: "url".to_string(),
                message: format!("Invalid URL: {}", url_str),
            })?;

            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(ToolError::InvalidParameter {
                    name: "url".to_string(),
                    message: "Only http/https protocols are supported".to_string(),
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
                    message: format!("Failed to create HTTP client: {}", e),
                })?;

            let response =
                client
                    .get(url_str)
                    .send()
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        tool: "web_fetch".to_string(),
                        message: format!("Request failed: {}", e),
                    })?;

            let status = response.status();
            if !status.is_success() {
                return Ok(ToolResult::success(format!(
                    "HTTP {}: request failed",
                    status
                )));
            }

            let html = response
                .text()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    tool: "web_fetch".to_string(),
                    message: format!("Failed to read response: {}", e),
                })?;

            let document = Html::parse_document(&html);
            let text = if let Some(sel_str) = parameters.get("selector").and_then(|v| v.as_str()) {
                // CSS selector mode
                let selector =
                    Selector::parse(sel_str).map_err(|_| ToolError::InvalidParameter {
                        name: "selector".to_string(),
                        message: format!("Invalid CSS selector: {}", sel_str),
                    })?;
                document
                    .select(&selector)
                    .map(|el| el.text().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                // Extract body text: remove script/style tags, get body text
                remove_noise(&document)
            };

            let text = clean_text(&text);
            let truncated = if text.len() > max_length {
                format!(
                    "{}...(truncated, original length {})",
                    &text[..max_length],
                    text.len()
                )
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
        "Extract structured information from HTML content: title, all links, paragraphs, tables, etc."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "html": {
                    "type": "string",
                    "description": "HTML content to parse"
                },
                "extract_type": {
                    "type": "string",
                    "enum": ["links", "headings", "paragraphs", "tables", "all"],
                    "description": "Extraction type (default 'all')"
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
                    "error": "Could not extract meaningful structured content from the HTML"
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
        // Fallback: simple extraction of all text
        let text: String = document.root_element().text().collect::<Vec<_>>().join(" ");
        let re = regex::Regex::new(r"\s+").unwrap();
        re.replace_all(&text, " ").to_string()
    })
}

fn clean_text(text: &str) -> String {
    // Collapse multiple blank lines
    let re = regex::Regex::new(r"\n{3,}").unwrap();
    let text = re.replace_all(text, "\n\n");
    // Collapse multiple spaces
    let re2 = regex::Regex::new(r" {2,}").unwrap();
    let text = re2.replace_all(&text, " ");
    // Collapse whitespace-only lines
    let re3 = regex::Regex::new(r"\n\s*\n\s*\n").unwrap();
    re3.replace_all(&text, "\n\n").to_string()
}
